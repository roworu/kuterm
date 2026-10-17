//! command queue: commands added while something runs, typed one by one each time the prompt
//! comes back. a failure can pause it, and an entry can wait for a time of day

use std::{
    collections::VecDeque,
    sync::atomic::Ordering,
    time::{Duration, SystemTime},
};

use alacritty_terminal::grid::Scroll;

use super::{InternalEvent, Terminal, prompt::HintLine};

/// a command waiting in the queue
#[derive(Clone, Debug, PartialEq)]
pub struct QueueEntry {
    pub id: u64,
    pub command: String,
    /// not typed before this time, none for right away
    pub at: Option<SystemTime>,
}

/// commands to type at the next prompts, first in first out
#[derive(Debug, Default)]
pub struct CommandQueue {
    entries: VecDeque<QueueEntry>,
    next_id: u64,
    /// stopped after a failure until resumed
    paused: bool,
}

impl CommandQueue {
    /// add `command`, typed at a prompt once `at` came, returns its id
    pub fn push(&mut self, command: String, at: Option<SystemTime>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push_back(QueueEntry { id, command, at });
        id
    }

    /// the waiting commands, next first
    pub fn entries(&self) -> &VecDeque<QueueEntry> {
        &self.entries
    }

    /// true once a failure stopped the queue
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// change the command of entry `id`, an empty one drops it
    pub fn edit(&mut self, id: u64, command: &str) -> bool {
        let command = command.trim();
        if command.is_empty() {
            return self.remove(id);
        }
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
            return false;
        };
        entry.command = command.to_string();
        true
    }

    /// drop entry `id`
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.id != id);
        before != self.entries.len()
    }

    /// move entry `id` to position `to`, clamped to the end
    pub fn move_to(&mut self, id: u64, to: usize) -> bool {
        let Some(from) = self.entries.iter().position(|entry| entry.id == id) else {
            return false;
        };
        let entry = self.entries.remove(from).expect("position is in range");
        self.entries.insert(to.min(self.entries.len()), entry);
        true
    }

    /// drop every entry and go on after a pause
    pub fn clear(&mut self) {
        self.entries.clear();
        self.paused = false;
    }

    /// go on after a failure paused the queue
    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// a command ended with `exit`, true when that paused the queue
    pub fn ended(&mut self, exit: Option<i32>, stop_on_failure: bool) -> bool {
        // like `&&`: the commands after a failed one would run on a broken state
        let failed = exit.is_some_and(|exit| exit != 0);
        let pauses = failed && stop_on_failure && !self.entries.is_empty() && !self.paused;
        self.paused |= pauses;
        pauses
    }

    /// the entry to type at an idle prompt at `now`, an entry waiting for its time holds back
    /// the ones after it
    pub fn take_due(&mut self, now: SystemTime) -> Option<QueueEntry> {
        if self.paused || self.entries.front()?.at.is_some_and(|at| at > now) {
            return None;
        }
        self.entries.pop_front()
    }
}

/// command and its time from typed text: "@18:00 make deploy" waits for the next 18:00
pub fn parse_entry(text: &str, now: SystemTime) -> Option<(String, Option<SystemTime>)> {
    let text = text.trim();
    let Some(rest) = text.strip_prefix('@') else {
        return (!text.is_empty()).then(|| (text.to_string(), None));
    };
    let (time, command) = rest.split_once(char::is_whitespace)?;
    let (hour, minute) = time.split_once(':')?;
    let (hour, minute): (u32, u32) = (hour.parse().ok()?, minute.parse().ok()?);
    if hour > 23 || minute > 59 || command.trim().is_empty() {
        return None;
    }
    let wait = seconds_until(local_seconds_of_day(now)?, hour * 3600 + minute * 60);
    Some((
        command.trim().to_string(),
        Some(now + Duration::from_secs(wait)),
    ))
}

