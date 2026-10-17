//! one line of text cut at its end with an ellipsis when its final width is too small

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, ShapedLine, SharedString, Style, TextAlign, Window, px,
};

const ELLIPSIS: &str = "…";

/// longest start of `text` that fits with an ellipsis after it, or all of it when it fits.
/// `fits` must be monotonic: a shorter text never needs more room than a longer one
fn truncate_end(text: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(text) {
        return text.to_string();
    }
    let cut = |end: usize| format!("{}{ELLIPSIS}", text[..end].trim_end());
    // char boundaries, so multi byte chars are never split
    let ends: Vec<usize> = text.char_indices().map(|(ix, _)| ix).collect();
    // binary search for the last end whose cut still fits, the empty cut is the fallback
    let (mut lo, mut hi) = (0, ends.len());
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if fits(&cut(ends[mid])) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    cut(ends[lo])
}

/// text that lays out at its full width, so flex rows size and align it like plain text,
/// and is cut to its final bounds when painted.
/// gpui's own text truncation cuts to whatever width the last layout probe asked about,
/// which is often narrower than the final one
pub(super) struct EndTruncated {
    text: SharedString,
}

impl EndTruncated {
    pub(super) fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl IntoElement for EndTruncated {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

fn shape(text: &str, window: &Window) -> ShapedLine {
    let style = window.text_style();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let text = SharedString::from(text.to_string());
    let run = style.to_run(text.len());
    window
        .text_system()
        .shape_line(text, font_size, &[run], None)
}

impl Element for EndTruncated {
    type RequestLayoutState = ();
    type PrepaintState = ShapedLine;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let full = shape(&self.text, window);
        let height = window.text_style().line_height_in_pixels(window.rem_size());
        let mut style = Style::default();
        style.size.width = full.width.into();
        style.size.height = height.into();
        // shrinks below its text, the cut happens in prepaint
        style.min_size.width = px(0.).into();
        style.flex_shrink = 1.;
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> ShapedLine {
        // a fraction of a pixel is layout rounding, not a text that does not fit
        let width = bounds.size.width + px(0.5);
        let text = truncate_end(&self.text, |text| shape(text, window).width <= width);
        shape(&text, window)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        line: &mut ShapedLine,
        window: &mut Window,
        cx: &mut App,
    ) {
        line.paint(
            bounds.origin,
            bounds.size.height,
            TextAlign::Left,
            None,
            window,
            cx,
        )
        .ok();
    }
}

#[cfg(test)]
mod tests {

    use gpui::{
        Bounds, Context, IntoElement, Pixels, Render, TestAppContext, VisualTestContext, Window,
        div, prelude::*, px, size,
    };

    use super::{ELLIPSIS, EndTruncated, truncate_end};

    // ---------- truncate_end, pure ----------

    /// char widths like a proportional font: wide letters, cjk and emoji take more room
    fn char_width(c: char) -> usize {
        match c {
            'W' | 'M' | 'm' | 'w' => 3,
            'i' | 'l' | '.' | ' ' => 1,
            c if c.len_utf8() >= 3 => 4,
            _ => 2,
        }
    }

    fn prop(width: usize) -> impl Fn(&str) -> bool {
        move |text: &str| text.chars().map(char_width).sum::<usize>() <= width
    }

    fn mono(width: usize) -> impl Fn(&str) -> bool {
        move |text: &str| text.chars().count() <= width
    }

    /// every cut that keeps a start of `text`, found by trying all char ends
    fn best_cut(text: &str, fits: &dyn Fn(&str) -> bool) -> String {
        if fits(text) {
            return text.to_string();
        }
        text.char_indices()
            .map(|(ix, _)| ix)
            .map(|end| format!("{}{ELLIPSIS}", text[..end].trim_end()))
            .filter(|cut| fits(cut))
            .max_by_key(|cut| cut.len())
            .unwrap_or_else(|| ELLIPSIS.to_string())
    }

    fn check_all_widths(text: &str, fits_for: impl Fn(usize) -> Box<dyn Fn(&str) -> bool>) {
        let total: usize = text.chars().map(char_width).sum::<usize>() + 5;
        for width in 0..=total {
            let fits = fits_for(width);
            let cut = truncate_end(text, &*fits);
            if fits(text) {
                assert_eq!(cut, text, "width {width}: fitting text changed");
                continue;
            }
            let kept = cut
                .strip_suffix(ELLIPSIS)
                .unwrap_or_else(|| panic!("width {width}: {cut:?} has no ellipsis"));
            assert!(
                text.starts_with(kept),
                "width {width}: {cut:?} not a start of {text:?}"
            );
            assert_eq!(
                kept,
                kept.trim_end(),
                "width {width}: space before ellipsis in {cut:?}"
            );
            if width > 0 && fits(ELLIPSIS) {
                assert!(fits(&cut), "width {width}: {cut:?} does not fit");
            }
            assert_eq!(
                cut,
                best_cut(text, &*fits),
                "width {width}: not the longest cut"
            );
        }
    }

    #[test]
    fn fitting_text_is_kept_whole_without_ellipsis() {
        assert_eq!(truncate_end("abcdef", mono(6)), "abcdef");
        assert_eq!(truncate_end("abcdef", mono(100)), "abcdef");
        assert_eq!(truncate_end("a", mono(1)), "a");
        assert_eq!(truncate_end("", mono(0)), "");
    }

    #[test]
    fn every_width_gives_the_longest_start_mono() {
        for text in ["a long title with several words in it", "x  y  z  w", "ab"] {
            check_all_widths(text, |w| Box::new(mono(w)));
        }
    }

    #[test]
    fn every_width_gives_the_longest_start_proportional() {
        for text in [
            "WWW iii mmm lll www",
            "Mixed wIdTh title.with.dots",
            "日本語のタブの名前です長い",
            "emoji 🚀🚀 in a title 🦀",
            "café naïve résumé",
        ] {
            check_all_widths(text, |w| Box::new(prop(w)));
        }
    }

    #[test]
    fn multi_byte_chars_are_never_split() {
        for text in ["日本語テキスト", "🚀🦀🚀🦀🚀", "ééééééé", "aé日🚀b"]
        {
            for width in 0..20 {
                // slicing inside a char would panic, so reaching the asserts is the check
                let cut = truncate_end(text, mono(width));
                let kept = cut.strip_suffix(ELLIPSIS).unwrap_or(&cut);
                assert!(text.starts_with(kept));
            }
        }
        assert_eq!(truncate_end("日本語テキスト", mono(4)), "日本語…");
        assert_eq!(truncate_end("🚀🦀🚀🦀🚀", mono(3)), "🚀🦀…");
    }

    #[test]
    fn trailing_spaces_before_ellipsis_are_trimmed() {
        assert_eq!(truncate_end("some folder name", mono(6)), "some…");
        assert_eq!(truncate_end("some     folder", mono(9)), "some…");
        assert_eq!(truncate_end("ab\t\tcdefgh", mono(5)), "ab…");
    }

    #[test]
    fn leading_spaces_are_kept_since_only_the_end_is_cut() {
        let cut = truncate_end("   title here", mono(6));
        assert!(cut.starts_with("   "), "{cut:?}");
        assert!(cut.ends_with(ELLIPSIS));
    }

    #[test]
    fn spaces_only_text_cut_to_ellipsis() {
        assert_eq!(truncate_end("          ", mono(3)), ELLIPSIS);
    }

    #[test]
    fn nothing_fits_gives_ellipsis_alone() {
        assert_eq!(truncate_end("title", mono(0)), ELLIPSIS);
        assert_eq!(truncate_end("title", mono(1)), ELLIPSIS);
        assert_eq!(truncate_end("title", |_| false), ELLIPSIS);
        assert_eq!(truncate_end("日本", |_| false), ELLIPSIS);
    }

    #[test]
    fn single_char_text_that_does_not_fit() {
        assert_eq!(truncate_end("W", prop(2)), ELLIPSIS);
        assert_eq!(truncate_end("日", mono(0)), ELLIPSIS);
    }

    // ---------- the element, laid out in a window ----------

    /// one EndTruncated title inside a container of a given width
    struct Row {
        text: String,
        width: Pixels,
    }

    impl Render for Row {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().child(
                div().flex().w(self.width).h(px(30.)).child(
                    div()
                        .debug_selector(|| "title".into())
                        .flex()
                        .min_w_0()
                        .overflow_hidden()
                        .child(EndTruncated::new(self.text.clone())),
                ),
            )
        }
    }

    fn open<'a>(
        text: &str,
        width: f32,
        cx: &'a mut TestAppContext,
    ) -> (gpui::Entity<Row>, &'a mut VisualTestContext) {
        let text = text.to_string();
        let (row, cx) = cx.add_window_view(move |_, _| Row {
            text,
            width: px(width),
        });
        cx.simulate_resize(size(px(1000.), px(600.)));
        cx.run_until_parked();
        (row, cx)
    }

