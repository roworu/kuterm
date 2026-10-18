//! completion specs: what a command takes, in the json form of fig's specs
//! (github.com/withfig/autocomplete). only the declarative parts are read, generators and
//! scripts are ignored, so a spec never runs anything

use std::{borrow::Cow, collections::HashMap, io::Read, path::Path, sync::OnceLock};

use serde::Deserialize;
use serde_json_lenient::Value;

macro_rules! spec {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!("../../assets/completions/", $name, ".json")),
        )
    };
}

/// specs kuterm brings, by the command they are for. sh tools/completions/generate.sh makes
/// most of them, see docs/COMPLETIONS.md
const BUNDLED: &[(&str, &str)] = &[
    spec!("apk"),
    spec!("apt"),
    spec!("apt-cache"),
    spec!("apt-get"),
    spec!("basename"),
    spec!("cargo"),
    spec!("cat"),
    spec!("cd"),
    spec!("chgrp"),
    spec!("chmod"),
    spec!("chown"),
    spec!("cp"),
    spec!("crontab"),
    spec!("curl"),
    spec!("cut"),
    spec!("date"),
    spec!("df"),
    spec!("diff"),
    spec!("dirname"),
    spec!("dnf"),
    spec!("docker"),
    spec!("dpkg"),
    spec!("du"),
    spec!("file"),
    spec!("find"),
    spec!("flatpak"),
    spec!("free"),
    spec!("git"),
    spec!("grep"),
    spec!("gunzip"),
    spec!("gzip"),
    spec!("head"),
    spec!("hostnamectl"),
    spec!("ip"),
    spec!("journalctl"),
    spec!("kill"),
    spec!("killall"),
    spec!("less"),
    spec!("ln"),
    spec!("ls"),
    spec!("man"),
    spec!("mkdir"),
    spec!("mktemp"),
    spec!("mount"),
    spec!("mv"),
    spec!("npm"),
    spec!("pacman"),
    spec!("passwd"),
    spec!("ping"),
    spec!("pip"),
    spec!("pip3"),
    spec!("ps"),
    spec!("realpath"),
    spec!("rm"),
    spec!("rmdir"),
    spec!("rpm"),
    spec!("rsync"),
    spec!("scp"),
    spec!("sed"),
    spec!("snap"),
    spec!("sort"),
    spec!("ss"),
    spec!("ssh"),
    spec!("stat"),
    spec!("su"),
    spec!("systemctl"),
    spec!("tail"),
    spec!("tar"),
    spec!("tee"),
    spec!("timedatectl"),
    spec!("touch"),
    spec!("tr"),
    spec!("umount"),
    spec!("uname"),
    spec!("uniq"),
    spec!("unzip"),
    spec!("useradd"),
    spec!("userdel"),
    spec!("usermod"),
    spec!("wc"),
    spec!("wget"),
    spec!("which"),
    spec!("xz"),
    spec!("yum"),
    spec!("zip"),
    spec!("zypper"),
];

/// specs too big to read whole: gzipped json lines, the command's spec first, then the specs
/// its subcommands point at with "loadSpec", like "aws/ec2"
const BUNDLES: &[(&str, &[u8])] = &[(
    "aws",
    include_bytes!("../../assets/completions/aws.jsonl.gz"),
)];

/// one name or several that mean the same, like `["-v", "--verbose"]`
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Names {
    One(String),
    Many(Vec<String>),
}

impl Default for Names {
    fn default() -> Self {
        Names::Many(Vec::new())
    }
}

impl Names {
    pub fn all(&self) -> &[String] {
        match self {
            Names::One(name) => std::slice::from_ref(name),
            Names::Many(names) => names,
        }
    }

    pub fn contains(&self, word: &str) -> bool {
        self.all().iter().any(|name| name == word)
    }
}

/// a command or subcommand
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Spec {
    pub name: Names,
    pub description: Option<String>,
    pub subcommands: Vec<Spec>,
    pub options: Vec<Opt>,
    pub args: Args,
    /// 0 to 100, higher is listed first, 50 when not set
    pub priority: Option<u8>,
    pub hidden: bool,
    /// name of the spec that has the rest of this subcommand. fig also allows code here,
    /// only a plain name is used
    pub load_spec: Value,
}

impl Spec {
    /// the spec this subcommand's content is in, none when it is all here
    pub fn load_spec(&self) -> Option<&str> {
        self.load_spec.as_str()
    }
}

