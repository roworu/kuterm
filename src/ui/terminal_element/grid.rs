//! turns grid cells into background rects and batched text runs

use alacritty_terminal::{
    selection::SelectionRange,
    term::cell::{Cell, Flags},
    vte::ansi::{Color, NamedColor},
};
use gpui::{
    App, Bounds, Font, Hsla, Pixels, Point, Rgba, StrikethroughStyle, TextAlign, TextRun,
    UnderlineStyle, Window, fill, point, px, size,
};

use super::box_drawing;
use crate::{
    terminal::{IndexedCell, TerminalBounds},
    theme::Theme,
};

// cell flags that change how its text is drawn, the rest only matter to the grid
const STYLE_FLAGS: Flags = Flags::BOLD
    .union(Flags::ITALIC)
    .union(Flags::DIM)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::STRIKEOUT);

/// terminal font in each style
pub(super) struct TerminalFonts {
    pub(super) regular: Font,
    pub(super) bold: Font,
    pub(super) italic: Font,
    pub(super) bold_italic: Font,
}

impl TerminalFonts {
    fn for_flags(&self, flags: Flags) -> &Font {
        match (flags.contains(Flags::BOLD), flags.contains(Flags::ITALIC)) {
            (false, false) => &self.regular,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (true, true) => &self.bold_italic,
        }
    }
}

/// font and color rules from settings that decide how cells look
pub(super) struct GridStyle<'a> {
    pub(super) fonts: &'a TerminalFonts,
    pub(super) bold_is_bright: bool,
    pub(super) minimum_contrast: f32,
    pub(super) dim_opacity: f32,
    pub(super) builtin_box_drawing: bool,
}

/// adjacent cells with same style, shaped and painted as one line
pub(super) struct BatchedTextRun {
    line: i32,
    column: i32,
    text: String,
    cell_count: usize,
    /// foreground, style flags and background (only when contrast is enforced) `style` was
    /// built from, cheaper to compare than the style
    key: (Color, Flags, Option<Hsla>),
    style: TextRun,
}

/// box drawing, block or powerline char drawn as shapes instead of a font glyph
pub(super) struct BoxGlyph {
    line: i32,
    column: i32,
    c: char,
    color: Hsla,
}

impl BoxGlyph {
    pub(super) fn paint(&self, origin: Point<Pixels>, dims: &TerminalBounds, window: &mut Window) {
        let cell = Bounds::new(
            point(
                origin.x + self.column as f32 * dims.cell_width,
                origin.y + self.line as f32 * dims.line_height,
            ),
            size(dims.cell_width, dims.line_height),
        );
        box_drawing::paint(self.c, cell, self.color, window);
    }
}

impl BatchedTextRun {
    fn push(&mut self, c: char, zerowidth: Option<&[char]>) {
        self.text.push(c);
        self.style.len += c.len_utf8();
        self.cell_count += 1;
        for &c in zerowidth.unwrap_or_default() {
            self.text.push(c);
            self.style.len += c.len_utf8();
        }
    }

    pub(super) fn paint(
        &mut self,
        origin: Point<Pixels>,
        dims: &TerminalBounds,
        font_size: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        let pos = point(
            origin.x + self.column as f32 * dims.cell_width,
            origin.y + self.line as f32 * dims.line_height,
        );
        // force_width keeps every glyph on the cell grid
        window
            .text_system()
            .shape_line(
                std::mem::take(&mut self.text).into(),
                font_size,
                std::slice::from_ref(&self.style),
                Some(dims.cell_width),
            )
            .paint(pos, dims.line_height, TextAlign::Left, None, window, cx)
            .ok();
    }
}

/// single line background fill spanning some cells
pub(super) struct LayoutRect {
    line: i32,
    column: i32,
    num_of_cells: usize,
    color: Hsla,
}

impl LayoutRect {
    pub(super) fn paint(&self, origin: Point<Pixels>, dims: &TerminalBounds, window: &mut Window) {
        let position = point(
            (origin.x + self.column as f32 * dims.cell_width).floor(),
            origin.y + self.line as f32 * dims.line_height,
        );
        let size = size(
            (dims.cell_width * self.num_of_cells as f32).ceil(),
            dims.line_height,
        );
        window.paint_quad(fill(Bounds::new(position, size), self.color));
    }
}

