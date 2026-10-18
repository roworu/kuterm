//! wheel scrolling, its animation and the scrollbar

use super::*;

/// glide of the viewport between two history offsets
#[derive(Clone, Copy, Debug)]
pub(super) struct ScrollAnimation {
    from: f32,
    to: f32,
    started: Instant,
}

impl ScrollAnimation {
    /// offset right now and whether the glide is over
    pub(super) fn position(&self, smooth: &SmoothScrollSettings) -> (f32, bool) {
        let t = (self.started.elapsed().as_secs_f32() * 1000. / smooth.duration).min(1.);
        let offset = self.from + (self.to - self.from) * smooth.easing.apply(t);
        (offset, t >= 1.)
    }
}

impl TerminalView {
    pub(super) fn scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line_height = self
            .terminal
            .read(cx)
            .last_content
            .terminal_bounds
            .line_height;
        let multiplier = Settings::get(cx).terminal.scroll_multiplier;
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => (delta.y * multiplier) as i32,
            ScrollDelta::Pixels(delta) => {
                // accumulate touchpad pixels until they add up to whole lines
                self.scroll_px += delta.y * multiplier;
                let lines = (self.scroll_px / line_height) as i32;
                self.scroll_px -= line_height * lines as f32;
                lines
            }
        };
        if lines != 0 && event.modifiers.control {
            // one step per event, a wheel notch is several lines
            let size = self.font_size(cx) + lines.signum() as f32;
            self.set_font_size(Some(size), cx);
            return;
        }
        if lines != 0 {
            let terminal = self.terminal.read(cx);
            if terminal.owns_mouse(&event.modifiers) {
                let cell = terminal.mouse_cell(event.position);
                let button = if lines > 0 {
                    MouseButton::WheelUp
                } else {
                    MouseButton::WheelDown
                };
                self.terminal.update(cx, |term, _| {
                    for _ in 0..lines.unsigned_abs() {
                        term.report_mouse(cell, button, MouseAction::Press, &event.modifiers);
                    }
                });
                return;
            }
            if !event.modifiers.shift
                && self
                    .terminal
                    .update(cx, |term, _| term.alternate_scroll(lines))
            {
                return;
            }
            if Settings::get(cx).terminal.smooth_scroll.active() {
                let terminal = self.terminal.read(cx);
                // keep adding to the target, so fast wheel spins are not lost mid glide
                let current = self
                    .scroll_animation
                    .map_or(terminal.last_content.display_offset as f32, |a| a.to);
                let target = (current + lines as f32).clamp(0., terminal.history_size() as f32);
                self.animate_scroll(target, cx);
            } else {
                self.terminal.update(cx, |term, _| term.scroll(lines));
            }
            self.show_scrollbar(cx);
            cx.notify();
        }
    }

    // starts from where a running glide is now, so retargeting never jumps
    pub(super) fn animate_scroll(&mut self, target: f32, cx: &mut Context<Self>) {
        let smooth = Settings::get(cx).terminal.smooth_scroll;
        let from = self.scroll_animation.map_or_else(
            || self.terminal.read(cx).last_content.display_offset as f32,
            |animation| animation.position(&smooth).0,
        );
        self.scroll_animation = Some(ScrollAnimation {
            from,
            to: target,
            started: Instant::now(),
        });
    }

    pub(super) fn show_scrollbar(&mut self, cx: &mut Context<Self>) {
        self.last_scroll = Some(Instant::now());
        let auto_hide = Settings::get(cx).terminal.scrollbar.auto_hide;
        if auto_hide == 0. {
            return;
        }
        let delay = Duration::from_secs_f32(auto_hide);
        self._hide_scrollbar = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        });
    }

    /// false once auto hide kicked in
    pub(crate) fn scrollbar_visible(&self, cx: &App) -> bool {
        let auto_hide = Settings::get(cx).terminal.scrollbar.auto_hide;
        auto_hide == 0.
            || self.dragging_scrollbar
            || self
                .last_scroll
                .is_some_and(|at| at.elapsed() < Duration::from_secs_f32(auto_hide))
    }

    /// click on the scrollbar jumps there and starts a drag
    pub fn scrollbar_down(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.dragging_scrollbar = true;
        self.scrollbar_drag(offset, cx);
    }

    /// move the viewport with the thumb while the button is held
    pub fn scrollbar_drag(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.dragging_scrollbar {
            if Settings::get(cx).terminal.smooth_scroll.active() {
                self.animate_scroll(offset as f32, cx);
            } else {
                self.terminal.update(cx, |term, _| term.scroll_to(offset));
            }
            self.show_scrollbar(cx);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::ui::terminal_view::tests::*;

    #[gpui::test]
    fn ctrl_wheel_changes_font_size_of_the_view(cx: &mut TestAppContext) {
        let (view, cx) = focused_view("{}", cx);
        let settings_size = cx.update(|_, cx| Settings::get(cx).terminal.font.size);
        let wheel = |y: f32, control: bool| ScrollWheelEvent {
            delta: ScrollDelta::Lines(gpui::point(0., y)),
            modifiers: Modifiers {
                control,
                ..Modifiers::default()
            },
            ..ScrollWheelEvent::default()
        };
        let font_size =
            |cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.font_size(cx));

        view.update_in(cx, |view, window, cx| {
            view.scroll_wheel(&wheel(1., true), window, cx)
        });
        assert_eq!(font_size(cx), settings_size + 1.);
        view.update_in(cx, |view, window, cx| {
            view.scroll_wheel(&wheel(-1., true), window, cx);
            view.scroll_wheel(&wheel(-1., true), window, cx);
        });
        assert_eq!(font_size(cx), settings_size - 1.);
        // plain wheel scrolls, size stays
        view.update_in(cx, |view, window, cx| {
            view.scroll_wheel(&wheel(1., false), window, cx)
        });
        assert_eq!(font_size(cx), settings_size - 1.);
        assert_eq!(
            cx.update(|_, cx| Settings::get(cx).terminal.font.size),
            settings_size
        );
    }

    #[gpui::test]
    fn scroll_multiplier_scales_wheel_steps(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"terminal": {"scroll_multiplier": 0.5}}"#, cx);
        let start = view.read_with(cx, |view, cx| view.font_size(cx));
        // ctrl-wheel changes the size by one per event that adds up to a whole line
        let wheel = ScrollWheelEvent {
            delta: ScrollDelta::Lines(gpui::point(0., 1.)),
            modifiers: Modifiers {
                control: true,
                ..Modifiers::default()
            },
            ..ScrollWheelEvent::default()
        };
        view.update_in(cx, |view, window, cx| view.scroll_wheel(&wheel, window, cx));
        assert_eq!(view.read_with(cx, |view, cx| view.font_size(cx)), start);

        cx.update(|_, cx| {
            let mut settings = Settings::get(cx).clone();
            settings.terminal.scroll_multiplier = 3.;
            cx.set_global(settings);
        });
        view.update_in(cx, |view, window, cx| view.scroll_wheel(&wheel, window, cx));
        assert_eq!(
            view.read_with(cx, |view, cx| view.font_size(cx)),
            start + 1.
        );
    }

    #[gpui::test]
    fn touchpad_pixels_add_up_to_whole_lines(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"terminal": {"scroll_multiplier": 1}}"#, cx);
        let start = view.read_with(cx, |view, cx| view.font_size(cx));
        let line_height = view.read_with(cx, |view, cx| {
            view.terminal
                .read(cx)
                .last_content
                .terminal_bounds
                .line_height
        });
        // ctrl makes each whole line visible as one step of the font size
        let swipe = |pixels: Pixels| ScrollWheelEvent {
            delta: ScrollDelta::Pixels(gpui::point(px(0.), pixels)),
            modifiers: Modifiers {
                control: true,
                ..Modifiers::default()
            },
            ..ScrollWheelEvent::default()
        };
        let size = |cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.font_size(cx));
        view.update_in(cx, |view, window, cx| {
            view.scroll_wheel(&swipe(line_height * 0.6), window, cx)
        });
        assert_eq!(size(cx), start, "less than a line scrolled");
        view.update_in(cx, |view, window, cx| {
            view.scroll_wheel(&swipe(line_height * 0.6), window, cx)
        });
        assert_eq!(size(cx), start + 1., "two swipes did not add up to a line");
        // what was left over is kept: 0.2 + 0.6 is still no line
        view.update_in(cx, |view, window, cx| {
            view.scroll_wheel(&swipe(line_height * 0.6), window, cx)
        });
        assert_eq!(size(cx), start + 1.);
        view.update_in(cx, |view, window, cx| {
            view.scroll_wheel(&swipe(line_height * -2.), window, cx)
        });
        assert_eq!(size(cx), start);
    }
}