/// seconds from `now` to the next `target`, both seconds since local midnight
fn seconds_until(now: u32, target: u32) -> u64 {
    const DAY: u32 = 24 * 3600;
    u64::from((target + DAY - now % DAY) % DAY)
}

/// seconds since local midnight at `time`
fn local_seconds_of_day(time: SystemTime) -> Option<u32> {
    #[cfg(unix)]
    {
        let secs = time.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs() as libc::time_t;
        // SAFETY: tm is a plain c struct written by localtime_r before being read
        let tm = unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&secs, &mut tm).is_null() {
                return None;
            }
            tm
        };
        Some((tm.tm_hour * 3600 + tm.tm_min * 60 + tm.tm_sec) as u32)
    }
    #[cfg(not(unix))]
    {
        let secs = time.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
        Some((secs % (24 * 3600)) as u32)
    }
}

impl Terminal {
    /// commands waiting to be typed at the next prompts
    pub fn queue(&self) -> &CommandQueue {
        &self.queue
    }

    /// change the queue, then type its next entry when the prompt is idle
    pub fn update_queue<R>(&mut self, change: impl FnOnce(&mut CommandQueue) -> R) -> R {
        let result = change(&mut self.queue);
        self.run_queue();
        result
    }

    /// type the next due entry when the shell waits at an empty prompt, true when it did
    pub fn run_queue(&mut self) -> bool {
        let typed = self.at_prompt
            && self.prompt_text().is_none_or(|text| text.trim().is_empty())
            && match self.queue.take_due(SystemTime::now()) {
                Some(entry) => {
                    // not user input, so it counts as no answer to a waiting program
                    self.events.push(InternalEvent::Scroll(Scroll::Bottom));
                    self.note_source(super::InputSource::Automation);
                    self.write_to_pty(format!("{}\r", entry.command).into_bytes());
                    // the prompt is gone once the shell reads the line
                    self.at_prompt = false;
                    true
                }
                None => false,
            };
        self.refresh_queue_hint();
        typed
    }

