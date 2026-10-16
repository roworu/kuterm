//! drawing block decorations next to the grid: gutter marks with their tooltip, chips and
//! the popover. laid out in prepaint from plain data the terminal keeps

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
    pub chips: Vec<TextBox>,
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
    for (row, chips) in terminal.visible_chips() {
        let text = chips
            .iter()
            .map(|chip| chip.text.as_str())
            .collect::<Vec<_>>()
            .join("  ");
        let top = origin.y + line_height * row as f32;
        let mut chip = text_box(
            &[(text, theme.text_muted)],
            point(dims.bounds.right(), top),
            area,
            font,
            font_size,
            line_height,
            theme,
            window,
        );
        chip.bounds.origin.y = top;
        layout.chips.push(chip);
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
