//! picking the tab icon from the profile, the running program or settings

use std::path::Path;

use crate::{
    settings::{TabIconSettings, TabIcons},
    terminal::{ForegroundProcess, children, process_info},
};

// sudo runs the command under a second sudo, a few levels cover every wrapper chain seen in practice
const MAX_WRAPPER_DEPTH: usize = 4;

/// icon of a tab running `process` in the foreground, may block on /proc reads
pub(super) fn tab_icon(
    settings: &TabIconSettings,
    icons: &TabIcons,
    profile_icon: Option<&str>,
    process: Option<ForegroundProcess>,
) -> String {
    if let Some(icon) = profile_icon {
        return icon.to_owned();
    }
    process
        .filter(|_| settings.dynamic)
        .map(|process| unwrap(icons, process))
        .and_then(|process| program_icon(icons, &process))
        .unwrap_or(&settings.default)
        .to_owned()
}

// the command a wrapper runs is its child, found through /proc rather than by parsing
// each wrapper's own options, like "-u root" in "sudo -u root htop"
fn unwrap(icons: &TabIcons, mut process: ForegroundProcess) -> ForegroundProcess {
    for _ in 0..MAX_WRAPPER_DEPTH {
        if !icons.wrappers.contains(&process.name) {
            break;
        }
        // a wrapper without a child, like sudo asking for a password, keeps its own icon
        let Some(child) = children(process.pid).into_iter().find_map(process_info) else {
            break;
        };
        process = child;
    }
    process
}

