//! Live system telemetry, sampled in the background so requests never block
//! on `/proc` and CPU percentages are measured over a real interval.

use std::cmp::Reverse;
use std::fmt;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use sysinfo::{
    CpuRefreshKind, Disks, MemoryRefreshKind, ProcessRefreshKind, RefreshKind, System, UpdateKind,
};

/// How often the background sampler refreshes CPU, memory and process data.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize)]
pub struct SystemStats {
    pub os: String,
    pub os_version: String,
    pub kernel: String,
    pub host: String,
    pub uptime_secs: u64,
    pub load_average: [f64; 3],
    pub cpu: CpuStats,
    pub memory: MemoryStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuStats {
    pub usage_percent: f32,
    pub cores: usize,
    pub brand: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryStats {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    /// Percentage of one core, like `top`, so it can exceed 100.
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub uid: Option<u32>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSort {
    Cpu,
    Memory,
}

/// Signals a client may send through `kill_process`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSignal {
    Term,
    Kill,
    Int,
    Hup,
    Stop,
    Cont,
}

impl ProcessSignal {
    pub const NAMES: [&'static str; 6] = ["TERM", "KILL", "INT", "HUP", "STOP", "CONT"];

    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_uppercase();
        match name.strip_prefix("SIG").unwrap_or(&name) {
            "TERM" => Some(Self::Term),
            "KILL" => Some(Self::Kill),
            "INT" => Some(Self::Int),
            "HUP" => Some(Self::Hup),
            "STOP" => Some(Self::Stop),
            "CONT" => Some(Self::Cont),
            _ => None,
        }
    }

    fn raw(self) -> libc::c_int {
        match self {
            Self::Term => libc::SIGTERM,
            Self::Kill => libc::SIGKILL,
            Self::Int => libc::SIGINT,
            Self::Hup => libc::SIGHUP,
            Self::Stop => libc::SIGSTOP,
            Self::Cont => libc::SIGCONT,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SignalError {
    /// PID 0/1 or the daemon itself.
    Protected(u32),
    NotFound(u32),
    /// The caller does not own the target process.
    PermissionDenied(u32),
    Os(u32, String),
}

impl fmt::Display for SignalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protected(pid) => write!(f, "process {pid} is protected and cannot be signalled"),
            Self::NotFound(pid) => write!(f, "no process with pid {pid}"),
            Self::PermissionDenied(pid) => {
                write!(
                    f,
                    "permission denied: process {pid} belongs to another user"
                )
            }
            Self::Os(pid, err) => write!(f, "failed to signal process {pid}: {err}"),
        }
    }
}

impl std::error::Error for SignalError {}

struct Snapshot {
    stats: SystemStats,
    processes: Vec<ProcessInfo>,
}

pub struct SystemMonitor {
    sys: Mutex<System>,
    snapshot: RwLock<Snapshot>,
}

fn refresh_kind() -> RefreshKind {
    RefreshKind::new()
        .with_cpu(CpuRefreshKind::everything())
        .with_memory(MemoryRefreshKind::everything())
        .with_processes(
            ProcessRefreshKind::new()
                .with_cpu()
                .with_memory()
                .with_user(UpdateKind::OnlyIfNotSet),
        )
}

impl SystemMonitor {
    /// Create a monitor and take an initial sample. CPU percentages become
    /// meaningful after the first background refresh.
    pub fn new() -> Self {
        let sys = System::new_with_specifics(refresh_kind());
        let snapshot = take_snapshot(&sys);
        Self {
            sys: Mutex::new(sys),
            snapshot: RwLock::new(snapshot),
        }
    }

    /// Re-sample the system now.
    pub fn refresh(&self) {
        let snapshot = {
            let mut sys = self.sys.lock().unwrap_or_else(|e| e.into_inner());
            sys.refresh_specifics(refresh_kind());
            take_snapshot(&sys)
        };
        *self.snapshot.write().unwrap_or_else(|e| e.into_inner()) = snapshot;
    }

    /// Refresh every `interval` on a background thread until the monitor is dropped.
    pub fn spawn_sampler(self: &Arc<Self>, interval: Duration) -> io::Result<()> {
        let monitor = Arc::downgrade(self);
        thread::Builder::new()
            .name("osaima-sampler".into())
            .spawn(move || loop {
                thread::sleep(interval);
                match monitor.upgrade() {
                    Some(monitor) => monitor.refresh(),
                    None => break,
                }
            })
            .map(|_| ())
    }

    pub fn stats(&self) -> SystemStats {
        self.snapshot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .stats
            .clone()
    }

    /// Top `limit` processes ordered by `sort`, highest first.
    pub fn processes(&self, sort: ProcessSort, limit: usize) -> Vec<ProcessInfo> {
        let mut procs = self
            .snapshot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .processes
            .clone();
        match sort {
            ProcessSort::Cpu => procs.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent)),
            ProcessSort::Memory => procs.sort_by_key(|p| Reverse(p.memory_bytes)),
        }
        procs.truncate(limit);
        procs
    }

    /// Mounted filesystems, read fresh on each call.
    pub fn disks(&self) -> Vec<DiskInfo> {
        Disks::new_with_refreshed_list()
            .list()
            .iter()
            .map(|d| DiskInfo {
                name: d.name().to_string_lossy().into_owned(),
                mount_point: d.mount_point().display().to_string(),
                file_system: d.file_system().to_string_lossy().into_owned(),
                total_bytes: d.total_space(),
                available_bytes: d.available_space(),
            })
            .collect()
    }