    // the waiting entries show under the prompt, so the queue is never invisible
    fn refresh_queue_hint(&mut self) {
        let entries = self.queue.entries();
        let hint = entries.front().filter(|_| self.at_prompt).map(|next| {
            let more = match entries.len() {
                1 => String::new(),
                count => format!(" (+{})", count - 1),
            };
            let state = if self.queue.paused() {
                "queue paused after a failure, next"
            } else {
                "queued next"
            };
            HintLine {
                text: format!("⧗ {state}: {}{more}", next.command),
                color: gpui::hsla(0., 0., 0.5, 1.),
            }
        });
        if self.hint_line != hint {
            self.hint_line = hint;
            self.dirty.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{CommandQueue, parse_entry, seconds_until};

    fn commands(queue: &CommandQueue) -> Vec<&str> {
        queue
            .entries()
            .iter()
            .map(|entry| entry.command.as_str())
            .collect()
    }

    #[test]
    fn entries_run_in_order_and_a_failure_pauses() {
        let now = SystemTime::now();
        let mut queue = CommandQueue::default();
        queue.push("one".into(), None);
        queue.push("two".into(), None);
        queue.push("three".into(), None);
        // the running command succeeded
        assert!(!queue.ended(Some(0), true));
        assert_eq!(queue.take_due(now).unwrap().command, "one");
        // one failed: two and three wait
        assert!(queue.ended(Some(1), true));
        assert!(queue.paused());
        assert_eq!(queue.take_due(now), None);
        // pausing twice tells only once
        assert!(!queue.ended(Some(1), true));
        queue.resume();
        assert_eq!(queue.take_due(now).unwrap().command, "two");
        // without stop on failure it goes on
        assert!(!queue.ended(Some(2), false));
        assert_eq!(queue.take_due(now).unwrap().command, "three");
        assert_eq!(queue.take_due(now), None);
        // nothing waits, nothing pauses
        assert!(!queue.ended(Some(1), true));
    }

    #[test]
    fn timed_entries_wait_and_hold_back_later_ones() {
        let now = SystemTime::now();
        let mut queue = CommandQueue::default();
        queue.push("later".into(), Some(now + Duration::from_secs(60)));
        queue.push("after".into(), None);
        assert_eq!(queue.take_due(now), None);
        let due = queue.take_due(now + Duration::from_secs(61)).unwrap();
        assert_eq!(due.command, "later");
        assert_eq!(queue.take_due(now).unwrap().command, "after");
    }

    #[test]
    fn entries_are_edited_dropped_and_moved() {
        let mut queue = CommandQueue::default();
        let a = queue.push("a".into(), None);
        let b = queue.push("b".into(), None);
        let c = queue.push("c".into(), None);
        assert!(queue.move_to(c, 0));
        assert_eq!(commands(&queue), ["c", "a", "b"]);
        assert!(queue.move_to(c, 99));
        assert_eq!(commands(&queue), ["a", "b", "c"]);
        assert!(queue.edit(a, " a2 "));
        assert!(queue.edit(b, "  "));
        assert_eq!(commands(&queue), ["a2", "c"]);
        assert!(queue.remove(c));
        assert!(!queue.remove(c));
        assert!(!queue.edit(99, "x"));
        assert!(!queue.move_to(99, 0));
        queue.ended(Some(1), true);
        queue.clear();
        assert!(!queue.paused());
        assert!(queue.entries().is_empty());
    }

    #[test]
    fn typed_text_gives_command_and_time() {
        let now = SystemTime::now();
        assert_eq!(
            parse_entry("  make test ", now),
            Some(("make test".into(), None))
        );
        assert_eq!(parse_entry("   ", now), None);
        let (command, at) = parse_entry("@18:00 make deploy", now).unwrap();
        assert_eq!(command, "make deploy");
        let wait = at.unwrap().duration_since(now).unwrap();
        assert!(wait < Duration::from_secs(24 * 3600));
        assert_eq!(parse_entry("@25:00 x", now), None);
        assert_eq!(parse_entry("@18:00", now), None);
        assert_eq!(parse_entry("@soon x", now), None);

        assert_eq!(seconds_until(17 * 3600, 18 * 3600), 3600);
        // a time already gone today is tomorrow
        assert_eq!(seconds_until(19 * 3600, 18 * 3600), 23 * 3600);
        assert_eq!(seconds_until(18 * 3600, 18 * 3600), 0);
    }

    #[test]
    fn queued_commands_run_when_the_prompt_comes_back() {
        use crate::{
            settings::{Shell, TerminalSettings},
            terminal::{
                tests::{profile, spawn_with, wait_for_text},
                with_shell_integration,
            },
        };
        use std::time::Instant;

        let bash = with_shell_integration(&profile(Shell::Program("/bin/bash".into())));
        let mut builder = spawn_with(&TerminalSettings::default(), &bash);
        let terminal = &mut builder.terminal;
        terminal.type_text(b"echo st_$((1+1)); sleep 1\r");
        wait_for_text(terminal, "st_2");
        terminal.update_queue(|queue| {
            queue.push("echo q_$((2+3))".into(), None);
            queue.push("false".into(), None);
            queue.push("echo never_$((1+1))".into(), None);
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut paused = false;
        while !paused {
            terminal.sync();
            paused = terminal
                .handle_scan_events()
                .iter()
                .any(|event| matches!(event, crate::terminal::Event::QueuePaused { .. }));
            assert!(Instant::now() < deadline, "queue never paused");
            std::thread::sleep(Duration::from_millis(50));
        }
        wait_for_text(terminal, "q_5");
        assert_eq!(commands(terminal.queue()), ["echo never_$((1+1))"]);
        assert!(terminal.hint_line().is_some());
        terminal.update_queue(|queue| queue.resume());
        wait_for_text(terminal, "never_2");
    }
}
