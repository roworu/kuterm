//! shell completion: what could go at the cursor of a command line, from specs of known
//! commands, programs in PATH and files on disk. it never runs anything to find out, and
//! knows nothing about the ui

#[cfg(unix)]
mod bridge;
mod parse;
pub mod protocol;
mod sources;
pub mod spec;

use std::path::{Path, PathBuf};

#[cfg(unix)]
pub use bridge::{Bridge, SOCKET_ENV};
pub use protocol::{ShellEvent, ShellMessage};
pub use spec::Specs;

use parse::LineContext;
use spec::{Arg, Opt, Spec, Suggestion};

/// the most candidates one completion gives, the best ones
const MAX_CANDIDATES: usize = 200;

/// commands that run the command after them
const WRAPPERS: &[&str] = &[
    "builtin", "command", "doas", "env", "exec", "nice", "nohup", "sudo", "time", "watch", "xargs",
];

// priority of specs that set none, as in fig
const DEFAULT_PRIORITY: u8 = 50;

/// what a candidate is, listed in this order when they match equally well
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Subcommand,
    Suggestion,
    Option,
    Command,
    Folder,
    File,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// shown in the list
    pub label: String,
    /// typed in place of the word, quoted for the shell
    pub insert: String,
    pub description: Option<String>,
    pub kind: Kind,
    /// how well it matches, lower is better
    score: u8,
    priority: u8,
}

/// a command line and where it is typed
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub buffer: String,
    /// in chars
    pub cursor: usize,
    pub cwd: PathBuf,
    /// PATH of the shell
    pub path_var: String,
    pub home: Option<PathBuf>,
}

/// candidates for the word at the cursor, which they replace
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Completions {
    /// char range of the typed word in the buffer
    pub start: usize,
    pub end: usize,
    /// the typed word as it is in the buffer, with its quotes
    pub typed: String,
    /// best first
    pub candidates: Vec<Candidate>,
}

/// candidates for the word the cursor is at the end of
pub fn complete(request: &Request, specs: &Specs) -> Completions {
    let chars: Vec<char> = request.buffer.chars().collect();
    let cursor = request.cursor.min(chars.len());
    let context = parse::parse(&chars[..cursor]);
    let mut found = Found {
        query: &context.prefix,
        candidates: Vec::new(),
    };
    match command_words(&context.words) {
        _ if context.redirect => add_paths(&mut found, &context, request, true),
        // the value of a wrapper's option, like the user of sudo -u
        None => {}
        Some([command, args @ ..]) => {
            let name = Path::new(command)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(command);
            match specs.get(name) {
                Some(spec) => add_from_spec(&mut found, specs, spec, args, &context, request),
                // files are what most commands take
                None if !context.prefix.starts_with('-') => {
                    add_paths(&mut found, &context, request, true)
                }
                None => {}
            }
        }
        Some([]) if context.prefix.contains('/') || context.prefix.starts_with('.') => {
            add_paths(&mut found, &context, request, true)
        }
        Some([]) => add_commands(&mut found, &context, request, specs),
    }
    let mut candidates = found.candidates;
    candidates.sort_by(|a, b| {
        (a.score, std::cmp::Reverse(a.priority), a.kind)
            .cmp(&(b.score, std::cmp::Reverse(b.priority), b.kind))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
    candidates.dedup_by(|a, b| a.insert == b.insert);
    candidates.truncate(MAX_CANDIDATES);
    Completions {
        start: context.start,
        end: cursor,
        typed: chars[context.start..cursor].iter().collect(),
        candidates,
    }
}

/// options of a wrapper that take the next word as their value, like `sudo -u root`
fn wrapper_values(wrapper: &str) -> &'static [&'static str] {
    match wrapper {
        "sudo" => &[
            "-u", "-g", "-h", "-p", "-C", "-D", "-R", "-T", "-U", "-r", "-t", "--user", "--group",
            "--host", "--prompt", "--chdir",
        ],
        "doas" => &["-u", "-C"],
        "env" => &["-u", "-C", "-S", "--unset", "--chdir", "--split-string"],
        "nice" => &["-n", "--adjustment"],
        "watch" => &["-n", "--interval"],
        "xargs" => &[
            "-I",
            "-n",
            "-L",
            "-P",
            "-d",
            "-E",
            "-s",
            "-a",
            "--max-args",
            "--max-procs",
            "--delimiter",
            "--arg-file",
        ],
        "time" => &["-f", "-o", "--format", "--output"],
        "exec" => &["-a"],
        _ => &[],
    }
}

/// the words from the command on, past variable assignments and wrappers like sudo with
/// their options. none when the typed word is the value of such an option
fn command_words(words: &[String]) -> Option<&[String]> {
    let mut ix = 0;
    let mut wrapper = None;
    while let Some(word) = words.get(ix) {
        let assignment = word.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
        });
        if WRAPPERS.contains(&word.as_str()) {
            wrapper = Some(word.as_str());
        } else if let Some(wrapper) = wrapper.filter(|_| word.starts_with('-')) {
            if wrapper_values(wrapper).contains(&word.as_str()) {
                ix += 1;
            }
        } else if !assignment {
            break;
        }
        ix += 1;
    }
    words.get(ix..)
}

