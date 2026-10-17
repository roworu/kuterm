//! audible and visual bell

use super::*;

// alpha of the default bell flash, enough to see without hiding the text
const BELL_FLASH_OPACITY: f32 = 0.2;

impl TerminalView {
    // a new ring restarts the flash
    pub(super) fn ring_bell(&mut self, cx: &mut Context<Self>) {
        if Settings::get(cx).bell.visual.enable {
            self.bell_at = Some(Instant::now());
            cx.notify();
        }
    }

    // fades with each frame until the duration is over
    pub(super) fn bell_flash(&mut self, cx: &App) -> Option<Hsla> {
        let flash = self.bell_color(cx);
        if flash.is_none() {
            self.bell_at = None;
        }
        flash
    }

    /// current color of the fading bell flash, none once it is over
    pub fn bell_color(&self, cx: &App) -> Option<Hsla> {
        let rang = self.bell_at?;
        let visual = Settings::get(cx).bell.visual;
        let t = rang.elapsed().as_secs_f32() * 1000. / visual.duration;
        // a zero duration gives nan or infinity, which shows no flash
        if !visual.enable || t.is_nan() || t >= 1. {
            return None;
        }
        let color = visual.color.unwrap_or_else(|| {
            let theme = self.terminal.read(cx).theme(cx);
            theme.terminal_foreground.opacity(BELL_FLASH_OPACITY)
        });
        Some(color.opacity(1. - t))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::ui::terminal_view::tests::*;

    #[gpui::test]
    fn bell_starts_a_flash_only_when_visual_bell_is_on(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"bell": {"visual": {"enable": true}}}"#, cx);
        let terminal = view.read_with(cx, |view, _| view.terminal.clone());
        terminal.update(cx, |_, cx| cx.emit(Event::Bell));
        assert!(view.read_with(cx, |view, _| view.bell_at.is_some()));

        cx.update(|_, cx| {
            let mut settings = Settings::get(cx).clone();
            settings.bell.visual.enable = false;
            cx.set_global(settings);
        });
        view.update(cx, |view, _| view.bell_at = None);
        terminal.update(cx, |_, cx| cx.emit(Event::Bell));
        assert!(view.read_with(cx, |view, _| view.bell_at.is_none()));
    }

    #[gpui::test]
    fn bell_flash_fades_and_ends(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(
            r##"{"bell": {"visual": {"enable": true, "color": "#ff0000", "duration": 200}}}"##,
            cx,
        );
        let flash = |cx: &mut VisualTestContext| view.update(cx, |view, cx| view.bell_flash(cx));
        view.update(cx, |view, _| view.bell_at = Some(Instant::now()));
        let first = flash(cx).expect("flash right after the bell");
        assert!(first.a > 0.5 && first.a <= 1.);
        view.update(cx, |view, _| {
            view.bell_at = Some(Instant::now() - Duration::from_millis(150))
        });
        let later = flash(cx).expect("flash before the duration ends");
        assert!(later.a < first.a && later.a > 0.);
        view.update(cx, |view, _| {
            view.bell_at = Some(Instant::now() - Duration::from_millis(250))
        });
        assert_eq!(flash(cx), None);
        assert!(view.read_with(cx, |view, _| view.bell_at.is_none()));
    }
}