/// a flag like `--verbose`, maybe taking arguments
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Opt {
    pub name: Names,
    pub description: Option<String>,
    pub args: Args,
    /// subcommands take it too
    pub is_persistent: bool,
    /// true or how many times it may be given, once when not set
    pub is_repeatable: Value,
    pub priority: Option<u8>,
    pub hidden: bool,
}

impl Opt {
    pub fn repeatable(&self) -> bool {
        match &self.is_repeatable {
            Value::Bool(repeatable) => *repeatable,
            Value::Number(times) => times.as_u64().is_some_and(|times| times > 1),
            _ => false,
        }
    }
}

/// one argument or a list of them, in order
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Args {
    // first, a struct would also take a list as its fields in order
    Many(Vec<Arg>),
    One(Box<Arg>),
}

impl Default for Args {
    fn default() -> Self {
        Args::Many(Vec::new())
    }
}

impl Args {
    pub fn all(&self) -> &[Arg] {
        match self {
            Args::Many(args) => args,
            Args::One(arg) => std::slice::from_ref(&**arg),
        }
    }

    /// the argument at `ix`, the last one again when it takes any number
    pub fn at(&self, ix: usize) -> Option<&Arg> {
        let all = self.all();
        all.get(ix)
            .or_else(|| all.last().filter(|arg| arg.is_variadic))
    }
}

/// a positional argument
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Arg {
    pub name: Option<String>,
    pub description: Option<String>,
    pub suggestions: Vec<Suggestion>,
    /// "filepaths", "folders" or both
    pub template: Templates,
    pub is_optional: bool,
    pub is_variadic: bool,
}

/// a fixed value an argument can take
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Suggestion {
    Plain(String),
    Full {
        name: Names,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        priority: Option<u8>,
        #[serde(default)]
        hidden: bool,
    },
}

/// files an argument takes
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Templates {
    One(String),
    Many(Vec<String>),
}

impl Default for Templates {
    fn default() -> Self {
        Templates::Many(Vec::new())
    }
}

impl Templates {
    fn has(&self, template: &str) -> bool {
        match self {
            Templates::One(one) => one == template,
            Templates::Many(many) => many.iter().any(|one| one == template),
        }
    }

    pub fn files(&self) -> bool {
        self.has("filepaths")
    }

    /// folders only, or files too
    pub fn folders(&self) -> bool {
        self.has("folders") || self.files()
    }
}

/// a spec's json, parsed the first time it is asked for
struct Entry {
    json: Cow<'static, str>,
    /// where the json is from, for errors
    origin: String,
    spec: OnceLock<Option<Spec>>,
}

impl Entry {
    fn new(json: impl Into<Cow<'static, str>>, origin: String) -> Entry {
        Entry {
            json: json.into(),
            origin,
            spec: OnceLock::new(),
        }
    }

    // a broken spec says so once, then counts as missing
    fn spec(&self) -> Option<&Spec> {
        self.spec
            .get_or_init(|| {
                parse(&self.json)
                    .inspect_err(|error| {
                        eprintln!("completion spec {} is broken: {error}", self.origin)
                    })
                    .ok()
            })
            .as_ref()
    }
}

/// a bundle, unpacked the first time one of its specs is asked for
struct Bundle {
    gz: &'static [u8],
    entries: OnceLock<HashMap<String, Entry>>,
}

