//! painting cursor in each shape

use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
    App, Bounds, Hsla, Pixels, Point, ShapedLine, TextAlign, Window, fill, outline, point, px, size,
};

pub(super) struct CursorLayout {
    pub(super) bounds: Bounds<Pixels>,
    pub(super) shape: CursorShape,
    pub(super) color: Hsla,
    pub(super) focused: bool,
    pub(super) text: ShapedLine,
    /// glyph offset from settings, so the glyph under a block matches the grid
    pub(super) text_offset: Point<Pixels>,
}

impl CursorLayout {
    pub(super) fn paint(&self, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let bounds = self.bounds + origin;
        let color = self.color;
        match self.shape {
            CursorShape::Block if self.focused => {
                window.paint_quad(fill(bounds, color));
                self.text
                    .paint(
                        bounds.origin + self.text_offset,
                        bounds.size.height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
            }
            CursorShape::Beam if self.focused => {
                window.paint_quad(fill(
                    Bounds::new(bounds.origin, size(px(2.), bounds.size.height)),
                    color,
                ));
            }
            CursorShape::Underline if self.focused => {
                let origin = point(bounds.origin.x, bounds.bottom() - px(2.));
                window.paint_quad(fill(
                    Bounds::new(origin, size(bounds.size.width, px(2.))),
                    color,
                ));
            }
            // unfocused cursors are drawn hollow
            // TODO: do we need a settings here to change its begaviour?
            _ => window.paint_quad(outline(bounds, color, gpui::BorderStyle::Solid)),
        }
    }
}