fn program_icon<'a>(icons: &'a TabIcons, process: &ForegroundProcess) -> Option<&'a str> {
    // scripts run by an interpreter show as "node" or "python", so their script name is tried first
    let script = icons
        .interpreters
        .contains(&process.name)
        .then(|| process.args.iter().find(|arg| !arg.starts_with('-')))
        .flatten()
        .and_then(|script| Path::new(script).file_stem())
        .map(|stem| stem.to_string_lossy().into_owned());
    script
        .iter()
        .chain([&process.name])
        .find_map(|name| icons.icon(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::terminal::foreground_process;
    use crate::terminal::{Kill, spawn};

    fn settings(json: &str) -> TabIconSettings {
        Settings::parse(json).unwrap().icons
    }

    fn icon_of(icons: &TabIcons, command: &str) -> String {
        let mut argv = command.split(' ').map(String::from);
        let process = ForegroundProcess {
            pid: 0,
            name: argv.next().unwrap(),
            args: argv.collect(),
            cwd: None,
        };
        program_icon(icons, &process)
            .unwrap_or(&settings("{}").default)
            .to_owned()
    }

    /// icons with one group giving the name of this test process `icon`
    fn icons_for_self(icon: &str) -> TabIcons {
        let process = foreground_process(std::process::id()).unwrap();
        let json = format!(
            r#"{{"groups": [{{"icon": "{icon}", "commands": ["{}"]}}]}}"#,
            process.name
        );
        TabIcons::parse(&json).unwrap()
    }

    /// icon table made for tests, so edits of the bundled file don't break them
    fn icons() -> TabIcons {
        TabIcons::parse(
            r#"{
                "interpreters": ["node", "python3"],
                "groups": [
                    {"icon": "R", "commands": ["ssh", "telnet"]},
                    {"icon": "A", "commands": ["claude", "pi", "codex", "aider"]},
                    {"icon": "M", "commands": ["htop", "btop"]},
                ],
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn unknown_programs_use_default() {
        let default = settings("{}").default;
        assert_eq!(icon_of(&icons(), "bash"), default);
        assert_eq!(icon_of(&icons(), "some-unknown-program --flag"), default);
    }

    #[test]
    fn programs_get_their_group_icon() {
        for (command, icon) in [
            ("ssh user@host", "R"),
            ("telnet host 23", "R"),
            ("claude", "A"),
            ("pi", "A"),
            ("htop", "M"),
            ("btop -u 1", "M"),
        ] {
            assert_eq!(icon_of(&icons(), command), icon, "{command}");
        }
    }

    #[test]
    fn scripts_are_matched_through_their_interpreter() {
        let default = settings("{}").default;
        let icons = icons();
        assert_eq!(icon_of(&icons, "node /usr/lib/node_modules/.bin/pi"), "A");
        assert_eq!(icon_of(&icons, "node --no-warnings /usr/bin/codex.js"), "A");
        assert_eq!(icon_of(&icons, "python3 -m aider"), "A");
        assert_eq!(icon_of(&icons, "node server.js"), default);
        assert_eq!(icon_of(&icons, "node"), default);
        // arguments of other programs are never matched
        assert_eq!(icon_of(&icons, "vim ssh"), default);
        assert_eq!(icon_of(&icons, "ruby /usr/bin/pi"), default);
    }

    #[test]
    fn script_wins_over_interpreter_group() {
        let icons = TabIcons::parse(
            r#"{"interpreters": ["node"],
                "groups": [{"icon": "N", "commands": ["node"]}, {"icon": "A", "commands": ["pi"]}]}"#,
        )
        .unwrap();
        assert_eq!(icon_of(&icons, "node /usr/bin/pi"), "A");
        assert_eq!(icon_of(&icons, "node app.js"), "N");
        assert_eq!(icon_of(&icons, "node"), "N");
    }

    #[test]
    fn profile_icon_is_fixed() {
        // even when the running program has an icon of its own
        let icons = icons_for_self("X");
        let settings = settings("{}");
        let pid = std::process::id();
        assert_eq!(
            tab_icon(&settings, &icons, None, foreground_process(pid)),
            "X"
        );
        assert_eq!(
            tab_icon(&settings, &icons, Some("P"), foreground_process(pid)),
            "P"
        );
        assert_eq!(
            tab_icon(
                &settings,
                &icons,
                Some("P"),
                foreground_process(u32::MAX - 1)
            ),
            "P"
        );
    }

    #[test]
    fn disabled_dynamic_icons_use_default() {
        let settings = settings(r#"{"icons": {"dynamic": false, "default": "D"}}"#);
        assert_eq!(
            tab_icon(
                &settings,
                &icons_for_self("X"),
                None,
                foreground_process(std::process::id())
            ),
            "D"
        );
    }

    #[test]
    fn missing_process_uses_default() {
        let settings = settings(r#"{"icons": {"default": "D"}}"#);
        assert_eq!(
            tab_icon(
                &settings,
                &TabIcons::default(),
                None,
                foreground_process(u32::MAX - 1)
            ),
            "D"
        );
    }

    fn icons_with_wrappers(wrappers: &str) -> TabIcons {
        let json = format!(
            r#"{{"wrappers": {wrappers}, "groups": [
                {{"icon": "W", "commands": ["sh"]}},
                {{"icon": "S", "commands": ["sleep"]}},
            ]}}"#
        );
        TabIcons::parse(&json).unwrap()
    }

    fn unwrapped_icon(icons: &TabIcons, pid: u32) -> String {
        let process = unwrap(icons, process_info(pid).unwrap());
        program_icon(icons, &process).unwrap().to_owned()
    }

    #[test]
    fn wrapped_commands_show_their_own_icon() {
        // the trailing ":" keeps sh from replacing itself with its command
        let sh = spawn("sleep 10; :", 1);
        assert_eq!(
            unwrapped_icon(&icons_with_wrappers(r#"["sh"]"#), sh.0.id()),
            "S"
        );
        // wrappers inside wrappers, like sudo under sudo
        let nested = spawn(r#"sh -c "sleep 10; :"; :"#, 2);
        assert_eq!(
            unwrapped_icon(&icons_with_wrappers(r#"["sh"]"#), nested.0.id()),
            "S"
        );
    }

    #[test]
    fn other_programs_are_not_unwrapped() {
        let sh = spawn("sleep 10; :", 1);
        assert_eq!(unwrapped_icon(&icons_with_wrappers("[]"), sh.0.id()), "W");
    }

    #[test]
    fn wrapper_without_child_keeps_its_icon() {
        let sleep = Kill(
            std::process::Command::new("sleep")
                .arg("10")
                .spawn()
                .unwrap(),
        );
        let icons = icons_with_wrappers(r#"["sleep"]"#);
        assert_eq!(unwrapped_icon(&icons, sleep.0.id()), "S");
    }

    #[test]
    fn wrapped_script_goes_through_its_interpreter() {
        // sh wraps "bash .../pi", which is matched by its script name
        let dir = std::env::temp_dir().join(format!("kuterm_wrap_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("pi");
        std::fs::write(&script, "sleep 10\n:\n").unwrap();
        let wrapper = spawn(&format!("bash {}; :", script.display()), 1);
        let icons = TabIcons::parse(
            r#"{"wrappers": ["sh"], "interpreters": ["bash"], "groups": [{"icon": "A", "commands": ["pi"]}]}"#,
        )
        .unwrap();
        let child = unwrap(&icons, process_info(wrapper.0.id()).unwrap());
        assert_eq!(child.args.first().map(String::as_str), script.to_str());
        assert_eq!(program_icon(&icons, &child), Some("A"));
        drop(wrapper);
        std::fs::remove_dir_all(&dir).ok();
    }
}
