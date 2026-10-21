//! The numbers that describe a process's behaviour over one sampling window.

use serde::{Deserialize, Serialize};

/// Cumulative scheduler counters of one process (all its threads added up).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Nanoseconds spent running on a CPU.
    pub run_ns: u64,
    /// Nanoseconds spent waiting on a run queue for a CPU.
    pub wait_ns: u64,
    /// How many times it was given the CPU.
    pub slices: u64,
    /// Times it gave up the CPU by itself (blocked on I/O, slept, waited).
    pub voluntary: u64,
    /// Times the kernel took the CPU away (its time ran out, or something more urgent came).
    pub involuntary: u64,
}

pub const FEATURE_NAMES: [&str; 5] = ["cpu_util", "vol_rate", "invol_rate", "slice_ms", "wait_ratio"];

/// What one window looked like. Rates are per second.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Features {
    /// CPU used, in cores (1.0 is one core fully busy).
    pub cpu_util: f64,
    /// Voluntary context switches per second: high for programs that wait for input.
    pub vol_rate: f64,
    /// Involuntary context switches per second: high for programs that always want more CPU.
    pub invol_rate: f64,
    /// Mean length of one run on a CPU, in milliseconds.
    pub slice_ms: f64,
    /// Time spent waiting for a CPU relative to time spent running.
    pub wait_ratio: f64,
}

/// A ratio above this means "starved"; capping it keeps one odd window from dominating.
const MAX_WAIT_RATIO: f64 = 100.0;

impl Features {
    /// The features of the window between two readings `seconds` apart. Counters that went
    /// backwards (a thread exited and took its totals with it) count as no change.
    pub fn between(prev: &Counters, now: &Counters, seconds: f64) -> Option<Features> {
        if seconds.is_nan() || seconds <= 0.0 {
            return None;
        }
        let run = now.run_ns.saturating_sub(prev.run_ns) as f64;
        let wait = now.wait_ns.saturating_sub(prev.wait_ns) as f64;
        let slices = now.slices.saturating_sub(prev.slices) as f64;
        let vol = now.voluntary.saturating_sub(prev.voluntary) as f64;
        let invol = now.involuntary.saturating_sub(prev.involuntary) as f64;
        Some(Features {
            cpu_util: run / 1e9 / seconds,
            vol_rate: vol / seconds,
            invol_rate: invol / seconds,
            slice_ms: if slices > 0.0 { run / slices / 1e6 } else { 0.0 },
            wait_ratio: if run > 0.0 { (wait / run).min(MAX_WAIT_RATIO) } else { 0.0 },
        })
    }

    /// The values the classifier reads: the rates and the ratio are squeezed with `ln(1 + x)`
    /// because they span several orders of magnitude. The trainer does the same.
    pub fn model_inputs(&self) -> [f64; 5] {
        [
            self.cpu_util,
            self.vol_rate.max(0.0).ln_1p(),
            self.invol_rate.max(0.0).ln_1p(),
            self.slice_ms.max(0.0).ln_1p(),
            self.wait_ratio.max(0.0).ln_1p(),
        ]
    }

    pub fn as_array(&self) -> [f64; 5] {
        [self.cpu_util, self.vol_rate, self.invol_rate, self.slice_ms, self.wait_ratio]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counters(run_ms: u64, wait_ms: u64, slices: u64, vol: u64, invol: u64) -> Counters {
        Counters {
            run_ns: run_ms * 1_000_000,
            wait_ns: wait_ms * 1_000_000,
            slices,
            voluntary: vol,
            involuntary: invol,
        }
    }

    #[test]
    fn a_window_is_turned_into_rates() {
        let before = counters(1000, 100, 50, 40, 10);
        let after = counters(1600, 160, 150, 100, 30);
        let f = Features::between(&before, &after, 2.0).unwrap();
        assert!((f.cpu_util - 0.3).abs() < 1e-9); // 600 ms of CPU in 2 s
        assert!((f.vol_rate - 30.0).abs() < 1e-9);
        assert!((f.invol_rate - 10.0).abs() < 1e-9);
        assert!((f.slice_ms - 6.0).abs() < 1e-9); // 600 ms over 100 runs
        assert!((f.wait_ratio - 0.1).abs() < 1e-9);
    }

    #[test]
    fn an_idle_window_is_all_zeros() {
        let c = counters(5, 1, 3, 2, 0);
        let f = Features::between(&c, &c, 1.0).unwrap();
        assert_eq!(f, Features::default());
    }

    #[test]
    fn no_time_between_readings_gives_nothing() {
        let c = Counters::default();
        assert!(Features::between(&c, &c, 0.0).is_none());
        assert!(Features::between(&c, &c, -1.0).is_none());
        assert!(Features::between(&c, &c, f64::NAN).is_none());
    }

    #[test]
    fn counters_that_went_backwards_count_as_no_change() {
        let before = counters(1000, 100, 50, 40, 10);
        let after = counters(900, 90, 40, 30, 5); // a busy thread exited
        let f = Features::between(&before, &after, 1.0).unwrap();
        assert_eq!(f, Features::default());
    }

    #[test]
    fn starvation_is_capped() {
        let before = Counters::default();
        let after = counters(1, 10_000, 1, 0, 0);
        let f = Features::between(&before, &after, 1.0).unwrap();
        assert_eq!(f.wait_ratio, MAX_WAIT_RATIO);
    }

    #[test]
    fn model_inputs_squeeze_the_wide_ranges() {
        let f = Features { cpu_util: 0.5, vol_rate: 0.0, invol_rate: 0.0, slice_ms: 0.0, wait_ratio: 0.0 };
        assert_eq!(f.model_inputs(), [0.5, 0.0, 0.0, 0.0, 0.0]);
        let busy = Features { cpu_util: 1.0, vol_rate: 99.0, invol_rate: 9.0, slice_ms: 3.0, wait_ratio: 1.0 };
        let inputs = busy.model_inputs();
        assert!((inputs[1] - 100f64.ln()).abs() < 1e-9);
        assert!((inputs[2] - 10f64.ln()).abs() < 1e-9);
        assert_eq!(FEATURE_NAMES.len(), inputs.len());
    }
}
