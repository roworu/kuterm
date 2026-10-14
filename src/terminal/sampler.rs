//! cpu, memory and listening tcp ports of each pane's process tree. one background thread
//! samples every pane once a second, only while something asked for numbers lately

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
// a tree nobody asked about for this long is dropped, and the thread stops with the last one
const IDLE_AFTER: Duration = Duration::from_secs(5);

/// numbers of one process tree, or of several added up
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeSample {
    /// of one core, so a tree busy on two cores is at 200
    pub cpu_percent: f32,
    pub rss_bytes: u64,
    /// listening tcp ports, sorted
    pub ports: Vec<u16>,
}

impl TreeSample {
    /// samples added up, ports listed once
    pub fn total<'a>(samples: impl IntoIterator<Item = &'a TreeSample>) -> TreeSample {
        let mut total = TreeSample::default();
        for sample in samples {
            total.cpu_percent += sample.cpu_percent;
            total.rss_bytes += sample.rss_bytes;
            total.ports.extend(&sample.ports);
        }
        total.ports.sort_unstable();
        total.ports.dedup();
        total
    }
}

struct Root {
    asked: Instant,
    sample: Option<TreeSample>,
    /// cpu ticks of the tree and when they were read, for the next cpu percent
    ticks: Option<(u64, Instant)>,
}

#[derive(Default)]
struct State {
    roots: HashMap<u32, Root>,
    running: bool,
}

/// samples process trees on a background thread, shared by all panes
#[derive(Default)]
pub struct ProcessSampler {
    state: Mutex<State>,
}

impl ProcessSampler {
    /// the sampler every pane shares
    pub fn shared() -> &'static ProcessSampler {
        static SAMPLER: LazyLock<ProcessSampler> = LazyLock::new(ProcessSampler::default);
        &SAMPLER
    }

    /// last numbers of the tree under `root`, none until the first tick. asking keeps it
    /// sampled for a few seconds
    pub fn sample(&'static self, root: u32) -> Option<TreeSample> {
        let mut state = self.state.lock().unwrap();
        let entry = state.roots.entry(root).or_insert_with(|| Root {
            asked: Instant::now(),
            sample: None,
            ticks: None,
        });
        entry.asked = Instant::now();
        let sample = entry.sample.clone();
        if !state.running {
            state.running = true;
            std::thread::spawn(move || self.run());
        }
        sample
    }

    fn run(&self) {
        loop {
            let roots: Vec<(u32, Option<(u64, Instant)>)> = {
                let mut state = self.state.lock().unwrap();
                state
                    .roots
                    .retain(|_, root| root.asked.elapsed() < IDLE_AFTER);
                if state.roots.is_empty() {
                    state.running = false;
                    return;
                }
                state
                    .roots
                    .iter()
                    .map(|(&pid, root)| (pid, root.ticks))
                    .collect()
            };
            // read outside the lock, panes asking meanwhile get the last numbers
            let listening = listening_ports();
            let measured: Vec<_> = roots
                .into_iter()
                .map(|(pid, ticks)| (pid, measure(pid, ticks, &listening)))
                .collect();
            {
                let mut state = self.state.lock().unwrap();
                for (pid, measured) in measured {
                    if let Some(root) = state.roots.get_mut(&pid) {
                        (root.sample, root.ticks) = match measured {
                            Some((sample, ticks)) => (Some(sample), Some(ticks)),
                            None => (None, None),
                        };
                    }
                }
            }
            std::thread::sleep(SAMPLE_INTERVAL);
        }
    }
}

/// numbers of the tree under `root`, cpu measured since `last` ticks, none when it is gone
#[cfg(target_os = "linux")]
fn measure(
    root: u32,
    last: Option<(u64, Instant)>,
    listening: &HashMap<u64, u16>,
) -> Option<(TreeSample, (u64, Instant))> {
    let now = Instant::now();
    let mut pids = vec![root];
    let mut ix = 0;
    while let Some(&pid) = pids.get(ix) {
        pids.extend(super::children(pid));
        ix += 1;
    }
    let page = page_size();
    let mut ticks = 0;
    let mut sample = TreeSample::default();
    for (n, &pid) in pids.iter().enumerate() {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
        let Some(cpu) = stat.ok().as_deref().and_then(cpu_ticks) else {
            // the root itself must be there, children may exit while they are read
            if n == 0 {
                return None;
            }
            continue;
        };
        ticks += cpu;
        sample.rss_bytes += std::fs::read_to_string(format!("/proc/{pid}/statm"))
            .ok()
            .and_then(|statm| statm.split_whitespace().nth(1)?.parse::<u64>().ok())
            .map_or(0, |pages| pages * page);
        sample
            .ports
            .extend(socket_inodes(pid).filter_map(|inode| listening.get(&inode)));
    }
    sample.ports.sort_unstable();
    sample.ports.dedup();
    if let Some((last_ticks, at)) = last {
        let seconds = now.duration_since(at).as_secs_f32();
        // exited children take their ticks with them, which is no negative use
        let used = ticks.saturating_sub(last_ticks) as f32 / clock_ticks();
        if seconds > 0. {
            sample.cpu_percent = used / seconds * 100.;
        }
    }
    Some((sample, (ticks, now)))
}