impl Bundle {
    fn entries(&self, command: &str) -> &HashMap<String, Entry> {
        self.entries.get_or_init(|| {
            let mut json = String::new();
            let read = flate2::read::GzDecoder::new(self.gz).read_to_string(&mut json);
            if let Err(error) = read {
                eprintln!("bundled completion specs of {command} are broken: {error}");
                return HashMap::new();
            }
            // generated lines start with their name, so finding it needs no json parser
            json.lines()
                .filter_map(|line| {
                    let rest = line.strip_prefix(r#"{"name":""#)?;
                    let name = &rest[..rest.find('"')?];
                    let origin = format!("{name} of the {command} bundle");
                    Some((name.to_string(), Entry::new(line.to_string(), origin)))
                })
                .collect()
        })
    }
}

/// specs by the command they are for
#[derive(Default)]
pub struct Specs {
    entries: HashMap<String, Entry>,
    bundles: HashMap<String, Bundle>,
}

impl Specs {
    /// the bundled specs, with every `<command>.json` of `dir` over them. each is only parsed
    /// when its command is completed, a broken one then shows an error on stderr
    pub fn load(dir: Option<&Path>) -> Specs {
        let mut specs = Specs::default();
        for (name, json) in BUNDLED {
            let entry = Entry::new(*json, format!("{name} bundled with kuterm"));
            specs.entries.insert(name.to_string(), entry);
        }
        for (command, gz) in BUNDLES {
            let bundle = Bundle {
                gz,
                entries: OnceLock::new(),
            };
            specs.bundles.insert(command.to_string(), bundle);
        }
        let Some(entries) = dir.and_then(|dir| std::fs::read_dir(dir).ok()) else {
            return specs;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            let (Some(name), Some("json")) = (
                path.file_stem().and_then(|stem| stem.to_str()),
                path.extension().and_then(|ext| ext.to_str()),
            ) else {
                continue;
            };
            match crate::settings::read_small_file(&path) {
                Ok(json) => {
                    let entry = Entry::new(json, path.display().to_string());
                    specs.entries.insert(name.to_string(), entry);
                }
                Err(error) => eprintln!("can't read {}: {error}", path.display()),
            }
        }
        specs
    }

    /// add a spec from json, as if it was a file of the specs folder
    #[cfg(test)]
    pub(crate) fn insert(&mut self, name: &str, json: &'static str) {
        let entry = Entry::new(json, name.to_string());
        self.entries.insert(name.to_string(), entry);
    }

