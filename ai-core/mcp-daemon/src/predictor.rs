//! Next-action predictor: a small model trained on the user's own history,
//! on the device, in milliseconds. No network, no external crates.
//!
//! What it predicts: the next *safe* request (a fast-path command that is not a
//! power action; the same filter `Behavior::predictions` applies) given
//!   * time of day and weekday,
//!   * the previous one and two requests (a session ends after 30 idle minutes),
//!   * the apps launched in the last hour (a proxy for "open apps").
//!
//! How: five component distributions over the candidate commands are blended
//! with learned mixture weights (a stacked n-gram / kernel-density model):
//!   1. prior          recency-free relative frequency
//!   2. time of day    circular Gaussian kernel density (sigma 30 min) over the
//!                     times the command was used, recency-weighted (half-life
//!                     21 days), same weekday-class counted fully, other 0.3
//!   3. previous       P(command | previous request), Dirichlet back-off to prior
//!   4. previous two   P(command | previous two requests), back-off to (3)
//!   5. open apps      P(command | app launched in the last hour), back-off to prior
//! The five mixture weights are fitted by exhaustive search on a 0.1 grid,
//! maximising held-out log-likelihood on a validation slice (60-80 % of the
//! history, in time order). The accuracy reported in `model_info` is measured
//! on the final 20 %, which neither the counts nor the weights ever saw before
//! being scored (prequential: predict, then learn).
//!
//! Incremental: `observe` updates the counts in O(1); the weights are refitted
//! every `REFIT_EVERY` observations.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub const NC: usize = 5;
pub const COMP: [&str; NC] = ["prior", "time of day", "previous request", "previous two requests", "open apps"];
const GAP: i64 = 1800; // idle seconds that end a session
const APP_WINDOW: i64 = 3600;
const MAX_EVENTS: usize = 6000;
const MAX_TIMES: usize = 800;
const HALF_LIFE: f64 = 21.0 * 86400.0;
const SIGMA_MIN: f64 = 30.0;
const DAY: i64 = 86400;
const MIN_FIT: usize = 40; // safe events needed before weights are tuned
const REFIT_EVERY: usize = 200;
pub const MIN_CONFIDENCE: f64 = 0.20;
const DEFAULT_W: [f64; NC] = [0.1, 0.3, 0.25, 0.25, 0.1];
const EXCLUDED: [&str; 9] = ["unknown", "greeting", "chat", "close_all_windows", "clean_storage", "delete_note", "shutdown", "orchestrate", "agent"];

#[derive(Clone, Debug)]
pub struct Event {
    pub t: i64,       // UTC seconds
    pub min: u16,     // local minute of day
    pub wd: u8,       // local weekday, Monday = 0
    pub ctx: String,  // what this event looks like as context for the next one
    pub class: Option<String>, // Some(..) when it is a safe candidate to suggest
    pub text: String, // the utterance, lower-case
    pub app: Option<String>,
}

/// Local minute-of-day and weekday (Monday = 0) for a UTC timestamp.
pub fn local_parts(t: i64) -> (u16, u8) {
    let tt = t as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&tt, &mut tm) };
    ((tm.tm_hour * 60 + tm.tm_min) as u16, ((tm.tm_wday + 6) % 7) as u8)
}

/// Build an event from a logged request. `source` is "fast" for the on-device
/// router; anything else (LLM plans) is context only and is never suggested.
pub fn build_event(t: i64, utterance: &str, intent: &str, source: &str, params: &Value, success: bool) -> Event {
    let (min, wd) = local_parts(t);
    build_event_at(t, min, wd, utterance, intent, source, params, success)
}

#[allow(clippy::too_many_arguments)]
pub fn build_event_at(t: i64, min: u16, wd: u8, utterance: &str, intent: &str, source: &str, params: &Value, success: bool) -> Event {
    let text = utterance.trim().to_lowercase();
    let app = if intent == "open_app" { params["app"].as_str().map(|s| s.to_lowercase()) } else { None };
    let safe = success && source == "fast" && !text.is_empty() && text.chars().count() <= 60
        && !EXCLUDED.contains(&intent) && params["action"].as_str().unwrap_or("") != "power";
    let class = safe.then(|| match &app { Some(a) => format!("open_app:{}", a), None => text.clone() });
    let ctx = class.clone().unwrap_or_else(|| intent.to_string());
    Event { t, min, wd, ctx, class, text, app }
}

