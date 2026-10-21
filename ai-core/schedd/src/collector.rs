//! Where the scheduler counters come from.
//!
//! [`ProcCollector`] reads the kernel's own per-thread accounting in `/proc`. It needs
//! no privilege and no BPF, and is what the tests use. An eBPF collector with finer
//! telemetry plugs into the same [`Collector`] trait.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::features::Counters;

/// One process (all its threads added up) at one moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    pub pid: i32,
    /// Start time in clock ticks since boot; together with `pid` it identifies a process,
    /// since pids get reused.
    pub start_time: u64,
    pub comm: String,
    pub uid: u32,
    /// Kernel threads are never touched.
    pub kernel_thread: bool,
    pub counters: Counters,
}

pub trait Collector {
    fn sample(&mut self) -> io::Result<Vec<Sample>>;
}

/// Reads `/proc` (or a copy of its layout, for tests).
pub struct ProcCollector {
    root: PathBuf,
}

impl ProcCollector {
    pub fn new() -> Self {
        Self { root: PathBuf::from("/proc") }
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Default for ProcCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector for ProcCollector {
    fn sample(&mut self) -> io::Result<Vec<Sample>> {
        let mut samples = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let Ok(entry) = entry else { continue };
            let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
            // A process can exit between listing and reading; that is not an error.
            if let Some(sample) = read_process(&entry.path(), pid) {
                samples.push(sample);
            }
        }
        Ok(samples)
    }
}

fn read_process(dir: &Path, pid: i32) -> Option<Sample> {
    let stat = parse_stat(&fs::read_to_string(dir.join("stat")).ok()?)?;
    let status = fs::read_to_string(dir.join("status")).ok()?;
    let uid = parse_uid(&status)?;

    // Counters are per thread, so add up every thread of the process.
    let mut counters = Counters::default();
    let mut any = false;
    if let Ok(tasks) = fs::read_dir(dir.join("task")) {
        for task in tasks.flatten() {
            let tdir = task.path();
            if let Ok(text) = fs::read_to_string(tdir.join("schedstat")) {
                if let Some((run, wait, slices)) = parse_schedstat(&text) {
                    counters.run_ns += run;
                    counters.wait_ns += wait;
                    counters.slices += slices;
                    any = true;
                }
            }
            if let Ok(text) = fs::read_to_string(tdir.join("status")) {
                let (vol, invol) = parse_switches(&text);
                counters.voluntary += vol;
                counters.involuntary += invol;
            }
        }
    }
    if !any {
        // No scheduler accounting for this process (kernel without it, or it just exited).
        return None;
    }
    Some(Sample {
        pid,
        start_time: stat.start_time,
        comm: stat.comm,
        uid,
        kernel_thread: stat.flags & PF_KTHREAD != 0,
        counters,
    })
}

/// The `PF_KTHREAD` bit in the `flags` field of `/proc/PID/stat`.
const PF_KTHREAD: u64 = 0x0020_0000;

#[derive(Debug, PartialEq, Eq)]
pub struct Stat {
    pub comm: String,
    pub flags: u64,
    pub start_time: u64,
}

/// `/proc/PID/stat` is `pid (comm) state ppid ...`; the name may itself contain spaces and
/// parentheses, so it is found by the first `(` and the last `)`.
pub fn parse_stat(text: &str) -> Option<Stat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open {
        return None;
    }
    let comm = text[open + 1..close].to_string();
    // After the name: state(field 3) ppid(4) ... flags(9) ... starttime(22)
    let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
    Some(Stat { comm, flags: rest.get(6)?.parse().ok()?, start_time: rest.get(19)?.parse().ok()? })
}

/// `/proc/PID/task/TID/schedstat` is `run_ns wait_ns timeslices`.
pub fn parse_schedstat(text: &str) -> Option<(u64, u64, u64)> {
    let mut it = text.split_whitespace().map(|v| v.parse::<u64>());
    Some((it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?))
}

/// The real user id from the `Uid:` line of `/proc/PID/status`.
pub fn parse_uid(status: &str) -> Option<u32> {
    status.lines().find_map(|l| l.strip_prefix("Uid:"))?.split_whitespace().next()?.parse().ok()
}

