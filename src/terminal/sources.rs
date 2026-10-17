//! input sources: what wrote into a pty, the user at the keyboard or something on their
//! behalf, so a block knows what typed its command and secret prompts can refuse automation

use serde::{Deserialize, Serialize};

/// what wrote input into a terminal
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    Keyboard,
    Paste,
    /// a command palette command or keymap entry typing text
    Macro,
    /// typed into another pane joined to the broadcast
    Broadcast,
    /// `kuterm msg send-text` or an allowed `run`
    Remote,
    /// kuterm on its own, like a run link or the command queue
    Automation,
    /// an ai agent through `kuterm mcp`
    Mcp,
}

const ALL: [InputSource; 7] = [
    InputSource::Keyboard,
    InputSource::Paste,
    InputSource::Macro,
    InputSource::Broadcast,
    InputSource::Remote,
    InputSource::Automation,
    InputSource::Mcp,
];

impl InputSource {
    /// false for sources that must not type into a program asking for a secret, since the
    /// user can't see what they type
    pub fn may_answer_secret(self) -> bool {
        matches!(self, InputSource::Keyboard | InputSource::Paste)
    }

    /// word for the source, like "typed"
    pub fn label(self) -> &'static str {
        match self {
            InputSource::Keyboard => "typed",
            InputSource::Paste => "pasted",
            InputSource::Macro => "macro",
            InputSource::Broadcast => "broadcast",
            InputSource::Remote => "remote",
            InputSource::Automation => "automation",
            InputSource::Mcp => "mcp",
        }
    }

    fn bit(self) -> u8 {
        1 << ALL.iter().position(|source| *source == self).unwrap_or(0)
    }
}

/// the sources that wrote since some point, in no order
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Sources(u8);

impl Sources {
    pub fn insert(&mut self, source: InputSource) {
        self.0 |= source.bit();
    }

    pub fn contains(self, source: InputSource) -> bool {
        self.0 & source.bit() != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// every source in it, keyboard first
    pub fn list(self) -> Vec<InputSource> {
        ALL.into_iter()
            .filter(|source| self.contains(*source))
            .collect()
    }

    /// the mix like "typed + pasted", empty when nothing wrote
    pub fn label(self) -> String {
        self.list()
            .into_iter()
            .map(InputSource::label)
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

impl FromIterator<InputSource> for Sources {
    fn from_iter<I: IntoIterator<Item = InputSource>>(iter: I) -> Self {
        let mut sources = Sources::default();
        for source in iter {
            sources.insert(source);
        }
        sources
    }
}

#[cfg(test)]
mod tests {
    use super::{InputSource, Sources};

    #[test]
    fn sources_mix_and_name_themselves() {
        let mut sources = Sources::default();
        assert!(sources.is_empty());
        assert_eq!(sources.label(), "");
        sources.insert(InputSource::Paste);
        sources.insert(InputSource::Keyboard);
        sources.insert(InputSource::Paste);
        assert_eq!(sources.label(), "typed + pasted");
        assert!(!sources.contains(InputSource::Mcp));
        let remote: Sources = [InputSource::Mcp, InputSource::Remote]
            .into_iter()
            .collect();
        assert_eq!(remote.list(), [InputSource::Remote, InputSource::Mcp]);
        assert!(InputSource::Keyboard.may_answer_secret());
        assert!(InputSource::Paste.may_answer_secret());
        for source in [
            InputSource::Macro,
            InputSource::Broadcast,
            InputSource::Remote,
            InputSource::Automation,
            InputSource::Mcp,
        ] {
            assert!(!source.may_answer_secret(), "{source:?}");
        }
    }

    #[test]
    fn blocks_know_what_typed_their_command() {
        use std::time::{Duration, Instant};

        use crate::{
            settings::{Shell, TerminalSettings},
            terminal::{
                tests::{profile, spawn_with, wait_for_text},
                with_shell_integration,
            },
        };

        let bash = with_shell_integration(&profile(Shell::Program("/bin/bash".into())));
        let mut builder = spawn_with(&TerminalSettings::default(), &bash);
        let terminal = &mut builder.terminal;
        // what is typed before the first prompt is typeahead, owned by no block
        let deadline = Instant::now() + Duration::from_secs(15);
        while !terminal.at_prompt() {
            assert!(Instant::now() < deadline, "bash never showed a prompt");
            terminal.handle_scan_events();
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut run = |source: InputSource, text: &str, until: &str| {
            terminal.input(source, format!("{text}\r").into_bytes());
            wait_for_text(terminal, until);
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                terminal.sync();
                terminal.handle_scan_events();
                let last = terminal.blocks().back().cloned();
                if let Some(block) = last.filter(|block| block.command.contains(until)) {
                    return block.source;
                }
                assert!(Instant::now() < deadline, "no block for {text}");
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        let typed = run(InputSource::Keyboard, "echo k_$((1+1)) k_2", "k_2");
        assert_eq!(typed.list(), [InputSource::Keyboard]);
        let remote = run(InputSource::Remote, "echo r_$((1+2)) r_3", "r_3");
        assert_eq!(remote.list(), [InputSource::Remote]);
    }
}
