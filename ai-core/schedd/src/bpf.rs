//! The eBPF collector.
//!
//! `bpf/telemetry.bpf.c` hooks the kernel's scheduler tracepoints and keeps per-thread
//! counters in a BPF map. This loads it with `bpftool` (which also pins it, so the
//! programs stay attached while the daemon runs), reads the map as JSON each round, adds
//! the threads up per process, and hands the result to the engine like any other
//! collector. Stopping the daemon removes the pins, which detaches the programs.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::collector::{parse_stat, parse_uid, Collector, Sample};
use crate::features::Counters;

/// What the kernel side recorded about one thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadStats {
    pub tid: u32,
    pub tgid: u32,
    pub comm: String,
    pub counters: Counters,
    /// The longest single wait for a CPU, in nanoseconds.
    pub wake_lat_max_ns: u64,
}

fn number(object: &Value, field: &str) -> u64 {
    object.get(field).and_then(Value::as_u64).unwrap_or(0)
}

/// A name as `bpftool` prints it: a string, or an array of byte values ending in zeros.
fn comm_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(bytes) => {
            let raw: Vec<u8> = bytes.iter().filter_map(Value::as_u64).map(|b| b as u8).take_while(|b| *b != 0).collect();
            String::from_utf8_lossy(&raw).into_owned()
        }
        _ => String::new(),
    }
}

/// Parse `bpftool -j map dump` of the telemetry map. Entries may be shaped
/// `{"key": 7, "value": {...}}` or wrapped in `{"formatted": {...}}`, depending on the
/// bpftool version.
pub fn parse_map_dump(json: &str) -> Result<Vec<ThreadStats>, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("bpftool printed something unexpected: {e}"))?;
    let entries = root.as_array().ok_or("bpftool did not print a list")?;
    let mut threads = Vec::new();
    for entry in entries {
        let entry = entry.get("formatted").unwrap_or(entry);
        let (Some(tid), Some(value)) = (entry.get("key").and_then(Value::as_u64), entry.get("value")) else {
            continue;
        };
        threads.push(ThreadStats {
            tid: tid as u32,
            tgid: number(value, "tgid") as u32,
            comm: value.get("comm").map(comm_of).unwrap_or_default(),
            counters: Counters {
                run_ns: number(value, "run_ns"),
                wait_ns: number(value, "wait_ns"),
                slices: number(value, "slices"),
                voluntary: number(value, "voluntary"),
                involuntary: number(value, "involuntary"),
            },
            wake_lat_max_ns: number(value, "wake_lat_max_ns"),
        });
    }
    Ok(threads)
}

/// Add the threads of each process up.
pub fn per_process(threads: &[ThreadStats]) -> HashMap<u32, Counters> {
    let mut processes: HashMap<u32, Counters> = HashMap::new();
    for t in threads.iter().filter(|t| t.tgid != 0) {
        let sum = processes.entry(t.tgid).or_default();
        sum.run_ns += t.counters.run_ns;
        sum.wait_ns += t.counters.wait_ns;
        sum.slices += t.counters.slices;
        sum.voluntary += t.counters.voluntary;
        sum.involuntary += t.counters.involuntary;
    }
    processes
}

pub struct BpfCollector {
    bpftool: String,
    pin_dir: PathBuf,
    proc_root: PathBuf,
}

const PF_KTHREAD: u64 = 0x0020_0000;

impl BpfCollector {
    /// Load `object` and attach its programs; they stay attached until this is dropped.
    pub fn attach(bpftool: &str, object: &Path, pin_dir: &Path) -> io::Result<Self> {
        let _ = fs::remove_dir_all(pin_dir); // a leftover from a daemon that did not exit cleanly
        fs::create_dir_all(pin_dir.join("maps"))?;
        let output = Command::new(bpftool)
            .args(["prog", "loadall"])
            .arg(object)
            .arg(pin_dir.join("progs"))
            .args(["autoattach", "pinmaps"])
            .arg(pin_dir.join("maps"))
            .output();
        let output = match output {
            Ok(o) => o,
            Err(e) => {
                let _ = fs::remove_dir_all(pin_dir);
                return Err(io::Error::new(e.kind(), format!("cannot run {bpftool}: {e}")));
            }
        };
        if !output.status.success() {
            let _ = fs::remove_dir_all(pin_dir);
            let message = String::from_utf8_lossy(&output.stderr);
            return Err(io::Error::other(format!("{bpftool} could not load {}: {}", object.display(), message.trim())));
        }
        Ok(Self { bpftool: bpftool.to_string(), pin_dir: pin_dir.to_path_buf(), proc_root: PathBuf::from("/proc") })
    }

