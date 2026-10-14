//! building tab titles from the blocks set in settings

use std::{
    collections::HashMap,
    io::Read,
    ops::Range,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::{
    settings::{MemoryUnit, TabTitleBlock},
    terminal::{ForegroundProcess, Progress, TreeSample},
};

// a hung command would freeze every tab title, so it is abandoned after this
const EXEC_TIMEOUT: Duration = Duration::from_secs(2);
// git status walks the whole work tree, too slow to run on every title refresh
const GIT_CACHE_FOR: Duration = Duration::from_secs(2);
// nerd font branch
const GIT_ICON: &str = "\u{e0a0}";
// samples a cpu graph shows as text, two per braille character
const TEXT_GRAPH_SAMPLES: usize = 16;
// nerd font spinner frames, titles are built with the first and turned with `spin`
const SPINNER: [char; 6] = [
    '\u{ee06}', '\u{ee07}', '\u{ee08}', '\u{ee09}', '\u{ee0a}', '\u{ee0b}',
];
/// time between spinner frames
pub(super) const SPINNER_INTERVAL: Duration = Duration::from_millis(120);

/// what a title is built from, collected on the main thread
pub(super) struct TitleInputs {
    pub number: usize,
    pub shell_pid: u32,
    pub title: String,
    pub profile_icon: Option<String>,
    pub progress: Option<Progress>,
    /// numbers of this tab's processes, none while nothing shows them
    pub sample: Option<TreeSample>,
    /// numbers of all tabs added up, none while nothing shows them
    pub total: Option<TreeSample>,
    pub memory_unit: MemoryUnit,
    /// time of day for clock blocks
    pub clock: String,
    /// columns and rows of the terminal
    pub size: (usize, usize),
}

/// byte range in a title and the port written there
pub(super) type PortRanges = Vec<(Range<usize>, u16)>;

/// what a click on a piece of a title does
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Click {
    /// open http://localhost:<port>
    Port(u16),
    /// switch the memory unit
    Memory,
    /// list cpu use per tab
    TotalCpu,
    /// list memory per tab
    TotalMemory,
    /// list tcp traffic per tab
    TotalNetwork,
}

/// text of one block, or of one port of a ports block
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Piece {
    pub text: String,
    /// where the tab icon goes, with no text
    pub icon: bool,
    pub click: Option<Click>,
    /// cpu use from 0 to 1, oldest first, drawn as a chart where the bar can draw. `text`
    /// holds the same as braille dots
    pub graph: Option<Vec<f32>>,
}

impl Piece {
    fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

/// one piece per block, ports split into one per port, may block on exec commands
pub(super) fn build_pieces(
    blocks: &[TabTitleBlock],
    inputs: &TitleInputs,
    process: Option<&ForegroundProcess>,
) -> Vec<Piece> {
    let cwd = process.and_then(|process| process.cwd.as_deref());
    let sample = inputs.sample.as_ref();
    let total = inputs.total.as_ref();
    let unit = inputs.memory_unit;
    let mut pieces = Vec::new();
    for block in blocks {
        let text = match block {
            TabTitleBlock::Number => inputs.number.to_string(),
            TabTitleBlock::Prompt => prompt(),
            TabTitleBlock::Folder => cwd.map(folder_name).unwrap_or_default(),
            TabTitleBlock::Command => process.map_or("", |process| &process.name).to_string(),
            TabTitleBlock::Title => inputs.title.clone(),
            TabTitleBlock::Text(text) => text.clone(),
            TabTitleBlock::Exec(command) => exec(command, cwd).unwrap_or_default(),
            // tabs draw the icon on their own, nudged down to the baseline, so it is no text
            TabTitleBlock::Icon => {
                pieces.push(Piece {
                    icon: true,
                    ..Piece::default()
                });
                continue;
            }
            TabTitleBlock::Progress => match inputs.progress {
                Some(Progress::Percent(percent)) => progress_icon(percent).to_string(),
                Some(Progress::Busy) => SPINNER[0].to_string(),
                None => String::new(),
            },
            TabTitleBlock::Cpu => sample.map(|s| percent(s.cpu_percent)).unwrap_or_default(),
            TabTitleBlock::Network => sample.map(network).unwrap_or_default(),
            TabTitleBlock::Clock => inputs.clock.clone(),
            TabTitleBlock::Size => format!("{}×{}", inputs.size.0, inputs.size.1),
            TabTitleBlock::Git => cwd.and_then(git).unwrap_or_default(),
            TabTitleBlock::CpuGraph | TabTitleBlock::TotalCpuGraph => {
                let (from, click) = if *block == TabTitleBlock::CpuGraph {
                    (sample, None)
                } else {
                    (total, Some(Click::TotalCpu))
                };
                let Some(from) = from.filter(|s| !s.cpu_history.is_empty()) else {
                    continue;
                };
                let graph = cpu_graph(&from.cpu_history);
                pieces.push(Piece {
                    text: braille(&graph),
                    icon: false,
                    click,
                    graph: Some(graph),
                });
                continue;
            }
            TabTitleBlock::Memory
            | TabTitleBlock::TotalCpu
            | TabTitleBlock::TotalMemory
            | TabTitleBlock::TotalNetwork => {
                let (text, click) = match (block, sample, total) {
                    (TabTitleBlock::Memory, Some(sample), _) => {
                        (bytes(sample.rss_bytes, unit), Click::Memory)
                    }
                    (TabTitleBlock::TotalCpu, _, Some(total)) => {
                        (percent(total.cpu_percent), Click::TotalCpu)
                    }
                    (TabTitleBlock::TotalMemory, _, Some(total)) => {
                        (bytes(total.rss_bytes, unit), Click::TotalMemory)
                    }
                    (TabTitleBlock::TotalNetwork, _, Some(total)) => {
                        (network(total), Click::TotalNetwork)
                    }
                    _ => continue,
                };
                pieces.push(Piece {
                    text,
                    click: Some(click),
                    ..Piece::default()
                });
                continue;
            }
            TabTitleBlock::Ports | TabTitleBlock::AllPorts => {
                let from = if *block == TabTitleBlock::Ports {
                    sample
                } else {
                    total
                };
                for (ix, &port) in from.into_iter().flat_map(|s| &s.ports).enumerate() {
                    if ix > 0 {
                        pieces.push(Piece::text(" "));
                    }
                    pieces.push(Piece {
                        text: format!(":{port}"),
                        click: Some(Click::Port(port)),
                        ..Piece::default()
                    });
                }
                continue;
            }
        };
        pieces.push(Piece::text(text));
    }
    pieces
}

/// join blocks into a title, the byte offsets icons go at and the byte range of each port,
/// may block on exec commands
pub(super) fn build_title(
    blocks: &[TabTitleBlock],
    inputs: &TitleInputs,
    process: Option<&ForegroundProcess>,
) -> (String, Vec<usize>, PortRanges) {
    let mut title = String::new();
    let mut icon_at = Vec::new();
    let mut ports_at = Vec::new();
    for piece in build_pieces(blocks, inputs, process) {
        if piece.icon {
            icon_at.push(title.len());
        }
        if let Some(Click::Port(port)) = piece.click {
            ports_at.push((title.len()..title.len() + piece.text.len(), port));
        }
        title.push_str(&piece.text);
    }
    (title, icon_at, ports_at)
}

/// cpu use like "34%"
pub(super) fn percent(cpu_percent: f32) -> String {
    format!("{:.0}%", cpu_percent)
}

/// size like "1.2 GB" in `unit`, with a decimal only below 10
pub(super) fn bytes(bytes: u64, unit: MemoryUnit) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let fixed = match unit {
        MemoryUnit::Auto => None,
        MemoryUnit::Kb => Some(1),
        MemoryUnit::Mb => Some(2),
        MemoryUnit::Gb => Some(3),
    };
    let mut value = bytes as f64;
    let mut unit = 0;
    while fixed.map_or(value >= 1024. && unit < UNITS.len() - 1, |fixed| {
        unit < fixed
    }) {
        value /= 1024.;
        unit += 1;
    }
    // a fixed unit can be too big for the number, "0.0 GB" would hide it
    if unit > 0 && value < 1. {
        format!("{value:.2} {}", UNITS[unit])
    // "9.96" would round up to "10.0", which reads worse than "10"
    } else if unit > 0 && value < 9.95 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// cpu percents scaled from 0 to 1. 100% is one busy core, a tree busy on more cores
/// scales the graph down so it still fits
pub(super) fn cpu_graph(history: &[f32]) -> Vec<f32> {
    let top = history.iter().copied().fold(100., f32::max);
    history
        .iter()
        .map(|cpu| (cpu / top).clamp(0., 1.))
        .collect()
}

/// the newest values of a graph as braille dots, two to a character, filled from the bottom.
/// the lowest row is always set, so an idle graph still shows as a line
pub(super) fn braille(graph: &[f32]) -> String {
    // dots from the bottom up, left column then right column
    const LEFT: [u32; 4] = [0x40, 0x04, 0x02, 0x01];
    const RIGHT: [u32; 4] = [0x80, 0x20, 0x10, 0x08];
    let newest = &graph[graph.len().saturating_sub(TEXT_GRAPH_SAMPLES)..];
    let level = |value: f32| ((value * 4.).round() as usize).clamp(1, 4);
    let mut values = newest.to_vec();
    if values.len() % 2 == 1 {
        values.insert(0, 0.);
    }
    values
        .chunks(2)
        .map(|pair| {
            let dots: u32 = LEFT[..level(pair[0])].iter().sum::<u32>()
                + RIGHT[..level(pair[1])].iter().sum::<u32>();
            char::from_u32(0x2800 + dots).unwrap_or(' ')
        })
        .collect()
}

/// tcp traffic like "↓1.2 MB/s ↑4.0 KB/s"
pub(super) fn network(sample: &TreeSample) -> String {
    format!(
        "↓{}/s ↑{}/s",
        bytes(sample.received_per_sec, MemoryUnit::Auto),
        bytes(sample.sent_per_sec, MemoryUnit::Auto)
    )
}

/// `title` with `icon` put in at each byte offset of `icon_at`
pub(super) fn insert_icons(mut title: String, icon_at: &[usize], icon: &str) -> String {
    // from the end, so the offsets before stay right
    for &at in icon_at.iter().rev() {
        title.insert_str(at, icon);
    }
    title
}

/// `pieces` with the tab icon as the text of icon pieces
pub(super) fn icons_as_text(pieces: Vec<Piece>, icon: &str) -> Vec<Piece> {
    pieces
        .into_iter()
        .map(|piece| if piece.icon { Piece::text(icon) } else { piece })
        .collect()
}

/// nerd font circle, empty at 0 and filled in eighths up to 100
fn progress_icon(percent: u8) -> &'static str {
    const ICONS: [&str; 9] = [
        "\u{f0766}",
        "\u{f0a9e}",
        "\u{f0a9f}",
        "\u{f0aa0}",
        "\u{f0aa1}",
        "\u{f0aa2}",
        "\u{f0aa3}",
        "\u{f0aa4}",
        "\u{f0aa5}",
    ];
    // rounded up, so any progress shows a slice
    ICONS[(usize::from(percent.min(100)) * 8).div_ceil(100)]
}

/// true when `title` has a spinner to turn
pub(super) fn has_spinner(title: &str) -> bool {
    title.contains(SPINNER)
}

/// `title` with every spinner turned to frame `frame`
pub(super) fn spin(title: &str, frame: usize) -> String {
    let to = SPINNER[frame % SPINNER.len()];
    title
        .chars()
        .map(|c| if SPINNER.contains(&c) { to } else { c })
        .collect()
}

fn prompt() -> String {
    // titles rebuild for every tab several times a second, and the user never changes
    static USER: OnceLock<String> = OnceLock::new();
    let user = USER.get_or_init(user_name);
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .ok()
        .or_else(system_host)
        .unwrap_or_default();
    format!("{user}@{}", host.trim())
}

// macos has neither hostname file
#[cfg(target_os = "macos")]
fn system_host() -> Option<String> {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let name = &buf[..buf.iter().position(|b| *b == 0)?];
    // "name.local" is shown as "name", like the \h of shell prompts
    let name = String::from_utf8_lossy(name);
    Some(name.split('.').next()?.to_string())
}

#[cfg(not(target_os = "macos"))]
fn system_host() -> Option<String> {
    None
}

// USER is often unset in containers and services, so fall back to other vars and /etc/passwd
fn user_name() -> String {
    ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .find_map(|var| std::env::var(var).ok().filter(|name| !name.is_empty()))
        .or_else(passwd_user)
        .unwrap_or_default()
}

fn passwd_user() -> Option<String> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .to_owned();
    std::fs::read_to_string("/etc/passwd")
        .ok()?
        .lines()
        .find_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            (fields.nth(1)? == uid).then(|| name.to_owned())
        })
}