struct Ctx {
    t: i64,
    min: u16,
    wd: u8,
    p1: String,
    p2: String,
    apps: Vec<String>,
}

fn ctx_at(events: &[Event], t: i64, min: u16, wd: u8, apps_override: Option<&[String]>) -> Ctx {
    let n = events.len();
    let (mut p1, mut p2) = ("<start>".to_string(), "<start>".to_string());
    if n > 0 && t - events[n - 1].t <= GAP {
        p1 = events[n - 1].ctx.clone();
        if n > 1 && events[n - 1].t - events[n - 2].t <= GAP {
            p2 = events[n - 2].ctx.clone();
        }
    }
    let mut apps: Vec<String> = Vec::new();
    if let Some(o) = apps_override {
        apps = o.iter().take(4).map(|s| s.to_lowercase()).collect();
    } else {
        for e in events.iter().rev() {
            if t - e.t > APP_WINDOW { break; }
            if let Some(a) = &e.app {
                if !apps.contains(a) && apps.len() < 4 { apps.push(a.clone()); }
            }
        }
    }
    Ctx { t, min, wd, p1, p2, apps }
}

fn circ(a: u16, b: u16) -> f64 {
    let d = (a as i32 - b as i32).abs();
    d.min(1440 - d) as f64
}

type Dist = HashMap<String, (f64, HashMap<String, f64>)>;

fn bump(d: &mut Dist, key: &str, class: &str) {
    let e = d.entry(key.to_string()).or_default();
    e.0 += 1.0;
    *e.1.entry(class.to_string()).or_default() += 1.0;
}

/// (count(c | key) + k * back_off(c)) / (count(key) + k)
fn backoff(d: Option<&(f64, HashMap<String, f64>)>, c: &str, lower: f64, k: f64) -> f64 {
    match d {
        Some((n, m)) => (m.get(c).copied().unwrap_or(0.0) + k * lower) / (n + k),
        None => lower,
    }
}

#[derive(Clone)]
pub struct Scored {
    pub class: String,
    pub p: f64,
    pub comps: [f64; NC],
}

#[derive(Clone)]
pub struct Model {
    pub events: Vec<Event>,
    uni: HashMap<String, f64>,
    uni_n: f64,
    bi: Dist,
    tri: Dist,
    app: Dist,
    times: HashMap<String, Vec<(i64, u16, u8)>>,
    texts: HashMap<String, String>,
    pub w: [f64; NC],
}

impl Model {
    pub fn new() -> Self {
        Self { events: vec![], uni: HashMap::new(), uni_n: 0.0, bi: HashMap::new(), tri: HashMap::new(), app: HashMap::new(),
               times: HashMap::new(), texts: HashMap::new(), w: DEFAULT_W }
    }

    pub fn classes(&self) -> usize { self.uni.len() }

    /// Learn from one more event (O(1) in the history size).
    pub fn observe(&mut self, e: Event) {
        let ctx = ctx_at(&self.events, e.t, e.min, e.wd, None);
        self.texts.insert(e.ctx.clone(), e.text.clone());
        if let Some(c) = &e.class {
            *self.uni.entry(c.clone()).or_default() += 1.0;
            self.uni_n += 1.0;
            bump(&mut self.bi, &ctx.p1, c);
            bump(&mut self.tri, &format!("{}\u{1}{}", ctx.p2, ctx.p1), c);
            for a in &ctx.apps { bump(&mut self.app, a, c); }
            let v = self.times.entry(c.clone()).or_default();
            v.push((e.t, e.min, e.wd));
            if v.len() > MAX_TIMES { v.remove(0); }
        }
        self.events.push(e);
        if self.events.len() > MAX_EVENTS + 500 {
            self.events.drain(0..500);
        }
    }

