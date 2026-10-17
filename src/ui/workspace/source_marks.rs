//! who typed this: a gutter mark next to each command telling what typed it, so commands an
//! agent or a script ran in your shell leave a visible trail

use gpui::Hsla;

use crate::{
    settings::MarkSources,
    terminal::{GutterMark, InputSource, Sources},
    theme::Theme,
};

/// nerd font icon and what it says for each source
fn icon(source: InputSource) -> (&'static str, &'static str) {
    match source {
        InputSource::Keyboard => ("\u{f007}", "typed"),
        InputSource::Paste => ("\u{f0ea}", "pasted"),
        InputSource::Macro => ("\u{f0e7}", "typed by a palette command or key"),
        InputSource::Broadcast => ("\u{f09e}", "typed into another pane of the broadcast"),
        InputSource::Remote => ("\u{f1e6}", "sent with kuterm msg"),
        InputSource::Automation => ("\u{f013}", "typed by kuterm, like a run link or the queue"),
        InputSource::Mcp => ("\u{f06a9}", "run by an ai agent through kuterm mcp"),
    }
}

/// the source that tells most about a mix, the one furthest from the user's own hands
fn strongest(sources: Sources) -> Option<InputSource> {
    const ORDER: [InputSource; 7] = [
        InputSource::Mcp,
        InputSource::Remote,
        InputSource::Automation,
        InputSource::Broadcast,
        InputSource::Macro,
        InputSource::Paste,
        InputSource::Keyboard,
    ];
    ORDER.into_iter().find(|source| sources.contains(*source))
}

/// the mark of a block whose command `command` came from `sources`, none when `mode` shows
/// none for it
pub(super) fn source_mark(
    sources: Sources,
    command: &str,
    mode: MarkSources,
    theme: &Theme,
) -> Option<GutterMark> {
    let shown = match mode {
        MarkSources::All => !sources.is_empty(),
        // what the user typed or pasted themselves needs no mark
        MarkSources::Others => sources
            .list()
            .iter()
            .any(|source| !matches!(source, InputSource::Keyboard | InputSource::Paste)),
        MarkSources::None => false,
    };
    if !shown {
        return None;
    }
    let source = strongest(sources)?;
    let (glyph, what) = icon(source);
    let color: Hsla = match source {
        InputSource::Keyboard | InputSource::Paste => theme.text_muted,
        InputSource::Mcp => theme.attention,
        _ => theme.text,
    };
    let mut tooltip = format!("{} · {what}", sources.label());
    if !command.is_empty() {
        tooltip.push_str(&format!(": {command}"));
    }
    Some(GutterMark {
        icon: glyph.into(),
        color,
        tooltip,
    })
}

#[cfg(test)]
mod tests {
    use super::source_mark;
    use crate::{
        settings::MarkSources,
        terminal::{InputSource, Sources},
        theme::Theme,
    };

    fn sources(list: &[InputSource]) -> Sources {
        list.iter().copied().collect()
    }

    #[test]
    fn marks_tell_the_furthest_source_and_follow_the_mode() {
        let theme = Theme::default();
        let typed = sources(&[InputSource::Keyboard]);
        assert_eq!(source_mark(typed, "ls", MarkSources::Others, &theme), None);
        let mark = source_mark(typed, "ls", MarkSources::All, &theme).unwrap();
        assert_eq!(mark.icon, "\u{f007}");
        assert_eq!(mark.tooltip, "typed · typed: ls");
        let mixed = sources(&[InputSource::Keyboard, InputSource::Remote]);
        let mark = source_mark(mixed, "make", MarkSources::Others, &theme).unwrap();
        assert_eq!(mark.icon, "\u{f1e6}");
        assert!(
            mark.tooltip.starts_with("typed + remote"),
            "{}",
            mark.tooltip
        );
        let agent = sources(&[InputSource::Mcp, InputSource::Paste]);
        assert_eq!(
            source_mark(agent, "", MarkSources::Others, &theme)
                .unwrap()
                .icon,
            "\u{f06a9}"
        );
        assert_eq!(source_mark(agent, "x", MarkSources::None, &theme), None);
        assert_eq!(
            source_mark(Sources::default(), "x", MarkSources::All, &theme),
            None
        );
    }
}