#[cfg(not(target_os = "linux"))]
fn measure(
    _root: u32,
    _last: Option<(u64, Instant)>,
    _listening: &HashMap<u64, u16>,
) -> Option<(TreeSample, (u64, Instant))> {
    None
}

/// user plus system time from a /proc/<pid>/stat line, in clock ticks
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn cpu_ticks(stat: &str) -> Option<u64> {
    // comm may contain spaces and parens, so count fields after the last ')'
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace().skip(11);
    let user: u64 = fields.next()?.parse().ok()?;
    let system: u64 = fields.next()?.parse().ok()?;
    Some(user + system)
}

/// socket inode to port of every listening tcp socket
fn listening_ports() -> HashMap<u64, u16> {
    ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .flat_map(|table| parse_listening(&table))
        .collect()
}

/// (inode, port) of the rows of a /proc/net/tcp table in the listen state
fn parse_listening(table: &str) -> Vec<(u64, u16)> {
    // st 0A is TCP_LISTEN
    const LISTEN: &str = "0A";
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (_, port) = fields.get(1)?.rsplit_once(':')?;
            if *fields.get(3)? != LISTEN {
                return None;
            }
            Some((
                fields.get(9)?.parse().ok()?,
                u16::from_str_radix(port, 16).ok()?,
            ))
        })
        .collect()
}

/// inodes of the sockets `pid` holds open
#[cfg(target_os = "linux")]
fn socket_inodes(pid: u32) -> impl Iterator<Item = u64> {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|fd| {
            let link = std::fs::read_link(fd.path()).ok()?;
            link.to_str()?
                .strip_prefix("socket:[")?
                .strip_suffix(']')?
                .parse()
                .ok()
        })
}

#[cfg(target_os = "linux")]
fn page_size() -> u64 {
    // sysconf only fails for unknown names
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 }
}

#[cfg(target_os = "linux")]
fn clock_ticks() -> f32 {
    unsafe { libc::sysconf(libc::_SC_CLK_TCK) as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_ticks_add_user_and_system_time() {
        let stat = "42 (a (b) c) S 1 42 42 0 -1 4194304 100 0 0 0 7 5 0 0 20 0 1 0 1 2 3";
        assert_eq!(cpu_ticks(stat), Some(12));
        assert_eq!(cpu_ticks("42 (short) S 1"), None);
    }

    #[test]
    fn only_listening_rows_are_read() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0000000000000000 100 0 0 10 0
   1: 0100007F:A1B2 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1000        0 4343 1 0000000000000000 20 4 30 10 -1";
        assert_eq!(parse_listening(table), vec![(4242, 8080)]);
    }

    #[test]
    fn totals_add_up_and_list_each_port_once() {
        let a = TreeSample {
            cpu_percent: 10.,
            rss_bytes: 100,
            ports: vec![5173, 8080],
        };
        let b = TreeSample {
            cpu_percent: 2.5,
            rss_bytes: 50,
            ports: vec![3000, 8080],
        };
        let total = TreeSample::total([&a, &b]);
        assert_eq!(total.cpu_percent, 12.5);
        assert_eq!(total.rss_bytes, 150);
        assert_eq!(total.ports, [3000, 5173, 8080]);
        assert_eq!(TreeSample::total([]), TreeSample::default());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn finds_ports_memory_and_cpu_of_a_tree() {
        use std::os::unix::process::CommandExt;

        // a listening socket held by this process
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sample, _) = measure(std::process::id(), None, &listening_ports()).unwrap();
        assert!(sample.ports.contains(&port), "{port} not in {sample:?}");
        assert!(sample.rss_bytes > 0);

        // a busy child, under a parent that only waits for it
        let mut parent = std::process::Command::new("sh")
            .args(["-c", "sh -c 'while :; do :; done' & wait"])
            .process_group(0)
            .spawn()
            .unwrap();
        let root = parent.id();
        let (_, ticks) = measure(root, None, &HashMap::new()).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let (sample, _) = measure(root, Some(ticks), &HashMap::new()).unwrap();
        // the busy child goes with its parent's group
        unsafe { libc::kill(-(root as i32), libc::SIGKILL) };
        parent.wait().ok();
        assert!(sample.cpu_percent > 10., "{sample:?}");
        assert!(sample.rss_bytes > 0);
        assert!(measure(root, None, &HashMap::new()).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shared_sampler_fills_in_after_a_tick() {
        let sampler = ProcessSampler::shared();
        let root = std::process::id();
        sampler.sample(root);
        let deadline = Instant::now() + Duration::from_secs(5);
        let sample = loop {
            if let Some(sample) = sampler.sample(root) {
                break sample;
            }
            assert!(Instant::now() < deadline, "no sample came");
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(sample.rss_bytes > 0);
    }
}
