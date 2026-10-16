//! shell integration: prompts and command output marked by the scanner, found in the grid

use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as AlacPoint, Side},
    selection::{Selection, SelectionType},
    term::{TermMode, cell::Flags},
};
use gpui::{Pixels, Point};

use crate::settings::{Profile, Shell, state_dir};

use super::{
    Link, Terminal,
    builder::ZedListener,
    links::{TextPatterns, pattern_matches},
    scanner::{OUTPUT_MARK, PROMPT_MARK},
};

/// first line of every marked prompt, top to bottom
fn prompt_lines(term: &Term<ZedListener>) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut last_id: Option<String> = None;
    for line in term.topmost_line().0..=term.bottommost_line().0 {
        let row = &term.grid()[Line(line)];
        let id = (0..term.columns()).find_map(|column| {
            let link = row[Column(column)].hyperlink()?;
            link.id()
                .starts_with(PROMPT_MARK)
                .then(|| link.id().to_string())
        });
        if let Some(id) = id
            && last_id.as_ref() != Some(&id)
        {
            lines.push(Line(line));
            last_id = Some(id);
        }
    }
    lines
}

/// first and last cell of the newest marked command output
fn last_output(term: &Term<ZedListener>) -> Option<(AlacPoint, AlacPoint)> {
    let mut newest: Option<(usize, AlacPoint, AlacPoint)> = None;
    for line in term.topmost_line().0..=term.bottommost_line().0 {
        let row = &term.grid()[Line(line)];
        for column in 0..term.columns() {
            let Some(link) = row[Column(column)].hyperlink() else {
                continue;
            };
            let Some(number) = link
                .id()
                .strip_prefix(OUTPUT_MARK)
                .and_then(|n| n.parse::<usize>().ok())
            else {
                continue;
            };
            let point = AlacPoint::new(Line(line), Column(column));
            newest = match newest {
                Some((n, start, _)) if n == number => Some((n, start, point)),
                Some((n, _, _)) if n > number => newest,
                _ => Some((number, point, point)),
            };
        }
    }
    newest.map(|(_, start, end)| (start, end))
}

/// cell after the last one of `point`'s line, the first of the next line at the edge
fn next_cell<T: EventListener>(term: &Term<T>, point: AlacPoint) -> AlacPoint {
    if point.column.0 + 1 < term.columns() {
        AlacPoint::new(point.line, point.column + 1)
    } else {
        AlacPoint::new(point.line + 1, Column(0))
    }
}

fn wraps<T: EventListener>(term: &Term<T>, line: Line) -> bool {
    term.grid()[line][term.last_column()]
        .flags
        .contains(Flags::WRAPLINE)
}

/// first cell of the command line being typed, right after the newest prompt mark. none when
/// output came after that prompt or the cursor is on another line than the command line,
/// like after a continuation prompt
fn prompt_input_start<T: EventListener>(term: &Term<T>) -> Option<AlacPoint> {
    let cursor = term.grid().cursor.point;
    // the newest mark is the closest one before the cursor
    let mut point = cursor;
    let mark = loop {
        if let Some(link) = term.grid()[point].hyperlink()
            && (link.id().starts_with(PROMPT_MARK) || link.id().starts_with(OUTPUT_MARK))
        {
            break link.id().to_string();
        }
        if point.column.0 > 0 {
            point.column -= 1;
        } else if point.line > term.topmost_line() {
            point = AlacPoint::new(point.line - 1, term.last_column());
        } else {
            return None;
        }
    };
    if !mark.starts_with(PROMPT_MARK) {
        return None;
    }
    let start = next_cell(term, point);
    // arrows only move along the line the cursor is on and the lines it wraps from
    ((start.line.0..cursor.line.0).all(|line| wraps(term, Line(line))) && start <= cursor)
        .then_some(start)
}

/// cells from `from` up to `to`, not counting `to`, a wide char counted once like the
/// shell counts it
fn chars_between<T: EventListener>(term: &Term<T>, from: AlacPoint, to: AlacPoint) -> usize {
    let mut count = 0;
    let mut point = from;
    while point < to {
        let flags = term.grid()[point].flags;
        if !flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            count += 1;
        }
        point = next_cell(term, point);
    }
    count
}

