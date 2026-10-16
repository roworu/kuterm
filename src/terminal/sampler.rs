//! cpu, memory, tcp traffic and listening tcp ports of each pane's process tree. one
//! background thread samples every pane once a second, only while something asked for numbers
//! lately

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
// a tree nobody asked about for this long is dropped, and the thread stops with the last one
const IDLE_AFTER: Duration = Duration::from_secs(5);
/// cpu percents kept for graphs, one per sample
pub const CPU_HISTORY: usize = 24;

/// numbers of one process tree, or of several added up
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeSample {
    /// of one core, so a tree busy on two cores is at 200
    pub cpu_percent: f32,
    pub rss_bytes: u64,
    /// listening tcp ports, sorted
    pub ports: Vec<u16>,
    /// tcp bytes received per second
    pub received_per_sec: u64,
    /// tcp bytes sent per second
    pub sent_per_sec: u64,
    /// cpu percent of the last samples, oldest first, up to `CPU_HISTORY`
    pub cpu_history: Vec<f32>,
    /// ports on this machine the tree has established tcp connections to, sorted
    pub connected_ports: Vec<u16>,
    /// unix sockets of the tree with the inode of their peer, the other end
    pub unix_sockets: Vec<(u64, u64)>,
    /// bytes the tree read and wrote so far, files, pipes and sockets alike
    pub io_bytes: u64,
}

impl TreeSample {
    /// samples added up, ports listed once
    pub fn total<'a>(samples: impl IntoIterator<Item = &'a TreeSample>) -> TreeSample {
        let mut total = TreeSample::default();
        for sample in samples {
            // the newest samples line up, a tree sampled for less time is shorter
            let len = sample.cpu_history.len();
            if len > total.cpu_history.len() {
                let missing = len - total.cpu_history.len();
                total
                    .cpu_history
                    .splice(0..0, std::iter::repeat_n(0., missing));
            }
            let offset = total.cpu_history.len() - len;
            for (sum, cpu) in total.cpu_history[offset..]
                .iter_mut()
                .zip(&sample.cpu_history)
            {
                *sum += cpu;
            }
            total.cpu_percent += sample.cpu_percent;
            total.rss_bytes += sample.rss_bytes;
            total.ports.extend(&sample.ports);
            total.received_per_sec += sample.received_per_sec;
            total.sent_per_sec += sample.sent_per_sec;
            total.connected_ports.extend(&sample.connected_ports);
            total.unix_sockets.extend(&sample.unix_sockets);
            total.io_bytes += sample.io_bytes;
        }
        total.ports.sort_unstable();
        total.ports.dedup();
        total.connected_ports.sort_unstable();
        total.connected_ports.dedup();
        total
    }
}

struct Root {
    asked: Instant,
    sample: Option<TreeSample>,
    /// counters of the last measure, for the next rates
    last: Option<Last>,
    cpu_history: VecDeque<f32>,
}

/// counters of a tree when it was last measured
#[derive(Debug)]
struct Last {
    /// cpu ticks of the tree
    ticks: u64,
    at: Instant,
    /// tcp bytes received and sent by socket inode
    sockets: HashMap<u64, (u64, u64)>,
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
            last: None,
            cpu_history: VecDeque::new(),
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
            let roots: Vec<(u32, Option<Last>)> = {
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
                    .iter_mut()
                    .map(|(&pid, root)| (pid, root.last.take()))
                    .collect()
            };
            // read outside the lock, panes asking meanwhile get the last numbers
            let tables = Tables {
                listening: listening_ports(),
                traffic: tcp_traffic(),
                established: established_local(),
                unix_peers: unix_peers(),
            };
            let measured: Vec<_> = roots
                .into_iter()
                .map(|(pid, last)| (pid, measure(pid, last.as_ref(), &tables)))
                .collect();
            {
                let mut state = self.state.lock().unwrap();
                for (pid, measured) in measured {
                    if let Some(root) = state.roots.get_mut(&pid) {
                        (root.sample, root.last) = match measured {
                            // the first measure has no cpu use yet, so it is not graphed
                            Some((mut sample, last)) => {
                                if root.sample.is_some() {
                                    root.cpu_history.push_back(sample.cpu_percent);
                                }
                                if root.cpu_history.len() > CPU_HISTORY {
                                    root.cpu_history.pop_front();
                                }
                                sample.cpu_history = root.cpu_history.iter().copied().collect();
                                (Some(sample), Some(last))
                            }
                            None => (None, None),
                        };
                    }
                }
            }
            std::thread::sleep(SAMPLE_INTERVAL);
        }
    }
}

