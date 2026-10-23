//! Health forecasting and anomaly detection, learned on the device from the
//! machine's own readings (no network, no extra crates).
//!
//! History: every 20 s sample goes into a short in-memory ring (3 h); one
//! sample every 5 min goes into a long ring (30 days) that is persisted as
//! JSON lines next to the behaviour database and reloaded at start-up.
//!
//! Forecasts use Theil-Sen regression (median of all pairwise slopes), which
//! shrugs off the cleanups, spikes and plateaus that wreck least squares:
//!   * disk:    used bytes over the last 14 days -> days until full
//!   * battery: percent over the current discharge -> time left
//!   * thermal: CPU temperature over the last 15 min -> minutes until the
//!              throttling zone if the trend continues
//! Anomalies: per metric and local hour of day, an exponentially weighted
//! mean and variance learned from the long ring; a reading is unusual when
//! it is >= 3 deviations above what is normal for this machine at this hour
//! (with a floor on the deviation so a very steady metric does not alarm on
//! noise) and above an absolute minimum.
//!
//! Everything here is a pure function over slices, so it is unit-tested on
//! synthetic series with known answers.

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const FINE_CAP: usize = 540; // 3 h at 20 s
const COARSE_CAP: usize = 8640; // 30 days at 5 min
const COARSE_EVERY: i64 = 300;
const MAX_GAP: i64 = 180; // a longer silence (suspend, daemon down) breaks a trend
const THROTTLE_C: f64 = 90.0;
const MIN_BUCKET_N: f64 = 36.0; // about three days of one hour's samples
const Z_FLAG: f64 = 3.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub t: i64,
    pub temp: Option<f64>,
    pub cpu: Option<f64>,       // CPU pressure, % (PSI some avg10)
    pub mem: Option<f64>,       // used fraction 0..1
    pub disk_used: Option<f64>, // bytes
    pub disk_total: Option<f64>,
    pub bat: Option<f64>,
    pub dis: bool,
}

impl Sample {
    pub fn from_readings(t: i64, r: &Value) -> Sample {
        let mem = match (r["memory"]["total_bytes"].as_f64(), r["memory"]["available_bytes"].as_f64()) {
            (Some(t), Some(a)) if t > 0.0 => Some(1.0 - a / t),
            _ => None,
        };
        let (dt, df) = (r["disk"]["total_bytes"].as_f64(), r["disk"]["free_bytes"].as_f64());
        Sample {
            t, temp: r["cpu_temp_c"].as_f64(), cpu: r["pressure"]["cpu"].as_f64(), mem,
            disk_used: dt.zip(df).map(|(t, f)| t - f), disk_total: dt,
            bat: r["battery"]["percent"].as_f64(), dis: r["battery"]["discharging"].as_bool().unwrap_or(false),
        }
    }
    fn to_line(&self) -> String {
        json!([self.t, self.temp, self.cpu, self.mem, self.disk_used, self.disk_total, self.bat, self.dis]).to_string()
    }
    fn from_line(l: &str) -> Option<Sample> {
        let v: Value = serde_json::from_str(l).ok()?;
        let a = v.as_array()?;
        Some(Sample { t: a.first()?.as_i64()?, temp: a.get(1)?.as_f64(), cpu: a.get(2)?.as_f64(), mem: a.get(3)?.as_f64(),
                      disk_used: a.get(4)?.as_f64(), disk_total: a.get(5)?.as_f64(), bat: a.get(6)?.as_f64(), dis: a.get(7)?.as_bool()? })
    }
}

// ---------------------------------------------------------------- Theil-Sen

pub struct Fit {
    pub slope: f64,
    pub intercept: f64,
    pub frac_up: f64, // share of pairwise slopes that are positive: trend agreement
    pub n: usize,
}