/// arrow keys that move the shell's cursor to `target` on the command line being typed,
/// none when `target` is not on it. a click after the end of the text goes to its end
fn arrows_to<T: EventListener>(term: &Term<T>, mut target: AlacPoint) -> Option<String> {
    let start = prompt_input_start(term)?;
    let cursor = term.grid().cursor.point;
    let mut last_line = cursor.line;
    while last_line < term.bottommost_line() && wraps(term, last_line) {
        last_line += 1;
    }
    if target < start || target.line > last_line {
        return None;
    }
    // past the last char typed, and past the cursor, which may sit after typed spaces
    let mut end = cursor;
    let mut point = start;
    while point.line <= last_line {
        if term.grid()[point].c != ' ' {
            end = end.max(next_cell(term, point));
        }
        point = next_cell(term, point);
    }
    target = target.min(end);
    // the right half of a wide char is the char
    if target.column.0 > 0 && term.grid()[target].flags.contains(Flags::WIDE_CHAR_SPACER) {
        target.column -= 1;
    }
    let app_cursor = term.mode().contains(TermMode::APP_CURSOR);
    let (key, count) = if target >= cursor {
        (
            if app_cursor { "\x1bOC" } else { "\x1b[C" },
            chars_between(term, cursor, target),
        )
    } else {
        (
            if app_cursor { "\x1bOD" } else { "\x1b[D" },
            chars_between(term, target, cursor),
        )
    };
    Some(key.repeat(count))
}

/// first and last cell marked with the hyperlink id `id`
pub(super) fn mark_cells<T: EventListener>(
    term: &Term<T>,
    id: &str,
) -> Option<(AlacPoint, AlacPoint)> {
    let mut found: Option<(AlacPoint, AlacPoint)> = None;
    for line in term.topmost_line().0..=term.bottommost_line().0 {
        let row = &term.grid()[Line(line)];
        for column in 0..term.columns() {
            if row[Column(column)]
                .hyperlink()
                .is_some_and(|link| link.id() == id)
            {
                let point = AlacPoint::new(Line(line), Column(column));
                found = Some((found.map_or(point, |(start, _)| start), point));
            }
        }
    }
    found
}

/// `target` with the line it names, like "src/main.rs:12" from "src/main.rs" and ":12: error",
/// none when it names no line of a file
fn file_line(target: &str, after: &str) -> Option<String> {
    if target.contains("://") {
        return None;
    }
    let text = format!("{target}{after}");
    let text = text.trim_end_matches(':');
    let parts: Vec<&str> = text.split(':').collect();
    let numbers = |part: &&str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    // a line, and maybe a column after it
    let first = parts.iter().position(numbers)?;
    let path = parts[..first].join(":");
    (first > 0
        && parts.len() - first <= 2
        && parts[first..].iter().all(numbers)
        && !path.is_empty())
    .then(|| text.to_string())
}

