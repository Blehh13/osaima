//! One round of measuring, deciding and setting priorities, with the safeguards.
//!
//! The safeguards, all of them on purpose:
//!
//! - only processes of ordinary users are touched (`min_uid`), never root's, never kernel
//!   threads, never this daemon, never anything on the ignore list
//! - a process needs a baseline reading first, so its first window is never judged
//! - a change needs the same verdict several windows in a row (`hysteresis`), so a process
//!   that merely had one busy second is left alone
//! - an unsure verdict (`min_confidence`) changes nothing
//! - the original settings are remembered and put back at shutdown
//! - a process the kernel refuses to change is left alone from then on, not retried forever

use std::collections::{HashMap, HashSet};
use std::io;

use crate::actuator::{Actuator, Original};
use crate::collector::{Collector, Sample};
use crate::features::{Counters, Features};
use crate::model::{Class, Classifier};
use crate::policy::{Policy, Setting};

#[derive(Clone, Debug)]
pub struct Config {
    /// Processes of users below this id are never touched (system and root services).
    pub min_uid: u32,
    /// Windows in a row with the same verdict before the priority is changed.
    pub hysteresis: u32,
    /// Verdicts less sure than this are ignored.
    pub min_confidence: f64,
    /// Process names never touched.
    pub ignore: Vec<String>,
    /// Decide and report, but change nothing.
    pub dry_run: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            min_uid: 1000,
            hysteresis: 3,
            min_confidence: 0.6,
            ignore: vec!["osaima-schedd".to_string()],
            dry_run: false,
        }
    }
}

/// A priority change decided in one round.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub pid: i32,
    pub comm: String,
    pub class: Class,
    pub confidence: f64,
    pub setting: Setting,
    /// False in a dry run, or when the kernel refused.
    pub applied: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct Report {
    /// Processes that were judged this round.
    pub judged: usize,
    /// How many of them were judged to be of each kind.
    pub counts: HashMap<Class, usize>,
    pub changes: Vec<Change>,
}

struct Tracked {
    start_time: u64,
    prev: Counters,
    /// The kind it has been given the priority of.
    applied: Option<Class>,
    /// A different verdict and how many windows in a row it has been given.
    candidate: Option<(Class, u32)>,
    originals: Vec<Original>,
    /// The kernel refused a change: leave it alone.
    refused: bool,
}

pub struct Engine<C, A> {
    collector: C,
    actuator: A,
    classifier: Classifier,
    policy: Policy,
    cfg: Config,
    own_pid: i32,
    tracked: HashMap<i32, Tracked>,
}

impl<C: Collector, A: Actuator> Engine<C, A> {
    pub fn new(collector: C, actuator: A, classifier: Classifier, policy: Policy, cfg: Config) -> Self {
        Engine { collector, actuator, classifier, policy, cfg, own_pid: std::process::id() as i32, tracked: HashMap::new() }
    }

    fn skip(&self, s: &Sample) -> bool {
        s.kernel_thread || s.pid <= 1 || s.pid == self.own_pid || s.uid < self.cfg.min_uid || self.cfg.ignore.contains(&s.comm)
    }