/// Voluntary and involuntary context switches from `/proc/PID/status`.
pub fn parse_switches(status: &str) -> (u64, u64) {
    let get = |key: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    (get("voluntary_ctxt_switches:"), get("nonvoluntary_ctxt_switches:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "4242 (Web Content (2)) S 1 4242 4242 0 -1 4194560 1000 0 0 0 10 5 0 0 20 0 8 0 987654 1000000 500 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0";

    #[test]
    fn the_stat_line_is_parsed_even_with_a_tricky_name() {
        let s = parse_stat(STAT).unwrap();
        assert_eq!(s.comm, "Web Content (2)");
        assert_eq!(s.flags, 4194560);
        assert_eq!(s.start_time, 987654);
        assert!(parse_stat("garbage").is_none());
        assert!(parse_stat("1 (x) S").is_none());
    }

    #[test]
    fn kernel_threads_are_recognised_by_their_flag() {
        let line = STAT.replace("4194560", &PF_KTHREAD.to_string());
        let s = parse_stat(&line).unwrap();
        assert_ne!(s.flags & PF_KTHREAD, 0);
        assert_eq!(parse_stat(STAT).unwrap().flags & PF_KTHREAD, 0);
    }

    #[test]
    fn schedstat_and_status_are_parsed() {
        assert_eq!(parse_schedstat("123456789 987 42\n"), Some((123456789, 987, 42)));
        assert_eq!(parse_schedstat("1 2"), None);
        assert_eq!(parse_schedstat("a b c"), None);
        let status = "Name:\tbash\nUid:\t1000\t1000\t1000\t1000\nvoluntary_ctxt_switches:\t77\nnonvoluntary_ctxt_switches:\t5\n";
        assert_eq!(parse_uid(status), Some(1000));
        assert_eq!(parse_switches(status), (77, 5));
        assert_eq!(parse_switches("Name:\tx\n"), (0, 0));
        assert_eq!(parse_uid("Name:\tx\n"), None);
    }

    /// A fake /proc with one process of two threads and one kernel thread.
    fn fake_proc() -> PathBuf {
        let root = std::env::temp_dir().join(format!("osaima-schedd-proc-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&root);
        let write = |path: PathBuf, text: &str| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        let status = |uid: u32, vol: u64, invol: u64| format!("Name:\tx\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nvoluntary_ctxt_switches:\t{vol}\nnonvoluntary_ctxt_switches:\t{invol}\n");
        write(root.join("100/stat"), STAT);
        write(root.join("100/status"), &status(1000, 10, 1));
        for (tid, run) in [(100, 1_000_000u64), (101, 2_000_000)] {
            write(root.join(format!("100/task/{tid}/schedstat")), &format!("{run} 500 10\n"));
            write(root.join(format!("100/task/{tid}/status")), &status(1000, 10, 1));
        }
        write(root.join("2/stat"), &STAT.replace("4194560", &PF_KTHREAD.to_string()).replace("4242 (Web Content (2))", "2 (kthreadd)"));
        write(root.join("2/status"), &status(0, 0, 0));
        write(root.join("2/task/2/schedstat"), "5 5 1\n");
        write(root.join("2/task/2/status"), &status(0, 0, 0));
        write(root.join("notaprocess/stat"), "ignored");
        write(root.join("300/stat"), STAT); // exited halfway: no status file
        root
    }

    #[test]
    fn a_fake_proc_is_read_and_threads_are_added_up() {
        let root = fake_proc();
        let mut samples = ProcCollector::at(&root).sample().unwrap();
        samples.sort_by_key(|s| s.pid);
        assert_eq!(samples.len(), 2, "{samples:?}");
        let kthread = &samples[0];
        assert_eq!((kthread.pid, kthread.uid, kthread.kernel_thread), (2, 0, true));
        let app = &samples[1];
        assert_eq!((app.pid, app.uid, app.kernel_thread), (100, 1000, false));
        assert_eq!(app.comm, "Web Content (2)");
        assert_eq!(app.start_time, 987654);
        assert_eq!(app.counters.run_ns, 3_000_000);
        assert_eq!(app.counters.wait_ns, 1000);
        assert_eq!(app.counters.slices, 20);
        assert_eq!(app.counters.voluntary, 20);
        assert_eq!(app.counters.involuntary, 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_real_proc_contains_this_test() {
        let samples = ProcCollector::new().sample().unwrap();
        let me = samples.iter().find(|s| s.pid == std::process::id() as i32);
        let me = me.expect("this process should be listed (is /proc/PID/schedstat available?)");
        assert!(!me.kernel_thread);
        assert_eq!(me.uid, unsafe { libc::getuid() });
        assert!(me.counters.slices > 0);
    }
}