    /// Component probabilities and the blended probability of every known command.
    fn score_all(&self, ctx: &Ctx) -> Vec<Scored> {
        let k = self.uni.len() as f64;
        if k == 0.0 { return vec![]; }
        let mut s: HashMap<&str, f64> = HashMap::new();
        let mut total = 0.0;
        for (c, v) in &self.times {
            let mut sum = 0.0;
            for &(t, m, wd) in v {
                let d = circ(m, ctx.min);
                if d > 4.0 * SIGMA_MIN { continue; }
                let age = (ctx.t - t).max(0) as f64;
                let dayw = if (wd >= 5) == (ctx.wd >= 5) { 1.0 } else { 0.3 };
                sum += (-std::f64::consts::LN_2 * age / HALF_LIFE).exp() * (-0.5 * (d / SIGMA_MIN).powi(2)).exp() * dayw;
            }
            total += sum;
            s.insert(c.as_str(), sum);
        }
        let bi = self.bi.get(&ctx.p1);
        let tri = self.tri.get(&format!("{}\u{1}{}", ctx.p2, ctx.p1));
        let apps: Vec<&(f64, HashMap<String, f64>)> = ctx.apps.iter().filter_map(|a| self.app.get(a)).collect();
        let an: f64 = apps.iter().map(|d| d.0).sum();
        self.uni.iter().map(|(c, &cnt)| {
            let pu = (cnt + 0.5) / (self.uni_n + 0.5 * k);
            let pt = (s.get(c.as_str()).copied().unwrap_or(0.0) + 0.5 * pu) / (total + 0.5);
            let pb = backoff(bi, c, pu, 2.0);
            let ptr = backoff(tri, c, pb, 2.0);
            let ac: f64 = apps.iter().map(|d| d.1.get(c).copied().unwrap_or(0.0)).sum();
            let pa = (ac + 2.0 * pu) / (an + 2.0);
            let comps = [pu, pt, pb, ptr, pa];
            let p = comps.iter().zip(self.w.iter()).map(|(a, b)| a * b).sum();
            Scored { class: c.clone(), p, comps }
        }).collect()
    }

    fn rank(&self, ctx: &Ctx) -> Vec<Scored> {
        let mut v = self.score_all(ctx);
        v.sort_by(|a, b| b.p.partial_cmp(&a.p).unwrap().then_with(|| a.class.cmp(&b.class)));
        v
    }

    /// Ranked commands for the moment right after `history`, at time `t`.
    pub fn rank_at(&self, history: &[Event], t: i64, min: u16, wd: u8) -> Vec<Scored> {
        self.rank(&ctx_at(history, t, min, wd, None))
    }

    fn text_of(&self, ctx_key: &str) -> String {
        self.texts.get(ctx_key).cloned().unwrap_or_else(|| ctx_key.to_string())
    }