fn folder_name(cwd: &Path) -> String {
    if std::env::var_os("HOME").is_some_and(|home| cwd == Path::new(&home)) {
        return "~".to_string();
    }
    cwd.file_name().map_or_else(
        || cwd.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// branch like " main* ↑1 ↓2", none outside a repository. cached for a moment per folder
fn git(cwd: &Path) -> Option<String> {
    // when each folder was read, and what it gave
    type Cache = HashMap<PathBuf, (Instant, Option<String>)>;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    if let Some((at, text)) = CACHE.lock().unwrap().get_or_insert_default().get(cwd)
        && at.elapsed() < GIT_CACHE_FOR
    {
        return text.clone();
    }
    // no locks, so a refresh never gets in the way of git commands typed in the tab
    let output = run(
        "git --no-optional-locks status --porcelain=v2 --branch 2>/dev/null",
        Some(cwd),
    );
    let text = output.as_deref().and_then(git_status);
    let mut cache = CACHE.lock().unwrap();
    let cache = cache.get_or_insert_default();
    // folders of closed tabs would pile up
    if cache.len() > 64 {
        cache.clear();
    }
    cache.insert(cwd.to_path_buf(), (Instant::now(), text.clone()));
    text
}

/// branch text from `git status --porcelain=v2 --branch` output
fn git_status(output: &str) -> Option<String> {
    let mut oid = "";
    let mut head = None;
    let (mut ahead, mut behind) = (0, 0);
    let mut dirty = false;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("# branch.oid ") {
            oid = value;
        } else if let Some(value) = line.strip_prefix("# branch.head ") {
            head = Some(value);
        } else if let Some(value) = line.strip_prefix("# branch.ab ") {
            let mut counts = value.split_whitespace();
            ahead = counts
                .next()
                .and_then(|n| n.parse::<i32>().ok())
                .unwrap_or(0);
            behind = counts
                .next()
                .and_then(|n| n.parse::<i32>().ok())
                .unwrap_or(0);
        } else if !line.starts_with('#') && !line.is_empty() {
            dirty = true;
        }
    }
    let head = match head? {
        // a detached head shows its commit, like git's own prompt does
        "(detached)" => oid.get(..7).unwrap_or(oid),
        head => head,
    };
    let mut text = format!("{GIT_ICON} {head}");
    if dirty {
        text.push('*');
    }
    if ahead > 0 {
        text.push_str(&format!(" ↑{ahead}"));
    }
    if behind < 0 {
        text.push_str(&format!(" ↓{}", -behind));
    }
    Some(text)
}

/// time of day now in strftime `format`
pub(super) fn clock(format: &str) -> String {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let Ok(format) = std::ffi::CString::new(format) else {
            return String::new();
        };
        let mut buf = [0u8; 128];
        let written = unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                return String::new();
            }
            libc::strftime(buf.as_mut_ptr().cast(), buf.len(), format.as_ptr(), &tm)
        };
        String::from_utf8_lossy(&buf[..written]).into_owned()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = format;
        String::new()
    }
}

