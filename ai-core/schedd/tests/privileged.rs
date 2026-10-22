//! Tests that need root and the compiled eBPF telemetry. They are ignored by default; CI
//! runs them with
//!
//!   sudo -E OSAIMA_BPF_OBJECT=... OSAIMA_BPFTOOL=... cargo test --test privileged -- --ignored
//!
//! They touch only the processes they start themselves.

use std::fs;
use std::io;
use std::path::Path;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use osaima_schedd::actuator::KernelActuator;
use osaima_schedd::bpf::BpfCollector;
use osaima_schedd::collector::{Collector, ProcCollector, Sample};
use osaima_schedd::engine::{Config, Engine};
use osaima_schedd::features::Features;
use osaima_schedd::model::{Class, Classifier};
use osaima_schedd::policy::Policy;

fn setup() -> (String, String) {
    let object = std::env::var("OSAIMA_BPF_OBJECT")
        .expect("set OSAIMA_BPF_OBJECT to the compiled telemetry.bpf.o");
    let bpftool = std::env::var("OSAIMA_BPFTOOL").unwrap_or_else(|_| "bpftool".into());
    assert_eq!(unsafe { libc::geteuid() }, 0, "these tests need root");
    (object, bpftool)
}

/// A process that uses a whole CPU until killed.
struct Hog(Child);
impl Hog {
    fn new() -> Self {
        Hog(Command::new("sh")
            .args(["-c", "while :; do :; done"])
            .spawn()
            .expect("sh"))
    }
    fn pid(&self) -> i32 {
        self.0.id() as i32
    }
}
impl Drop for Hog {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Only lets the processes a test started through, so nothing else on the machine is touched.
struct Only<C> {
    inner: C,
    pids: Vec<i32>,
}
impl<C: Collector> Collector for Only<C> {
    fn sample(&mut self) -> io::Result<Vec<Sample>> {
        Ok(self
            .inner
            .sample()?
            .into_iter()
            .filter(|s| self.pids.contains(&s.pid))
            .collect())
    }
}

fn find(samples: &[Sample], pid: i32) -> Sample {
    samples
        .iter()
        .find(|s| s.pid == pid)
        .unwrap_or_else(|| panic!("pid {pid} not in the measurements"))
        .clone()
}

fn nice_of(pid: i32) -> i32 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let rest: Vec<&str> = stat[stat.rfind(')').unwrap() + 1..]
        .split_whitespace()
        .collect();
    rest[16].parse().unwrap() // field 19
}

fn policy_of(pid: i32) -> i32 {
    unsafe { libc::sched_getscheduler(pid) }
}

#[test]
#[ignore = "needs root and the compiled eBPF telemetry"]
fn ebpf_and_proc_agree_about_a_busy_and_an_idle_process() {
    let (object, bpftool) = setup();
    let mut bpf = BpfCollector::attach(
        &bpftool,
        Path::new(&object),
        Path::new("/sys/fs/bpf/osaima-schedd-test-agree"),
    )
    .expect("loading the telemetry");
    let mut proc_ = ProcCollector::new();
    let hog = Hog::new();
    let idle = Command::new("sleep").arg("60").spawn().unwrap();
    let idle_pid = idle.id() as i32;
    let _idle_guard = Hog(idle);

    thread::sleep(Duration::from_millis(500));
    let (b0, p0) = (bpf.sample().unwrap(), proc_.sample().unwrap());
    let started = Instant::now();
    thread::sleep(Duration::from_secs(3));
    let (b1, p1) = (bpf.sample().unwrap(), proc_.sample().unwrap());
    let seconds = started.elapsed().as_secs_f64();

    let feature = |before: &[Sample], after: &[Sample], pid: i32| {
        Features::between(
            &find(before, pid).counters,
            &find(after, pid).counters,
            seconds,
        )
        .unwrap()
    };
    let (bpf_hog, proc_hog) = (feature(&b0, &b1, hog.pid()), feature(&p0, &p1, hog.pid()));
    println!("busy: eBPF {bpf_hog:?}\n      /proc {proc_hog:?}");
    assert!(
        bpf_hog.cpu_util > 0.6,
        "the eBPF telemetry saw {} cores in use",
        bpf_hog.cpu_util
    );
    assert!(
        (bpf_hog.cpu_util - proc_hog.cpu_util).abs() < 0.1,
        "eBPF {} against /proc {}",
        bpf_hog.cpu_util,
        proc_hog.cpu_util
    );
    assert_eq!(Classifier::Rules.classify(&bpf_hog).0, Class::Batch);

    // the sleeper is idle but may not have run at all: it can be missing from the map
    if let (Some(_), Some(_)) = (
        b0.iter().find(|s| s.pid == idle_pid),
        b1.iter().find(|s| s.pid == idle_pid),
    ) {
        let quiet = feature(&b0, &b1, idle_pid);
        assert!(
            quiet.cpu_util < 0.05,
            "an idle process used {} cores",
            quiet.cpu_util
        );
    }
}

#[test]
#[ignore = "needs root and the compiled eBPF telemetry"]
fn a_cpu_hog_is_made_nicer_and_put_back_afterwards() {
    let (object, bpftool) = setup();
    let hog = Hog::new();
    let pid = hog.pid();
    assert_eq!(nice_of(pid), 0);
    let bpf = BpfCollector::attach(
        &bpftool,
        Path::new(&object),
        Path::new("/sys/fs/bpf/osaima-schedd-test-engine"),
    )
    .expect("loading the telemetry");
    let cfg = Config {
        min_uid: 0,
        hysteresis: 2,
        ..Config::default()
    };
    let mut engine = Engine::new(
        Only {
            inner: bpf,
            pids: vec![pid],
        },
        KernelActuator,
        Classifier::Rules,
        Policy::default(),
        cfg,
    );

    let mut changed = None;
    for _ in 0..8 {
        thread::sleep(Duration::from_secs(1));
        let report = engine.tick(1.0).unwrap();
        if let Some(c) = report.changes.first() {
            changed = Some(c.clone());
            break;
        }
    }
    let change = changed.expect("the hog should have been judged and changed within eight seconds");
    assert_eq!(
        (change.pid, change.class, change.applied),
        (pid, Class::Batch, true),
        "{change:?}"
    );
    assert_eq!(nice_of(pid), 10);
    assert_eq!(policy_of(pid), libc::SCHED_BATCH);

    engine.shutdown();
    assert_eq!(nice_of(pid), 0, "the original priority is put back");
    assert_eq!(policy_of(pid), libc::SCHED_OTHER);
}

#[test]
#[ignore = "needs root and the compiled eBPF telemetry"]
fn stopping_removes_the_telemetry() {
    let (object, bpftool) = setup();
    let pins = Path::new("/sys/fs/bpf/osaima-schedd-test-stop");
    let bpf =
        BpfCollector::attach(&bpftool, Path::new(&object), pins).expect("loading the telemetry");
    assert!(pins.join("maps/tasks").exists());
    drop(bpf);
    assert!(
        !pins.exists(),
        "the pins are removed, which detaches the programs"
    );
}