/// socket tables of the whole machine, read once per tick for every tree
#[derive(Default)]
struct Tables {
    /// inode to listening tcp port
    listening: HashMap<u64, u16>,
    /// inode to tcp bytes received and sent
    traffic: HashMap<u64, (u64, u64)>,
    /// inode to the port of the local end it is connected to
    established: HashMap<u64, u16>,
    /// unix socket inode to the inode of its peer
    unix_peers: HashMap<u64, u64>,
}

/// numbers of the tree under `root`, rates measured since `last`, none when it is gone
#[cfg(target_os = "linux")]
fn measure(root: u32, last: Option<&Last>, tables: &Tables) -> Option<(TreeSample, Last)> {
    let (listening, traffic) = (&tables.listening, &tables.traffic);
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
    // forked children share their parent's sockets, so each is counted once
    let mut inodes = HashSet::new();
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
        inodes.extend(socket_inodes(pid));
        sample.io_bytes += std::fs::read_to_string(format!("/proc/{pid}/io"))
            .ok()
            .map_or(0, |io| io_bytes(&io));
    }
    sample.connected_ports = inodes
        .iter()
        .filter_map(|inode| tables.established.get(inode).copied())
        .collect();
    sample.connected_ports.sort_unstable();
    sample.connected_ports.dedup();
    sample.unix_sockets = inodes
        .iter()
        .filter_map(|inode| Some((*inode, *tables.unix_peers.get(inode)?)))
        .collect();
    sample.unix_sockets.sort_unstable();
    sample.ports = inodes
        .iter()
        .filter_map(|inode| listening.get(inode).copied())
        .collect();
    sample.ports.sort_unstable();
    sample.ports.dedup();
    let sockets: HashMap<u64, (u64, u64)> = inodes
        .iter()
        .filter_map(|inode| Some((*inode, *traffic.get(inode)?)))
        .collect();
    if let Some(last) = last {
        let seconds = now.duration_since(last.at).as_secs_f32();
        // exited children take their ticks with them, which is no negative use
        let used = ticks.saturating_sub(last.ticks) as f32 / clock_ticks();
        // sockets opened since the last measure moved all their bytes in between
        let (mut received, mut sent) = (0, 0);
        for (inode, (now_received, now_sent)) in &sockets {
            let (was_received, was_sent) = last.sockets.get(inode).copied().unwrap_or_default();
            received += now_received.saturating_sub(was_received);
            sent += now_sent.saturating_sub(was_sent);
        }
        if seconds > 0. {
            sample.cpu_percent = used / seconds * 100.;
            sample.received_per_sec = (received as f32 / seconds) as u64;
            sample.sent_per_sec = (sent as f32 / seconds) as u64;
        }
    }
    Some((
        sample,
        Last {
            ticks,
            at: now,
            sockets,
        },
    ))
}

#[cfg(not(target_os = "linux"))]
fn measure(_root: u32, _last: Option<&Last>, _tables: &Tables) -> Option<(TreeSample, Last)> {
    None
}

/// characters read and written, from a /proc/<pid>/io file
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn io_bytes(io: &str) -> u64 {
    io.lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            matches!(name, "rchar" | "wchar")
                .then(|| value.trim().parse::<u64>().ok())
                .flatten()
        })
        .sum()
}

/// true when the hex address of a /proc/net/tcp row is a loopback one
fn loopback(address: &str) -> bool {
    match address.len() {
        // little endian ipv4, 127.x.x.x has 7F as its last byte
        8 => address.ends_with("7F"),
        // ::1, or ::ffff:127.x.x.x mapped
        32 => {
            address == "00000000000000000000000001000000"
                || (address.starts_with("0000000000000000FFFF0000") && address.ends_with("7F"))
        }
        _ => false,
    }
}