    /// Measure every process, judge those with a baseline, and change priorities that need it.
    /// `seconds` is the time since the previous round.
    pub fn tick(&mut self, seconds: f64) -> io::Result<Report> {
        let samples = self.collector.sample()?;
        let mut report = Report::default();
        let mut live = HashSet::new();

        for s in samples {
            if self.skip(&s) {
                continue;
            }
            live.insert(s.pid);
            let tracked = match self.tracked.get_mut(&s.pid) {
                Some(t) if t.start_time == s.start_time => t,
                _ => {
                    // new process, or the pid was reused: start from a baseline
                    self.tracked.insert(
                        s.pid,
                        Tracked { start_time: s.start_time, prev: s.counters, applied: None, candidate: None, originals: Vec::new(), refused: false },
                    );
                    continue;
                }
            };
            let window = Features::between(&tracked.prev, &s.counters, seconds);
            tracked.prev = s.counters;
            let Some(features) = window else { continue };
            let (class, confidence) = self.classifier.classify(&features);
            report.judged += 1;
            *report.counts.entry(class).or_insert(0) += 1;

            if tracked.refused || confidence < self.cfg.min_confidence {
                continue;
            }
            if tracked.applied == Some(class) {
                tracked.candidate = None;
                continue;
            }
            let votes = match tracked.candidate {
                Some((c, n)) if c == class => n + 1,
                _ => 1,
            };
            if votes < self.cfg.hysteresis {
                tracked.candidate = Some((class, votes));
                continue;
            }

            // the verdict has held long enough: set the priority
            tracked.candidate = None;
            let setting = self.policy.setting(class);
            let mut change = Change { pid: s.pid, comm: s.comm.clone(), class, confidence, setting, applied: false, error: None };
            if self.cfg.dry_run {
                tracked.applied = Some(class);
            } else {
                match set_priority(&mut self.actuator, &mut tracked.originals, s.pid, setting) {
                    Ok(()) => {
                        tracked.applied = Some(class);
                        change.applied = true;
                    }
                    Err(e) => {
                        tracked.refused = true;
                        change.error = Some(e.to_string());
                    }
                }
            }
            report.changes.push(change);
        }

        // processes that are gone need nothing put back
        self.tracked.retain(|pid, _| live.contains(pid));
        Ok(report)
    }

    /// Put every changed process back the way it was.
    pub fn shutdown(&mut self) {
        for t in self.tracked.values() {
            if t.applied.is_some() && !t.originals.is_empty() {
                let _ = self.actuator.restore(&t.originals);
            }
        }
    }

    pub fn classifier_name(&self) -> &'static str {
        self.classifier.describe()
    }

    /// How many processes are being tracked, and how many have been given a priority.
    pub fn counts(&self) -> (usize, usize) {
        (self.tracked.len(), self.tracked.values().filter(|t| t.applied.is_some()).count())
    }
}