fn median(v: &mut Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

/// Theil-Sen line through (x, y) points; subsampled to at most 120 points.
pub fn theil_sen(pts: &[(f64, f64)]) -> Option<Fit> {
    if pts.len() < 3 { return None; }
    let step = (pts.len() + 119) / 120;
    let p: Vec<(f64, f64)> = pts.iter().step_by(step).copied().collect();
    let mut slopes = Vec::new();
    for i in 0..p.len() {
        for j in i + 1..p.len() {
            let dx = p[j].0 - p[i].0;
            if dx.abs() > 1e-12 { slopes.push((p[j].1 - p[i].1) / dx); }
        }
    }
    if slopes.is_empty() { return None; }
    let frac_up = slopes.iter().filter(|s| **s > 0.0).count() as f64 / slopes.len() as f64;
    let slope = median(&mut slopes);
    let mut res: Vec<f64> = p.iter().map(|(x, y)| y - slope * x).collect();
    Some(Fit { slope, intercept: median(&mut res), frac_up, n: p.len() })
}

fn mk(id: &str, kind: &str, level: &str, title: String, detail: String, advice: String, evidence: Value) -> Value {
    json!({ "id": id, "kind": kind, "level": level, "title": title, "detail": detail, "advice": advice, "evidence": evidence, "source": "model" })
}

fn r1(x: f64) -> f64 { (x * 10.0).round() / 10.0 }

// ---------------------------------------------------------------- forecasts

/// Disk: days until full from the last 14 days of used bytes.
pub fn forecast_disk(coarse: &[Sample], now: i64) -> Option<Value> {
    let mut pts: Vec<&Sample> = coarse.iter().filter(|s| now - s.t <= 14 * 86400 && s.disk_used.is_some()).collect();
    // a cleanup (a sudden drop of >= 1 GB or 1 % of the disk) starts a new regime: trend only what came after it
    if let Some(i) = (1..pts.len()).rev().find(|&i| {
        let (a, b) = (pts[i - 1], pts[i]);
        a.disk_used.unwrap() - b.disk_used.unwrap() >= (1073741824.0f64).max(0.01 * b.disk_total.unwrap_or(0.0))
    }) {
        pts.drain(..i);
    }
    if pts.len() < 24 { return None; }
    let span = (pts.last()?.t - pts[0].t) as f64 / 86400.0;
    if span < 2.0 { return None; }
    let t0 = pts[0].t;
    let xy: Vec<(f64, f64)> = pts.iter().map(|s| ((s.t - t0) as f64 / 86400.0, s.disk_used.unwrap())).collect();
    let fit = theil_sen(&xy)?;
    let last = pts.last()?;
    let total = last.disk_total?;
    let free = total - last.disk_used?;
    const GB: f64 = 1073741824.0;
    if fit.slope < 0.05 * GB || fit.frac_up < 0.6 { return None; }
    let days = free / fit.slope;
    if days > 90.0 { return None; }
    let (level, title) = if days < 1.0 {
        ("bad", "The disk will be full within a day".to_string())
    } else {
        (if days <= 3.0 { "bad" } else if days <= 14.0 { "warn" } else { "info" }, format!("Disk full in about {} days", days.round() as i64))
    };
    Some(mk("forecast:disk", "disk", level, title,
        format!("The disk has been filling by about {:.1} GB a day over the last {:.0} days and has {:.1} GB left.", fit.slope / GB, span.ceil(), free / GB),
        "Ask me to \"find large files\" or empty the Trash before it runs out.".into(),
        json!({ "slope_gb_per_day": (fit.slope / GB * 100.0).round() / 100.0, "free_gb": r1(free / GB), "days_left": r1(days),
                "window_days": r1(span), "points": fit.n, "method": "Theil-Sen" })))
}

/// Battery: time left from the current unbroken discharge.
pub fn forecast_battery(fine: &[Sample]) -> Option<Value> {
    let mut run: Vec<&Sample> = Vec::new();
    for s in fine.iter().rev() {
        if !s.dis || s.bat.is_none() { break; }
        if let Some(prev) = run.last() { if prev.t - s.t > MAX_GAP { break; } }
        run.push(s);
    }
    run.reverse();
    if run.len() < 20 { return None; }
    let span = (run.last()?.t - run[0].t) as f64;
    if span < 600.0 { return None; }
    let t0 = run[0].t;
    let xy: Vec<(f64, f64)> = run.iter().map(|s| ((s.t - t0) as f64 / 3600.0, s.bat.unwrap())).collect();
    let fit = theil_sen(&xy)?;
    let rate = -fit.slope; // % per hour
    let pct = run.last()?.bat?;
    if rate < 1.0 { return None; }
    let hours = pct / rate;
    if hours > 8.0 { return None; }
    let mins = (hours * 60.0).round() as i64;
    let left = if mins >= 90 { format!("about {:.1} hours", hours) } else { format!("about {} minutes", mins) };
    Some(mk("forecast:battery", "battery", if hours <= 0.5 { "bad" } else if hours <= 1.5 { "warn" } else { "info" },
        format!("Battery lasts {} at this rate", left),
        format!("It has been draining {:.0}% an hour for the last {:.0} minutes and is at {:.0}%.", rate, span / 60.0, pct),
        "Plug in the charger, or lower the screen brightness and close heavy programs to stretch it.".into(),
        json!({ "drain_percent_per_hour": r1(rate), "percent": pct, "hours_left": (hours * 100.0).round() / 100.0, "window_minutes": r1(span / 60.0), "method": "Theil-Sen" })))
}

/// Thermal: will it reach the throttling zone soon if the trend continues?
pub fn forecast_thermal(fine: &[Sample]) -> Option<Value> {
    let now = fine.last()?.t;
    let mut run: Vec<&Sample> = Vec::new();
    for s in fine.iter().rev() {
        if now - s.t > 900 || s.temp.is_none() { break; }
        if let Some(prev) = run.last() { if prev.t - s.t > MAX_GAP { break; } }
        run.push(s);
    }
    run.reverse();
    if run.len() < 30 { return None; }
    let t0 = run[0].t;
    let xy: Vec<(f64, f64)> = run.iter().map(|s| ((s.t - t0) as f64 / 60.0, s.temp.unwrap())).collect();
    let fit = theil_sen(&xy)?;
    let mut last3: Vec<f64> = run.iter().rev().take(3).map(|s| s.temp.unwrap()).collect();
    let temp = median(&mut last3);
    if fit.slope < 0.15 || fit.frac_up < 0.6 || temp < 65.0 || temp >= THROTTLE_C { return None; }
    let mins = (THROTTLE_C - temp) / fit.slope;
    if mins > 30.0 { return None; }
    Some(mk("forecast:thermal", "thermal", if mins <= 10.0 { "warn" } else { "info" },
        format!("Likely to throttle in about {} minutes if this continues", mins.round().max(1.0) as i64),
        format!("The processor is at {:.0}\u{b0}C and has been warming by {:.1}\u{b0}C a minute; above about {:.0}\u{b0}C it slows itself down.", temp, fit.slope, THROTTLE_C),
        "Close what is using the processor, or let it rest, before it starts slowing down.".into(),
        json!({ "temp_c": r1(temp), "slope_c_per_min": (fit.slope * 100.0).round() / 100.0, "minutes_to_throttle": r1(mins), "window_minutes": r1((now - t0) as f64 / 60.0), "method": "Theil-Sen" })))
}

// ---------------------------------------------------------------- anomalies

#[derive(Clone, Copy, Default, Debug)]
pub struct Stat { pub n: f64, pub mean: f64, pub var: f64 }

impl Stat {
    pub fn update(&mut self, x: f64) {
        self.n += 1.0;
        let a = (1.0 / self.n).max(0.02);
        let d = x - self.mean;
        self.mean += a * d;
        self.var = (1.0 - a) * (self.var + a * d * d);
    }
}

pub const METRICS: [&str; 3] = ["cpu", "memory", "temperature"];
const FLOOR_SD: [f64; 3] = [8.0, 0.04, 3.0];
const MIN_ABS: [f64; 3] = [25.0, 0.5, 60.0];

#[derive(Clone)]
pub struct Anomaly {
    pub stats: [[Stat; 24]; 3],
}

impl Anomaly {
    pub fn new() -> Self { Self { stats: [[Stat::default(); 24]; 3] } }

    fn values(s: &Sample) -> [Option<f64>; 3] { [s.cpu, s.mem, s.temp] }

    pub fn learn(&mut self, s: &Sample, hour: usize) {
        for (m, v) in Self::values(s).iter().enumerate() {
            if let Some(x) = v { self.stats[m][hour % 24].update(*x); }
        }
    }

    pub fn ready_buckets(&self) -> usize {
        (0..24).filter(|h| self.stats.iter().all(|m| m[*h].n >= MIN_BUCKET_N)).count()
    }

    /// z-score (against this hour's learned normal) per metric for a current reading.
    pub fn score(&self, metric: usize, hour: usize, x: f64) -> Option<(f64, Stat)> {
        let st = self.stats[metric][hour % 24];
        if st.n < MIN_BUCKET_N { return None; }
        Some(((x - st.mean) / st.var.sqrt().max(FLOOR_SD[metric]), st))
    }

    /// Unusual readings now, from the last few fine samples (their median).
    pub fn detect(&self, recent: &[Sample], hour: usize) -> Vec<Value> {
        let mut out = Vec::new();
        let tail: Vec<&Sample> = recent.iter().rev().take(3).collect();
        if tail.len() < 3 { return out; }
        for m in 0..3 {
            let mut xs: Vec<f64> = tail.iter().filter_map(|s| Self::values(s)[m]).collect();
            if xs.len() < 3 { continue; }
            let x = median(&mut xs);
            let Some((z, st)) = self.score(m, hour, x) else { continue };
            if z < Z_FLAG || x < MIN_ABS[m] { continue; }
            let pretty = |v: f64| match m { 0 => format!("{:.0}%", v), 1 => format!("{:.0}%", v * 100.0), _ => format!("{:.0}\u{b0}C", v) };
            let what = match m { 0 => "Processor load", 1 => "Memory use", _ => "Processor temperature" };
            let advice = match m {
                0 => "Check what is running. Ask me \"what's using the processor?\" for the list.",
                1 => "Close programs or browser tabs you aren't using; ask me what is using the memory.",
                _ => "Close heavy programs and check that the vents are clear.",
            };
            out.push(mk(&format!("anomaly:{}", METRICS[m]), METRICS[m], if z >= 5.0 { "warn" } else { "info" },
                format!("{} is unusually high ({})", what, pretty(x)),
                format!("At this time of day it is normally about {} on this computer; now it is {}.", pretty(st.mean), pretty(x)),
                advice.into(),
                json!({ "value": (x * 1000.0).round() / 1000.0, "normal": (st.mean * 1000.0).round() / 1000.0, "z": r1(z),
                        "normal_sd": (st.var.sqrt() * 1000.0).round() / 1000.0, "hour": hour, "samples": st.n, "method": "hourly EWMA z-score" })));
        }
        out
    }
}

// ---------------------------------------------------------------- state

pub struct History {
    fine: VecDeque<Sample>,
    coarse: VecDeque<Sample>,
    anomaly: Anomaly,
    path: Option<PathBuf>,
    lines: usize,
}

fn hour_of(t: i64) -> usize { (crate::predictor::local_parts(t).0 / 60) as usize }

impl History {
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut h = Self { fine: VecDeque::new(), coarse: VecDeque::new(), anomaly: Anomaly::new(), path: path.clone(), lines: 0 };
        if let Some(p) = path {
            if let Ok(text) = std::fs::read_to_string(&p) {
                for l in text.lines() {
                    h.lines += 1;
                    if let Some(s) = Sample::from_line(l) {
                        if h.coarse.back().map_or(true, |b| s.t > b.t) { h.push_coarse(s); }
                    }
                }
            }
        }
        h
    }

    fn push_coarse(&mut self, s: Sample) {
        self.anomaly.learn(&s, hour_of(s.t));
        self.coarse.push_back(s);
        if self.coarse.len() > COARSE_CAP { self.coarse.pop_front(); }
    }

    pub fn ingest(&mut self, s: Sample) {
        self.fine.push_back(s.clone());
        if self.fine.len() > FINE_CAP { self.fine.pop_front(); }
        if self.coarse.back().map_or(true, |b| s.t - b.t >= COARSE_EVERY) {
            self.push_coarse(s.clone());
            if let Some(p) = &self.path {
                if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
                if self.lines >= COARSE_CAP + 720 {
                    let body: String = self.coarse.iter().map(|c| c.to_line() + "\n").collect();
                    let tmp = p.with_extension("tmp");
                    if std::fs::write(&tmp, body).is_ok() && std::fs::rename(&tmp, p).is_ok() { self.lines = self.coarse.len(); }
                } else if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                    if writeln!(f, "{}", s.to_line()).is_ok() { self.lines += 1; }
                }
            }
        }
    }

    /// (forecasts, anomalies) as of the latest sample.
    pub fn insights(&self) -> (Vec<Value>, Vec<Value>) {
        let fine: Vec<Sample> = self.fine.iter().cloned().collect();
        let coarse: Vec<Sample> = self.coarse.iter().cloned().collect();
        let Some(now) = fine.last().map(|s| s.t) else { return (vec![], vec![]) };
        let mut f: Vec<Value> = [forecast_disk(&coarse, now), forecast_battery(&fine), forecast_thermal(&fine)].into_iter().flatten().collect();
        f.sort_by_key(|v| match v["level"].as_str() { Some("bad") => 0, Some("warn") => 1, _ => 2 });
        (f, self.anomaly.detect(&fine, hour_of(now)))
    }

    pub fn info(&self) -> Value {
        let span = match (self.coarse.front(), self.coarse.back()) { (Some(a), Some(b)) => (b.t - a.t) as f64 / 86400.0, _ => 0.0 };
        json!({ "kind": "Theil-Sen trend forecasts + hourly EWMA z-score anomalies (pure Rust, on-device)",
                "recent_samples": self.fine.len(), "history_samples": self.coarse.len(), "history_days": r1(span),
                "anomaly_hours_ready": self.anomaly.ready_buckets(), "persisted": self.path.is_some() })
    }
}

