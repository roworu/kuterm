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
        Settings::parse(json).unwrap().tab_icon
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
        let settings = settings(r#"{"tab_icon": {"dynamic": false, "default": "D"}}"#);
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
        let settings = settings(r#"{"tab_icon": {"default": "D"}}"#);
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

#[cfg(test)]
mod tab_icon_logic {

    use std::{
        collections::HashSet,
        os::unix::process::CommandExt,
        path::{Path, PathBuf},
        process::{Child, Command},
        time::{Duration, Instant},
    };

    use super::tab_icon;
    use crate::{
        settings::{Settings, TabIconPosition, TabIconSettings, TabIcons},
        terminal::foreground_process,
    };

    const DEFAULT: &str = "D";
    const REMOTE: &str = "R";
    const AI: &str = "A";
    const MONITOR: &str = "M";

    /// tab icon settings with every key set explicitly
    fn tab_settings(dynamic: bool, position: &str, default: &str) -> TabIconSettings {
        let json = format!(
            r#"{{"tab_icon": {{"dynamic": {dynamic}, "position": "{position}", "default": "{default}"}}}}"#
        );
        Settings::parse(&json).unwrap().tab_icon
    }

    fn dynamic() -> TabIconSettings {
        tab_settings(true, "left", DEFAULT)
    }

    /// test groups and interpreters, both replacing the bundled ones whole
    fn fixture() -> TabIcons {
        TabIcons::parse(
            r#"{
            "interpreters": ["node", "python3"],
            "wrappers": [],
            "groups": [
                {"icon": "R", "commands": ["ssh", "telnet", "mosh"]},
                {"icon": "A", "commands": ["claude", "pi", "codex", "aider"]},
                {"icon": "M", "commands": ["htop", "btop", "top"]},
            ],
        }"#,
        )
        .unwrap()
    }

    fn groups(json: &str) -> TabIcons {
        TabIcons::parse(json).unwrap()
    }

    /// killed on drop so failed asserts leave no processes behind
    struct Proc(Child);

    impl Drop for Proc {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    impl Proc {
        fn pid(&self) -> u32 {
            self.0.id()
        }
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("agent_tab_icon_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// start a process whose argv is `argv0 args..`, running `program`, once /proc shows argv0
    fn spawn(program: &str, argv0: &str, args: &[&str]) -> Proc {
        let mut command = Command::new(program);
        command.arg0(argv0).args(args);
        let child = Proc(command.spawn().unwrap());
        let want = Path::new(argv0)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if foreground_process(child.pid()).is_some_and(|p| p.name == want) {
                return child;
            }
            assert!(Instant::now() < deadline, "{argv0} never showed in /proc");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// a long running process named `name`
    fn named(name: &str) -> Proc {
        spawn("sleep", name, &["30"])
    }

    /// a shell script at `dir/file` run as `interpreter dir/file`
    fn script(interpreter: &str, file: &str) -> Proc {
        let dir = tmp_dir(interpreter);
        let path = dir.join(file);
        // the trailing "true" keeps sh from exec'ing sleep, so the argv stays
        std::fs::write(&path, "sleep 30; true\n").unwrap();
        spawn("/bin/sh", interpreter, &[path.to_str().unwrap()])
    }

    fn icon_pid(settings: &TabIconSettings, icons: &TabIcons, pid: u32) -> String {
        tab_icon(settings, icons, None, foreground_process(pid))
    }

    fn icon_name(settings: &TabIconSettings, icons: &TabIcons, name: &str) -> String {
        let p = named(name);
        icon_pid(settings, icons, p.pid())
    }

    /// icon of `name` with dynamic icons on, default "D" and the given icons
    fn icon_of(icons: &TabIcons, name: &str) -> String {
        icon_name(&dynamic(), icons, name)
    }

    // settings

    #[test]
    fn tab_icon_settings_are_parsed() {
        let s = tab_settings(true, "left", "X");
        assert!(s.dynamic);
        assert_eq!(s.position, TabIconPosition::Left);
        assert_eq!(s.default, "X");
        let s = tab_settings(false, "right", "Y");
        assert!(!s.dynamic);
        assert_eq!(s.position, TabIconPosition::Right);
        assert_eq!(s.default, "Y");
    }

    #[test]
    fn left_out_tab_icon_keys_keep_defaults() {
        let defaults = Settings::default().tab_icon;
        assert_eq!(Settings::parse("{}").unwrap().tab_icon, defaults);
        let s = Settings::parse(r#"{"tab_icon": {"default": "Z"}}"#)
            .unwrap()
            .tab_icon;
        assert_eq!(s.default, "Z");
        assert_eq!(s.dynamic, defaults.dynamic);
        assert_eq!(s.position, defaults.position);
    }

    #[test]
    fn bundled_default_icon_is_one_glyph() {
        // presence in the font is checked by test_tab_icon_glyphs.sh
        let default = Settings::default().tab_icon.default;
        assert_eq!(default.chars().count(), 1, "{default:?} is not one glyph");
    }

    #[test]
    fn profile_icon_is_parsed() {
        let settings = Settings::parse(
            r#"{"profiles": [{"name": "a", "command": "system", "icon": "Z", "default": true}]}"#,
        )
        .unwrap();
        assert_eq!(settings.default_profile().icon.as_deref(), Some("Z"));
    }

    #[test]
    fn bad_tab_icon_settings_are_rejected() {
        for json in [
            r#"{"tab_icon": {"position": "center"}}"#,
            r#"{"tab_icon": {"dynamic": "yes"}}"#,
            r#"{"tab_icon": {"default": 1}}"#,
        ] {
            assert!(
                Settings::parse(json).is_err(),
                "expected error for {json:?}"
            );
        }
    }

    #[test]
    fn stale_icon_overrides_in_settings_has_no_effect() {
        // old user files may still carry it; it must not break parsing or change icons
        let settings = Settings::parse(
            r#"{"tab_icon": {"dynamic": true, "position": "left", "default": "D",
            "icon_overrides": {"htop": "H", "mc": "O"}}}"#,
        )
        .expect("stale icon_overrides broke settings");
        assert_eq!(settings.tab_icon, dynamic());
        assert_eq!(icon_name(&settings.tab_icon, &fixture(), "htop"), MONITOR);
        assert_eq!(icon_name(&settings.tab_icon, &fixture(), "mc"), DEFAULT);
    }

    // dynamic icons

    #[test]
    fn programs_without_group_use_default() {
        let icons = fixture();
        assert_eq!(icon_of(&icons, "bash"), DEFAULT);
        assert_eq!(icon_of(&icons, "zsh"), DEFAULT);
        assert_eq!(icon_of(&icons, "not-a-known-program"), DEFAULT);
    }

    #[test]
    fn custom_default_is_used() {
        let settings = tab_settings(true, "left", "Q");
        assert_eq!(icon_name(&settings, &fixture(), "bash"), "Q");
    }

    #[test]
    fn group_members_share_the_group_icon() {
        let icons = fixture();
        for (name, icon) in [
            ("ssh", REMOTE),
            ("telnet", REMOTE),
            ("mosh", REMOTE),
            ("claude", AI),
            ("codex", AI),
            ("htop", MONITOR),
            ("btop", MONITOR),
            ("top", MONITOR),
        ] {
            assert_eq!(icon_of(&icons, name), icon, "{name}");
        }
    }

    #[test]
    fn full_path_argv0_is_matched_by_file_name() {
        assert_eq!(icon_of(&fixture(), "/usr/local/bin/htop"), MONITOR);
    }

    #[test]
    fn interpreter_script_is_matched() {
        let icons = fixture();
        let node = script("node", "pi");
        assert_eq!(icon_pid(&dynamic(), &icons, node.pid()), AI);
        let python = script("python3", "aider");
        assert_eq!(icon_pid(&dynamic(), &icons, python.pid()), AI);
    }

    #[test]
    fn interpreter_script_with_extension_is_matched() {
        let node = script("node", "codex.js");
        assert_eq!(icon_pid(&dynamic(), &fixture(), node.pid()), AI);
    }

    #[test]
    fn interpreter_with_unknown_script_uses_default() {
        let node = script("node", "server.js");
        assert_eq!(icon_pid(&dynamic(), &fixture(), node.pid()), DEFAULT);
    }

    #[test]
    fn interpreter_skips_flags_before_script() {
        let dir = tmp_dir("flags");
        let path = dir.join("pi");
        std::fs::write(&path, "sleep 30; true\n").unwrap();
        // "-e" is a harmless sh flag here, standing in for "node --no-warnings"
        let p = spawn("/bin/sh", "node", &["-e", path.to_str().unwrap()]);
        assert_eq!(icon_pid(&dynamic(), &fixture(), p.pid()), AI);
    }

    #[test]
    fn arguments_of_non_interpreters_are_ignored() {
        let p = spawn("/bin/sh", "bash", &["-c", "sleep 30; true", "ssh"]);
        assert_eq!(icon_pid(&dynamic(), &fixture(), p.pid()), DEFAULT);
    }

    #[test]
    fn script_group_wins_over_interpreter_group() {
        let icons = groups(
            r#"{"interpreters": ["node"], "groups": [{"icon": "N", "commands": ["node"]}, {"icon": "A", "commands": ["pi"]}]}"#,
        );
        let pi = script("node", "pi");
        assert_eq!(icon_pid(&dynamic(), &icons, pi.pid()), "A");
        let app = script("node", "app.js");
        assert_eq!(icon_pid(&dynamic(), &icons, app.pid()), "N");
    }

    #[test]
    fn missing_process_uses_default() {
        assert_eq!(icon_pid(&dynamic(), &fixture(), u32::MAX - 1), DEFAULT);
    }

    // disabling

    #[test]
    fn disabled_dynamic_always_uses_default() {
        let settings = tab_settings(false, "left", DEFAULT);
        let icons = fixture();
        assert_eq!(icon_name(&settings, &icons, "ssh"), DEFAULT);
        assert_eq!(icon_name(&settings, &icons, "htop"), DEFAULT);
        let node = script("node", "pi");
        assert_eq!(icon_pid(&settings, &icons, node.pid()), DEFAULT);
    }

    // profile icon

    #[test]
    fn profile_icon_wins_over_everything() {
        let settings = dynamic();
        let icons = fixture();
        let ssh = named("ssh");
        assert_eq!(
            tab_icon(&settings, &icons, Some("P"), foreground_process(ssh.pid())),
            "P"
        );
        let htop = named("htop");
        assert_eq!(
            tab_icon(&settings, &icons, Some("P"), foreground_process(htop.pid())),
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
    fn profile_icon_is_kept_with_dynamic_disabled() {
        let settings = tab_settings(false, "left", DEFAULT);
        let p = named("bash");
        assert_eq!(
            tab_icon(
                &settings,
                &fixture(),
                Some("P"),
                foreground_process(p.pid())
            ),
            "P"
        );
    }

    // user groups (tab_icons.jsonc)

    #[test]
    fn user_group_adds_new_command() {
        let icons = groups(r#"{"groups": [{"icon": "O", "commands": ["mc"]}]}"#);
        assert_eq!(icon_of(&icons, "mc"), "O");
        assert_eq!(icon_of(&icons, "bash"), DEFAULT);
    }

    #[test]
    fn user_groups_replace_bundled_groups_whole() {
        let mine = r#"[{"icon": "O", "commands": ["mc"]}]"#;
        let icons = groups(&format!(r#"{{"groups": {mine}}}"#));
        assert_eq!(icons.groups.len(), 1);
        assert_eq!(icons.groups[0].icon, "O");
        assert_eq!(icons.groups[0].commands, ["mc"]);
        // any bundled command is gone now
        for command in TabIcons::default().groups.iter().flat_map(|g| &g.commands) {
            assert_eq!(icons.icon(command), None, "{command}");
        }
    }

    #[test]
    fn empty_user_groups_disable_program_icons() {
        let icons = groups(r#"{"groups": []}"#);
        assert!(icons.groups.is_empty());
        assert_eq!(icon_of(&icons, "htop"), DEFAULT);
    }

    #[test]
    fn left_out_groups_keep_bundled() {
        let icons = groups(r#"{"interpreters": ["node"]}"#);
        assert_eq!(icons.groups, TabIcons::default().groups);
        assert_eq!(icons.interpreters, ["node"]);
    }

    #[test]
    fn left_out_interpreters_keep_bundled() {
        let icons = groups(r#"{"groups": [{"icon": "X", "commands": ["pi"]}]}"#);
        assert_eq!(icons.interpreters, TabIcons::default().interpreters);
    }

    #[test]
    fn user_interpreters_replace_bundled() {
        let icons =
            groups(r#"{"interpreters": ["ruby"], "groups": [{"icon": "X", "commands": ["pi"]}]}"#);
        assert_eq!(icons.interpreters, ["ruby"]);
        let node = script("node", "pi");
        assert_eq!(icon_pid(&dynamic(), &icons, node.pid()), DEFAULT);
        let ruby = script("ruby", "pi");
        assert_eq!(icon_pid(&dynamic(), &icons, ruby.pid()), "X");
    }

    #[test]
    fn first_group_wins() {
        let icons = groups(
            r#"{"groups": [{"icon": "A", "commands": ["htop"]}, {"icon": "B", "commands": ["htop", "mc"]}]}"#,
        );
        assert_eq!(icons.icon("htop"), Some("A"));
        assert_eq!(icon_of(&icons, "htop"), "A");
        assert_eq!(icon_of(&icons, "mc"), "B");
    }

    #[test]
    fn user_group_can_set_shell_icon() {
        let icons = groups(r#"{"groups": [{"icon": "B", "commands": ["bash"]}]}"#);
        assert_eq!(icon_of(&icons, "bash"), "B");
    }

    #[test]
    fn user_file_accepts_comments_and_trailing_commas() {
        let icons = groups(
            "// mine\n{\n  \"groups\": [\n    { \"icon\": \"H\", \"commands\": [\"htop\",], },\n  ],\n}\n",
        );
        assert_eq!(icons.icon("htop"), Some("H"));
    }

    #[test]
    fn invalid_user_json_is_rejected() {
        for json in [
            "{",
            "not json",
            r#"{"groups": "htop"}"#,
            r#"{"groups": [{"icon": "H"}]}"#,
            r#"{"groups": [{"commands": ["htop"]}]}"#,
            r#"{"groups": [{"icon": 1, "commands": ["htop"]}]}"#,
            r#"{"interpreters": "node"}"#,
        ] {
            assert!(
                TabIcons::parse(json).is_err(),
                "expected error for {json:?}"
            );
        }
    }

    // bundled tab icons sanity

    #[test]
    fn bundled_tab_icons_are_single_glyphs() {
        for group in &TabIcons::default().groups {
            assert_eq!(
                group.icon.chars().count(),
                1,
                "{:?} is not one glyph",
                group.icon
            );
        }
    }

    #[test]
    fn bundled_commands_are_in_one_group_only() {
        let mut seen = HashSet::new();
        for command in TabIcons::default().groups.iter().flat_map(|g| &g.commands) {
            assert!(seen.insert(command), "{command} is in two groups");
        }
    }
}

#[cfg(test)]
mod tab_icon_wrappers {

    use std::{
        path::{Path, PathBuf},
        process::{Child, Command},
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    use super::tab_icon;
    use crate::{
        settings::{Settings, TabIconSettings, TabIcons},
        terminal::{children, foreground_process, process_info},
    };

    const DEFAULT: &str = "D";
    const TOOL: &str = "T";
    const WRAPPER: &str = "W";
    const AI: &str = "A";

    /// tab icon settings with every key set explicitly
    fn tab_settings(dynamic: bool) -> TabIconSettings {
        let json = format!(
            r#"{{"tab_icon": {{"dynamic": {dynamic}, "position": "left", "default": "{DEFAULT}"}}}}"#
        );
        Settings::parse(&json).unwrap().tab_icon
    }

    fn dynamic() -> TabIconSettings {
        tab_settings(true)
    }

    /// icons where `wrapa` and `wrapb` wrap, `node` is an interpreter, and `tool`, `pi`, the
    /// wrappers and `outer` have icons of their own
    fn fixture() -> TabIcons {
        icons(r#"["wrapa", "wrapb"]"#)
    }

    fn icons(wrappers: &str) -> TabIcons {
        TabIcons::parse(&format!(
            r#"{{
            "wrappers": {wrappers},
            "interpreters": ["node"],
            "groups": [
                {{"icon": "T", "commands": ["tool"]}},
                {{"icon": "W", "commands": ["wrapa", "wrapb"]}},
                {{"icon": "O", "commands": ["outer"]}},
                {{"icon": "A", "commands": ["pi"]}},
            ],
        }}"#
        ))
        .unwrap()
    }

    /// killed on drop, together with everything it started, so failed asserts leave nothing behind
    struct Proc(Child);

    impl Drop for Proc {
        fn drop(&mut self) {
            kill_tree(self.0.id());
            let _ = self.0.wait();
        }
    }

    fn kill_tree(pid: u32) {
        for child in children(pid) {
            kill_tree(child);
        }
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }

    impl Proc {
        fn pid(&self) -> u32 {
            self.0.id()
        }
    }

    /// fresh directory per call since tests run in parallel
    fn tmp_dir(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("agent_tab_wrap_{}_{name}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn find<const N: usize>(candidates: [&'static str; N]) -> &'static str {
        candidates
            .into_iter()
            .find(|p| Path::new(p).exists())
            .expect("binary not found")
    }

    /// `dir/name` linking to sh, so it runs as a shell showing up as `name`
    fn shell_named(dir: &Path, name: &str) -> PathBuf {
        link(dir, name, find(["/bin/sh", "/usr/bin/sh"]))
    }

    /// `dir/name` linking to sleep, so it runs as a long program showing up as `name`
    fn program_named(dir: &Path, name: &str) -> PathBuf {
        link(
            dir,
            name,
            find(["/usr/bin/gnusleep", "/usr/bin/sleep", "/bin/sleep"]),
        )
    }

    fn link(dir: &Path, name: &str, target: &str) -> PathBuf {
        let path = dir.join(name);
        std::os::unix::fs::symlink(target, &path).unwrap();
        path
    }

    /// run `wrapper -c script`, waiting until the process tree is `depth` levels deep
    fn run_under(wrapper: &Path, script: &str, depth: usize) -> Proc {
        // the trailing ":" keeps sh from replacing itself with the last command
        let script = format!("{script}; :");
        let proc = Proc(Command::new(wrapper).args(["-c", &script]).spawn().unwrap());
        wait_depth(proc.pid(), depth);
        proc
    }

    /// wait until following first children from `pid` reaches `depth` levels, returns the last pid
    fn wait_depth(pid: u32, depth: usize) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut pid = pid;
        for _ in 0..depth {
            pid = loop {
                if let Some(&next) = children(pid).first()
                    && process_info(next).is_some()
                {
                    break next;
                }
                assert!(Instant::now() < deadline, "process tree never grew");
                std::thread::sleep(Duration::from_millis(10));
            };
        }
        // wait for the exec of the last level, so /proc shows its final name
        std::thread::sleep(Duration::from_millis(50));
        pid
    }

    fn quote(path: &Path) -> String {
        format!("'{}'", path.display())
    }

    fn icon_of(icons: &TabIcons, pid: u32) -> String {
        tab_icon(&dynamic(), icons, None, foreground_process(pid))
    }

    // parsing wrappers

    #[test]
    fn user_wrappers_replace_bundled() {
        let icons = TabIcons::parse(r#"{"wrappers": ["x", "y"]}"#).unwrap();
        assert_eq!(icons.wrappers, ["x", "y"]);
        let icons = TabIcons::parse(r#"{"wrappers": []}"#).unwrap();
        assert!(icons.wrappers.is_empty());
    }

    #[test]
    fn left_out_wrappers_keep_bundled() {
        let icons = TabIcons::parse(r#"{"groups": [{"icon": "X", "commands": ["x"]}]}"#).unwrap();
        assert_eq!(icons.wrappers, TabIcons::default().wrappers);
    }

    #[test]
    fn user_wrappers_leave_other_keys_bundled() {
        let icons = TabIcons::parse(r#"{"wrappers": ["x"]}"#).unwrap();
        assert_eq!(icons.groups, TabIcons::default().groups);
        assert_eq!(icons.interpreters, TabIcons::default().interpreters);
    }

    #[test]
    fn invalid_wrappers_are_rejected() {
        for json in [
            r#"{"wrappers": "x"}"#,
            r#"{"wrappers": [1]}"#,
            r#"{"wrappers": {"x": true}}"#,
        ] {
            assert!(
                TabIcons::parse(json).is_err(),
                "expected error for {json:?}"
            );
        }
    }

    // process helpers

    #[test]
    fn process_info_reports_pid_and_children_are_listed() {
        let dir = tmp_dir("info");
        let wrapa = shell_named(&dir, "wrapa");
        let tool = program_named(&dir, "tool");
        let p = run_under(&wrapa, &format!("{} 30", quote(&tool)), 1);
        let parent = process_info(p.pid()).unwrap();
        assert_eq!(parent.pid, p.pid());
        assert_eq!(parent.name, "wrapa");
        let kids = children(p.pid());
        assert_eq!(kids.len(), 1, "{kids:?}");
        let child = process_info(kids[0]).unwrap();
        assert_eq!(child.pid, kids[0]);
        assert_eq!(child.name, "tool");
        assert_eq!(child.args, ["30"]);
        assert!(children(kids[0]).is_empty());
    }

    #[test]
    fn missing_process_has_no_children() {
        assert!(children(u32::MAX - 1).is_empty());
        assert!(process_info(u32::MAX - 1).is_none());
    }

    // unwrapping

    #[test]
    fn wrapped_command_shows_its_own_icon() {
        let dir = tmp_dir("wrapped");
        let wrapa = shell_named(&dir, "wrapa");
        let tool = program_named(&dir, "tool");
        let p = run_under(&wrapa, &format!("{} 30", quote(&tool)), 1);
        // the wrapper has an icon of its own, but the command it runs wins
        assert_eq!(icon_of(&fixture(), p.pid()), TOOL);
    }

    #[test]
    fn wrapper_options_do_not_matter() {
        // like "sudo -u root htop": the child is found in /proc, not by parsing arguments
        let dir = tmp_dir("options");
        let wrapa = shell_named(&dir, "wrapa");
        let tool = program_named(&dir, "tool");
        let script = format!("{} 30", quote(&tool));
        let p = Proc(
            Command::new(&wrapa)
                .args(["-e", "-u", "-c", &format!("{script}; :"), "outer", "tool"])
                .spawn()
                .unwrap(),
        );
        wait_depth(p.pid(), 1);
        assert_eq!(icon_of(&fixture(), p.pid()), TOOL);
    }

    #[test]
    fn wrapped_command_without_icon_uses_default() {
        let dir = tmp_dir("unknown");
        let wrapa = shell_named(&dir, "wrapa");
        let other = program_named(&dir, "unknownprog");
        let p = run_under(&wrapa, &format!("{} 30", quote(&other)), 1);
        assert_eq!(icon_of(&fixture(), p.pid()), DEFAULT);
    }

    #[test]
    fn same_wrapper_nested_is_unwrapped() {
        // sudo runs the command under a second sudo
        let dir = tmp_dir("same");
        let wrapa = shell_named(&dir, "wrapa");
        let tool = program_named(&dir, "tool");
        let inner = format!("{} 30; :", quote(&tool));
        let p = run_under(&wrapa, &format!("{} -c \"{inner}\"", quote(&wrapa)), 2);
        assert_eq!(icon_of(&fixture(), p.pid()), TOOL);
    }

    #[test]
    fn different_wrappers_nested_are_unwrapped() {
        let dir = tmp_dir("mixed");
        let wrapa = shell_named(&dir, "wrapa");
        let wrapb = shell_named(&dir, "wrapb");
        let tool = program_named(&dir, "tool");
        let inner = format!("{} 30; :", quote(&tool));
        let p = run_under(&wrapa, &format!("{} -c \"{inner}\"", quote(&wrapb)), 2);
        assert_eq!(icon_of(&fixture(), p.pid()), TOOL);
    }

    /// a chain of `levels` wrappers named `wrapa`, the last one running `tool`
    fn chain(levels: usize) -> Proc {
        let dir = tmp_dir(&format!("chain{levels}"));
        let wrapa = shell_named(&dir, "wrapa");
        let tool = program_named(&dir, "tool");
        // each level is a script file, so no quoting nests
        for level in 1..levels {
            let next = if level + 1 == levels {
                format!("{} 30", quote(&tool))
            } else {
                format!(
                    "{} {}",
                    quote(&wrapa),
                    quote(&dir.join(format!("l{}", level + 1)))
                )
            };
            std::fs::write(dir.join(format!("l{level}")), format!("{next}\n:\n")).unwrap();
        }
        let first = if levels == 1 {
            format!("{} 30", quote(&tool))
        } else {
            format!("{} {}", quote(&wrapa), quote(&dir.join("l1")))
        };
        run_under(&wrapa, &first, levels)
    }

    #[test]
    fn three_nested_wrappers_are_unwrapped() {
        let p = chain(3);
        assert_eq!(icon_of(&fixture(), p.pid()), TOOL);
    }

    #[test]
    fn deep_wrapper_chain_stops_at_limit() {
        // a chain deeper than any real wrapper setup is not followed to the end
        let p = chain(12);
        let start = Instant::now();
        let icon = icon_of(&fixture(), p.pid());
        assert_ne!(icon, TOOL, "wrapper chain followed without limit");
        // stopped on a wrapper, which keeps its own icon
        assert_eq!(icon, WRAPPER);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn wrapper_without_child_keeps_its_icon() {
        // like sudo waiting for a password
        let dir = tmp_dir("childless");
        let wrapa = program_named(&dir, "wrapa");
        let p = Proc(Command::new(&wrapa).arg("30").spawn().unwrap());
        std::thread::sleep(Duration::from_millis(50));
        assert!(children(p.pid()).is_empty());
        assert_eq!(icon_of(&fixture(), p.pid()), WRAPPER);
    }

    #[test]
    fn wrapper_without_child_or_icon_uses_default() {
        let dir = tmp_dir("childless_default");
        let wrapa = program_named(&dir, "wrapa");
        let p = Proc(Command::new(&wrapa).arg("30").spawn().unwrap());
        std::thread::sleep(Duration::from_millis(50));
        let icons = TabIcons::parse(r#"{"wrappers": ["wrapa"], "interpreters": [], "groups": []}"#)
            .unwrap();
        assert_eq!(icon_of(&icons, p.pid()), DEFAULT);
    }

    #[test]
    fn non_wrapper_is_not_followed() {
        let dir = tmp_dir("outer");
        let outer = shell_named(&dir, "outer");
        let tool = program_named(&dir, "tool");
        let p = run_under(&outer, &format!("{} 30", quote(&tool)), 1);
        assert_eq!(icon_of(&fixture(), p.pid()), "O");
    }

    #[test]
    fn non_wrapper_without_icon_is_not_followed() {
        let dir = tmp_dir("plain");
        let plain = shell_named(&dir, "plainshell");
        let tool = program_named(&dir, "tool");
        let p = run_under(&plain, &format!("{} 30", quote(&tool)), 1);
        assert_eq!(icon_of(&fixture(), p.pid()), DEFAULT);
    }

    #[test]
    fn empty_wrappers_disable_unwrapping() {
        let dir = tmp_dir("empty");
        let wrapa = shell_named(&dir, "wrapa");
        let tool = program_named(&dir, "tool");
        let p = run_under(&wrapa, &format!("{} 30", quote(&tool)), 1);
        assert_eq!(icon_of(&icons("[]"), p.pid()), WRAPPER);
    }

    #[test]
    fn unwrapping_stops_at_first_non_wrapper() {
        // wrapa -> outer -> tool: outer is not a wrapper, so tool is never reached
        let dir = tmp_dir("stop");
        let wrapa = shell_named(&dir, "wrapa");
        let outer = shell_named(&dir, "outer");
        let tool = program_named(&dir, "tool");
        let inner = format!("{} 30; :", quote(&tool));
        let p = run_under(&wrapa, &format!("{} -c \"{inner}\"", quote(&outer)), 2);
        assert_eq!(icon_of(&fixture(), p.pid()), "O");
    }

    #[test]
    fn interpreter_script_is_matched_after_unwrap() {
        let dir = tmp_dir("interp");
        let wrapa = shell_named(&dir, "wrapa");
        let node = shell_named(&dir, "node");
        let script = dir.join("pi");
        std::fs::write(&script, "sleep 30\n:\n").unwrap();
        let p = run_under(&wrapa, &format!("{} {}", quote(&node), quote(&script)), 1);
        assert_eq!(icon_of(&fixture(), p.pid()), AI);
    }

    #[test]
    fn interpreter_with_flags_is_matched_after_unwrap() {
        let dir = tmp_dir("interp_flags");
        let wrapa = shell_named(&dir, "wrapa");
        let node = shell_named(&dir, "node");
        let script = dir.join("pi.js");
        std::fs::write(&script, "sleep 30\n:\n").unwrap();
        // "-e" is a harmless sh flag standing in for "node --no-warnings"
        let p = run_under(
            &wrapa,
            &format!("{} -e {}", quote(&node), quote(&script)),
            1,
        );
        assert_eq!(icon_of(&fixture(), p.pid()), AI);
    }

    #[test]
    fn disabled_dynamic_ignores_wrapped_command() {
        let dir = tmp_dir("disabled");
        let wrapa = shell_named(&dir, "wrapa");
        let tool = program_named(&dir, "tool");
        let p = run_under(&wrapa, &format!("{} 30", quote(&tool)), 1);
        assert_eq!(
            tab_icon(
                &tab_settings(false),
                &fixture(),
                None,
                foreground_process(p.pid())
            ),
            DEFAULT
        );
    }

    #[test]
    fn profile_icon_wins_over_wrapped_command() {
        let dir = tmp_dir("profile");
        let wrapa = shell_named(&dir, "wrapa");
        let tool = program_named(&dir, "tool");
        let p = run_under(&wrapa, &format!("{} 30", quote(&tool)), 1);
        assert_eq!(
            tab_icon(
                &dynamic(),
                &fixture(),
                Some("P"),
                foreground_process(p.pid())
            ),
            "P"
        );
    }

    #[test]
    fn icon_returns_to_wrapper_when_child_exits() {
        // the wrapper outlives its first command, then waits in a child-less state
        let dir = tmp_dir("exit");
        let wrapa = shell_named(&dir, "wrapa");
        let tool = program_named(&dir, "tool");
        let wrapper = Proc(
            Command::new(&wrapa)
                .args(["-c", &format!("{} 1; read x; :", quote(&tool))])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
        wait_depth(wrapper.pid(), 1);
        assert_eq!(icon_of(&fixture(), wrapper.pid()), TOOL);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !children(wrapper.pid()).is_empty() {
            assert!(Instant::now() < deadline, "child never exited");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(icon_of(&fixture(), wrapper.pid()), WRAPPER);
    }
}