pub(super) fn layout_grid(
    cells: &[IndexedCell],
    display_offset: usize,
    selection: Option<SelectionRange>,
    style: &GridStyle,
    theme: &Theme,
) -> (Vec<LayoutRect>, Vec<BatchedTextRun>, Vec<BoxGlyph>) {
    let mut rects: Vec<LayoutRect> = Vec::new();
    let mut runs: Vec<BatchedTextRun> = Vec::new();
    let mut glyphs: Vec<BoxGlyph> = Vec::new();

    for indexed in cells {
        let cell = &indexed.cell;
        let line = indexed.point.line.0 + display_offset as i32;
        let column = indexed.point.column.0 as i32;

        let (mut fg, mut bg) = (cell.fg, cell.bg);
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }

        let selected = selection.is_some_and(|range| range.contains(indexed.point));
        let background = if selected {
            Some(theme.selection)
        } else {
            (!is_default_background(&bg)).then(|| theme.convert_color(&bg))
        };
        if let Some(color) = background {
            match rects.last_mut() {
                Some(last)
                    if last.color == color
                        && last.line == line
                        && last.column + last.num_of_cells as i32 == column =>
                {
                    last.num_of_cells += 1
                }
                _ => rects.push(LayoutRect {
                    line,
                    column,
                    num_of_cells: 1,
                    color,
                }),
            }
        }

        // wide chars take two cells, the spacer is only a placeholder
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || is_blank(cell) {
            continue;
        }

        // contrast depends on what is behind the text, so the background splits runs then
        let contrast_background =
            (style.minimum_contrast > 1.).then(|| background.unwrap_or(theme.terminal_background));

        if style.builtin_box_drawing && box_drawing::supports(cell.c) {
            glyphs.push(BoxGlyph {
                line,
                column,
                c: cell.c,
                color: text_color(cell, fg, contrast_background, style, theme),
            });
            continue;
        }

        // most cells continue the run before them, so the style is only built for new runs
        let key = (fg, cell.flags & STYLE_FLAGS, contrast_background);
        match runs.last_mut() {
            Some(run)
                if run.key == key
                    && run.line == line
                    && run.column + run.cell_count as i32 == column =>
            {
                run.push(cell.c, cell.zerowidth())
            }
            _ => {
                let mut run = BatchedTextRun {
                    line,
                    column,
                    text: String::new(),
                    cell_count: 0,
                    key,
                    style: TextRun {
                        len: 0,
                        ..cell_style(cell, fg, contrast_background, style, theme)
                    },
                };
                run.push(cell.c, cell.zerowidth());
                runs.push(run);
            }
        }
    }
    (rects, runs, glyphs)
}

fn is_default_background(color: &Color) -> bool {
    matches!(color, Color::Named(NamedColor::Background))
}

fn is_blank(cell: &Cell) -> bool {
    cell.c == ' '
        && is_default_background(&cell.bg)
        && !cell
            .flags
            .intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT | Flags::INVERSE)
}

fn dim_color(fg: Color, dim_opacity: f32, theme: &Theme) -> Hsla {
    match fg {
        Color::Named(named) if named.to_dim() != named => {
            theme.convert_color(&Color::Named(named.to_dim()))
        }
        Color::Indexed(index @ 0..=7) => theme.get_color_at_index(259 + index as usize),
        Color::Indexed(index @ 8..=15) => theme.get_color_at_index(index as usize - 8),
        _ => {
            let mut color = theme.convert_color(&fg);
            color.a *= dim_opacity;
            color
        }
    }
}

// normal ansi colors 0-7 turn into their bright versions 8-15
fn bright_color(fg: Color) -> Color {
    match fg {
        Color::Named(named) if (named as usize) < 8 => Color::Named(named.to_bright()),
        Color::Indexed(index @ 0..=7) => Color::Indexed(index + 8),
        fg => fg,
    }
}