/// the command line typed after prompt mark `prompt`, on its line and the lines it wraps to
pub(super) fn command_after_prompt<T: EventListener>(
    term: &Term<T>,
    prompt: usize,
) -> Option<String> {
    let (_, end) = mark_cells(term, &format!("{PROMPT_MARK}{prompt}"))?;
    let start = next_cell(term, end);
    let mut last = start.line;
    while last < term.bottommost_line() && wraps(term, last) {
        last += 1;
    }
    let text = term.bounds_to_string(start, AlacPoint::new(last, term.last_column()));
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

impl Terminal {
    /// command line of the last command started, as the shell sent it or as typed after
    /// its prompt
    pub fn last_command_line(&self) -> Option<String> {
        let (prompt, output) = self.last_command?;
        if let Some(block) = self
            .blocks
            .list
            .iter()
            .rev()
            .find(|block| block.mark == output)
            && !block.command.is_empty()
        {
            return Some(block.command.clone());
        }
        command_after_prompt(&self.term.lock(), prompt?)
    }

    /// first match of `patterns` in the output of the last command that names a line of a
    /// file, its target with the line, like "src/main.rs:12"
    pub fn last_output_location(&self, patterns: &mut TextPatterns) -> Option<Link> {
        let (_, output) = self.last_command?;
        let term = self.term.lock();
        let (start, end) = mark_cells(&term, &format!("{OUTPUT_MARK}{output}"))?;
        let end = AlacPoint::new(end.line, term.last_column());
        pattern_matches(&term, patterns, start, end)
            .into_iter()
            .find_map(|mut link| {
                // the line number often follows the match, like "src/main.rs:12:5: error"
                let mut after = String::new();
                let mut point = *link.range.end();
                while point.column < term.last_column() {
                    point.column += 1;
                    let c = term.grid()[point].c;
                    if c != ':' && !c.is_ascii_digit() {
                        break;
                    }
                    after.push(c);
                }
                link.target = file_line(&link.target, &after)?;
                Some(link)
            })
    }

    /// move the shell's cursor to the clicked cell of the command line being typed, with
    /// arrow keys. false when the click is elsewhere or a command runs
    pub fn click_moves_cursor(&mut self, position: Point<Pixels>) -> bool {
        if self.command_started.is_some() {
            return false;
        }
        let (target, _) = self.mouse_point(position);
        let term = self.term.lock();
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return false;
        }
        let Some(keys) = arrows_to(&term, target) else {
            return false;
        };
        drop(term);
        if !keys.is_empty() {
            self.input(super::InputSource::Keyboard, keys.into_bytes());
        }
        true
    }

    /// scroll so the prompt above the top of the view, or below it, is the first line shown.
    /// false when there is no such prompt
    pub fn jump_to_prompt(&mut self, up: bool) -> bool {
        let mut term = self.term.lock();
        let top = Line(-(term.grid().display_offset() as i32));
        let prompts = prompt_lines(&term);
        let target = if up {
            prompts.iter().rev().find(|line| **line < top)
        } else {
            prompts.iter().find(|line| **line > top)
        };
        let Some(target) = target else {
            return false;
        };
        let offset = -target.0;
        let delta = offset - term.grid().display_offset() as i32;
        term.scroll_display(Scroll::Delta(delta));
        drop(term);
        self.dirty.store(true, Ordering::Release);
        true
    }

    /// select the whole lines of the output of the last command, false when none is marked
    pub fn select_last_output(&mut self) -> bool {
        let mut term = self.term.lock();
        let Some((start, end)) = last_output(&term) else {
            return false;
        };
        let mut selection = Selection::new(SelectionType::Lines, start, Side::Left);
        selection.update(end, Side::Right);
        term.selection = Some(selection);
        term.scroll_to_point(start);
        drop(term);
        self.dirty.store(true, Ordering::Release);
        true
    }

    /// text of the output of the last command, none when none is marked
    pub fn last_output_text(&self) -> Option<String> {
        let term = self.term.lock();
        let (start, end) = last_output(&term)?;
        let text = term.bounds_to_string(
            AlacPoint::new(start.line, Column(0)),
            AlacPoint::new(end.line, term.last_column()),
        );
        Some(text.trim_end().to_string())
    }
}

const BASH_SCRIPT: &str = include_str!("../../assets/shell/kuterm.bash");
const ZSH_SCRIPT: &str = include_str!("../../assets/shell/zsh/.zshenv");
const FISH_SCRIPT: &str = include_str!("../../assets/shell/fish/kuterm.fish");
const KOUT_SCRIPT: &str = include_str!("../../assets/shell/kout");

/// let the helpers in bin/ run, the shell scripts are only read
#[cfg(unix)]
fn executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if path.parent().and_then(Path::file_name) != Some("bin".as_ref()) {
        return Ok(());
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn executable(_: &Path) -> std::io::Result<()> {
    Ok(())
}

/// folder the integration scripts are written to, none when it can't be written
fn scripts_dir() -> Option<PathBuf> {
    let dir = state_dir()?.join("shell-integration");
    for (path, contents) in [
        ("kuterm.bash", BASH_SCRIPT),
        ("zsh/.zshenv", ZSH_SCRIPT),
        ("fish/fish/vendor_conf.d/kuterm.fish", FISH_SCRIPT),
        ("bin/kout", KOUT_SCRIPT),
    ] {
        let path = dir.join(path);
        // rewritten only when kuterm brings a new version
        if std::fs::read_to_string(&path).ok().as_deref() == Some(contents) {
            continue;
        }
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, contents))
            .and_then(|()| executable(&path));
        if let Err(error) = written {
            eprintln!("failed to write {}: {error}", path.display());
            return None;
        }
    }
    Some(dir)
}

