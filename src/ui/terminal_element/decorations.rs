//! drawing block decorations next to the grid: gutter marks with their tooltip and the
//! popover. laid out in prepaint from plain data the terminal keeps

use gpui::{
    App, Bounds, Font, Hsla, Pixels, Point, ShapedLine, TextRun, Window, fill, point, px, size,
};

use crate::{
    terminal::{GutterMark, Popover, Terminal, TerminalBounds},
    theme::Theme,
};

// cells the gutter takes left of the grid while some mark is set
pub const GUTTER_CELLS: f32 = 2.;
// longest popover line, longer ones are cut
const POPOVER_MAX_CHARS: usize = 120;

/// a box with lines of text in it
pub struct TextBox {
    pub bounds: Bounds<Pixels>,
    pub lines: Vec<ShapedLine>,
    pub background: Hsla,
    pub border: Hsla,
}

impl TextBox {
    pub fn paint(&self, line_height: Pixels, window: &mut Window, cx: &mut App) {
        window.paint_quad(fill(self.bounds, self.border));
        let inner = Bounds::new(
            self.bounds.origin + point(px(1.), px(1.)),
            size(
                self.bounds.size.width - px(2.),
                self.bounds.size.height - px(2.),
            ),
        );
        window.paint_quad(fill(inner, self.background));
        for (ix, line) in self.lines.iter().enumerate() {
            let at = inner.origin + point(px(4.), line_height * ix as f32);
            line.paint(at, line_height, gpui::TextAlign::Left, None, window, cx)
                .ok();
        }
    }
}

/// everything decorations draw, relative to the window
#[derive(Default)]
pub struct DecorationsLayout {
    pub gutter: Vec<(Point<Pixels>, ShapedLine)>,
    pub boxes: Vec<TextBox>,
}