/// how well `name` matches what was typed, none when it doesn't
fn score(name: &str, query: &str) -> Option<u8> {
    if name.starts_with(query) {
        return Some(0);
    }
    let (name, query) = (name.to_lowercase(), query.to_lowercase());
    if name.starts_with(&query) {
        Some(1)
    } else if query.chars().count() >= 3 && name.contains(&query) {
        Some(2)
    } else {
        None
    }
}

struct Found<'a> {
    query: &'a str,
    candidates: Vec<Candidate>,
}

impl Found<'_> {
    /// add a candidate when one of `names` matches, it shows the best matching one
    fn add(
        &mut self,
        names: &[String],
        insert: impl FnOnce(&str) -> String,
        description: Option<&String>,
        kind: Kind,
        priority: Option<u8>,
        hidden: bool,
    ) -> Option<&mut Candidate> {
        let (score, name) = names
            .iter()
            .filter_map(|name| Some((score(name, self.query)?, name)))
            .min_by_key(|(score, name)| (*score, std::cmp::Reverse(name.len())))?;
        // hidden ones only once typed in full
        if (hidden && name != self.query) || name.chars().any(char::is_control) {
            return None;
        }
        let label = match names {
            [_, _, ..] if kind == Kind::Option => names.join(", "),
            _ => name.clone(),
        };
        self.candidates.push(Candidate {
            label,
            insert: insert(name),
            description: description.cloned(),
            kind,
            score,
            priority: priority.unwrap_or(DEFAULT_PRIORITY).min(100),
        });
        self.candidates.last_mut()
    }
}

fn add_commands(found: &mut Found, context: &LineContext, request: &Request, specs: &Specs) {
    for name in sources::programs(&request.path_var).iter() {
        // a spec is only parsed for its description once its command matches
        if score(name, found.query).is_none() {
            continue;
        }
        let description = specs.get(name).and_then(|spec| spec.description.as_ref());
        let insert = |name: &str| format!("{} ", sources::escape(name, context.quote, true));
        found.add(
            std::slice::from_ref(name),
            insert,
            description,
            Kind::Command,
            None,
            false,
        );
    }
}

/// files and folders the typed word could be, `files` false keeps folders only
fn add_paths(found: &mut Found, context: &LineContext, request: &Request, files: bool) {
    let typed = &context.prefix;
    // only the name after the last slash is matched
    let query = &typed[typed.rfind('/').map_or(0, |ix| ix + 1)..];
    let mut paths = Found {
        query,
        candidates: Vec::new(),
    };
    for entry in sources::entries(typed, &request.cwd, request.home.as_deref()) {
        if !entry.dir && !files {
            continue;
        }
        let (kind, label) = if entry.dir {
            (Kind::Folder, format!("{}/", entry.name))
        } else {
            (Kind::File, entry.name.clone())
        };
        // a folder stays open, so its contents can be completed next
        let insert = |_: &str| match entry.dir {
            true => sources::escape(&format!("{}/", entry.path), context.quote, false),
            false => format!("{} ", sources::escape(&entry.path, context.quote, true)),
        };
        let added = paths.add(
            std::slice::from_ref(&entry.name),
            insert,
            None,
            kind,
            None,
            false,
        );
        if let Some(candidate) = added {
            candidate.label = label;
        }
    }
    found.candidates.append(&mut paths.candidates);
}

fn add_arg(found: &mut Found, arg: &Arg, context: &LineContext, request: &Request) {
    let insert = |name: &str| format!("{} ", sources::escape(name, context.quote, true));
    for suggestion in &arg.suggestions {
        let (names, description, priority, hidden) = match suggestion {
            Suggestion::Plain(name) => (std::slice::from_ref(name), &arg.description, None, false),
            Suggestion::Full {
                name,
                description,
                priority,
                hidden,
            } => (name.all(), description, *priority, *hidden),
        };
        let kind = Kind::Suggestion;
        found.add(names, insert, description.as_ref(), kind, priority, hidden);
    }
    if arg.template.folders() {
        add_paths(found, context, request, arg.template.files());
    }
}