    fn title(cx: &mut VisualTestContext) -> Bounds<Pixels> {
        cx.debug_bounds("title").expect("no title drawn")
    }

    const LONG: &str =
        "a much much much much much much much much much much longer title than the row";

    #[gpui::test]
    fn short_text_lays_out_at_its_full_width(cx: &mut TestAppContext) {
        let (_, cx) = open("tab", 800., cx);
        let short = title(cx);
        assert!(short.size.width > px(0.), "empty title {short:?}");
        assert!(
            short.size.width < px(800.),
            "short title filled the row {short:?}"
        );
        assert!(short.size.height > px(0.));

        // a longer text is wider when both fit, so the width follows the text, not the row
        let (_, cx) = open("tab tab tab", 800., cx);
        assert!(title(cx).size.width > short.size.width);
    }

    #[gpui::test]
    fn long_text_shrinks_to_the_row(cx: &mut TestAppContext) {
        let (_, cx) = open(LONG, 800., cx);
        let full = title(cx).size.width;
        assert!(
            full > px(150.),
            "full text too narrow for the test: {full:?}"
        );
        for width in [150., 80., 20.] {
            let (_, cx) = open(LONG, width, cx);
            let b = title(cx);
            assert!(
                (b.size.width - px(width)).abs() <= px(1.),
                "row {width}: title is {b:?}"
            );
        }
    }