    /// One plain sentence on why this command ranks first, from the component
    /// that lifts it most above its base rate. Returns (reason, usual time if time-driven).
    fn reason(&self, ctx: &Ctx, sc: &Scored) -> (String, Option<String>, usize) {
        let text = self.text_of(&sc.class);
        let cnt = |d: Option<&(f64, HashMap<String, f64>)>| d.map(|(n, m)| (m.get(&sc.class).copied().unwrap_or(0.0), *n)).unwrap_or((0.0, 0.0));
        // evidence actually seen in each context (a back-off value is not evidence)
        let seen = |i: usize| -> f64 {
            match i {
                2 => cnt(self.bi.get(&ctx.p1)).0,
                3 => cnt(self.tri.get(&format!("{}\u{1}{}", ctx.p2, ctx.p1))).0,
                4 => ctx.apps.iter().map(|a| cnt(self.app.get(a)).0).fold(0.0, f64::max),
                1 => self.times.get(&sc.class).map_or(0.0, |v| v.iter()
                        .filter(|(t, m, _)| (ctx.t - t).abs() <= 28 * DAY && circ(*m, ctx.min) <= 45.0)
                        .map(|(t, _, _)| t.div_euclid(DAY)).collect::<HashSet<_>>().len() as f64),
                _ => 99.0,
            }
        };
        let (mut k, mut best) = (0usize, 0.0f64);
        for i in 1..NC {
            let lift = self.w[i] * (sc.comps[i] - sc.comps[0]);
            if lift > best && seen(i) >= 2.0 { best = lift; k = i; }
        }
        match k {
            2 => {
                let (a, b) = cnt(self.bi.get(&ctx.p1));
                (format!("after you \u{201c}{}\u{201d} you usually \u{201c}{}\u{201d} ({:.0} of {:.0} times)", self.text_of(&ctx.p1), text, a, b), None, k)
            }
            3 => {
                let (a, b) = cnt(self.tri.get(&format!("{}\u{1}{}", ctx.p2, ctx.p1)));
                (format!("after \u{201c}{}\u{201d} then \u{201c}{}\u{201d} you usually \u{201c}{}\u{201d} ({:.0} of {:.0} times)", self.text_of(&ctx.p2), self.text_of(&ctx.p1), text, a, b), None, k)
            }
            4 => {
                let mut pick: Option<(&String, f64, f64)> = None;
                for a in &ctx.apps {
                    if let Some((n, m)) = self.app.get(a) {
                        let c = m.get(&sc.class).copied().unwrap_or(0.0);
                        if pick.map_or(true, |p| c / n > p.1 / p.2) { pick = Some((a, c, *n)); }
                    }
                }
                match pick {
                    Some((a, c, n)) => (format!("while {} is open you usually \u{201c}{}\u{201d} ({:.0} of {:.0} times)", a, text, c, n), None, k),
                    None => (format!("\u{201c}{}\u{201d} is one of your common requests", text), None, 0),
                }
            }
            1 => {
                let empty = vec![];
                let times = self.times.get(&sc.class).unwrap_or(&empty);
                let near: Vec<&(i64, u16, u8)> = times.iter().filter(|(t, m, _)| (ctx.t - t).abs() <= 28 * DAY && circ(*m, ctx.min) <= 45.0).collect();
                let days: HashSet<i64> = near.iter().map(|(t, _, _)| t.div_euclid(DAY)).collect();
                let all_days: HashSet<i64> = self.events.iter().filter(|e| ctx.t - e.t <= 28 * DAY).map(|e| e.t.div_euclid(DAY)).collect();
                let mut mins: Vec<u16> = near.iter().map(|(_, m, _)| *m).collect();
                mins.sort_unstable();
                let usual = mins.get(mins.len() / 2).map(|m| format!("{:02}:{:02}", m / 60, m % 60));
                (format!("around {} you usually \u{201c}{}\u{201d} (on {} of the last {} days you used the assistant)",
                         usual.clone().unwrap_or_else(|| "this time".into()), text, days.len(), all_days.len().max(days.len())), usual, k)
            }
            _ => (format!("\u{201c}{}\u{201d} is your most frequent request", text), None, 0),
        }
    }
}

/// The deployed rule the model is compared with (`Behavior::predictions`):
/// commands used on at least 3 distinct days of the last 28 within 45 minutes
/// of this time of day and not in the last two hours, most days first.
/// Returns (class, days) pairs.
pub fn heuristic_rank(events: &[Event], t: i64, min: u16) -> Vec<(String, usize)> {
    let mut recent: HashSet<&str> = HashSet::new();
    let mut by_text: HashMap<&str, (String, HashSet<i64>)> = HashMap::new();
    for e in events.iter().rev() {
        let age = t - e.t;
        if age > 28 * DAY { break; }
        let Some(class) = &e.class else { continue };
        if age < 7200 { recent.insert(e.text.as_str()); }
        if circ(e.min, min) <= 45.0 {
            by_text.entry(e.text.as_str()).or_insert_with(|| (class.clone(), HashSet::new())).1.insert(e.t.div_euclid(DAY));
        }
    }
    let mut v: Vec<(String, usize, &str)> = by_text.into_iter()
        .filter(|(text, (_, d))| d.len() >= 3 && !recent.contains(text))
        .map(|(text, (c, d))| (c, d.len(), text)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(b.2)));
    v.into_iter().map(|(c, d, _)| (c, d)).collect()
}