/// what goes next after the words typed after the command, by its spec: like fig, the
/// words pick subcommands, options and their arguments, the rest are arguments
fn add_from_spec<'a>(
    found: &mut Found,
    specs: &'a Specs,
    spec: &'a Spec,
    words: &[String],
    context: &LineContext,
    request: &Request,
) {
    let mut node: &Spec = spec;
    let mut persistent: Vec<&Opt> = spec.options.iter().filter(|o| o.is_persistent).collect();
    let mut used: Vec<&Opt> = Vec::new();
    // arguments of the last option that are still to come, with the index of the next one
    let mut pending: Option<(&Opt, usize)> = None;
    let mut arg_ix = 0;
    let mut options_done = false;
    for word in words {
        if let Some((option, ix)) = pending.take() {
            if ix + 1 < option.args.all().len() {
                pending = Some((option, ix + 1));
            }
            continue;
        }
        if !options_done && word == "--" {
            options_done = true;
            continue;
        }
        if !options_done && word.starts_with('-') && word.len() > 1 {
            let (name, value) = match word.split_once('=') {
                Some((name, _)) => (name, true),
                None => (word.as_str(), false),
            };
            let option = node
                .options
                .iter()
                .chain(persistent.iter().copied())
                .find(|option| option.name.contains(name));
            if let Some(option) = option {
                used.push(option);
                let takes = option
                    .args
                    .all()
                    .first()
                    .is_some_and(|arg| !arg.is_optional);
                if takes && !value {
                    pending = Some((option, 0));
                }
            }
            continue;
        }
        if arg_ix == 0
            && let Some(sub) = node.subcommands.iter().find(|sub| sub.name.contains(word))
        {
            // big specs keep a subcommand's content in a spec of its own
            node = sub
                .load_spec()
                .and_then(|name| specs.get(name))
                .unwrap_or(sub);
            persistent.extend(sub.options.iter().filter(|o| o.is_persistent));
            continue;
        }
        arg_ix += 1;
    }

    if let Some((option, ix)) = pending {
        if let Some(arg) = option.args.at(ix) {
            add_arg(found, arg, context, request);
        }
        return;
    }
    if let Some(name) = &context.option {
        let option = node
            .options
            .iter()
            .chain(persistent.iter().copied())
            .find(|option| option.name.contains(name));
        if let Some(arg) = option.and_then(|option| option.args.at(0)) {
            add_arg(found, arg, context, request);
        }
        return;
    }
    if !options_done && context.prefix.starts_with('-') {
        let options = node.options.iter().chain(persistent.iter().copied());
        for option in options {
            let given = used.iter().any(|used| std::ptr::eq(*used, option));
            if given && !option.repeatable() {
                continue;
            }
            let insert = |name: &str| format!("{name} ");
            found.add(
                option.name.all(),
                insert,
                option.description.as_ref(),
                Kind::Option,
                option.priority,
                option.hidden,
            );
        }
        return;
    }
    if arg_ix == 0 {
        for sub in &node.subcommands {
            let insert = |name: &str| format!("{} ", sources::escape(name, context.quote, true));
            found.add(
                sub.name.all(),
                insert,
                sub.description.as_ref(),
                Kind::Subcommand,
                sub.priority,
                sub.hidden,
            );
        }
    }
    if let Some(arg) = node.args.at(arg_ix) {
        add_arg(found, arg, context, request);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOOL: &str = r#"{
        "name": "tool",
        "description": "a test tool",
        "options": [
            { "name": ["-v", "--verbose"], "description": "say more", "isPersistent": true },
            { "name": "--color", "args": { "suggestions": ["always", "never"] } },
            { "name": "--out", "args": { "template": "folders" } },
            { "name": "--secret", "hidden": true },
        ],
        "subcommands": [
            { "name": ["remove", "rm"], "description": "remove files",
              "args": { "template": "filepaths", "isVariadic": true } },
            { "name": "run", "priority": 80, "options": [{ "name": "--fast" }],
              "args": [{ "suggestions": [{ "name": "first", "description": "1st" }] }, { "suggestions": ["second"] }] },
            { "name": "Rebuild" },
        ],
    }"#;

    /// a folder with "sub dir/" and "file one", with "tool" as the only program in PATH
    fn fixture(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kuterm-complete-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("sub dir/deeper")).unwrap();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("file one"), "").unwrap();
        std::fs::write(dir.join("sub dir/inner.txt"), "").unwrap();
        let tool = dir.join("bin/tool");
        std::fs::write(&tool, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        dir
    }

    fn complete_at(dir: &Path, line: &str, cursor: usize) -> Completions {
        let mut specs = Specs::load(None);
        specs.insert("tool", TOOL);
        complete(
            &Request {
                buffer: line.into(),
                cursor,
                cwd: dir.to_path_buf(),
                path_var: dir.join("bin").to_string_lossy().into_owned(),
                home: Some(dir.join("sub dir")),
            },
            &specs,
        )
    }

    fn complete_in(dir: &Path, line: &str) -> Completions {
        complete_at(dir, line, line.chars().count())
    }

    fn labels(completions: &Completions) -> Vec<&str> {
        completions
            .candidates
            .iter()
            .map(|c| c.label.as_str())
            .collect()
    }

    fn inserts(completions: &Completions) -> Vec<&str> {
        completions
            .candidates
            .iter()
            .map(|c| c.insert.as_str())
            .collect()
    }

    #[test]
    fn commands_come_first_then_what_their_spec_says() {
        let dir = fixture("spec");
        let completions = complete_in(&dir, "to");
        assert_eq!(labels(&completions), ["tool"]);
        let tool = &completions.candidates[0];
        assert_eq!(tool.insert, "tool ");
        assert_eq!(tool.description.as_deref(), Some("a test tool"));
        assert_eq!((completions.start, completions.end), (0, 2));

        // higher priority first, then by name, case doesn't matter
        let completions = complete_in(&dir, "tool ");
        assert_eq!(labels(&completions), ["run", "Rebuild", "remove"]);
        assert_eq!(completions.start, 5);
        // an alias matches too
        assert_eq!(
            inserts(&complete_in(&dir, "tool r")),
            ["run ", "remove ", "Rebuild "]
        );
        assert_eq!(inserts(&complete_in(&dir, "sudo FOO=1 tool rm")), ["rm "]);
        // options of a wrapper and their values are skipped
        assert_eq!(
            inserts(&complete_in(&dir, "sudo -E -u root tool rm")),
            ["rm "]
        );
        assert_eq!(labels(&complete_in(&dir, "sudo -u to")), Vec::<&str>::new());
        assert_eq!(labels(&complete_in(&dir, "sudo -E to")), ["tool"]);
        // exact case first
        assert_eq!(labels(&complete_in(&dir, "tool Re")), ["Rebuild", "remove"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn options_and_their_arguments() {
        let dir = fixture("options");
        let completions = complete_in(&dir, "tool --");
        // hidden ones only show once typed
        assert_eq!(labels(&completions), ["--color", "--out", "-v, --verbose"]);
        assert_eq!(inserts(&completions)[2], "--verbose ");
        assert_eq!(labels(&complete_in(&dir, "tool --secret")), ["--secret"]);
        // given ones are not offered again, persistent ones reach subcommands
        assert_eq!(
            labels(&complete_in(&dir, "tool run -")),
            ["--fast", "-v, --verbose"]
        );
        assert_eq!(labels(&complete_in(&dir, "tool run -v -")), ["--fast"]);

        assert_eq!(
            labels(&complete_in(&dir, "tool --color ")),
            ["always", "never"]
        );
        assert_eq!(
            labels(&complete_in(&dir, "tool --color=never ")),
            ["run", "Rebuild", "remove"]
        );
        // an option argument is no subcommand
        assert_eq!(
            labels(&complete_in(&dir, "tool --color never ")),
            ["run", "Rebuild", "remove"]
        );
        assert_eq!(
            labels(&complete_in(&dir, "tool --out ")),
            ["bin/", "sub dir/"]
        );
        // the value of --name= completes in place
        let value = complete_in(&dir, "tool --color=n");
        assert_eq!(inserts(&value), ["never "]);
        assert_eq!((value.start, value.end), (13, 14));
        assert_eq!(labels(&complete_in(&dir, "tool --out=s")), ["sub dir/"]);
        assert!(complete_in(&dir, "tool --verbose=").candidates.is_empty());
        // after -- everything is an argument
        assert!(complete_in(&dir, "tool rm -- -").candidates.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn positional_arguments_in_order() {
        let dir = fixture("args");
        let completions = complete_in(&dir, "tool run ");
        assert_eq!(labels(&completions), ["first"]);
        assert_eq!(
            completions.candidates[0].description.as_deref(),
            Some("1st")
        );
        assert_eq!(labels(&complete_in(&dir, "tool run first ")), ["second"]);
        assert!(
            complete_in(&dir, "tool run first second ")
                .candidates
                .is_empty()
        );
        // variadic file arguments go on
        assert_eq!(
            labels(&complete_in(&dir, "tool rm a b ")),
            ["bin/", "sub dir/", "file one"]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn paths_are_quoted_and_folders_stay_open() {
        let dir = fixture("paths");
        // commands without a spec take files
        let completions = complete_in(&dir, "cat ");
        assert_eq!(labels(&completions), ["bin/", "sub dir/", "file one"]);
        assert_eq!(inserts(&completions), ["bin/", "sub\\ dir/", "file\\ one "]);
        assert_eq!(
            inserts(&complete_in(&dir, "cat sub\\ dir/in")),
            ["sub\\ dir/inner.txt "]
        );
        assert_eq!(inserts(&complete_in(&dir, "cat 'sub")), ["'sub dir/"]);
        assert_eq!(
            inserts(&complete_in(&dir, "cat 'sub dir/i")),
            ["'sub dir/inner.txt' "]
        );
        assert_eq!(inserts(&complete_in(&dir, "cat \"fi")), ["\"file one\" "]);
        assert_eq!(inserts(&complete_in(&dir, "cat ~/de")), ["~/deeper/"]);
        // redirections and paths typed as the command
        assert_eq!(labels(&complete_in(&dir, "tool run > fi")), ["file one"]);
        assert_eq!(labels(&complete_in(&dir, "./bi")), ["bin/"]);
        // cd only takes folders
        assert_eq!(labels(&complete_in(&dir, "cd ")), ["bin/", "sub dir/"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_word_ends_at_the_cursor() {
        let dir = fixture("cursor");
        // typed "tool r|m x", the text after the cursor is left alone
        let completions = complete_at(&dir, "tool rm x", 6);
        assert_eq!((completions.start, completions.end), (5, 6));
        assert_eq!(completions.typed, "r");
        assert_eq!(labels(&completions)[0], "run");
        // a cursor past the end is the end
        assert_eq!(complete_at(&dir, "to", 9).end, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn services_of_a_bundle_are_walked_into() {
        let dir = fixture("bundle");
        assert!(labels(&complete_in(&dir, "aws ec")).contains(&"ec2"));
        let operations = complete_in(&dir, "aws ec2 describe-inst");
        assert!(labels(&operations).contains(&"describe-instances"));
        assert!(operations.candidates[0].description.is_some());
        let options = complete_in(&dir, "aws ec2 describe-instances --inst");
        assert_eq!(labels(&options), ["--instance-ids"]);
        // options of the aws command itself work after the service too
        let global = complete_in(&dir, "aws ec2 describe-instances --regi");
        assert_eq!(labels(&global), ["--region"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn matches_are_ranked() {
        assert_eq!(score("checkout", "che"), Some(0));
        assert_eq!(score("Checkout", "che"), Some(1));
        assert_eq!(score("git-checkout", "check"), Some(2));
        // short queries only match the start, or everything would match
        assert_eq!(score("ab", "b"), None);
        assert_eq!(score("x", ""), Some(0));
    }

    #[test]
    fn options_with_several_arguments_and_unknown_options() {
        let dir = fixture("option-args");
        let mut specs = Specs::load(None);
        specs.insert(
            "pair",
            r#"{
                "name": "pair",
                "options": [{ "name": "--size",
                    "args": [{ "suggestions": ["wide"] }, { "suggestions": ["tall"] }] }],
                "subcommands": [{ "name": "go" }],
            }"#,
        );
        let at = |line: &str| {
            let request = Request {
                buffer: line.into(),
                cursor: line.chars().count(),
                cwd: dir.clone(),
                path_var: dir.join("bin").to_string_lossy().into_owned(),
                home: None,
            };
            let completions = complete(&request, &specs);
            completions
                .candidates
                .iter()
                .map(|c| c.label.clone())
                .collect::<Vec<_>>()
        };
        // each argument of the option is asked for in turn, then the subcommands come back
        assert_eq!(at("pair --size "), ["wide"]);
        assert_eq!(at("pair --size wide "), ["tall"]);
        assert_eq!(at("pair --size wide tall "), ["go"]);
        // an option the spec doesn't know takes no argument, the next word is a subcommand
        assert_eq!(
            labels(&complete_in(&dir, "tool --nope ")),
            ["run", "Rebuild", "remove"]
        );
        assert_eq!(labels(&complete_in(&dir, "tool --nope run ")), ["first"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