/// Remember the original settings the first time, then apply the new one.
fn set_priority<A: Actuator>(actuator: &mut A, originals: &mut Vec<Original>, pid: i32, setting: Setting) -> io::Result<()> {
    if originals.is_empty() {
        *originals = actuator.read(pid)?;
    }
    actuator.apply(pid, setting)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::policy::Sched;
    use std::collections::VecDeque;

    /// Hands out one prepared reading per call.
    struct Script(VecDeque<Vec<Sample>>);
    impl Collector for Script {
        fn sample(&mut self) -> io::Result<Vec<Sample>> {
            Ok(self.0.pop_front().expect("the script ran out of rounds"))
        }
    }

    #[derive(Default)]
    struct Recorder {
        applied: Vec<(i32, Setting)>,
        restored: Vec<Vec<Original>>,
        refuse: bool,
    }
    impl Actuator for &mut Recorder {
        fn read(&mut self, pid: i32) -> io::Result<Vec<Original>> {
            Ok(vec![Original { tid: pid, nice: 0, policy: 0 }])
        }
        fn apply(&mut self, pid: i32, setting: Setting) -> io::Result<()> {
            if self.refuse {
                return Err(io::Error::from_raw_os_error(libc::EPERM));
            }
            self.applied.push((pid, setting));
            Ok(())
        }
        fn restore(&mut self, originals: &[Original]) -> io::Result<()> {
            self.restored.push(originals.to_vec());
            Ok(())
        }
    }

    /// A reading of a process after `n` one-second windows of a given behaviour:
    /// per window `run_ms` of CPU in `slices` runs, with `vol` voluntary and `invol` involuntary switches.
    fn proc(pid: i32, n: u64, run_ms: u64, slices: u64, vol: u64, invol: u64) -> Sample {
        Sample {
            pid,
            start_time: 1000,
            comm: format!("proc{pid}"),
            uid: 1000,
            kernel_thread: false,
            counters: Counters { run_ns: n * run_ms * 1_000_000, wait_ns: 0, slices: n * slices, voluntary: n * vol, involuntary: n * invol },
        }
    }
    fn batch(pid: i32, n: u64) -> Sample {
        proc(pid, n, 1000, 10, 1, 50)
    }
    fn interactive(pid: i32, n: u64) -> Sample {
        proc(pid, n, 40, 100, 100, 0)
    }

    fn engine(rounds: Vec<Vec<Sample>>, rec: &mut Recorder, cfg: Config) -> Engine<Script, &mut Recorder> {
        Engine::new(Script(rounds.into()), rec, Classifier::Rules, Policy::default(), cfg)
    }

    #[test]
    fn a_steady_batch_job_is_made_nicer_after_three_windows() {
        let mut rec = Recorder::default();
        let rounds = (0..5).map(|n| vec![batch(10, n)]).collect();
        let mut e = engine(rounds, &mut rec, Config::default());
        assert_eq!(e.tick(1.0).unwrap().judged, 0, "the first reading is only a baseline");
        for _ in 0..2 {
            let r = e.tick(1.0).unwrap();
            assert_eq!(r.judged, 1);
            assert!(r.changes.is_empty(), "two windows are not enough");
        }
        let r = e.tick(1.0).unwrap();
        assert_eq!(r.changes.len(), 1);
        let c = &r.changes[0];
        assert_eq!((c.pid, c.class, c.applied), (10, Class::Batch, true));
        assert_eq!(c.setting, Setting { nice: 10, sched: Sched::Batch });
        assert!(e.tick(1.0).unwrap().changes.is_empty(), "it is not changed again");
        assert_eq!(e.counts(), (1, 1));
        drop(e);
        assert_eq!(rec.applied, vec![(10, Setting { nice: 10, sched: Sched::Batch })]);
    }

    #[test]
    fn an_interactive_program_is_favoured() {
        let mut rec = Recorder::default();
        let rounds = (0..4).map(|n| vec![interactive(20, n)]).collect();
        let mut e = engine(rounds, &mut rec, Config::default());
        let mut classes = Vec::new();
        for _ in 0..4 {
            classes.extend(e.tick(1.0).unwrap().changes.iter().map(|c| c.class));
        }
        assert_eq!(classes, vec![Class::Interactive]);
        drop(e);
        assert!(rec.applied[0].1.nice < 0);
    }

    #[test]
    fn a_process_that_keeps_changing_its_mind_is_left_alone() {
        let mut rec = Recorder::default();
        // one busy window, one quiet one, and so on: never three alike in a row
        let mut readings = Vec::new();
        let mut counters = (0u64, 0u64);
        for round in 0..9 {
            if round % 2 == 0 {
                counters.0 += 1;
            } else {
                counters.1 += 1;
            }
            let mut s = proc(30, 0, 0, 0, 0, 0);
            s.counters = Counters { run_ns: counters.0 * 1_000_000_000, slices: counters.0 * 10 + counters.1, voluntary: 1, involuntary: counters.0 * 50, wait_ns: 0 };
            readings.push(vec![s]);
        }
        let mut e = engine(readings, &mut rec, Config::default());
        for _ in 0..9 {
            assert!(e.tick(1.0).unwrap().changes.is_empty());
        }
    }

    #[test]
    fn what_must_not_be_touched_is_not_touched() {
        let mut rec = Recorder::default();
        let mut root = batch(40, 0);
        root.uid = 0;
        let mut kernel = batch(41, 0);
        kernel.kernel_thread = true;
        let mut ignored = batch(42, 0);
        ignored.comm = "osaima-schedd".into();
        let init = batch(1, 0);
        let own = batch(std::process::id() as i32, 0);
        let mut system_user = batch(43, 0);
        system_user.uid = 999;
        let ordinary = batch(44, 0);
        let all = |n: u64| {
            [&root, &kernel, &ignored, &init, &own, &system_user, &ordinary]
                .into_iter()
                .map(|s| Sample { counters: batch(s.pid, n).counters, ..s.clone() })
                .collect::<Vec<_>>()
        };
        let rounds = (0..5).map(all).collect();
        let mut e = engine(rounds, &mut rec, Config::default());
        let mut pids = Vec::new();
        for _ in 0..5 {
            pids.extend(e.tick(1.0).unwrap().changes.iter().map(|c| c.pid));
        }
        assert_eq!(pids, vec![44], "only the ordinary user's process");
    }

    #[test]
    fn a_reused_pid_starts_again_from_a_baseline() {
        let mut rec = Recorder::default();
        let mut reborn = batch(50, 4);
        reborn.start_time = 2000;
        let rounds = vec![vec![batch(50, 0)], vec![batch(50, 1)], vec![batch(50, 2)], vec![reborn]];
        let mut e = engine(rounds, &mut rec, Config::default());
        for _ in 0..3 {
            let _ = e.tick(1.0).unwrap();
        }
        let r = e.tick(1.0).unwrap();
        assert_eq!(r.judged, 0, "a different process now has this pid");
    }

    #[test]
    fn a_dry_run_decides_but_changes_nothing() {
        let mut rec = Recorder::default();
        let rounds = (0..5).map(|n| vec![batch(60, n)]).collect();
        let mut e = engine(rounds, &mut rec, Config { dry_run: true, ..Config::default() });
        let mut changes = Vec::new();
        for _ in 0..5 {
            changes.extend(e.tick(1.0).unwrap().changes);
        }
        assert_eq!(changes.len(), 1);
        assert!(!changes[0].applied);
        drop(e);
        assert!(rec.applied.is_empty());
    }

    #[test]
    fn a_refusal_is_reported_once_and_not_retried() {
        let mut rec = Recorder { refuse: true, ..Recorder::default() };
        let rounds = (0..8).map(|n| vec![batch(70, n)]).collect();
        let mut e = engine(rounds, &mut rec, Config::default());
        let mut changes = Vec::new();
        for _ in 0..8 {
            changes.extend(e.tick(1.0).unwrap().changes);
        }
        assert_eq!(changes.len(), 1);
        assert!(!changes[0].applied);
        assert!(changes[0].error.is_some());
        assert_eq!(e.counts().1, 0);
    }

    #[test]
    fn shutdown_puts_back_what_was_changed() {
        let mut rec = Recorder::default();
        let rounds = (0..5).map(|n| vec![batch(80, n), interactive(81, n)]).collect();
        let mut e = engine(rounds, &mut rec, Config::default());
        for _ in 0..5 {
            let _ = e.tick(1.0).unwrap();
        }
        e.shutdown();
        drop(e);
        let mut restored: Vec<i32> = rec.restored.iter().flatten().map(|o| o.tid).collect();
        restored.sort();
        assert_eq!(restored, vec![80, 81]);
    }

    #[test]
    fn an_unsure_model_changes_nothing() {
        let mut rec = Recorder::default();
        let unsure = Model { version: 1, classes: vec![Class::Batch, Class::Background], mean: vec![0.0; 5], scale: vec![1.0; 5], weights: vec![vec![0.0; 5]; 2], bias: vec![0.0, 0.0] };
        let rounds = (0..6).map(|n| vec![batch(90, n)]).collect();
        let mut e = Engine::new(Script(rounds), &mut rec, Classifier::Trained(unsure), Policy::default(), Config::default());
        for _ in 0..6 {
            assert!(e.tick(1.0).unwrap().changes.is_empty());
        }
    }

    #[test]
    fn processes_that_exit_are_forgotten() {
        let mut rec = Recorder::default();
        let rounds = vec![vec![batch(100, 0), batch(101, 0)], vec![batch(101, 1)]];
        let mut e = engine(rounds, &mut rec, Config::default());
        let _ = e.tick(1.0).unwrap();
        assert_eq!(e.counts().0, 2);
        let _ = e.tick(1.0).unwrap();
        assert_eq!(e.counts().0, 1);
    }
}
