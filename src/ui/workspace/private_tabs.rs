//! the look of incognito and sandboxed tabs: a frame and label on the pane and an icon of
//! their own in the tab, so nobody mistakes them for an ordinary tab

use gpui::{AnyElement, Context, div, prelude::*};

use super::{Pane, Workspace};
use crate::{terminal::Terminal, theme::Theme};

// nerd font icons of the tab
const INCOGNITO_ICON: &str = "\u{f21b}";
const SANDBOX_ICON: &str = "\u{f132}";

/// the icon a tab of `terminal` shows whatever runs in it, none for an ordinary tab
pub(super) fn private_icon(terminal: &Terminal) -> Option<String> {
    if terminal.incognito {
        Some(INCOGNITO_ICON.into())
    } else if terminal.sandboxed {
        Some(SANDBOX_ICON.into())
    } else {
        None
    }
}

/// what the label on the pane says, none for an ordinary tab
pub(super) fn frame_label(incognito: bool, sandboxed: bool) -> Option<&'static str> {
    match (incognito, sandboxed) {
        (true, true) => Some("incognito, sandboxed"),
        (true, false) => Some("incognito"),
        (false, true) => Some("sandboxed"),
        (false, false) => None,
    }
}

impl Workspace {
    /// frame and label over the pane while its active tab is incognito or sandboxed
    pub(super) fn render_profile_frame(
        &self,
        pane: &Pane,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let terminal = pane
            .tabs
            .get(pane.active)?
            .view
            .read(cx)
            .terminal()
            .read(cx);
        let label = frame_label(terminal.incognito, terminal.sandboxed)?;
        let theme = Theme::get(cx);
        // magenta for incognito like private browser windows, yellow for a sandbox
        let color = if terminal.incognito {
            theme.ansi[5]
        } else {
            theme.ansi[3]
        };
        let icon = private_icon(terminal).unwrap_or_default();
        let pane_id = pane.id;
        Some(
            div()
                .debug_selector(move || format!("pane-private-{pane_id}"))
                .absolute()
                .inset_0()
                .border_2()
                .border_color(color)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .px_2()
                        .rounded_br_md()
                        .bg(color)
                        .text_color(theme.terminal_background)
                        .text_sm()
                        .child(format!("{icon} {label}")),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::frame_label;

    #[test]
    fn label_says_what_the_tab_is() {
        assert_eq!(frame_label(false, false), None);
        assert_eq!(frame_label(true, false), Some("incognito"));
        assert_eq!(frame_label(false, true), Some("sandboxed"));
        assert_eq!(frame_label(true, true), Some("incognito, sandboxed"));
    }
}