    #[gpui::test]
    fn zero_width_row_does_not_panic(cx: &mut TestAppContext) {
        let (_, cx) = open(LONG, 0., cx);
        assert!(title(cx).size.width <= px(1.));
        let (_, cx) = open("", 0., cx);
        assert!(title(cx).size.width <= px(1.));
    }

    #[gpui::test]
    fn empty_and_multi_byte_text_render(cx: &mut TestAppContext) {
        for text in ["", "日本語のタブの名前です長い", "🚀 rocket", "café"] {
            for width in [5., 40., 800.] {
                let (_, cx) = open(text, width, cx);
                let b = title(cx);
                assert!(
                    b.size.width <= px(width) + px(1.),
                    "{text:?} in {width}: {b:?}"
                );
            }
        }
    }

    #[gpui::test]
    fn bounds_are_stable_over_redraws(cx: &mut TestAppContext) {
        let (row, cx) = open(LONG, 150., cx);
        let first = title(cx);
        for _ in 0..5 {
            row.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            assert_eq!(title(cx), first);
        }
    }

    #[gpui::test]
    fn width_follows_a_resized_row(cx: &mut TestAppContext) {
        let (row, cx) = open(LONG, 300., cx);
        for width in [120., 60., 300., 2000.] {
            row.update(cx, |row, cx| {
                row.width = px(width);
                cx.notify();
            });
            cx.run_until_parked();
            let b = title(cx);
            assert!(b.size.width <= px(width) + px(1.), "row {width}: {b:?}");
        }
        // in a huge row the text is back to its full width, not stuck at an older cut
        let full = title(cx).size.width;
        assert!(full > px(300.), "text did not grow back: {full:?}");
    }
}