    /// Send `signal` to `pid` on behalf of `caller_uid`.
    ///
    /// Non-root callers may only signal their own processes; PID 0, PID 1 and
    /// the daemon itself are always refused.
    pub fn signal_process(
        &self,
        pid: u32,
        signal: ProcessSignal,
        caller_uid: u32,
    ) -> Result<(), SignalError> {
        if pid <= 1 || pid == std::process::id() {
            return Err(SignalError::Protected(pid));
        }
        // Ownership is read fresh from /proc so a stale snapshot can't be abused.
        let owner = std::fs::metadata(format!("/proc/{pid}"))
            .map_err(|_| SignalError::NotFound(pid))?
            .uid();
        if caller_uid != 0 && owner != caller_uid {
            return Err(SignalError::PermissionDenied(pid));
        }
        let raw_pid = libc::pid_t::try_from(pid).map_err(|_| SignalError::NotFound(pid))?;
        // SAFETY: kill(2) has no memory-safety preconditions.
        if unsafe { libc::kill(raw_pid, signal.raw()) } == 0 {
            Ok(())
        } else {
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::ESRCH) => Err(SignalError::NotFound(pid)),
                Some(libc::EPERM) => Err(SignalError::PermissionDenied(pid)),
                _ => Err(SignalError::Os(pid, err.to_string())),
            }
        }
    }
}

impl Default for SystemMonitor {
    fn default() -> Self {
        Self::new()
    }
}

fn take_snapshot(sys: &System) -> Snapshot {
    let load = System::load_average();
    let stats = SystemStats {
        os: System::name().unwrap_or_else(|| "Unknown".into()),
        os_version: System::os_version().unwrap_or_default(),
        kernel: System::kernel_version().unwrap_or_else(|| "Unknown".into()),
        host: System::host_name().unwrap_or_else(|| "Unknown".into()),
        uptime_secs: System::uptime(),
        load_average: [load.one, load.five, load.fifteen],
        cpu: CpuStats {
            usage_percent: sys.global_cpu_info().cpu_usage(),
            cores: sys.cpus().len(),
            brand: sys
                .cpus()
                .first()
                .map(|c| c.brand().trim().to_string())
                .unwrap_or_default(),
        },
        memory: MemoryStats {
            total_bytes: sys.total_memory(),
            used_bytes: sys.used_memory(),
            available_bytes: sys.available_memory(),
            swap_total_bytes: sys.total_swap(),
            swap_used_bytes: sys.used_swap(),
        },
    };
    let processes = sys
        .processes()
        .values()
        // sysinfo also lists individual threads; keep whole processes only.
        .filter(|p| p.thread_kind().is_none())
        .map(|p| ProcessInfo {
            pid: p.pid().as_u32(),
            name: p.name().to_string(),
            cpu_percent: p.cpu_usage(),
            memory_bytes: p.memory(),
            uid: p.user_id().map(|uid| **uid),
            status: format!("{:?}", p.status()),
        })
        .collect();
    Snapshot { stats, processes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_are_populated() {
        let monitor = SystemMonitor::new();
        let stats = monitor.stats();
        assert!(stats.cpu.cores > 0);
        assert!(stats.memory.total_bytes > 0);
        assert!(stats.memory.used_bytes <= stats.memory.total_bytes);
    }

    #[test]
    fn processes_include_self_and_respect_limit() {
        let monitor = SystemMonitor::new();
        let all = monitor.processes(ProcessSort::Memory, usize::MAX);
        assert!(all.iter().any(|p| p.pid == std::process::id()));
        assert!(all
            .windows(2)
            .all(|w| w[0].memory_bytes >= w[1].memory_bytes));
        assert_eq!(
            monitor.processes(ProcessSort::Cpu, 3).len(),
            all.len().min(3)
        );
    }

    #[test]
    fn protected_pids_are_refused() {
        let monitor = SystemMonitor::new();
        for pid in [0, 1, std::process::id()] {
            assert_eq!(
                monitor.signal_process(pid, ProcessSignal::Term, 0),
                Err(SignalError::Protected(pid))
            );
        }
    }

    #[test]
    fn missing_process_is_not_found() {
        let monitor = SystemMonitor::new();
        let pid = 4_000_000; // above the kernel's maximum pid_max
        assert_eq!(
            monitor.signal_process(pid, ProcessSignal::Term, 0),
            Err(SignalError::NotFound(pid))
        );
    }

    #[test]
    fn other_users_processes_are_denied() {
        let monitor = SystemMonitor::new();
        let target = monitor
            .processes(ProcessSort::Cpu, usize::MAX)
            .into_iter()
            .find(|p| p.uid == Some(0) && p.pid > 1);
        if let Some(target) = target {
            assert_eq!(
                monitor.signal_process(target.pid, ProcessSignal::Cont, 65_534),
                Err(SignalError::PermissionDenied(target.pid))
            );
        }
    }

    #[test]
    fn signal_names_parse() {
        assert_eq!(ProcessSignal::parse("sigterm"), Some(ProcessSignal::Term));
        assert_eq!(ProcessSignal::parse("KILL"), Some(ProcessSignal::Kill));
        assert_eq!(ProcessSignal::parse("SEGV"), None);
    }
}