fn shape(
    text: &str,
    font: &Font,
    color: Hsla,
    font_size: Pixels,
    window: &mut Window,
) -> ShapedLine {
    window.text_system().shape_line(
        text.to_string().into(),
        font_size,
        &[TextRun {
            len: text.len(),
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    )
}

/// a box of `lines` at `at`, kept inside `within`
#[allow(clippy::too_many_arguments)]
fn text_box(
    lines: &[(String, Hsla)],
    at: Point<Pixels>,
    within: Bounds<Pixels>,
    font: &Font,
    font_size: Pixels,
    line_height: Pixels,
    theme: &Theme,
    window: &mut Window,
) -> TextBox {
    let lines: Vec<ShapedLine> = lines
        .iter()
        .map(|(text, color)| shape(text, font, *color, font_size, window))
        .collect();
    let width = lines
        .iter()
        .map(|line| line.width)
        .fold(px(0.), Pixels::max)
        + px(10.);
    let height = line_height * lines.len() as f32 + px(2.);
    let x = at.x.min(within.right() - width).max(within.left());
    let y = at.y.min(within.bottom() - height).max(within.top());
    TextBox {
        bounds: Bounds::new(point(x, y), size(width, height)),
        lines,
        background: theme.tab_bar_background,
        border: theme.border,
    }
}

/// lay out the decorations of `terminal` around the grid at `dims`, `area` is the whole
/// element. `mouse` picks the tooltip shown
#[allow(clippy::too_many_arguments)]
pub fn layout_decorations(
    terminal: &Terminal,
    dims: &TerminalBounds,
    area: Bounds<Pixels>,
    mouse: Point<Pixels>,
    font: &Font,
    font_size: Pixels,
    theme: &Theme,
    window: &mut Window,
) -> DecorationsLayout {
    let mut layout = DecorationsLayout::default();
    let origin = dims.bounds.origin;
    let line_height = dims.line_height;
    let gutter_x = origin.x - dims.cell_width * GUTTER_CELLS;
    let mut tooltip = None;
    for (row, mark) in terminal.visible_gutter() {
        let top = origin.y + line_height * row as f32;
        let GutterMark {
            icon,
            color,
            tooltip: text,
        } = mark;
        let cell = Bounds::new(
            point(gutter_x, top),
            size(dims.cell_width * GUTTER_CELLS, line_height),
        );
        if cell.contains(&mouse) && !text.is_empty() {
            tooltip = Some((text, point(origin.x, top + line_height)));
        }
        layout.gutter.push((
            point(gutter_x, top),
            shape(&icon, font, color, font_size, window),
        ));
    }
    if let Some((text, at)) = tooltip {
        let tooltip = text_box(
            &[(text, theme.text)],
            at,
            area,
            font,
            font_size,
            line_height,
            theme,
            window,
        );
        layout.boxes.push(tooltip);
    }
    if let Some(popover) = &terminal.decorations().popover {
        layout.boxes.extend(popover_box(
            popover,
            terminal.last_content.display_offset,
            dims,
            area,
            font,
            font_size,
            theme,
            window,
        ));
    }
    layout
}

/// the popover under its range, or above it when there is no room below
#[allow(clippy::too_many_arguments)]
fn popover_box(
    popover: &Popover,
    display_offset: usize,
    dims: &TerminalBounds,
    area: Bounds<Pixels>,
    font: &Font,
    font_size: Pixels,
    theme: &Theme,
    window: &mut Window,
) -> Option<TextBox> {
    let (start, _) = popover.range;
    let row = start.line.0 + display_offset as i32;
    if row < 0 || row as usize >= dims.num_lines() || popover.content.is_empty() {
        return None;
    }
    let lines: Vec<(String, Hsla)> = popover
        .content
        .iter()
        .map(|line| {
            let text: String = line.text.chars().take(POPOVER_MAX_CHARS).collect();
            let color = if line.highlight {
                theme.text
            } else {
                theme.text_muted
            };
            (text, color)
        })
        .collect();
    let line_height = dims.line_height;
    let origin = dims.bounds.origin;
    let x = origin.x + dims.cell_width * start.column.0 as f32;
    let below = origin.y + line_height * (row + 1) as f32;
    let height = line_height * lines.len() as f32;
    let y = if below + height <= area.bottom() {
        below
    } else {
        origin.y + line_height * row as f32 - height - px(2.)
    };
    Some(text_box(
        &lines,
        point(x, y),
        area,
        font,
        font_size,
        line_height,
        theme,
        window,
    ))
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::index::{Column, Line, Point as AlacPoint};
    use gpui::{TestAppContext, font};

    use super::*;
    use crate::terminal::snippet;

    /// 20 columns by 5 lines of 5x10 cells at the window origin
    fn dims() -> TerminalBounds {
        TerminalBounds::new(
            px(10.),
            px(5.),
            Bounds::new(point(px(0.), px(0.)), size(px(100.), px(50.))),
        )
    }

    /// a peek of `lines` file lines over column 2 of `line`
    fn popover_at(line: i32, lines: u32) -> Popover {
        let at = AlacPoint::new(Line(line), Column(2));
        // a file of its own per call, tests run side by side
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file =
            std::env::temp_dir().join(format!("kuterm-popover-{}-{number}", std::process::id()));
        std::fs::write(&file, "first\nsecond\n").unwrap();
        let content = if lines == 0 {
            Vec::new()
        } else {
            snippet(&file, 1, lines - 1).unwrap()
        };
        std::fs::remove_file(&file).ok();
        assert_eq!(content.len(), lines as usize);
        Popover {
            range: (at, at),
            content,
        }
    }

    /// bounds of the popover box, none when nothing is drawn
    fn placed(
        popover: &Popover,
        display_offset: usize,
        cx: &mut TestAppContext,
    ) -> Option<Bounds<Pixels>> {
        let cx = cx.add_empty_window();
        cx.update(|window, _| {
            let dims = dims();
            popover_box(
                popover,
                display_offset,
                &dims,
                dims.bounds,
                &font("Mono"),
                px(9.),
                &Theme::default(),
                window,
            )
            .map(|text_box| text_box.bounds)
        })
    }

    #[gpui::test]
    fn popover_goes_below_its_line_or_above_when_there_is_no_room(cx: &mut TestAppContext) {
        let below = placed(&popover_at(1, 2), 0, cx).expect("no popover on screen");
        assert_eq!(below.top(), px(20.), "not right under line 1");
        // under line 3 two lines would cross the bottom, so they end above line 3
        let above = placed(&popover_at(3, 2), 0, cx).expect("no popover on screen");
        assert!(above.bottom() <= px(30.), "{above:?} covers line 3");
        assert!(above.top() >= px(0.));
    }

    #[gpui::test]
    fn popover_off_screen_or_empty_draws_nothing(cx: &mut TestAppContext) {
        // scrolled out below and above the view
        assert!(placed(&popover_at(5, 2), 0, cx).is_none());
        assert!(placed(&popover_at(-2, 2), 1, cx).is_none());
        // in history, scrolled into view
        assert!(placed(&popover_at(-2, 2), 2, cx).is_some());
        assert!(placed(&popover_at(1, 0), 0, cx).is_none());
    }
}
