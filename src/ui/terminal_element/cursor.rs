//! painting cursor in each shape

use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
    App, Bounds, Hsla, Pixels, Point, ShapedLine, TextAlign, Window, fill, outline, point, size,
};

use crate::settings::CursorUnfocused;

pub(super) struct CursorLayout {
    pub(super) bounds: Bounds<Pixels>,
    pub(super) shape: CursorShape,
    pub(super) color: Hsla,
    pub(super) focused: bool,
    /// how the cursor looks while not focused
    pub(super) unfocused: CursorUnfocused,
    /// width of a bar, height of an underline
    pub(super) thickness: Pixels,
    pub(super) text: ShapedLine,
    /// glyph offset from settings, so the glyph under a block matches the grid
    pub(super) text_offset: Point<Pixels>,
}

impl CursorLayout {
    /// shape drawn right now, none when nothing is drawn
    fn painted_shape(&self) -> Option<CursorShape> {
        if self.focused {
            return Some(self.shape);
        }
        match self.unfocused {
            CursorUnfocused::Hollow => Some(CursorShape::HollowBlock),
            CursorUnfocused::Same => Some(self.shape),
            CursorUnfocused::None => None,
        }
    }

    pub(super) fn paint(&self, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let bounds = self.bounds + origin;
        let color = self.color;
        let thickness = self.thickness;
        match self.painted_shape() {
            None => {}
            Some(CursorShape::Block) => {
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
            Some(CursorShape::Beam) => {
                window.paint_quad(fill(
                    Bounds::new(bounds.origin, size(thickness, bounds.size.height)),
                    color,
                ));
            }
            Some(CursorShape::Underline) => {
                let origin = point(bounds.origin.x, bounds.bottom() - thickness);
                window.paint_quad(fill(
                    Bounds::new(origin, size(bounds.size.width, thickness)),
                    color,
                ));
            }
            Some(_) => window.paint_quad(outline(bounds, color, gpui::BorderStyle::Solid)),
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, px};

    use super::*;

    fn layout(shape: CursorShape, focused: bool, unfocused: CursorUnfocused) -> CursorLayout {
        CursorLayout {
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(10.), px(20.))),
            shape,
            color: gpui::red(),
            focused,
            unfocused,
            thickness: px(2.),
            text: ShapedLine::default(),
            text_offset: point(px(0.), px(0.)),
        }
    }

    #[test]
    fn unfocused_setting_picks_the_painted_shape() {
        let painted =
            |focused, unfocused| layout(CursorShape::Beam, focused, unfocused).painted_shape();
        for unfocused in [
            CursorUnfocused::Hollow,
            CursorUnfocused::Same,
            CursorUnfocused::None,
        ] {
            assert_eq!(painted(true, unfocused), Some(CursorShape::Beam));
        }
        assert_eq!(
            painted(false, CursorUnfocused::Hollow),
            Some(CursorShape::HollowBlock)
        );
        assert_eq!(
            painted(false, CursorUnfocused::Same),
            Some(CursorShape::Beam)
        );
        assert_eq!(painted(false, CursorUnfocused::None), None);
    }
}