#[derive(Clone, Debug, Default)]
pub struct FitInfo {
    pub trained_on: usize,
    pub classes: usize,
    pub weights: [f64; NC],
    pub tuned: bool,
    pub holdout_n: usize,
    pub top1: f64,
    pub top3: f64,
    pub base_top1: f64,
    pub base_top3: f64,
    pub conf_cover: f64, // share of holdout requests where the model was confident enough to speak
    pub conf_prec: f64,  // how often it was right when it did
}

pub fn fit(events: Vec<Event>) -> (Model, FitInfo) {
    fit_with(events, [true; NC])
}

fn masked_default(mask: &[bool; NC]) -> [f64; NC] {
    let mut w = DEFAULT_W;
    for i in 0..NC { if !mask[i] { w[i] = 0.0; } }
    let s: f64 = w.iter().sum();
    if s <= 0.0 { let mut o = [0.0; NC]; if let Some(i) = mask.iter().position(|&m| m) { o[i] = 1.0; } return o; }
    w.map(|x| x / s)
}

/// Train on `events` (time-ordered). `mask` disables components (ablations).
pub fn fit_with(events: Vec<Event>, mask: [bool; NC]) -> (Model, FitInfo) {
    let n = events.len();
    let mut m = Model::new();
    m.w = masked_default(&mask);
    let safe_total = events.iter().filter(|e| e.class.is_some()).count();
    let mut info = FitInfo { trained_on: n, weights: m.w, ..Default::default() };
    if safe_total < MIN_FIT {
        for e in events { m.observe(e); }
        info.classes = m.classes();
        return (m, info);
    }
    let (a, b) = (n * 6 / 10, n * 8 / 10);
    for e in &events[..a] { m.observe(e.clone()); }
    // validation slice: collect the component probabilities of each target
    let mut samples: Vec<[f64; NC]> = Vec::new();
    for e in &events[a..b] {
        if let Some(c) = &e.class {
            if m.uni.contains_key(c) {
                let ctx = ctx_at(&m.events, e.t, e.min, e.wd, None);
                if let Some(s) = m.score_all(&ctx).into_iter().find(|s| &s.class == c) { samples.push(s.comps); }
            }
        }
        m.observe(e.clone());
    }
    if samples.len() >= 10 {
        let ll = |w: &[f64; NC]| -> f64 { samples.iter().map(|c| c.iter().zip(w).map(|(a, b)| a * b).sum::<f64>().max(1e-6).ln()).sum() };
        let mut best = (ll(&m.w), m.w);
        let mut idx = [0usize; NC];
        for i0 in 0..=10 { for i1 in 0..=(10 - i0) { for i2 in 0..=(10 - i0 - i1) { for i3 in 0..=(10 - i0 - i1 - i2) {
            idx[0] = i0; idx[1] = i1; idx[2] = i2; idx[3] = i3; idx[4] = 10 - i0 - i1 - i2 - i3;
            if (0..NC).any(|i| !mask[i] && idx[i] != 0) { continue; }
            let w = idx.map(|i| i as f64 / 10.0);
            let l = ll(&w);
            if l > best.0 { best = (l, w); }
        }}}}
        // shrink toward the defaults so a lucky validation slice cannot zero a component out
        for i in 0..NC { m.w[i] = 0.85 * best.1[i] + 0.15 * masked_default(&mask)[i]; }
        info.tuned = true;
    }
    info.weights = m.w;
    // report slice: predict, then learn
    let (mut h, mut t1, mut t3, mut b1, mut b3, mut cn, mut ch) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    for e in &events[b..] {
        if let Some(c) = &e.class {
            let ctx = ctx_at(&m.events, e.t, e.min, e.wd, None);
            let r = m.rank(&ctx);
            h += 1;
            if r.first().map_or(false, |s| &s.class == c) { t1 += 1; }
            if let Some(f) = r.first().filter(|s| s.p >= MIN_CONFIDENCE) { cn += 1; if &f.class == c { ch += 1; } }
            if r.iter().take(3).any(|s| &s.class == c) { t3 += 1; }
            let base = heuristic_rank(&m.events, e.t, e.min);
            if base.first().map_or(false, |s| &s.0 == c) { b1 += 1; }
            if base.iter().take(3).any(|s| &s.0 == c) { b3 += 1; }
        }
        m.observe(e.clone());
    }
    if h > 0 {
        let f = |x: usize| x as f64 / h as f64;
        info.holdout_n = h;
        info.top1 = f(t1); info.top3 = f(t3); info.base_top1 = f(b1); info.base_top3 = f(b3);
        info.conf_cover = f(cn);
        info.conf_prec = if cn > 0 { ch as f64 / cn as f64 } else { 0.0 };
    }
    info.classes = m.classes();
    (m, info)
}