fn text_color(
    cell: &Cell,
    fg: Color,
    background: Option<Hsla>,
    style: &GridStyle,
    theme: &Theme,
) -> Hsla {
    let color = if cell.flags.contains(Flags::DIM) {
        dim_color(fg, style.dim_opacity, theme)
    } else if style.bold_is_bright && cell.flags.contains(Flags::BOLD) {
        theme.convert_color(&bright_color(fg))
    } else {
        theme.convert_color(&fg)
    };
    match background {
        Some(background) => with_contrast(color, background, style.minimum_contrast),
        None => color,
    }
}

// wcag relative luminance
fn luminance(color: Rgba) -> f32 {
    let linear = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `fg` moved toward black or white, whichever stands out more on `bg`, just enough to reach
/// `minimum` contrast. alpha is kept
fn with_contrast(fg: Hsla, bg: Hsla, minimum: f32) -> Hsla {
    let (fg_rgba, bg_rgba): (Rgba, Rgba) = (fg.into(), bg.into());
    if contrast(fg_rgba, bg_rgba) >= minimum {
        return fg;
    }
    let (black, white): (Rgba, Rgba) = (gpui::black().into(), gpui::white().into());
    let target = if contrast(black, bg_rgba) > contrast(white, bg_rgba) {
        black
    } else {
        white
    };
    let mix = |t: f32| Rgba {
        r: fg_rgba.r + (target.r - fg_rgba.r) * t,
        g: fg_rgba.g + (target.g - fg_rgba.g) * t,
        b: fg_rgba.b + (target.b - fg_rgba.b) * t,
        a: fg_rgba.a,
    };
    // contrast grows the closer the mix gets to the target, so bisect the smallest step
    let (mut low, mut high) = (0., 1.);
    for _ in 0..16 {
        let middle = (low + high) / 2.;
        if contrast(mix(middle), bg_rgba) >= minimum {
            high = middle;
        } else {
            low = middle;
        }
    }
    mix(high).into()
}

fn cell_style(
    cell: &Cell,
    fg: Color,
    background: Option<Hsla>,
    style: &GridStyle,
    theme: &Theme,
) -> TextRun {
    let color = text_color(cell, fg, background, style, theme);
    // gpui's underline shader applies alpha twice, so pass its square root to match the text
    let line_color = Hsla {
        a: color.a.sqrt(),
        ..color
    };
    let underline = cell
        .flags
        .intersects(Flags::ALL_UNDERLINES)
        .then(|| UnderlineStyle {
            color: Some(line_color),
            thickness: px(1.),
            wavy: cell.flags.contains(Flags::UNDERCURL),
        });
    let strikethrough = cell
        .flags
        .contains(Flags::STRIKEOUT)
        .then(|| StrikethroughStyle {
            color: Some(line_color),
            thickness: px(1.),
        });
    TextRun {
        len: cell.c.len_utf8(),
        font: style.fonts.for_flags(cell.flags).clone(),
        color,
        background_color: None,
        underline,
        strikethrough,
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::{
        Term,
        event::VoidListener,
        term::{Config, test::TermSize},
        vte::ansi::Processor,
    };
    use gpui::{Rgba, font, rgb};

    use super::*;

    fn themes() -> [(&'static str, Theme); 2] {
        [
            ("dark", Theme::bundled(true)),
            ("light", Theme::bundled(false)),
        ]
    }

    /// xterm 256 color palette entry 16-255, computed here instead of asking the theme
    fn xterm(index: u8) -> Hsla {
        let (r, g, b) = match index {
            16..=231 => {
                let i = index - 16;
                let step = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                (step(i / 36), step((i % 36) / 6), step(i % 6))
            }
            _ => {
                let v = (index - 232) * 10 + 8;
                (v, v, v)
            }
        };
        rgb(u32::from_be_bytes([0, r, g, b])).into()
    }

    #[track_caller]
    fn assert_color(got: Hsla, want: Hsla, what: &str) {
        let (a, b): (Rgba, Rgba) = (got.into(), want.into());
        let same = (a.r - b.r).abs() < 0.003
            && (a.g - b.g).abs() < 0.003
            && (a.b - b.b).abs() < 0.003
            && (a.a - b.a).abs() < 0.003;
        assert!(same, "{what}: got {a:?}, want {b:?}");
    }

    fn fonts() -> TerminalFonts {
        TerminalFonts {
            regular: font("Mono"),
            bold: font("Mono Bold"),
            italic: font("Mono Italic"),
            bold_italic: font("Mono Bold Italic"),
        }
    }

    // builtin box drawing off, so box chars end up in text runs like any other char
    fn plain_style(fonts: &TerminalFonts) -> GridStyle<'_> {
        GridStyle {
            fonts,
            bold_is_bright: false,
            minimum_contrast: 1.,
            dim_opacity: 0.7,
            builtin_box_drawing: false,
        }
    }

    /// rects, runs and builtin glyphs of a real 40x3 alacritty term after feeding it `bytes`
    fn layout_styled(
        bytes: &str,
        selection: Option<SelectionRange>,
        style: &GridStyle,
        theme: &Theme,
    ) -> (Vec<LayoutRect>, Vec<BatchedTextRun>, Vec<BoxGlyph>) {
        let mut term = Term::new(Config::default(), &TermSize::new(40, 3), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, bytes.as_bytes());
        let cells: Vec<IndexedCell> = term
            .renderable_content()
            .display_iter
            .map(|indexed| IndexedCell {
                point: indexed.point,
                cell: indexed.cell.clone(),
            })
            .collect();
        layout_grid(&cells, 0, selection, style, theme)
    }

    /// rects and runs of a real 40x3 alacritty term after feeding it `bytes`
    fn layout_with(
        bytes: &str,
        selection: Option<SelectionRange>,
        theme: &Theme,
    ) -> (Vec<LayoutRect>, Vec<BatchedTextRun>) {
        let fonts = fonts();
        let (rects, runs, _) = layout_styled(bytes, selection, &plain_style(&fonts), theme);
        (rects, runs)
    }

    fn layout(bytes: &str, theme: &Theme) -> Vec<BatchedTextRun> {
        layout_with(bytes, None, theme).1
    }

    /// the text run that paints column `col` of line 0
    fn run_at(runs: &[BatchedTextRun], col: i32) -> &BatchedTextRun {
        runs.iter()
            .find(|run| {
                run.line == 0 && run.column <= col && col < run.column + run.cell_count as i32
            })
            .unwrap_or_else(|| panic!("no text run at column {col}"))
    }

    #[test]
    fn dim_normal_named_colors_use_ansi_dim() {
        let normal = [
            NamedColor::Black,
            NamedColor::Red,
            NamedColor::Green,
            NamedColor::Yellow,
            NamedColor::Blue,
            NamedColor::Magenta,
            NamedColor::Cyan,
            NamedColor::White,
        ];
        for (name, theme) in themes() {
            for (i, named) in normal.into_iter().enumerate() {
                let got = dim_color(Color::Named(named), 0.7, &theme);
                assert_color(got, theme.ansi_dim[i], &format!("{name} dim {named:?}"));
                assert_eq!(got.a, 1.0, "{name} dim {named:?} is opaque");
            }
        }
    }

    #[test]
    fn dim_indexed_16_to_255_fade() {
        for (name, theme) in themes() {
            for i in 16..=255u8 {
                let got = dim_color(Color::Indexed(i), 0.4, &theme);
                let mut want = xterm(i);
                want.a *= 0.4;
                assert_color(got, want, &format!("{name} dim 38;5;{i}"));
            }
        }
    }

    #[test]
    fn sgr_22_and_0_end_dim() {
        for (name, theme) in themes() {
            let runs = layout(
                "\x1b[2;31mA\x1b[22mB\x1b[2mC\x1b[0mD\x1b[2mE\x1b[1mF",
                &theme,
            );
            assert_color(run_at(&runs, 0).style.color, theme.ansi_dim[1], name);
            assert_color(run_at(&runs, 1).style.color, theme.ansi[1], name);
            assert_color(run_at(&runs, 2).style.color, theme.ansi_dim[1], name);
            assert_color(
                run_at(&runs, 3).style.color,
                theme.terminal_foreground,
                name,
            );
            assert_color(run_at(&runs, 4).style.color, theme.dim_foreground, name);
            // bold on top of dim keeps dim
            assert_color(run_at(&runs, 5).style.color, theme.dim_foreground, name);
        }
    }

    #[test]
    fn dim_and_normal_cells_are_separate_runs_and_equal_dim_cells_batch() {
        for (name, theme) in themes() {
            let runs = layout("\x1b[31mab\x1b[2mcd\x1b[22mef", &theme);
            let line: Vec<_> = runs.iter().filter(|r| r.line == 0).collect();
            assert_eq!(line.len(), 3, "{name}");
            assert_eq!(line[1].text, "cd");
            assert_eq!(line[1].cell_count, 2);
            assert_color(line[1].style.color, theme.ansi_dim[1], name);
        }
    }

    #[test]
    fn selection_paints_its_cells_including_blanks() {
        use alacritty_terminal::index::{Column, Line, Point as AlacPoint};

        for (name, theme) in themes() {
            // "ab" on red background, then blanks, selected from column 1 to 4
            let range = SelectionRange::new(
                AlacPoint::new(Line(0), Column(1)),
                AlacPoint::new(Line(0), Column(4)),
                false,
            );
            let (rects, runs) = layout_with("\x1b[41mab\x1b[0m", Some(range), &theme);
            let line: Vec<_> = rects.iter().filter(|r| r.line == 0).collect();
            assert_eq!(line.len(), 2, "{name}");
            assert_eq!((line[0].column, line[0].num_of_cells), (0, 1));
            assert_color(line[0].color, theme.ansi[1], name);
            assert_eq!((line[1].column, line[1].num_of_cells), (1, 4));
            assert_color(line[1].color, theme.selection, name);
            // text keeps its color, only the background changes
            assert_color(
                run_at(&runs, 1).style.color,
                theme.terminal_foreground,
                name,
            );
            assert!(rects.iter().all(|r| r.line == 0), "{name}");
        }
    }

    #[test]
    fn styled_cells_use_their_font() {
        let fonts = fonts();
        let style = plain_style(&fonts);
        let theme = Theme::bundled(true);
        let (_, runs, _) =
            layout_styled("a\x1b[1mb\x1b[3mc\x1b[22md\x1b[0me", None, &style, &theme);
        let family = |col| run_at(&runs, col).style.font.family.clone();
        assert_eq!(family(0), fonts.regular.family);
        assert_eq!(family(1), fonts.bold.family);
        assert_eq!(family(2), fonts.bold_italic.family);
        assert_eq!(family(3), fonts.italic.family);
        assert_eq!(family(4), fonts.regular.family);
    }

    #[test]
    fn bold_is_bright_brightens_only_normal_colors() {
        let fonts = fonts();
        let bright = GridStyle {
            bold_is_bright: true,
            ..plain_style(&fonts)
        };
        for (name, theme) in themes() {
            let bytes =
                "\x1b[1;31mA\x1b[38;5;2mB\x1b[38;5;9mC\x1b[38;5;100mD\x1b[22;31mE\x1b[1;2;31mF";
            let (_, runs, _) = layout_styled(bytes, None, &bright, &theme);
            assert_color(run_at(&runs, 0).style.color, theme.ansi[9], name);
            assert_color(run_at(&runs, 1).style.color, theme.ansi[10], name);
            assert_color(run_at(&runs, 2).style.color, theme.ansi[9], name);
            assert_color(run_at(&runs, 3).style.color, xterm(100), name);
            // not bold
            assert_color(run_at(&runs, 4).style.color, theme.ansi[1], name);
            // dim wins over bright
            assert_color(run_at(&runs, 5).style.color, theme.ansi_dim[1], name);

            let (_, runs, _) = layout_styled("\x1b[1;31mA", None, &plain_style(&fonts), &theme);
            assert_color(run_at(&runs, 0).style.color, theme.ansi[1], name);
        }
    }

    #[test]
    fn dim_opacity_fades_colors_without_dim_version() {
        let fonts = fonts();
        let style = GridStyle {
            dim_opacity: 0.25,
            ..plain_style(&fonts)
        };
        let theme = Theme::bundled(true);
        let (_, runs, _) = layout_styled("\x1b[2;38;5;100mA\x1b[31mB", None, &style, &theme);
        let mut want = xterm(100);
        want.a = 0.25;
        assert_color(run_at(&runs, 0).style.color, want, "faded");
        assert_color(
            run_at(&runs, 1).style.color,
            theme.ansi_dim[1],
            "dim theme color",
        );
    }

    #[test]
    fn minimum_contrast_moves_text_until_it_passes() {
        let rgba = |color: Hsla| -> Rgba { color.into() };
        let gray: Hsla = rgb(0x808080).into();
        for (bg, minimum) in [
            (0x000000, 7.),
            (0xffffff, 4.5),
            (0x777777, 3.),
            (0x202020, 21.),
        ] {
            let bg: Hsla = rgb(bg).into();
            let got = with_contrast(gray, bg, minimum);
            let ratio = contrast(rgba(got), rgba(bg));
            // 21 can only be reached with pure black on white, so the best is taken
            let best = contrast(rgba(gpui::black()), rgba(bg))
                .max(contrast(rgba(gpui::white()), rgba(bg)));
            assert!(ratio >= minimum.min(best) - 0.01, "{ratio} below {minimum}");
            // moved just enough, not all the way
            if minimum < best - 0.5 {
                assert!(ratio < minimum + 0.1, "{ratio} far above {minimum}");
            }
        }
        // text that already passes is kept
        let white: Hsla = gpui::white();
        assert_eq!(with_contrast(white, gpui::black(), 21.), white);
        // alpha is kept
        let faded = Hsla { a: 0.5, ..gray };
        assert_eq!(with_contrast(faded, gray, 4.5).a, 0.5);
    }

    #[test]
    fn minimum_contrast_uses_the_cell_background() {
        let fonts = fonts();
        let style = GridStyle {
            minimum_contrast: 4.5,
            ..plain_style(&fonts)
        };
        let theme = Theme::bundled(true);
        // same color as text and background, then the same text on the default background
        let (_, runs, _) = layout_styled("\x1b[31;41mAB\x1b[49mCD", None, &style, &theme);
        let line: Vec<_> = runs.iter().filter(|run| run.line == 0).collect();
        // the background splits otherwise equal runs
        assert_eq!(line.len(), 2);
        let (on_red, on_default) = (line[0].style.color, line[1].style.color);
        assert!(contrast(on_red.into(), theme.ansi[1].into()) >= 4.49);
        assert!(contrast(on_default.into(), theme.terminal_background.into()) >= 4.49);
        assert_ne!(on_red, on_default);
    }

    #[test]
    fn builtin_box_drawing_takes_box_chars_out_of_text_runs() {
        let fonts = fonts();
        let style = GridStyle {
            builtin_box_drawing: true,
            ..plain_style(&fonts)
        };
        let theme = Theme::bundled(true);
        let (_, runs, glyphs) =
            layout_styled("a\u{2500}\x1b[32m\u{2588}\u{e0b0}b", None, &style, &theme);
        let text: String = runs.iter().map(|run| run.text.as_str()).collect();
        assert_eq!(text, "ab");
        let chars: Vec<_> = glyphs.iter().map(|g| (g.column, g.c)).collect();
        assert_eq!(chars, [(1, '\u{2500}'), (2, '\u{2588}'), (3, '\u{e0b0}')]);
        assert_color(glyphs[0].color, theme.terminal_foreground, "default fg");
        assert_color(glyphs[1].color, theme.ansi[2], "green");

        // off, they stay text
        let (_, runs, glyphs) =
            layout_styled("a\u{2500}\u{2588}", None, &plain_style(&fonts), &theme);
        assert!(glyphs.is_empty());
        assert_eq!(runs[0].text, "a\u{2500}\u{2588}");
    }
}