    /// spec of the command `name`, or one a "loadSpec" names, like "aws/ec2"
    pub fn get(&self, name: &str) -> Option<&Spec> {
        if let Some(entry) = self.entries.get(name) {
            return entry.spec();
        }
        let command = name.split('/').next()?;
        let bundle = self.bundles.get(command)?;
        bundle.entries(command).get(name)?.spec()
    }
}

/// a spec from json, rejecting specs without a name
pub fn parse(json: &str) -> Result<Spec, String> {
    let spec: Spec = serde_json_lenient::from_str(json).map_err(|error| error.to_string())?;
    check(&spec, "spec")?;
    Ok(spec)
}

// every command, subcommand and option needs a name to be typed by
fn check(spec: &Spec, at: &str) -> Result<(), String> {
    let named = |names: &Names| names.all().iter().any(|name| !name.is_empty());
    if !named(&spec.name) {
        return Err(format!("{at}: a command without a name"));
    }
    let at = &spec.name.all()[0];
    if let Some(option) = spec.options.iter().find(|option| !named(&option.name)) {
        return Err(format!("{at}: an option without a name: {option:?}"));
    }
    spec.subcommands.iter().try_for_each(|sub| check(sub, at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_spec_is_listed_and_parses() {
        let mut listed: Vec<String> = BUNDLED
            .iter()
            .map(|(name, _)| format!("{name}.json"))
            .collect();
        listed.extend(BUNDLES.iter().map(|(name, _)| format!("{name}.jsonl.gz")));
        listed.sort();
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/completions");
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(
            listed, files,
            "BUNDLED and BUNDLES must list every file of {dir}"
        );

        let specs = Specs::load(None);
        for (name, _) in BUNDLED {
            let spec = specs
                .get(name)
                .unwrap_or_else(|| panic!("{name} doesn't parse"));
            assert!(spec.name.contains(name), "{name} is named {:?}", spec.name);
        }
        // every "loadSpec" leads somewhere
        for (command, _) in BUNDLES {
            let spec = specs
                .get(command)
                .unwrap_or_else(|| panic!("{command} doesn't parse"));
            for sub in &spec.subcommands {
                if let Some(load) = sub.load_spec() {
                    assert!(specs.get(load).is_some(), "{command}: {load} is missing");
                }
            }
        }
    }

    #[test]
    fn bundles_are_read_a_part_at_a_time() {
        let specs = Specs::load(None);
        let aws = specs.get("aws").unwrap();
        let ec2 = aws
            .subcommands
            .iter()
            .find(|s| s.name.contains("ec2"))
            .unwrap();
        assert_eq!(ec2.load_spec(), Some("aws/ec2"));
        let bundle = &specs.bundles["aws"];
        let parsed = bundle
            .entries("aws")
            .values()
            .filter(|e| e.spec.get().is_some())
            .count();
        assert_eq!(parsed, 1, "only the aws spec itself is parsed");
        let ec2 = specs.get("aws/ec2").unwrap();
        let describe = ec2
            .subcommands
            .iter()
            .find(|s| s.name.contains("describe-instances"))
            .unwrap();
        assert!(
            describe
                .options
                .iter()
                .any(|o| o.name.contains("--instance-ids"))
        );
    }

    #[test]
    fn fig_shapes_are_read() {
        let spec = parse(
            r#"{
                "name": "tool",
                "subcommands": [{ "name": ["remove", "rm"], "args": { "template": "filepaths", "isVariadic": true } }],
                "options": [
                    { "name": ["-v", "--verbose"], "isRepeatable": 3 },
                    { "name": "--out", "args": { "name": "file", "template": ["folders"] }, "isPersistent": true },
                ],
                "args": [{ "suggestions": ["a", { "name": ["b", "bee"], "description": "B" }] }, { "name": "n" }],
                // generators run commands, they are never read
                "generators": { "script": ["rm", "-rf", "/"] },
            }"#,
        )
        .unwrap();
        assert!(spec.subcommands[0].name.contains("rm"));
        assert!(spec.subcommands[0].args.at(5).unwrap().template.files());
        assert!(spec.options[0].repeatable());
        let out = &spec.options[1];
        assert!(out.is_persistent && out.args.at(0).unwrap().template.folders());
        assert!(!out.args.at(0).unwrap().template.files());
        assert_eq!(spec.args.all().len(), 2);
        assert!(spec.args.at(2).is_none());
        assert!(
            matches!(&spec.args.all()[0].suggestions[1], Suggestion::Full { name, .. } if name.contains("bee"))
        );
    }

    #[test]
    fn broken_specs_are_rejected() {
        assert!(parse("{").is_err());
        assert!(parse(r#"{"description": "no name"}"#).is_err());
        assert!(parse(r#"{"name": "a", "options": [{"description": "x"}]}"#).is_err());
        assert!(parse(r#"{"name": "a", "subcommands": [{"name": ""}]}"#).is_err());
        assert!(parse(r#"{"name": "a", "options": "nope"}"#).is_err());
    }

    #[test]
    fn user_specs_go_over_bundled_ones_and_broken_ones_are_skipped() {
        let dir = std::env::temp_dir().join(format!("kuterm-specs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let git = r#"{"name": "git", "description": "mine"}"#;
        std::fs::write(dir.join("git.json"), git).unwrap();
        std::fs::write(dir.join("new.json"), r#"{"name": "new"}"#).unwrap();
        std::fs::write(dir.join("bad.json"), "{ nope").unwrap();
        std::fs::write(dir.join("notes.txt"), r#"{"name": "notes"}"#).unwrap();
        let specs = Specs::load(Some(&dir));
        std::fs::remove_dir_all(&dir).ok();
        let git = specs.get("git").unwrap();
        assert_eq!(git.description.as_deref(), Some("mine"));
        assert!(specs.get("new").is_some());
        assert!(specs.get("cargo").is_some());
        assert!(specs.get("bad").is_none());
        assert!(specs.get("notes").is_none());
        assert!(specs.get("missing/part").is_none());
    }

    #[test]
    fn is_repeatable_takes_a_bool_or_a_count() {
        let repeatable = |value: &str| {
            let json = format!(
                r#"{{"name": "t", "options": [{{"name": "-v", "isRepeatable": {value}}}]}}"#
            );
            parse(&json).unwrap().options[0].repeatable()
        };
        assert!(repeatable("true"));
        assert!(!repeatable("false"));
        // a count of one is the same as not repeatable
        assert!(!repeatable("1"));
        assert!(repeatable("2"));
        assert!(!repeatable(r#""often""#));
    }

    #[test]
    fn a_folder_named_like_a_spec_is_skipped() {
        let dir = std::env::temp_dir().join(format!("kuterm-specs-dir-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder.json")).unwrap();
        std::fs::write(dir.join("fine.json"), r#"{"name": "fine"}"#).unwrap();
        let specs = Specs::load(Some(&dir));
        std::fs::remove_dir_all(&dir).ok();
        assert!(specs.get("folder").is_none());
        assert!(specs.get("fine").is_some());
    }
}