/// The model plus bookkeeping for incremental retraining.
pub struct Predictor {
    model: Model,
    info: FitInfo,
    since_fit: usize,
}

impl Predictor {
    pub fn from_events(events: Vec<Event>) -> Self {
        let (model, info) = fit(events);
        Self { model, info, since_fit: 0 }
    }

    pub fn observe(&mut self, e: Event) {
        self.model.observe(e);
        self.since_fit += 1;
        self.info.trained_on += 1;
        if self.since_fit >= REFIT_EVERY {
            let evs = self.model.events.clone();
            let (m, i) = fit(evs);
            self.model = m;
            self.info = i;
            self.since_fit = 0;
        }
    }

    /// True once there is enough history for the model to be trusted over the SQL rule.
    pub fn ready(&self) -> bool {
        self.info.tuned
    }

    /// Top predictions for "now": confident, context-driven, and not what was just done.
    pub fn predict(&self, t: i64, n: usize, open_apps: Option<&[String]>) -> Vec<Value> {
        let (min, wd) = local_parts(t);
        let ctx = ctx_at(&self.model.events, t, min, wd, open_apps);
        let last = self.model.events.last().filter(|e| t - e.t < 600).and_then(|e| e.class.clone());
        let mut out = Vec::new();
        for sc in self.model.rank(&ctx).into_iter().take(n + 3) {
            if sc.p < MIN_CONFIDENCE || last.as_deref() == Some(sc.class.as_str()) { continue; }
            let (reason, usual, kind) = self.model.reason(&ctx, &sc);
            if kind == 0 { continue; } // nothing beyond "you do this a lot": not a prediction
            let text = self.model.text_of(&sc.class);
            let comps: serde_json::Map<String, Value> = COMP.iter().zip(sc.comps.iter().zip(self.model.w.iter()))
                .map(|(name, (p, w))| (name.to_string(), json!((p * w * 1000.0).round() / 1000.0))).collect();
            let mut v = json!({ "text": text, "class": sc.class, "confidence": (sc.p * 1000.0).round() / 1000.0,
                                "reason": reason, "driver": COMP[kind], "components": comps, "source": "model" });
            if let Some(u) = usual { v["usual_time"] = json!(u); }
            out.push(v);
            if out.len() >= n { break; }
        }
        out
    }

    pub fn info(&self) -> Value {
        let i = &self.info;
        let weights: serde_json::Map<String, Value> = COMP.iter().zip(i.weights.iter()).map(|(n, w)| (n.to_string(), json!(w))).collect();
        json!({
            "kind": "stacked n-gram + time-of-day kernel density (pure Rust, on-device)",
            "trained_on": i.trained_on, "classes": i.classes, "tuned": i.tuned, "weights": weights,
            "accuracy": { "holdout_requests": i.holdout_n, "top1": round3(i.top1), "top3": round3(i.top3),
                          "confident_coverage": round3(i.conf_cover), "confident_precision": round3(i.conf_prec),
                          "baseline_top1": round3(i.base_top1), "baseline_top3": round3(i.base_top3),
                          "note": "last 20% of your history, predicted before it was learned; baseline is the old 'usual time' rule" },
            "refit_every": REFIT_EVERY, "since_refit": self.since_fit,
        })
    }
}

fn round3(x: f64) -> f64 { (x * 1000.0).round() / 1000.0 }