fn exec(command: &str, cwd: Option<&Path>) -> Option<String> {
    Some(run(command, cwd)?.lines().next()?.trim().to_string())
}

/// whole output of a shell command, none when it failed to start or ran too long
fn run(command: &str, cwd: Option<&Path>) -> Option<String> {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    // read on another thread, so a full pipe can't stall the child until the timeout
    let (output_tx, output_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).ok();
        output_tx.send(output).ok();
    });
    let deadline = Instant::now() + EXEC_TIMEOUT;
    while let Ok(None) = child.try_wait() {
        if Instant::now() > deadline {
            child.kill().ok();
            child.wait().ok();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // background jobs of sh may still hold the pipe, so the reader is left to finish alone
    let output = output_rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()?;
    Some(String::from_utf8_lossy(&output).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::foreground_process;

    fn build(blocks: &[TabTitleBlock]) -> String {
        let inputs = TitleInputs {
            number: 3,
            shell_pid: std::process::id(),
            title: "vim".into(),
            profile_icon: None,
            progress: None,
            sample: None,
            total: None,
            memory_unit: MemoryUnit::Auto,
            clock: "14:05".into(),
            size: (120, 40),
        };
        build_title(
            blocks,
            &inputs,
            foreground_process(inputs.shell_pid).as_ref(),
        )
        .0
    }

    #[test]
    fn joins_blocks_in_order() {
        let title = build(&[
            TabTitleBlock::Number,
            TabTitleBlock::Text(": ".into()),
            TabTitleBlock::Title,
            TabTitleBlock::Text(" - ".into()),
            TabTitleBlock::Exec("echo first; echo second".into()),
        ]);
        assert_eq!(title, "3: vim - first");
    }

    #[test]
    fn icons_are_left_out_of_the_text_at_their_offsets() {
        let inputs = TitleInputs {
            number: 3,
            shell_pid: std::process::id(),
            title: "vim".into(),
            profile_icon: None,
            progress: None,
            sample: None,
            total: None,
            memory_unit: MemoryUnit::Auto,
            clock: "14:05".into(),
            size: (120, 40),
        };
        let blocks = [
            TabTitleBlock::Icon,
            TabTitleBlock::Number,
            TabTitleBlock::Text(" - ".into()),
            TabTitleBlock::Icon,
            TabTitleBlock::Title,
            TabTitleBlock::Icon,
        ];
        let (title, icon_at, _) = build_title(&blocks, &inputs, None);
        assert_eq!(title, "3 - vim");
        assert_eq!(icon_at, [0, 4, 7]);
        assert_eq!(insert_icons(title, &icon_at, "I"), "I3 - IvimI");
    }

    #[test]
    fn progress_fills_the_circle() {
        let inputs = |progress| TitleInputs {
            number: 1,
            shell_pid: std::process::id(),
            title: String::new(),
            profile_icon: None,
            progress,
            sample: None,
            total: None,
            memory_unit: MemoryUnit::Auto,
            clock: "14:05".into(),
            size: (120, 40),
        };
        let build = |progress| build_title(&[TabTitleBlock::Progress], &inputs(progress), None).0;
        assert_eq!(build(None), "");
        assert_eq!(build(Some(Progress::Percent(0))), "\u{f0766}");
        assert_eq!(build(Some(Progress::Percent(1))), "\u{f0a9e}");
        assert_eq!(build(Some(Progress::Percent(50))), "\u{f0aa1}");
        assert_eq!(build(Some(Progress::Percent(100))), "\u{f0aa5}");
    }

    #[test]
    fn busy_progress_spins() {
        let inputs = TitleInputs {
            number: 1,
            shell_pid: std::process::id(),
            title: String::new(),
            profile_icon: None,
            progress: Some(Progress::Busy),
            sample: None,
            total: None,
            memory_unit: MemoryUnit::Auto,
            clock: "14:05".into(),
            size: (120, 40),
        };
        let blocks = [TabTitleBlock::Text("a".into()), TabTitleBlock::Progress];
        let busy = build_title(&blocks, &inputs, None).0;
        assert!(has_spinner(&busy));
        assert!(!has_spinner("a\u{f0aa1}"));
        let frames: Vec<_> = (0..SPINNER.len()).map(|frame| spin(&busy, frame)).collect();
        assert!(
            frames
                .iter()
                .all(|frame| has_spinner(frame) && frame.starts_with('a'))
        );
        let distinct: std::collections::HashSet<_> = frames.iter().collect();
        assert_eq!(distinct.len(), SPINNER.len());
        // frames go round, and turning a turned title works too
        assert_eq!(spin(&frames[3], SPINNER.len() + 2), frames[2]);
    }

    #[test]
    fn folder_and_exec_use_process_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let folder = cwd.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(build(&[TabTitleBlock::Folder]), folder);
        assert_eq!(
            build(&[TabTitleBlock::Exec("pwd".into())]),
            cwd.display().to_string()
        );
    }

    #[test]
    fn folder_name_shortens_home_and_root() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(folder_name(Path::new(&home)), "~");
        assert_eq!(folder_name(Path::new("/")), "/");
        assert_eq!(folder_name(Path::new("/usr/lib")), "lib");
    }

    #[test]
    fn prompt_is_user_at_host() {
        let prompt = build(&[TabTitleBlock::Prompt]);
        let (user, host) = prompt.split_once('@').unwrap();
        assert_eq!(user, user_name());
        assert!(!user.is_empty());
        assert!(!host.is_empty());
    }

    #[test]
    fn failing_and_hung_commands_are_empty() {
        assert_eq!(build(&[TabTitleBlock::Exec("exit 1".into())]), "");
        let start = Instant::now();
        assert_eq!(build(&[TabTitleBlock::Exec("sleep 10".into())]), "");
        assert_eq!(
            build(&[TabTitleBlock::Exec("echo hi; sleep 10 &".into())]),
            ""
        );
        assert!(start.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn exec_timeout_is_bounded() {
        // output printed before the hang is dropped too
        let start = Instant::now();
        assert_eq!(
            build(&[TabTitleBlock::Exec("echo early; sleep 10".into())]),
            ""
        );
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn exec_background_child_holding_stdout_is_bounded() {
        // sh exits right away but a background child keeps the pipe open, the title must not wait for it
        let start = Instant::now();
        let _ = build(&[TabTitleBlock::Exec("echo hi; sleep 8 &".into())]);
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "took {:?}",
            start.elapsed()
        );
    }

    fn sampled_inputs() -> TitleInputs {
        TitleInputs {
            number: 1,
            shell_pid: std::process::id(),
            title: String::new(),
            profile_icon: None,
            progress: None,
            sample: Some(TreeSample {
                cpu_percent: 34.4,
                rss_bytes: 1288490189,
                ports: vec![5173, 8080],
                received_per_sec: 1536,
                sent_per_sec: 12,
                cpu_history: Vec::new(),
            }),
            total: Some(TreeSample {
                cpu_percent: 150.,
                rss_bytes: 3 << 20,
                ports: vec![3000, 5173, 8080],
                received_per_sec: 5 << 20,
                sent_per_sec: 1 << 10,
                cpu_history: Vec::new(),
            }),
            memory_unit: MemoryUnit::Auto,
            clock: "14:05".into(),
            size: (120, 40),
        }
    }

    #[test]
    fn sampled_blocks_show_numbers_and_clickable_ports() {
        let inputs = sampled_inputs();
        let blocks = [
            TabTitleBlock::Text("[".into()),
            TabTitleBlock::Ports,
            TabTitleBlock::Text("] ".into()),
            TabTitleBlock::Cpu,
            TabTitleBlock::Text(" ".into()),
            TabTitleBlock::Memory,
            TabTitleBlock::Text(" ".into()),
            TabTitleBlock::Network,
        ];
        let (title, _, ports_at) = build_title(&blocks, &inputs, None);
        assert_eq!(title, "[:5173 :8080] 34% 1.2 GB ↓1.5 KB/s ↑12 B/s");
        assert_eq!(ports_at, [(1..6, 5173), (7..12, 8080)]);
        let pieces = build_pieces(
            &[
                TabTitleBlock::Memory,
                TabTitleBlock::TotalCpu,
                TabTitleBlock::TotalMemory,
                TabTitleBlock::TotalNetwork,
                TabTitleBlock::AllPorts,
            ],
            &TitleInputs {
                memory_unit: MemoryUnit::Mb,
                ..inputs
            },
            None,
        );
        let shown: Vec<_> = pieces
            .iter()
            .map(|piece| (piece.text.as_str(), piece.click))
            .collect();
        assert_eq!(
            shown,
            [
                ("1229 MB", Some(Click::Memory)),
                ("150%", Some(Click::TotalCpu)),
                ("3.0 MB", Some(Click::TotalMemory)),
                ("↓5.0 MB/s ↑1.0 KB/s", Some(Click::TotalNetwork)),
                (":3000", Some(Click::Port(3000))),
                (" ", None),
                (":5173", Some(Click::Port(5173))),
                (" ", None),
                (":8080", Some(Click::Port(8080))),
            ]
        );
    }

    #[test]
    fn sampled_blocks_are_empty_before_numbers() {
        let inputs = TitleInputs {
            sample: None,
            total: None,
            ..sampled_inputs()
        };
        let blocks = [
            TabTitleBlock::Ports,
            TabTitleBlock::Cpu,
            TabTitleBlock::Memory,
            TabTitleBlock::TotalCpu,
            TabTitleBlock::TotalMemory,
            TabTitleBlock::AllPorts,
            TabTitleBlock::Network,
            TabTitleBlock::TotalNetwork,
        ];
        assert_eq!(
            build_title(&blocks, &inputs, None),
            (String::new(), vec![], vec![])
        );
        assert!(blocks.iter().all(TabTitleBlock::sampled));
        assert!(!TabTitleBlock::Folder.sampled());
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(percent(0.), "0%");
        assert_eq!(percent(34.4), "34%");
        assert_eq!(percent(250.6), "251%");
        let auto = |n| bytes(n, MemoryUnit::Auto);
        assert_eq!(auto(0), "0 B");
        assert_eq!(auto(1023), "1023 B");
        assert_eq!(auto(1536), "1.5 KB");
        assert_eq!(auto(34 << 20), "34 MB");
        assert_eq!(auto((9.97 * (1u64 << 20) as f64) as u64), "10 MB");
        assert_eq!(auto(1288490189), "1.2 GB");
        assert_eq!(auto(5 << 40), "5.0 TB");
        assert_eq!(bytes(34 << 20, MemoryUnit::Kb), "34816 KB");
        assert_eq!(bytes(1536, MemoryUnit::Mb), "0.00 MB");
        assert_eq!(bytes(34 << 20, MemoryUnit::Gb), "0.03 GB");
        assert_eq!(bytes(5 << 40, MemoryUnit::Gb), "5120 GB");
    }

    #[test]
    fn clock_size_and_graph_blocks() {
        let mut inputs = sampled_inputs();
        inputs.sample.as_mut().unwrap().cpu_history = vec![0., 50., 100., 400.];
        let pieces = build_pieces(
            &[
                TabTitleBlock::Clock,
                TabTitleBlock::Size,
                TabTitleBlock::CpuGraph,
                TabTitleBlock::TotalCpuGraph,
            ],
            &inputs,
            None,
        );
        assert_eq!(pieces[0].text, "14:05");
        assert_eq!(pieces[1].text, "120×40");
        // 400% on four cores sets the top, the rest is scaled to it
        assert_eq!(pieces[2].graph, Some(vec![0., 0.125, 0.25, 1.]));
        assert_eq!(pieces[2].text, "⣀⣸");
        // the total has no history yet, so it is left out
        assert_eq!(pieces.len(), 3);
    }

    #[test]
    fn braille_fills_from_the_bottom() {
        assert_eq!(braille(&[]), "");
        assert_eq!(braille(&[0., 0.]), "⣀");
        assert_eq!(braille(&[1., 1.]), "⣿");
        assert_eq!(braille(&[0.5, 0.25]), "⣄");
        // an odd count is padded at the old end, only the newest samples fit
        assert_eq!(braille(&[1.]), "⣸");
        assert_eq!(braille(&[1.; 40]).chars().count(), TEXT_GRAPH_SAMPLES / 2);
    }

    #[test]
    fn git_status_shows_branch_changes_and_upstream() {
        let clean = "# branch.oid 1234567890abcdef\n# branch.head main\n";
        assert_eq!(git_status(clean).unwrap(), format!("{GIT_ICON} main"));
        let busy = "# branch.oid 1234567890abcdef\n# branch.head dev\n\
            # branch.upstream origin/dev\n# branch.ab +2 -1\n1 .M N... 1 1 1 a b src/x.rs\n";
        assert_eq!(git_status(busy).unwrap(), format!("{GIT_ICON} dev* ↑2 ↓1"));
        let detached = "# branch.oid 1234567890abcdef\n# branch.head (detached)\n? new\n";
        assert_eq!(
            git_status(detached).unwrap(),
            format!("{GIT_ICON} 1234567*")
        );
        assert_eq!(git_status(""), None);
    }

    #[test]
    fn git_block_reads_the_repository_of_the_folder() {
        let dir = std::env::temp_dir().join(format!("kuterm-git-block-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(git(&dir), None);
        let ok = Command::new("git")
            .args(["init", "-q", "-b", "trunk"])
            .current_dir(&dir)
            .status()
            .is_ok_and(|status| status.success());
        // git is in the dev image, a host without it has nothing to check
        if ok {
            // the "no repository" answer is cached for a moment
            std::thread::sleep(GIT_CACHE_FOR);
            assert_eq!(git(&dir), Some(format!("{GIT_ICON} trunk")));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clock_follows_its_format() {
        let time = clock("%H:%M");
        assert_eq!(time.len(), 5, "{time}");
        assert_eq!(&time[2..3], ":");
        assert_eq!(clock("text"), "text");
    }

    #[test]
    fn icons_turn_into_text() {
        let pieces = build_pieces(
            &[TabTitleBlock::Icon, TabTitleBlock::Text("a".into())],
            &sampled_inputs(),
            None,
        );
        let texts: Vec<_> = icons_as_text(pieces, "I")
            .into_iter()
            .map(|piece| piece.text)
            .collect();
        assert_eq!(texts, ["I", "a"]);
    }
}