static HIST: Mutex<Option<History>> = Mutex::new(None);

fn history_path() -> PathBuf {
    if let Ok(p) = std::env::var("OSAIMA_HEALTH_HISTORY") { return PathBuf::from(p); }
    let data = std::env::var("XDG_DATA_HOME").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".local/share"));
    data.join("osaima/health_history.jsonl")
}

/// Called by the health loop with each new set of readings.
pub fn record(readings: &Value) {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut g = HIST.lock().unwrap();
    g.get_or_insert_with(|| History::new(Some(history_path()))).ingest(Sample::from_readings(t, readings));
}

pub fn insights() -> (Vec<Value>, Vec<Value>, Value) {
    let mut g = HIST.lock().unwrap();
    let h = g.get_or_insert_with(|| History::new(Some(history_path())));
    let (f, a) = h.insights();
    (f, a, h.info())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny deterministic noise in [-1, 1].
    fn noise(i: i64) -> f64 {
        let x = (i as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let y = (x ^ (x >> 29)).wrapping_mul(0xBF58476D1CE4E5B9);
        ((y >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0
    }
    fn sample(t: i64) -> Sample { Sample { t, temp: None, cpu: None, mem: None, disk_used: None, disk_total: None, bat: None, dis: false } }
    const GB: f64 = 1073741824.0;

    #[test]
    fn theil_sen_exact_and_robust() {
        let pts: Vec<(f64, f64)> = (0..50).map(|i| (i as f64, 3.0 * i as f64 + 7.0)).collect();
        let f = theil_sen(&pts).unwrap();
        assert!((f.slope - 3.0).abs() < 1e-9 && (f.intercept - 7.0).abs() < 1e-9);
        let mut bad = pts.clone();
        for i in (0..50).step_by(5) { bad[i].1 += 5000.0; }
        assert!((theil_sen(&bad).unwrap().slope - 3.0).abs() < 0.2);
        assert!(theil_sen(&pts[..2]).is_none());
    }

    #[test]
    fn disk_full_forecast_with_a_cleanup_in_the_middle() {
        // 12 days, +2 GB/day with noise, a 15 GB cleanup on day 6; 100 GB disk
        let n = 12 * 288;
        let coarse: Vec<Sample> = (0..n).map(|i| {
            let day = i as f64 / 288.0;
            let used = 40.0 + 2.0 * day - if day > 6.0 { 15.0 } else { 0.0 } + 0.3 * noise(i);
            let mut s = sample(i * 300);
            s.disk_total = Some(100.0 * GB);
            s.disk_used = Some(used * GB);
            s
        }).collect();
        let last = coarse.last().unwrap();
        let free = (last.disk_total.unwrap() - last.disk_used.unwrap()) / GB;
        let f = forecast_disk(&coarse, last.t).expect("forecast");
        let days = f["evidence"]["days_left"].as_f64().unwrap();
        let truth = free / 2.0;
        assert!((days - truth).abs() / truth < 0.15, "forecast {} vs truth {}", days, truth);
        assert!(f["title"].as_str().unwrap().starts_with("Disk full in about"));
        let flat: Vec<Sample> = coarse.iter().map(|s| { let mut s = s.clone(); s.disk_used = Some(50.0 * GB + 0.2 * GB * noise(s.t)); s }).collect();
        assert!(forecast_disk(&flat, last.t).is_none());
        assert!(forecast_disk(&coarse[..100], coarse[99].t).is_none());
    }

    #[test]
    fn battery_time_left() {
        // 60 % draining 20 %/h for 30 min, sampled every 20 s, integer percent
        let fine: Vec<Sample> = (0..90).map(|i| {
            let mut s = sample(i * 20);
            s.dis = true;
            s.bat = Some((60.0 - 20.0 * (i as f64 * 20.0) / 3600.0).floor());
            s
        }).collect();
        let f = forecast_battery(&fine).expect("forecast");
        let h = f["evidence"]["hours_left"].as_f64().unwrap();
        let truth = fine.last().unwrap().bat.unwrap() / 20.0;
        assert!((h - truth).abs() < 0.25, "{} vs {}", h, truth);
        let charging: Vec<Sample> = fine.iter().map(|s| { let mut s = s.clone(); s.dis = false; s }).collect();
        assert!(forecast_battery(&charging).is_none());
        let slow: Vec<Sample> = (0..90).map(|i| { let mut s = sample(i * 20); s.dis = true; s.bat = Some(80.0 - (i / 80) as f64); s }).collect();
        assert!(forecast_battery(&slow).is_none());
    }

    #[test]
    fn thermal_trend() {
        // 70 C rising 1 C/min with noise for 12 minutes
        let fine: Vec<Sample> = (0..36).map(|i| { let mut s = sample(i * 20); s.temp = Some(70.0 + i as f64 * 20.0 / 60.0 + 0.4 * noise(i)); s }).collect();
        let f = forecast_thermal(&fine).expect("forecast");
        let m = f["evidence"]["minutes_to_throttle"].as_f64().unwrap();
        let truth = 90.0 - fine.last().unwrap().temp.unwrap();
        assert!((m - truth).abs() < 2.5, "{} vs {}", m, truth);
        let steady: Vec<Sample> = (0..36).map(|i| { let mut s = sample(i * 20); s.temp = Some(80.0 + 0.5 * noise(i)); s }).collect();
        assert!(forecast_thermal(&steady).is_none());
        let cool: Vec<Sample> = (0..36).map(|i| { let mut s = sample(i * 20); s.temp = Some(40.0 + i as f64 / 6.0); s }).collect();
        assert!(forecast_thermal(&cool).is_none());
    }

    #[test]
    fn anomaly_per_hour_of_day() {
        // 10 days of 5-minute samples: busy (cpu ~60 %) during hour 10, quiet (~8 %) otherwise
        let mut a = Anomaly::new();
        for i in 0..(10 * 288) {
            let hour = ((i / 12) % 24) as usize;
            let mut s = sample(i * 300);
            s.cpu = Some(if hour == 10 { 60.0 } else { 8.0 } + 3.0 * noise(i));
            s.mem = Some(0.5 + 0.02 * noise(i + 7));
            s.temp = Some(55.0 + 2.0 * noise(i + 99));
            a.learn(&s, hour);
        }
        let reading = |cpu: f64, mem: f64, temp: f64| -> Vec<Sample> {
            (0..3).map(|i| { let mut s = sample(i); s.cpu = Some(cpu); s.mem = Some(mem); s.temp = Some(temp); s }).collect()
        };
        assert!(a.detect(&reading(60.0, 0.5, 55.0), 10).is_empty());
        let hits = a.detect(&reading(60.0, 0.5, 55.0), 3);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["kind"], "cpu");
        assert!(hits[0]["evidence"]["z"].as_f64().unwrap() >= 3.0);
        let kinds: Vec<String> = a.detect(&reading(8.0, 0.95, 88.0), 3).iter().map(|v| v["kind"].as_str().unwrap().to_string()).collect();
        assert_eq!(kinds, vec!["memory", "temperature"]);
        for h in 0..24 { assert!(a.detect(&reading(if h == 10 { 60.0 } else { 8.0 }, 0.5, 55.0), h).is_empty(), "false alarm at {}", h); }
        assert!(Anomaly::new().detect(&reading(99.0, 0.99, 99.0), 3).is_empty());
    }

    #[test]
    fn history_persists_bounded_and_reloads() {
        let path = std::env::temp_dir().join(format!("osaima-health-hist-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut h = History::new(Some(path.clone()));
        for i in 0..40 {
            let mut s = sample(1_000_000 + i * 20);
            s.cpu = Some(5.0);
            h.ingest(s);
        }
        // 40 samples over 13 minutes -> 3 coarse points on disk
        assert_eq!(h.coarse.len(), 3);
        let again = History::new(Some(path.clone()));
        assert_eq!(again.coarse.len(), 3);
        assert_eq!(again.coarse[1].cpu, Some(5.0));
        let _ = std::fs::remove_file(&path);
        let mut b = History::new(None);
        for i in 0..(COARSE_CAP as i64 + 50) { b.ingest(sample(i * 300)); }
        assert_eq!(b.coarse.len(), COARSE_CAP);
        assert!(b.fine.len() <= FINE_CAP);
    }

    #[test]
    fn sample_from_readings_shape() {
        let r = json!({ "cpu_temp_c": 61.5, "pressure": { "cpu": 12.0 }, "memory": { "total_bytes": 1000, "available_bytes": 250 },
                        "disk": { "total_bytes": 100, "free_bytes": 30 }, "battery": { "percent": 80, "discharging": true } });
        let s = Sample::from_readings(5, &r);
        assert_eq!((s.temp, s.cpu, s.mem, s.disk_used, s.bat, s.dis), (Some(61.5), Some(12.0), Some(0.75), Some(70.0), Some(80.0), true));
        assert_eq!(Sample::from_line(&s.to_line()), Some(s));
    }
}