/// (inode, port of the other end) of the established rows of a /proc/net/tcp table whose
/// other end is on this machine
fn parse_established(table: &str) -> Vec<(u64, u16)> {
    // st 01 is TCP_ESTABLISHED
    const ESTABLISHED: &str = "01";
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (local, _) = fields.get(1)?.rsplit_once(':')?;
            let (remote, port) = fields.get(2)?.rsplit_once(':')?;
            if *fields.get(3)? != ESTABLISHED || !(loopback(remote) || remote == local) {
                return None;
            }
            Some((
                fields.get(9)?.parse().ok()?,
                u16::from_str_radix(port, 16).ok()?,
            ))
        })
        .collect()
}

/// socket inode to the port of the local server of every established local connection
fn established_local() -> HashMap<u64, u16> {
    ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .flat_map(|table| parse_established(&table))
        .collect()
}

/// unix socket inode to its peer's inode, from sock_diag like `ss -x` reads them
#[cfg(target_os = "linux")]
fn unix_peers() -> HashMap<u64, u64> {
    let mut peers = HashMap::new();
    if let Err(error) = dump_unix(&mut peers) {
        eprintln!("failed to read unix sockets: {error}");
    }
    peers
}

#[cfg(not(target_os = "linux"))]
fn unix_peers() -> HashMap<u64, u64> {
    HashMap::new()
}

// from linux/unix_diag.h
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const UDIAG_SHOW_PEER: u32 = 4;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const UNIX_DIAG_PEER: u16 = 2;

/// add the peer of every connected unix socket
#[cfg(target_os = "linux")]
fn dump_unix(peers: &mut HashMap<u64, u64>) -> std::io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_SOCK_DIAG,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // nlmsghdr, then unix_diag_req asking for the peers of sockets in any state
    let mut request = [0u8; NLMSG_HEADER + 24];
    let len = request.len() as u32;
    request[0..4].copy_from_slice(&len.to_ne_bytes());
    request[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    let flags = (libc::NLM_F_REQUEST | libc::NLM_F_DUMP) as u16;
    request[6..8].copy_from_slice(&flags.to_ne_bytes());
    request[16] = libc::AF_UNIX as u8;
    request[20..24].copy_from_slice(&u32::MAX.to_ne_bytes());
    request[28..32].copy_from_slice(&UDIAG_SHOW_PEER.to_ne_bytes());
    let mut kernel: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    kernel.nl_family = libc::AF_NETLINK as u16;
    let sent = unsafe {
        libc::sendto(
            fd.as_raw_fd(),
            request.as_ptr().cast(),
            request.len(),
            0,
            (&raw const kernel).cast(),
            size_of::<libc::sockaddr_nl>() as u32,
        )
    };
    if sent < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut buf = vec![0u8; 64 << 10];
    loop {
        let read = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if read < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if parse_unix_messages(&buf[..read as usize], peers)? {
            return Ok(());
        }
    }
}