/// "2026-10-17T15:35:24Z" -> UTC seconds.
pub fn parse_iso(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 { return None; }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    // days-from-civil (Howard Hinnant)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(t: i64, text: &str, intent: &str) -> Event {
        build_event_at(t, ((t / 60) % 1440) as u16, ((t / DAY) % 7) as u8, text, intent, "fast", &json!({}), true)
    }

    #[test]
    fn iso_roundtrip() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2026-10-17T15:35:24Z"), Some(1_792_251_324));
        assert_eq!(parse_iso("garbage"), None);
    }

    #[test]
    fn safety_filters() {
        let mk = |u: &str, i: &str, s: &str, p: Value| build_event_at(0, 0, 0, u, i, s, &p, true);
        assert!(mk("open the terminal", "open_app", "fast", json!({"app": "terminal"})).class.is_some());
        assert!(mk("shut down", "os_action", "fast", json!({"action": "power"})).class.is_none());
        assert!(mk("shutdown now", "shutdown", "fast", json!({})).class.is_none());
        assert!(mk("clean up my disk", "clean_storage", "fast", json!({})).class.is_none());
        assert!(mk("move files", "orchestrate", "llm", json!({})).class.is_none());
        assert!(mk("write an email to bob", "generate_text", "llm", json!({})).class.is_none());
        assert!(mk(&"x".repeat(61), "timer", "fast", json!({})).class.is_none());
        assert_eq!(mk("launch terminal", "open_app", "fast", json!({"app": "Terminal"})).class.as_deref(), Some("open_app:terminal"));
    }

    #[test]
    fn learns_a_sequence_and_explains_it() {
        // every day: terminal, then (4 min later) browser; plus some unrelated noise
        let mut evs = vec![];
        for d in 0..20 {
            let t0 = d * DAY + 9 * 3600 + (d % 3) * 60;
            evs.push(ev(t0, "open the terminal", "open_app"));
            evs.push(ev(t0 + 240, "open the browser", "open_app"));
            if d % 2 == 0 { evs.push(ev(d * DAY + 15 * 3600, "mute", "os_action")); }
        }
        let mut p = Predictor::from_events(evs.clone());
        assert!(p.ready());
        // the model has the open_app intent without an app, so classes are the texts
        let t = 20 * DAY + 9 * 3600 + 300;
        p.observe(ev(t - 280, "open the terminal", "open_app"));
        let r = p.predict(t, 3, None);
        assert!(!r.is_empty(), "{:?}", r);
        assert_eq!(r[0]["text"], "open the browser");
        assert!(r[0]["confidence"].as_f64().unwrap() > 0.5, "{}", r[0]);
        assert!(r[0]["reason"].as_str().unwrap().contains("open the terminal"), "{}", r[0]["reason"]);
    }

    #[test]
    fn no_prediction_without_a_reason() {
        let evs: Vec<Event> = (0..60).map(|i| ev(i * 5000, &format!("thing {}", i % 7), "timer")).collect();
        let p = Predictor::from_events(evs);
        // irregular spacing, nothing time- or order-specific enough: whatever it says must have a driver and confidence
        for v in p.predict(61 * 5000, 3, None) {
            assert!(v["confidence"].as_f64().unwrap() >= MIN_CONFIDENCE);
            assert_ne!(v["driver"], "prior");
        }
    }

    #[test]
    fn untuned_when_history_is_short() {
        let evs: Vec<Event> = (0..10).map(|i| ev(i * 3600, "mute", "os_action")).collect();
        let p = Predictor::from_events(evs);
        assert!(!p.ready());
    }

    #[test]
    fn incremental_matches_batch_counts() {
        let evs: Vec<Event> = (0..100).map(|i| ev(i * 1000, if i % 3 == 0 { "a" } else { "b" }, "timer")).collect();
        let mut inc = Model::new();
        for e in &evs { inc.observe(e.clone()); }
        let mut batch = Model::new();
        for e in evs.iter().cloned() { batch.observe(e); }
        let ctx = ctx_at(&evs, 101_000, 10, 1, None);
        let (x, y) = (inc.rank(&ctx), batch.rank(&ctx));
        assert_eq!(x.len(), y.len());
        assert!((x[0].p - y[0].p).abs() < 1e-12);
        let s: f64 = x.iter().map(|s| s.p).sum();
        assert!((s - 1.0).abs() < 1e-9, "probabilities sum to {}", s);
    }
}