    fn dump(&self) -> io::Result<String> {
        let output = Command::new(&self.bpftool).args(["-j", "map", "dump", "pinned"]).arg(self.pin_dir.join("maps/tasks")).output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!("cannot read the telemetry map: {}", String::from_utf8_lossy(&output.stderr).trim())));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

impl Drop for BpfCollector {
    fn drop(&mut self) {
        // removing the pins is what detaches the programs and frees the map
        let _ = fs::remove_dir_all(&self.pin_dir);
    }
}

impl Collector for BpfCollector {
    fn sample(&mut self) -> io::Result<Vec<Sample>> {
        let threads = parse_map_dump(&self.dump()?).map_err(io::Error::other)?;
        let mut samples = Vec::new();
        for (tgid, counters) in per_process(&threads) {
            let dir = self.proc_root.join(tgid.to_string());
            // the process may have exited since the map was written
            let (Ok(stat), Ok(status)) = (fs::read_to_string(dir.join("stat")), fs::read_to_string(dir.join("status"))) else {
                continue;
            };
            let (Some(stat), Some(uid)) = (parse_stat(&stat), parse_uid(&status)) else { continue };
            samples.push(Sample {
                pid: tgid as i32,
                start_time: stat.start_time,
                comm: stat.comm,
                uid,
                kernel_thread: stat.flags & PF_KTHREAD != 0,
                counters,
            });
        }
        Ok(samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &str = r#"[
        {"key": 100, "value": {"run_ns": 5000, "wait_ns": 100, "slices": 3, "voluntary": 2, "involuntary": 1,
          "wake_lat_max_ns": 70, "run_start": 0, "runnable_since": 0, "tgid": 100, "comm": "firefox"}},
        {"key": 101, "value": {"run_ns": 1000, "wait_ns": 10, "slices": 1, "voluntary": 1, "involuntary": 0,
          "wake_lat_max_ns": 10, "run_start": 0, "runnable_since": 0, "tgid": 100, "comm": [87, 111, 114, 107, 0, 0]}},
        {"key": 7, "value": {"run_ns": 1, "wait_ns": 0, "slices": 1, "voluntary": 0, "involuntary": 0,
          "wake_lat_max_ns": 0, "run_start": 0, "runnable_since": 0, "tgid": 0, "comm": "swapper"}}
    ]"#;

    #[test]
    fn a_map_dump_is_parsed() {
        let threads = parse_map_dump(PLAIN).unwrap();
        assert_eq!(threads.len(), 3);
        assert_eq!(threads[0].tid, 100);
        assert_eq!(threads[0].comm, "firefox");
        assert_eq!(threads[0].counters, Counters { run_ns: 5000, wait_ns: 100, slices: 3, voluntary: 2, involuntary: 1 });
        assert_eq!(threads[0].wake_lat_max_ns, 70);
        assert_eq!(threads[1].comm, "Work", "a name printed as bytes");
    }

    #[test]
    fn the_wrapped_shape_is_parsed_too() {
        let wrapped = r#"[{"formatted": {"key": 5, "value": {"run_ns": 9, "wait_ns": 0, "slices": 1, "voluntary": 0, "involuntary": 0, "wake_lat_max_ns": 0, "tgid": 5, "comm": "x"}}}]"#;
        let threads = parse_map_dump(wrapped).unwrap();
        assert_eq!((threads[0].tid, threads[0].tgid, threads[0].counters.run_ns), (5, 5, 9));
    }

    #[test]
    fn nonsense_is_an_error_and_odd_entries_are_skipped() {
        assert!(parse_map_dump("not json").is_err());
        assert!(parse_map_dump("{}").is_err());
        assert!(parse_map_dump("[]").unwrap().is_empty());
        assert!(parse_map_dump(r#"[{"nothing": 1}, {"key": 1}]"#).unwrap().is_empty());
    }

    #[test]
    fn threads_are_added_up_per_process_and_idle_is_dropped() {
        let totals = per_process(&parse_map_dump(PLAIN).unwrap());
        assert_eq!(totals.len(), 1, "the tgid 0 entry (idle) is dropped");
        let firefox = totals[&100];
        assert_eq!(firefox, Counters { run_ns: 6000, wait_ns: 110, slices: 4, voluntary: 3, involuntary: 1 });
    }

    #[test]
    fn a_missing_bpftool_is_a_clear_error() {
        let dir = std::env::temp_dir().join(format!("osaima-schedd-pin-{}", std::process::id()));
        let err = BpfCollector::attach("/nonexistent/bpftool", Path::new("/nonexistent/x.bpf.o"), &dir).err().expect("attaching should fail");
        assert!(err.to_string().contains("cannot run"), "{err}");
        assert!(!dir.exists(), "nothing is left behind");
    }
}