/// add (inode, peer) of the unix_diag messages in one netlink read, true once done
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_unix_messages(mut buf: &[u8], peers: &mut HashMap<u64, u64>) -> std::io::Result<bool> {
    const NLMSG_ERROR: u16 = 2;
    const NLMSG_DONE: u16 = 3;
    // unix_diag_msg is 16 bytes, its inode at 4
    const MSG: usize = 16;
    while buf.len() >= NLMSG_HEADER {
        let len = u32::from_ne_bytes(buf[0..4].try_into().unwrap()) as usize;
        let kind = u16::from_ne_bytes(buf[4..6].try_into().unwrap());
        if len < NLMSG_HEADER || len > buf.len() {
            break;
        }
        match kind {
            NLMSG_DONE => return Ok(true),
            NLMSG_ERROR => return Err(std::io::Error::other("unix_diag dump refused")),
            SOCK_DIAG_BY_FAMILY => {
                let msg = &buf[NLMSG_HEADER..len];
                if let Some(inode) = msg.get(4..8) {
                    let inode = u32::from_ne_bytes(inode.try_into().unwrap());
                    let mut attrs = msg.get(MSG..).unwrap_or_default();
                    while attrs.len() >= 4 {
                        let attr_len = u16::from_ne_bytes([attrs[0], attrs[1]]) as usize;
                        let attr = u16::from_ne_bytes([attrs[2], attrs[3]]);
                        if attr_len < 4 || attr_len > attrs.len() {
                            break;
                        }
                        if attr == UNIX_DIAG_PEER
                            && let Some(peer) = attrs.get(4..8)
                        {
                            let peer = u32::from_ne_bytes(peer.try_into().unwrap());
                            peers.insert(inode.into(), peer.into());
                        }
                        attrs = attrs
                            .get(attr_len.next_multiple_of(4)..)
                            .unwrap_or_default();
                    }
                }
            }
            _ => {}
        }
        buf = buf.get(len.next_multiple_of(4)..).unwrap_or_default();
    }
    Ok(false)
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

/// tcp bytes received and sent of every tcp socket by inode. /proc has no byte counts per
/// socket, so they come from the kernel's sock_diag netlink, the way `ss -i` reads them
#[cfg(target_os = "linux")]
fn tcp_traffic() -> HashMap<u64, (u64, u64)> {
    let mut traffic = HashMap::new();
    for family in [libc::AF_INET, libc::AF_INET6] {
        if let Err(error) = dump_tcp(family as u8, &mut traffic) {
            eprintln!("failed to read tcp traffic: {error}");
        }
    }
    traffic
}

#[cfg(not(target_os = "linux"))]
fn tcp_traffic() -> HashMap<u64, (u64, u64)> {
    HashMap::new()
}

// netlink message types and sizes from linux/netlink.h and linux/inet_diag.h
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const SOCK_DIAG_BY_FAMILY: u16 = 20;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const NLMSG_HEADER: usize = 16;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const INET_DIAG_INFO: u16 = 2;

/// add the traffic of every tcp socket of one address family
#[cfg(target_os = "linux")]
fn dump_tcp(family: u8, traffic: &mut HashMap<u64, (u64, u64)>) -> std::io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_SOCK_DIAG,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // nlmsghdr, then inet_diag_req_v2 asking for tcp_info of sockets in any state
    let mut request = [0u8; NLMSG_HEADER + 56];
    let len = request.len() as u32;
    request[0..4].copy_from_slice(&len.to_ne_bytes());
    request[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    let flags = (libc::NLM_F_REQUEST | libc::NLM_F_DUMP) as u16;
    request[6..8].copy_from_slice(&flags.to_ne_bytes());
    request[16] = family;
    request[17] = libc::IPPROTO_TCP as u8;
    request[18] = 1 << (INET_DIAG_INFO - 1);
    request[20..24].copy_from_slice(&u32::MAX.to_ne_bytes());
    let mut kernel: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    kernel.nl_family = libc::AF_NETLINK as u16;
    let sent = unsafe {
        libc::sendto(
            fd.as_raw_fd(),
            request.as_ptr().cast(),
            request.len(),
            0,
            (&raw const kernel).cast(),
            size_of::<libc::sockaddr_nl>() as u32,
        )
    };
    if sent < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut buf = vec![0u8; 64 << 10];
    loop {
        let read = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if read < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if parse_diag_messages(&buf[..read as usize], traffic)? {
            return Ok(());
        }
    }
}

/// add (inode, (received, sent)) of the sock_diag messages in one netlink read, true once
/// the dump is done
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_diag_messages(
    mut buf: &[u8],
    traffic: &mut HashMap<u64, (u64, u64)>,
) -> std::io::Result<bool> {
    const NLMSG_ERROR: u16 = 2;
    const NLMSG_DONE: u16 = 3;
    while buf.len() >= NLMSG_HEADER {
        let len = u32::from_ne_bytes(buf[0..4].try_into().unwrap()) as usize;
        let kind = u16::from_ne_bytes(buf[4..6].try_into().unwrap());
        if len < NLMSG_HEADER || len > buf.len() {
            break;
        }
        match kind {
            NLMSG_DONE => return Ok(true),
            NLMSG_ERROR => return Err(std::io::Error::other("sock_diag dump refused")),
            SOCK_DIAG_BY_FAMILY => {
                if let Some((inode, bytes)) = parse_diag(&buf[NLMSG_HEADER..len]) {
                    traffic.insert(inode, bytes);
                }
            }
            _ => {}
        }
        buf = buf.get(len.next_multiple_of(4)..).unwrap_or_default();
    }
    Ok(false)
}

/// inode and (received, sent) bytes of one inet_diag_msg with its tcp_info
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_diag(msg: &[u8]) -> Option<(u64, (u64, u64))> {
    // inet_diag_msg is 72 bytes and ends with the inode
    const MSG: usize = 72;
    // tcpi_bytes_acked and tcpi_bytes_received in tcp_info, older kernels send it shorter
    const SENT: usize = 120;
    const RECEIVED: usize = 128;
    let u64_at =
        |info: &[u8], at: usize| Some(u64::from_ne_bytes(info.get(at..at + 8)?.try_into().ok()?));
    let inode = u32::from_ne_bytes(msg.get(MSG - 4..MSG)?.try_into().ok()?);
    let mut attrs = msg.get(MSG..)?;
    while attrs.len() >= 4 {
        let len = u16::from_ne_bytes([attrs[0], attrs[1]]) as usize;
        let kind = u16::from_ne_bytes([attrs[2], attrs[3]]);
        if len < 4 || len > attrs.len() {
            break;
        }
        if kind == INET_DIAG_INFO {
            let info = &attrs[4..len];
            return Some((inode.into(), (u64_at(info, RECEIVED)?, u64_at(info, SENT)?)));
        }
        attrs = attrs.get(len.next_multiple_of(4)..).unwrap_or_default();
    }
    None
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
            cpu_history: vec![1., 2., 3.],
            ..TreeSample::default()
        };
        let b = TreeSample {
            cpu_percent: 2.5,
            rss_bytes: 50,
            ports: vec![3000, 8080],
            received_per_sec: 7,
            sent_per_sec: 9,
            cpu_history: vec![10.],
            ..TreeSample::default()
        };
        let total = TreeSample::total([&a, &b]);
        assert_eq!(total.cpu_percent, 12.5);
        assert_eq!(total.rss_bytes, 150);
        assert_eq!(total.ports, [3000, 5173, 8080]);
        assert_eq!((total.received_per_sec, total.sent_per_sec), (7, 9));
        // the newest samples line up
        assert_eq!(total.cpu_history, [1., 2., 13.]);
        assert_eq!(TreeSample::total([]), TreeSample::default());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn finds_ports_memory_and_cpu_of_a_tree() {
        use std::os::unix::process::CommandExt;

        // a listening socket held by this process
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let tables = Tables {
            listening: listening_ports(),
            ..Tables::default()
        };
        let (sample, _) = measure(std::process::id(), None, &tables).unwrap();
        assert!(sample.ports.contains(&port), "{port} not in {sample:?}");
        assert!(sample.rss_bytes > 0);

        // a busy child, under a parent that only waits for it
        let mut parent = std::process::Command::new("sh")
            .args(["-c", "sh -c 'while :; do :; done' & wait"])
            .process_group(0)
            .spawn()
            .unwrap();
        let root = parent.id();
        let (_, last) = measure(root, None, &Tables::default()).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let (sample, _) = measure(root, Some(&last), &Tables::default()).unwrap();
        // the busy child goes with its parent's group
        unsafe { libc::kill(-(root as i32), libc::SIGKILL) };
        parent.wait().ok();
        assert!(sample.cpu_percent > 10., "{sample:?}");
        assert!(sample.rss_bytes > 0);
        assert!(measure(root, None, &Tables::default()).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn tcp_traffic_of_a_tree_is_a_rate() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        let pid = std::process::id();
        let tables = || Tables {
            traffic: tcp_traffic(),
            ..Tables::default()
        };
        let (first, last) = measure(pid, None, &tables()).unwrap();
        // the first measure has nothing to compare with
        assert_eq!((first.received_per_sec, first.sent_per_sec), (0, 0));
        client.write_all(&[7; 100_000]).unwrap();
        let mut got = vec![0; 100_000];
        server.read_exact(&mut got).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let (sample, _) = measure(pid, Some(&last), &tables()).unwrap();
        // both ends live in this process, other tests may add their own sockets
        assert!(sample.sent_per_sec >= 100_000, "{sample:?}");
        assert!(sample.received_per_sec >= 100_000, "{sample:?}");
    }

    #[test]
    fn established_rows_to_this_machine_are_read() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1
   1: 0100007F:A1B2 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1000        0 4343 1
   2: 0A00020F:A1B3 0A00020F:0BB8 01 00000000:00000000 00:00000000 00000000  1000        0 4444 1
   3: 0A00020F:A1B4 5DB8D822:01BB 01 00000000:00000000 00:00000000 00000000  1000        0 4545 1";
        // the loopback one and the one to its own address, not the one to a far host
        assert_eq!(parse_established(table), vec![(4343, 8080), (4444, 3000)]);
        assert!(loopback("0100007F"));
        assert!(loopback("00000000000000000000000001000000"));
        assert!(loopback("0000000000000000FFFF00000100007F"));
        assert!(!loopback("5DB8D822"));
        assert_eq!(io_bytes("rchar: 10\nwchar: 5\nsyscr: 99\n"), 15);
    }

    #[test]
    fn unix_diag_messages_give_inode_and_peer() {
        let mut msg = vec![0u8; 16];
        msg[4..8].copy_from_slice(&77u32.to_ne_bytes());
        msg.extend(8u16.to_ne_bytes());
        msg.extend(UNIX_DIAG_PEER.to_ne_bytes());
        msg.extend(88u32.to_ne_bytes());
        let mut buf = Vec::new();
        for (kind, body) in [(SOCK_DIAG_BY_FAMILY, &msg[..]), (3, &[0u8; 4][..])] {
            buf.extend(((NLMSG_HEADER + body.len()) as u32).to_ne_bytes());
            buf.extend(kind.to_ne_bytes());
            buf.extend([0u8; 10]);
            buf.extend(body);
        }
        let mut peers = HashMap::new();
        assert!(parse_unix_messages(&buf, &mut peers).unwrap());
        assert_eq!(peers, HashMap::from([(77, 88)]));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn local_connections_and_unix_peers_of_a_tree_are_found() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let _client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let _server = listener.accept().unwrap();
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let tables = Tables {
            established: established_local(),
            unix_peers: unix_peers(),
            ..Tables::default()
        };
        let (sample, _) = measure(std::process::id(), None, &tables).unwrap();
        assert!(sample.connected_ports.contains(&port), "{sample:?}");
        // both ends of the pair are held here, each names the other
        let inode = |stream: &std::os::unix::net::UnixStream| {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(format!(
                "/proc/self/fd/{}",
                std::os::fd::AsRawFd::as_raw_fd(stream)
            ))
            .unwrap()
            .ino()
        };
        assert!(
            sample.unix_sockets.contains(&(inode(&a), inode(&b))),
            "{:?}",
            sample.unix_sockets
        );
        assert!(sample.io_bytes > 0);
    }

    #[test]
    fn diag_messages_give_inode_and_bytes() {
        let mut msg = vec![0u8; 72];
        msg[68..72].copy_from_slice(&4242u32.to_ne_bytes());
        // an attribute that is not tcp_info comes first and is skipped
        msg.extend(8u16.to_ne_bytes());
        msg.extend(1u16.to_ne_bytes());
        msg.extend([0; 4]);
        let mut info = vec![0u8; 136];
        info[120..128].copy_from_slice(&300u64.to_ne_bytes());
        info[128..136].copy_from_slice(&500u64.to_ne_bytes());
        msg.extend((4 + info.len() as u16).to_ne_bytes());
        msg.extend(INET_DIAG_INFO.to_ne_bytes());
        msg.extend(&info);
        let mut buf = Vec::new();
        for (kind, body) in [(SOCK_DIAG_BY_FAMILY, &msg[..]), (3, &[0u8; 4][..])] {
            buf.extend(((NLMSG_HEADER + body.len()) as u32).to_ne_bytes());
            buf.extend(kind.to_ne_bytes());
            buf.extend([0; 10]);
            buf.extend(body);
        }
        let mut traffic = HashMap::new();
        assert!(parse_diag_messages(&buf, &mut traffic).unwrap());
        assert_eq!(traffic, HashMap::from([(4242, (500, 300))]));
        // a tcp_info cut short by an old kernel gives nothing
        assert_eq!(parse_diag(&msg[..msg.len() - 8]), None);
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
