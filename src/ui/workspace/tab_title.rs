//! building tab titles from the blocks set in settings

use std::{
    io::Read,
    ops::Range,
    path::Path,
    process::{Command, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};

use crate::{
    settings::TabTitleBlock,
    terminal::{ForegroundProcess, Progress, TreeSample},
};

// a hung command would freeze every tab title, so it is abandoned after this
const EXEC_TIMEOUT: Duration = Duration::from_secs(2);
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
}

/// byte range in a title and the port written there
pub(super) type PortRanges = Vec<(Range<usize>, u16)>;

/// what a click on a piece of a title does
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Click {
    /// open http://localhost:<port>
    Port(u16),
    /// list cpu use per tab
    TotalCpu,
    /// list memory per tab
    TotalMemory,
}

/// text of one block, or of one port of a ports block
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Piece {
    pub text: String,
    /// where the tab icon goes, with no text
    pub icon: bool,
    pub click: Option<Click>,
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
            TabTitleBlock::Memory => sample.map(|s| bytes(s.rss_bytes)).unwrap_or_default(),
            TabTitleBlock::TotalCpu | TabTitleBlock::TotalMemory => {
                let (text, click) = match (block, total) {
                    (_, None) => continue,
                    (TabTitleBlock::TotalCpu, Some(total)) => {
                        (percent(total.cpu_percent), Click::TotalCpu)
                    }
                    (_, Some(total)) => (bytes(total.rss_bytes), Click::TotalMemory),
                };
                pieces.push(Piece {
                    text,
                    icon: false,
                    click: Some(click),
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
                        icon: false,
                        click: Some(Click::Port(port)),
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

/// size like "1.2 GB", with a decimal only below 10
pub(super) fn bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    // "9.96" would round up to "10.0", which reads worse than "10"
    if unit > 0 && value < 9.95 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
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

fn exec(command: &str, cwd: Option<&Path>) -> Option<String> {
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
    Some(
        String::from_utf8_lossy(&output)
            .lines()
            .next()?
            .trim()
            .to_string(),
    )
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
            }),
            total: Some(TreeSample {
                cpu_percent: 150.,
                rss_bytes: 3 << 20,
                ports: vec![3000, 5173, 8080],
            }),
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
        ];
        let (title, _, ports_at) = build_title(&blocks, &inputs, None);
        assert_eq!(title, "[:5173 :8080] 34% 1.2 GB");
        assert_eq!(ports_at, [(1..6, 5173), (7..12, 8080)]);
        let pieces = build_pieces(
            &[
                TabTitleBlock::TotalCpu,
                TabTitleBlock::TotalMemory,
                TabTitleBlock::AllPorts,
            ],
            &inputs,
            None,
        );
        let shown: Vec<_> = pieces
            .iter()
            .map(|piece| (piece.text.as_str(), piece.click))
            .collect();
        assert_eq!(
            shown,
            [
                ("150%", Some(Click::TotalCpu)),
                ("3.0 MB", Some(Click::TotalMemory)),
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
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(34 << 20), "34 MB");
        assert_eq!(bytes((9.97 * (1u64 << 20) as f64) as u64), "10 MB");
        assert_eq!(bytes(1288490189), "1.2 GB");
        assert_eq!(bytes(5 << 40), "5.0 TB");
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
