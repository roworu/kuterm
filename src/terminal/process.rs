//! finding what runs in the foreground of a shell, read from /proc on linux and libproc on macos

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
pub use macos::{children, foreground_process, process_info};

/// program in the foreground of a terminal
#[derive(Debug)]
pub struct ForegroundProcess {
    pub pid: u32,
    pub name: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

/// foreground process of the terminal `shell_pid` runs in, none when /proc can't tell
#[cfg(not(target_os = "macos"))]
pub fn foreground_process(shell_pid: u32) -> Option<ForegroundProcess> {
    let stat = std::fs::read_to_string(format!("/proc/{shell_pid}/stat")).ok()?;
    // comm may contain spaces and parens, so count fields after the last ')'
    let tpgid: i32 = stat
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(5)?
        .parse()
        .ok()?;
    // -1 when there is no controlling terminal, then the shell itself is shown
    let pid = if tpgid > 0 { tpgid as u32 } else { shell_pid };
    process_info(pid)
}

/// name, arguments and folder of any process, none when it is gone
#[cfg(not(target_os = "macos"))]
pub fn process_info(pid: u32) -> Option<ForegroundProcess> {
    let (name, args) = process_name(pid)?;
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok();
    Some(ForegroundProcess {
        pid,
        name,
        args,
        cwd,
    })
}

/// processes started by `pid`, oldest first per thread
#[cfg(not(target_os = "macos"))]
pub fn children(pid: u32) -> Vec<u32> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return Vec::new();
    };
    tasks
        .flatten()
        .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
        .flat_map(|list| {
            list.split_whitespace()
                .filter_map(|child| child.parse().ok())
                .collect::<Vec<u32>>()
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn process_name(pid: u32) -> Option<(String, Vec<String>)> {
    // comm is cut to 15 bytes, so prefer argv[0] and use comm only as a fallback
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let mut argv = cmdline
        .strip_suffix(&[0])
        .unwrap_or(&cmdline)
        .split(|b| *b == 0)
        .map(|arg| String::from_utf8_lossy(arg).into_owned());
    let arg0 = argv.next()?;
    let args = argv.collect();
    let name = arg0_name(&arg0).or_else(|| {
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        Some(comm.trim().to_string())
    })?;
    Some((name, args))
}

fn arg0_name(arg0: &str) -> Option<String> {
    // login shells start with a dash in argv[0], like "-bash"
    Path::new(arg0.trim_start_matches('-'))
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{ffi::CStr, mem, os::unix::ffi::OsStrExt, ptr};

    use super::*;

    /// foreground process of the terminal `shell_pid` runs in, none when libproc can't tell
    pub fn foreground_process(shell_pid: u32) -> Option<ForegroundProcess> {
        let tpgid = bsd_info(shell_pid)?.e_tpgid;
        // 0 when there is no controlling terminal, then the shell itself is shown
        let pid = if tpgid > 0 { tpgid } else { shell_pid };
        process_info(pid)
    }

    /// name, arguments and folder of any process, none when it is gone
    pub fn process_info(pid: u32) -> Option<ForegroundProcess> {
        let info = bsd_info(pid)?;
        let mut argv = argv(pid).unwrap_or_default().into_iter();
        // pbi_comm is cut to 16 bytes and pbi_name to 32, so prefer argv[0]
        let name = argv
            .next()
            .as_deref()
            .and_then(arg0_name)
            .filter(|name| !name.is_empty())
            .or_else(|| c_string(&info.pbi_name))
            .or_else(|| c_string(&info.pbi_comm))?;
        Some(ForegroundProcess {
            pid,
            name,
            args: argv.collect(),
            cwd: cwd(pid),
        })
    }

    /// processes started by `pid`
    pub fn children(pid: u32) -> Vec<u32> {
        let mut pids: Vec<libc::pid_t> = vec![0; 1024];
        let size = (pids.len() * mem::size_of::<libc::pid_t>()) as libc::c_int;
        // returns the number of pids, not bytes
        let count = unsafe { libc::proc_listchildpids(pid as _, pids.as_mut_ptr().cast(), size) };
        pids.truncate(count.clamp(0, pids.len() as libc::c_int) as usize);
        pids.into_iter().map(|pid| pid as u32).collect()
    }

    fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
        // SAFETY: proc_bsdinfo is a plain c struct, all zero is valid
        unsafe { pid_info(pid, libc::PROC_PIDTBSDINFO) }
    }

    fn cwd(pid: u32) -> Option<PathBuf> {
        // SAFETY: proc_vnodepathinfo is a plain c struct, all zero is valid
        let info: libc::proc_vnodepathinfo = unsafe { pid_info(pid, libc::PROC_PIDVNODEPATHINFO)? };
        // libc splits the nul terminated path into nested arrays, but the bytes are contiguous
        let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
        let path = path.to_bytes();
        (!path.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(path)))
    }

    fn argv(pid: u32) -> Option<Vec<String>> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
        let mut sysctl = |buf: *mut libc::c_void, size: &mut libc::size_t| unsafe {
            libc::sysctl(mib.as_mut_ptr(), 3, buf, size, ptr::null_mut(), 0) == 0
        };
        let mut size: libc::size_t = 0;
        // a null buffer asks for the size first
        if !sysctl(ptr::null_mut(), &mut size) {
            return None;
        }
        let mut buf = vec![0u8; size];
        if !sysctl(buf.as_mut_ptr().cast(), &mut size) {
            return None;
        }
        buf.truncate(size);
        // laid out as argc, exec path, nul padding, then argc nul terminated args
        let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?) as usize;
        let rest = &buf[4..];
        let rest = &rest[rest.iter().position(|b| *b == 0)?..];
        let rest = &rest[rest.iter().position(|b| *b != 0)?..];
        Some(
            rest.split(|b| *b == 0)
                .take(argc)
                .map(|arg| String::from_utf8_lossy(arg).into_owned())
                .collect(),
        )
    }

    /// `T` must be the plain c struct proc_pidinfo fills for `flavor`
    unsafe fn pid_info<T>(pid: u32, flavor: libc::c_int) -> Option<T> {
        let mut info: T = unsafe { mem::zeroed() };
        let size = mem::size_of::<T>() as libc::c_int;
        let read = unsafe { libc::proc_pidinfo(pid as _, flavor, 0, (&raw mut info).cast(), size) };
        (read == size).then_some(info)
    }

    fn c_string(chars: &[libc::c_char]) -> Option<String> {
        let bytes: Vec<u8> = chars
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8)
            .collect();
        (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    use super::*;

    /// kills and reaps the child when the test ends, even on panic
    pub(crate) struct Kill(pub(crate) Child);

    impl Drop for Kill {
        fn drop(&mut self) {
            self.0.kill().ok();
            self.0.wait().ok();
        }
    }

    /// `sh -c script`, waiting until its process tree is `depth` levels deep
    pub(crate) fn spawn(script: &str, depth: usize) -> Kill {
        let child = Kill(Command::new("sh").args(["-c", script]).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut pid = child.0.id();
        for _ in 0..depth {
            pid = loop {
                if let Some(&next) = children(pid).first() {
                    break next;
                }
                assert!(Instant::now() < deadline, "{script} never started");
                std::thread::sleep(Duration::from_millis(20));
            };
            // fork returns before exec, so wait for the command line to show up
            while process_info(pid).is_none_or(|info| info.name.is_empty()) {
                assert!(Instant::now() < deadline, "{script} never exec'd");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        child
    }

    fn wait_for(pid: u32, want: &str) -> Option<ForegroundProcess> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let p = foreground_process(pid);
            if p.as_ref().is_some_and(|p| p.name == want) || Instant::now() > deadline {
                return p;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    // with a controlling terminal the tty's foreground process is reported instead of the pid,
    // so name checks only work without one, like in the container
    fn has_no_ctty() -> bool {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        stat.rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(5)
            .unwrap()
            == "-1"
    }

    #[test]
    fn missing_pid_is_none() {
        assert!(foreground_process(u32::MAX - 1).is_none());
        assert!(foreground_process(0).is_none());
        assert!(process_info(u32::MAX - 1).is_none());
        assert!(children(u32::MAX - 1).is_empty());
    }

    #[test]
    fn children_are_listed() {
        // the trailing ":" keeps sh from replacing itself with sleep
        let parent = spawn("sleep 10; :", 1);
        let pid = parent.0.id();
        let child = children(pid)[0];
        let info = process_info(child).unwrap();
        assert_eq!(info.pid, child);
        assert_eq!(info.name, "sleep");
        assert_eq!(info.args, ["10"]);
        assert!(children(child).is_empty());
        let sh = process_info(pid).unwrap();
        assert_eq!(sh.pid, pid);
        assert_eq!(sh.name, "sh");
    }

    #[test]
    fn comm_with_parens_and_spaces_parses() {
        if !has_no_ctty() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("kuterm_proc_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("we ird) (x 1 2");
        std::fs::copy("/bin/bash", &exe).unwrap();
        // other tests fork while the copy's write fd is open, so exec may briefly fail with ETXTBSY
        let mut tries = 0;
        let child = loop {
            match Command::new(&exe)
                .args(["-c", "sleep 10; :"])
                .current_dir(&dir)
                .spawn()
            {
                Ok(child) => break Kill(child),
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 100 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => panic!("{e}"),
            }
        };
        let p = wait_for(child.0.id(), "we ird) (x 1 2").expect("parsed");
        assert_eq!(p.name, "we ird) (x 1 2");
        assert_eq!(p.cwd.as_deref(), Some(dir.as_path()));
        drop(child);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn login_dash_and_path_are_stripped() {
        if !has_no_ctty() {
            return;
        }
        let child = Kill(
            Command::new("bash")
                .args(["-c", "exec -a -/usr/bin/mysleep sleep 10"])
                .spawn()
                .unwrap(),
        );
        let p = wait_for(child.0.id(), "mysleep").expect("some");
        assert_eq!(p.name, "mysleep");
        assert_eq!(p.args, ["10"]);
    }
}
