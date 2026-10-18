//! building tab titles from the blocks set in settings

use std::{ops::Range, time::Duration};

use super::title_sources::*;
use crate::{
    settings::{MemoryUnit, TabTitleBlock},
    terminal::{ForegroundProcess, PaneContext, Progress, TreeSample},
};

// samples a cpu graph shows as text, two per braille character
const TEXT_GRAPH_SAMPLES: usize = 16;
// nerd font spinner frames, titles are built with the first and turned with `spin`
const SPINNER: [char; 6] = [
    '\u{ee06}', '\u{ee07}', '\u{ee08}', '\u{ee09}', '\u{ee0a}', '\u{ee0b}',
];
/// time between spinner frames
pub(super) const SPINNER_INTERVAL: Duration = Duration::from_millis(120);

/// what a title is built from, collected on the main thread
#[derive(Default)]
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
    /// where the tab runs, filled off the main thread before the title is built
    pub context: PaneContext,
    /// commands waiting in the tab's queue
    pub queued: usize,
    /// ctrl+s paused output
    pub xoff: bool,
    /// names of programs stopped with ctrl+z
    pub stopped: Vec<String>,
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
    /// list the queued commands
    Queue,
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
            TabTitleBlock::Kube => inputs.context.kube_context.clone().unwrap_or_default(),
            TabTitleBlock::Aws => inputs.context.aws_profile.clone().unwrap_or_default(),
            TabTitleBlock::Venv => inputs.context.venv.clone().unwrap_or_default(),
            TabTitleBlock::Xoff if inputs.xoff => "paused (ctrl+q)".to_string(),
            TabTitleBlock::Xoff => String::new(),
            TabTitleBlock::StoppedJobs => inputs
                .stopped
                .iter()
                .map(|name| format!("{name}, stopped"))
                .collect::<Vec<_>>()
                .join(" "),
            TabTitleBlock::Queue => {
                if inputs.queued > 0 {
                    pieces.push(Piece {
                        text: format!("⧗ {}", inputs.queued),
                        click: Some(Click::Queue),
                        ..Piece::default()
                    });
                }
                continue;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::foreground_process;

    /// tab 3 running vim in this process, as tests read the process tree
    fn vim() -> TitleInputs {
        TitleInputs {
            number: 3,
            shell_pid: std::process::id(),
            title: "vim".into(),
            clock: "14:05".into(),
            size: (120, 40),
            ..TitleInputs::default()
        }
    }

    fn build(blocks: &[TabTitleBlock]) -> String {
        let inputs = vim();
        build_title(
            blocks,
            &inputs,
            foreground_process(inputs.shell_pid).as_ref(),
        )
        .0
    }

    #[test]
    fn context_blocks_show_what_is_known() {
        let mut inputs = TitleInputs {
            context: PaneContext {
                kube_context: Some("prod".into()),
                venv: Some("venv".into()),
                ..PaneContext::default()
            },
            ..TitleInputs::default()
        };
        let blocks = [
            TabTitleBlock::Kube,
            TabTitleBlock::Text("|".into()),
            TabTitleBlock::Aws,
            TabTitleBlock::Text("|".into()),
            TabTitleBlock::Venv,
        ];
        assert_eq!(build_title(&blocks, &inputs, None).0, "prod||venv");
        inputs.context = PaneContext::default();
        assert_eq!(build_title(&blocks, &inputs, None).0, "||");
        let pty = [TabTitleBlock::Xoff, TabTitleBlock::StoppedJobs];
        assert_eq!(build_title(&pty, &inputs, None).0, "");
        inputs.xoff = true;
        inputs.stopped = vec!["vim".into()];
        assert_eq!(
            build_title(&pty, &inputs, None).0,
            "paused (ctrl+q)vim, stopped"
        );
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
        let inputs = vim();
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
            progress,
            ..TitleInputs::default()
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
            progress: Some(Progress::Busy),
            ..TitleInputs::default()
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

    fn sampled_inputs() -> TitleInputs {
        TitleInputs {
            number: 1,
            shell_pid: std::process::id(),
            sample: Some(TreeSample {
                cpu_percent: 34.4,
                rss_bytes: 1288490189,
                ports: vec![5173, 8080],
                received_per_sec: 1536,
                sent_per_sec: 12,
                ..TreeSample::default()
            }),
            total: Some(TreeSample {
                cpu_percent: 150.,
                rss_bytes: 3 << 20,
                ports: vec![3000, 5173, 8080],
                received_per_sec: 5 << 20,
                sent_per_sec: 1 << 10,
                ..TreeSample::default()
            }),
            clock: "14:05".into(),
            size: (120, 40),
            ..TitleInputs::default()
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