/// `profile` changed to load kuterm's shell integration into bash, zsh or fish. a command
/// with its own arguments is left alone, it may not be a shell at all
pub fn with_shell_integration(profile: &Profile) -> Profile {
    let mut profile = profile.clone();
    let program = match &profile.command {
        Shell::System => std::env::var("SHELL").ok(),
        Shell::Program(program) => Some(program.clone()),
        Shell::WithArguments { .. } => None,
    };
    let Some(program) = program else {
        return profile;
    };
    let name = Path::new(&program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    if !matches!(name.as_str(), "bash" | "zsh" | "fish") {
        return profile;
    }
    let Some(dir) = scripts_dir() else {
        return profile;
    };
    let inherited = |key: &str| {
        profile
            .env
            .get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
    };
    let path = inherited("PATH").unwrap_or_default();
    match name.as_str() {
        // macos starts the system shell through login, which only keeps the environment
        "bash" if !(cfg!(target_os = "macos") && profile.command == Shell::System) => {
            let rcfile = dir.join("kuterm.bash").to_string_lossy().into_owned();
            profile.command = Shell::WithArguments {
                program,
                args: vec!["--rcfile".into(), rcfile],
            };
        }
        "zsh" => {
            let original = inherited("ZDOTDIR").unwrap_or_default();
            profile.env.insert("KUTERM_ZDOTDIR".into(), original);
            let zdotdir = dir.join("zsh").to_string_lossy().into_owned();
            profile.env.insert("ZDOTDIR".into(), zdotdir);
        }
        "fish" => {
            let data = inherited("XDG_DATA_DIRS")
                .filter(|dirs| !dirs.is_empty())
                .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
            let ours = dir.join("fish").to_string_lossy().into_owned();
            profile
                .env
                .insert("XDG_DATA_DIRS".into(), format!("{ours}:{data}"));
        }
        _ => return profile,
    }
    profile
        .env
        .insert("KUTERM_SHELL_INTEGRATION".into(), "1".into());
    // kout and other helpers
    let bin = dir.join("bin").to_string_lossy().into_owned();
    profile.env.insert("PATH".into(), format!("{bin}:{path}"));
    profile
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use alacritty_terminal::index::{Column, Line, Point as AlacPoint};

    use super::{arrows_to, file_line, with_shell_integration};
    use crate::{
        settings::{Shell, TerminalSettings},
        terminal::{
            Event,
            tests::{profile, screen_text, spawn_with, wait_for_text},
        },
    };

    /// alacritty term `columns` wide after printing a marked prompt and then `typed`
    fn typed_after_prompt(
        columns: usize,
        typed: &str,
    ) -> alacritty_terminal::Term<alacritty_terminal::event::VoidListener> {
        use alacritty_terminal::{
            Term,
            event::VoidListener,
            term::{Config, test::TermSize},
            vte::ansi::Processor,
        };
        let mut term = Term::new(Config::default(), &TermSize::new(columns, 5), VoidListener);
        let mut parser: Processor = Processor::new();
        let prompt = "\x1b]8;id=kuterm-prompt-0;kuterm:\x1b\\$ \x1b]8;;\x1b\\";
        parser.advance(&mut term, format!("{prompt}{typed}").as_bytes());
        term
    }

    fn at(line: i32, column: usize) -> AlacPoint {
        AlacPoint::new(Line(line), Column(column))
    }

    #[test]
    fn clicks_on_the_command_line_count_arrows_over_wrapped_lines() {
        // "$ echo hel" wraps into "lo world", the cursor after "world"
        let term = typed_after_prompt(10, "echo hello world");
        assert_eq!(term.grid().cursor.point, at(1, 8));
        let left = |n: usize| Some("\x1b[D".repeat(n));
        assert_eq!(arrows_to(&term, at(0, 2)), left(16));
        assert_eq!(arrows_to(&term, at(1, 0)), left(8));
        // after the end of the text goes to the end
        assert_eq!(arrows_to(&term, at(1, 9)), Some(String::new()));
        // the prompt itself and lines below are no part of the command line
        assert_eq!(arrows_to(&term, at(0, 1)), None);
        assert_eq!(arrows_to(&term, at(2, 0)), None);

        // the other way, in the cursor mode the program asked for
        let mut term = typed_after_prompt(10, "echo hello world\x1b[?1h");
        term.grid_mut().cursor.point = at(0, 6);
        assert_eq!(arrows_to(&term, at(1, 1)), Some("\x1bOC".repeat(5)));
        term.grid_mut().cursor.point = at(1, 2);
        assert_eq!(arrows_to(&term, at(0, 2)), Some("\x1bOD".repeat(10)));
    }

    #[test]
    fn wide_chars_take_one_arrow() {
        let term = typed_after_prompt(20, "a\u{4f60}\u{597d}b");
        assert_eq!(term.grid().cursor.point, at(0, 8));
        // both halves of a wide char are the char
        assert_eq!(arrows_to(&term, at(0, 3)), Some("\x1b[D".repeat(3)));
        assert_eq!(arrows_to(&term, at(0, 4)), Some("\x1b[D".repeat(3)));
        assert_eq!(arrows_to(&term, at(0, 2)), Some("\x1b[D".repeat(4)));
    }

    #[test]
    fn no_arrows_once_output_follows_the_prompt() {
        let term = typed_after_prompt(20, "ls\r\n\x1b]8;id=kuterm-output-1;kuterm:\x1b\\out");
        assert_eq!(arrows_to(&term, at(0, 2)), None);
        assert_eq!(arrows_to(&term, at(1, 0)), None);
        // a continuation prompt is a line the arrows can't reach
        let term = typed_after_prompt(20, "echo 'a\r\n> b");
        assert_eq!(arrows_to(&term, at(0, 3)), None);
    }

    #[test]
    fn marks_find_prompts_and_the_last_output() {
        // three prompts with a command each, printed the way an integrated shell does
        let script = concat!(
            "i=0; while [ $i -lt 40 ]; do ",
            "printf '\\033]133;A\\007$ \\033]133;B\\007cmd_%s\\n\\033]133;C\\007' $i; ",
            "printf 'out_%s\\nmore_%s\\n\\033]133;D;0\\007' $i $i; i=$((i+1)); done; ",
            "printf '\\033]133;A\\007$ \\033]133;B\\007\\033]7;file://h/tmp\\007'; echo; echo marks_ready; sleep 5"
        );
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "marks_ready");
        terminal.handle_scan_events();
        assert_eq!(terminal.reported_cwd(), Some(std::path::Path::new("/tmp")));
        assert_eq!(
            terminal.last_output_text().as_deref(),
            Some("out_39\nmore_39")
        );
        assert!(terminal.select_last_output());
        assert_eq!(
            terminal.selection_text().as_deref(),
            Some("out_39\nmore_39\n")
        );

        // up from the bottom, each jump puts an older prompt at the top of the view
        assert!(terminal.jump_to_prompt(true));
        terminal.sync();
        let first = screen_text(terminal).lines().next().unwrap().to_string();
        assert!(first.starts_with("$ cmd_"), "{first:?}");
        assert!(terminal.jump_to_prompt(true));
        terminal.sync();
        let second = screen_text(terminal).lines().next().unwrap().to_string();
        assert!(
            second.starts_with("$ cmd_") && second < first,
            "{second:?} {first:?}"
        );
        assert!(terminal.jump_to_prompt(false));
        terminal.sync();
        assert_eq!(screen_text(terminal).lines().next().unwrap(), first);
    }

    #[test]
    fn command_end_after_a_start_reports_its_duration() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf '\\033]133;D;0\\007\\033]133;C\\007'; sleep 0.3; printf '\\033]133;D;3\\007\\033]9;all done\\007'; echo end_ready; sleep 5".into(),
            ],
        });
        let started = Instant::now();
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "end_ready");
        let events = terminal.handle_scan_events();
        // the first end has no start, so it ended nothing
        let [
            Event::CommandStarted { .. },
            Event::CommandFinished { duration, exit },
            Event::Notify { title, body },
        ] = events.as_slice()
        else {
            panic!("{events:?}");
        };
        assert_eq!(*exit, Some(3));
        assert!(*duration >= Duration::from_millis(250) && *duration <= started.elapsed());
        assert_eq!((title.as_str(), body.as_str()), ("", "all done"));
    }

    #[test]
    fn progress_tells_busy_and_idle_but_not_an_idle_that_ends_the_command() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                concat!(
                    "printf '\\033]133;C\\007\\033]9;4;3\\007\\033]9;4;1;50\\007\\033]9;4;0\\007'; ",
                    "printf '\\033]9;4;3\\007\\033]9;4;0\\007\\033]133;D;0\\007'; ",
                    "echo busy_ready; sleep 5"
                )
                .into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "busy_ready");
        let busy: Vec<bool> = terminal
            .handle_scan_events()
            .into_iter()
            .filter_map(|event| match event {
                Event::BusyChanged(busy) => Some(busy),
                _ => None,
            })
            .collect();
        // percent after busy is still busy, the second idle came with the end of the command
        assert_eq!(busy, [true, false, true]);
    }

    #[test]
    fn file_lines_need_a_line_number() {
        // what follows the match is cut at the first char other than ':' or a digit
        assert_eq!(
            file_line("src/main.rs", ":12:5:").as_deref(),
            Some("src/main.rs:12:5")
        );
        assert_eq!(
            file_line("src/main.rs:12", "").as_deref(),
            Some("src/main.rs:12")
        );
        assert_eq!(file_line("src/main.rs", ":"), None);
        assert_eq!(file_line("src/main.rs", ""), None);
        assert_eq!(file_line("12", ":5"), None);
        assert_eq!(file_line("http://host", ":8080"), None);
    }

    #[test]
    fn last_command_line_and_the_first_file_line_of_its_output() {
        use crate::{
            settings::{PatternAction, TextPattern},
            terminal::TextPatterns,
        };

        // a failed command printing a path with its line, then a command printing nothing
        let script = concat!(
            "printf '\\033]133;A\\007$ \\033]133;B\\007make all\\n\\033]133;C\\007'; ",
            "printf 'build.log ok\\nsrc/main.rs:12:5: error\\n\\033]133;D;2\\007'; ",
            "printf '\\033]133;A\\007$ \\033]133;B\\007\\n'; echo lines_ready; sleep 5"
        );
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "lines_ready");
        terminal.handle_scan_events();
        assert_eq!(terminal.last_command_line().as_deref(), Some("make all"));
        let mut patterns = TextPatterns::default();
        patterns.update(&[TextPattern {
            regex: r"[\w/]+\.\w+".into(),
            action: PatternAction::Copy,
            name: Some("path".into()),
        }]);
        // build.log has no line, the first file:line is taken
        let link = terminal.last_output_location(&mut patterns).unwrap();
        assert_eq!(link.target, "src/main.rs:12:5");
        assert_eq!(link.name.as_deref(), Some("path"));
    }

    #[test]
    fn bash_integration_marks_commands_and_reports_the_folder() {
        let bash = profile(Shell::Program("/bin/bash".into()));
        let integrated = with_shell_integration(&bash);
        let Shell::WithArguments { args, .. } = &integrated.command else {
            panic!("bash keeps no arguments: {:?}", integrated.command);
        };
        assert_eq!(args[0], "--rcfile");
        let mut builder = spawn_with(&TerminalSettings::default(), &integrated);
        let terminal = &mut builder.terminal;
        terminal.input(
            crate::terminal::InputSource::Keyboard,
            b"cd /tmp && echo integ_$((40+2))\r".to_vec(),
        );
        wait_for_text(terminal, "integ_42");
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut finished = false;
        while !(finished && terminal.reported_cwd() == Some(std::path::Path::new("/tmp"))) {
            terminal.sync();
            finished |= terminal
                .handle_scan_events()
                .iter()
                .any(|event| matches!(event, Event::CommandFinished { exit: Some(0), .. }));
            assert!(Instant::now() < deadline, "no marks from bash");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(terminal.last_output_text().as_deref(), Some("integ_42"));
    }

    #[test]
    fn incognito_bash_keeps_no_history_file() {
        let mut bash = profile(Shell::Program("/bin/bash".into()));
        bash.incognito = true;
        let integrated = with_shell_integration(&bash);
        let mut builder = spawn_with(&TerminalSettings::default(), &integrated);
        let terminal = &mut builder.terminal;
        terminal.input(
            crate::terminal::InputSource::Keyboard,
            b"echo hist=$HISTFILE=$KUTERM_INCOGNITO\r".to_vec(),
        );
        wait_for_text(terminal, "hist==1");
    }

    /// next message from a shell `matches` takes, skipping the others
    #[cfg(unix)]
    fn wait_shell(
        events: &mut futures::channel::mpsc::UnboundedReceiver<crate::completion::ShellEvent>,
        what: &str,
        matches: impl Fn(&crate::completion::ShellMessage) -> bool,
    ) -> crate::completion::ShellEvent {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match events.try_recv() {
                Ok(event) if matches(&event.message) => return event,
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
            assert!(Instant::now() < deadline, "no {what} from zsh");
        }
    }

    #[cfg(unix)]
    #[test]
    fn zsh_reports_its_line_and_takes_completions() {
        use crate::completion::{Bridge, SOCKET_ENV, ShellMessage, protocol};

        let Some(zsh) = ["/usr/bin/zsh", "/bin/zsh"]
            .into_iter()
            .find(|path| std::path::Path::new(path).exists())
        else {
            eprintln!("no zsh, skipped");
            return;
        };
        // a home of its own: no user files, and an empty .zshrc skips zsh's first run wizard
        let home = std::env::temp_dir().join(format!("kuterm-zsh-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".zshrc"), "PS1='$ '\n").unwrap();
        let mut zsh = profile(Shell::Program(zsh.into()));
        let home_str = home.to_string_lossy().into_owned();
        zsh.env.insert("HOME".into(), home_str.clone());
        zsh.env.insert("ZDOTDIR".into(), home_str);
        let mut zsh = with_shell_integration(&zsh);
        let mut bridge = Bridge::new().unwrap();
        let mut events = bridge.take_events().unwrap();
        zsh.env
            .insert(SOCKET_ENV.into(), bridge.path().to_string_lossy().into());
        let mut builder = spawn_with(&TerminalSettings::default(), &zsh);
        let terminal = &mut builder.terminal;
        terminal.set_completion_bridge(bridge);

        wait_shell(
            &mut events,
            "hello",
            |m| matches!(m, ShellMessage::Hello { shell } if shell == "zsh"),
        );
        wait_shell(&mut events, "PATH", |m| {
            matches!(m, ShellMessage::Env { .. })
        });
        terminal.input(
            crate::terminal::InputSource::Keyboard,
            b"echo comp_".to_vec(),
        );
        let typed = wait_shell(
            &mut events,
            "the typed line",
            |m| matches!(m, ShellMessage::Line { buffer, cursor: 10, .. } if buffer == "echo comp_"),
        );
        let ShellMessage::Line { seq, cwd, .. } = &typed.message else {
            unreachable!()
        };
        assert!(cwd.is_absolute());

        // an answer to an older line is dropped
        let stale = protocol::accept(seq - 1, 5, 10, "wrong ").unwrap();
        assert!(terminal.send_to_shell(typed.conn, &stale));
        let fresh = protocol::accept(*seq, 5, 10, "comp_done ").unwrap();
        assert!(terminal.send_to_shell(typed.conn, &fresh));
        wait_shell(
            &mut events,
            "the completed line",
            |m| matches!(m, ShellMessage::Line { buffer, cursor: 15, .. } if buffer == "echo comp_done "),
        );

        // the completed line runs as typed
        terminal.input(crate::terminal::InputSource::Keyboard, b"\r".to_vec());
        wait_shell(&mut events, "the end of the line", |m| {
            *m == ShellMessage::End
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        while !screen_text(terminal)
            .lines()
            .any(|line| line.trim_end() == "comp_done")
        {
            assert!(Instant::now() < deadline, "{}", screen_text(terminal));
            std::thread::sleep(Duration::from_millis(50));
            terminal.sync();
        }
        assert!(!screen_text(terminal).contains("wrong"));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn zsh_and_fish_are_pointed_at_the_scripts() {
        let zsh = with_shell_integration(&profile(Shell::Program("/usr/bin/zsh".into())));
        assert!(zsh.env["ZDOTDIR"].ends_with("shell-integration/zsh"));
        assert!(zsh.env.contains_key("KUTERM_ZDOTDIR"));
        let fish = with_shell_integration(&profile(Shell::Program("fish".into())));
        assert!(fish.env["XDG_DATA_DIRS"].contains("shell-integration/fish:"));
        // commands with arguments and other programs are left alone
        let other = profile(Shell::Program("/bin/sh".into()));
        assert_eq!(with_shell_integration(&other), other);
        let args = profile(Shell::WithArguments {
            program: "bash".into(),
            args: vec!["-l".into()],
        });
        assert_eq!(with_shell_integration(&args), args);
    }
}
