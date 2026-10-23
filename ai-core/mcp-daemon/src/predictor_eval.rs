//! Evaluation of the next-action predictor on synthetic but realistic
//! histories. Run with:  cargo test --release predictor_eval -- --nocapture
//!
//! Four personas with noisy routines (chains of requests that start around a
//! usual time with jitter, steps that are sometimes skipped, days that are
//! skipped, weekday/weekend differences, random one-off requests, and LLM-path
//! requests that only serve as context). Each history is split in time order
//! (60 % train, 20 % tune the mixture weights, 20 % report); every reported
//! request is predicted before the model learns it. The baseline is the
//! deployed SQL rule (`Behavior::predictions`, re-implemented over the same
//! events as `predictor::heuristic_rank`).

use crate::predictor::*;
use serde_json::json;

const DAY: i64 = 86400;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f64 {
        let (a, b) = (self.next().max(1e-12), self.next());
        (-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos()
    }
}

/// text, intent, app
type Step = (&'static str, &'static str, Option<&'static str>, f64); // + probability the step happens

struct Chain {
    start_min: f64,
    sd: f64,
    p_day: f64,
    days: &'static [u8], // weekdays (Mon = 0) on which it can happen
    steps: &'static [Step],
}

struct Persona {
    name: &'static str,
    chains: &'static [Chain],
    noise_per_day: f64,
}

const WEEK: &[u8] = &[0, 1, 2, 3, 4];
const WEEKEND: &[u8] = &[5, 6];

const OPEN_TERM: Step = ("open the terminal", "open_app", Some("terminal"), 1.0);
const OPEN_BROWSER: Step = ("open the browser", "open_app", Some("browser"), 1.0);

const NOISE: &[(&str, &str, Option<&str>)] = &[
    ("what time is it", "time", None), ("mute", "os_action", None), ("set brightness to 40", "os_action", None),
    ("open the calculator", "open_app", Some("calculator")), ("what's the weather", "weather", None),
    ("set a timer for 5 minutes", "timer", None), ("open the file manager", "open_app", Some("files")),
    ("take a screenshot", "os_action", None), ("switch to workspace 2", "workspace", None),
    ("open the browser", "open_app", Some("browser")), ("search the web for rust lifetimes", "search_web", None),
    ("turn the volume up", "os_action", None),
];

const PERSONAS: &[Persona] = &[
    Persona { name: "developer", noise_per_day: 5.0, chains: &[
        Chain { start_min: 8.0 * 60.0 + 45.0, sd: 20.0, p_day: 0.9, days: WEEK, steps: &[OPEN_TERM, OPEN_BROWSER, ("open the file manager", "open_app", Some("files"), 0.6)] },
        Chain { start_min: 13.0 * 60.0, sd: 25.0, p_day: 0.8, days: WEEK, steps: &[("set a timer for 25 minutes", "timer", None, 1.0), ("mute", "os_action", None, 0.7)] },
        Chain { start_min: 17.0 * 60.0 + 45.0, sd: 30.0, p_day: 0.75, days: WEEK, steps: &[("what's the weather", "weather", None, 1.0), ("set brightness to 30", "os_action", None, 0.7)] },
        Chain { start_min: 11.0 * 60.0, sd: 60.0, p_day: 0.7, days: WEEKEND, steps: &[OPEN_BROWSER, ("search the web for linux news", "search_web", None, 0.6)] },
    ]},
    Persona { name: "student", noise_per_day: 6.0, chains: &[
        Chain { start_min: 7.0 * 60.0 + 30.0, sd: 15.0, p_day: 0.85, days: WEEK, steps: &[("what's the weather", "weather", None, 1.0), ("open the calculator", "open_app", Some("calculator"), 0.5), ("set a timer for 10 minutes", "timer", None, 0.4)] },
        Chain { start_min: 20.0 * 60.0, sd: 40.0, p_day: 0.85, days: &[0, 1, 2, 3, 4, 6], steps: &[OPEN_BROWSER, ("open the notes", "open_app", Some("notes"), 0.8), ("set a timer for 45 minutes", "timer", None, 0.8)] },
        Chain { start_min: 22.5 * 60.0, sd: 25.0, p_day: 0.6, days: &[0, 1, 2, 3, 4, 5, 6], steps: &[("set brightness to 20", "os_action", None, 1.0), ("mute", "os_action", None, 0.6)] },
    ]},
    Persona { name: "office worker", noise_per_day: 4.0, chains: &[
        Chain { start_min: 9.0 * 60.0, sd: 10.0, p_day: 0.95, days: WEEK, steps: &[OPEN_BROWSER, ("open the file manager", "open_app", Some("files"), 0.9), ("open the calculator", "open_app", Some("calculator"), 0.3)] },
        Chain { start_min: 12.0 * 60.0 + 30.0, sd: 15.0, p_day: 0.9, days: WEEK, steps: &[("mute", "os_action", None, 1.0), ("set a timer for 30 minutes", "timer", None, 0.8)] },
        Chain { start_min: 9.0 * 60.0 + 40.0, sd: 10.0, p_day: 0.9, days: &[0], steps: &[("open the calendar", "open_app", Some("calendar"), 1.0), ("what's the weather", "weather", None, 0.6)] },
        Chain { start_min: 16.0 * 60.0 + 40.0, sd: 12.0, p_day: 0.85, days: &[4], steps: &[("take a screenshot", "os_action", None, 1.0), ("turn the volume up", "os_action", None, 0.5)] },
    ]},
    Persona { name: "night owl (irregular)", noise_per_day: 8.0, chains: &[
        Chain { start_min: 14.0 * 60.0, sd: 90.0, p_day: 0.6, days: &[0, 1, 2, 3, 4, 5, 6], steps: &[OPEN_TERM, OPEN_BROWSER, ("search the web for rust crates", "search_web", None, 0.5)] },
        Chain { start_min: 23.0 * 60.0, sd: 60.0, p_day: 0.65, days: &[0, 1, 2, 3, 4, 5, 6], steps: &[("set brightness to 15", "os_action", None, 1.0), ("open the terminal", "open_app", Some("terminal"), 0.6), ("set a timer for 20 minutes", "timer", None, 0.4)] },
        Chain { start_min: 18.0 * 60.0, sd: 70.0, p_day: 0.5, days: WEEKEND, steps: &[("what's the weather", "weather", None, 1.0), ("mute", "os_action", None, 0.5)] },
    ]},
];

pub fn generate(p: &Persona, days: usize, seed: u64) -> Vec<Event> {
    let mut r = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
    for _ in 0..8 { r.next(); }
    let mut evs: Vec<Event> = Vec::new();
    let mut push = |evs: &mut Vec<Event>, d: usize, minute: f64, text: &str, intent: &str, app: Option<&str>, source: &str| {
        let m = minute.clamp(0.0, 1439.0);
        let t = d as i64 * DAY + (m * 60.0) as i64;
        let params = match app { Some(a) => json!({ "app": a }), None => json!({}) };
        evs.push(build_event_at(t, m as u16, (d % 7) as u8, text, intent, source, &params, true));
    };
    for d in 0..days {
        let wd = (d % 7) as u8;
        for c in p.chains {
            if !c.days.contains(&wd) || r.next() > c.p_day { continue; }
            let mut m = c.start_min + c.sd * r.gauss();
            for (i, s) in c.steps.iter().enumerate() {
                if i > 0 { if r.next() > s.3 { continue; } m += 1.0 + 3.0 * r.next() * 2.0; }
                push(&mut evs, d, m, s.0, s.1, s.2, "fast");
            }
        }
        // one-off requests (some through the slow LLM path: context, never suggested)
        let k = (p.noise_per_day + 2.0 * r.gauss()).max(0.0).round() as usize;
        for _ in 0..k {
            let m = 7.0 * 60.0 + r.next() * 16.0 * 60.0;
            if r.next() < 0.15 { push(&mut evs, d, m, "plan a trip for me", "orchestrate", None, "llm"); continue; }
            // Zipf-ish: low indexes are more common
            let i = ((r.next().powf(1.8)) * NOISE.len() as f64) as usize;
            let n = NOISE[i.min(NOISE.len() - 1)];
            push(&mut evs, d, m, n.0, n.1, n.2, "fast");
        }
    }
    evs.sort_by_key(|e| e.t);
    evs
}

fn mean(v: &[f64]) -> f64 { v.iter().sum::<f64>() / v.len().max(1) as f64 }
fn sd(v: &[f64]) -> f64 { let m = mean(v); (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt() }
fn pct(x: f64) -> String { format!("{:.1}", x * 100.0) }

const SEEDS: u64 = 10;

struct Row { name: String, top1: Vec<f64>, top3: Vec<f64>, b1: Vec<f64>, b3: Vec<f64>, cover: Vec<f64>, prec: Vec<f64>, n: Vec<f64> }

fn run(p: &Persona, days: usize, mask: [bool; NC]) -> Row {
    let mut row = Row { name: p.name.into(), top1: vec![], top3: vec![], b1: vec![], b3: vec![], cover: vec![], prec: vec![], n: vec![] };
    for seed in 1..=SEEDS {
        let (_, info) = fit_with(generate(p, days, seed), mask);
        row.top1.push(info.top1); row.top3.push(info.top3); row.b1.push(info.base_top1); row.b3.push(info.base_top3);
        row.cover.push(info.conf_cover); row.prec.push(info.conf_prec); row.n.push(info.holdout_n as f64);
    }
    row
}

#[test]
fn evaluation_tables() {
    println!("\n### Main result: 28 days, {} seeds per persona, last 20 % of each history held out\n", SEEDS);
    println!("| persona | held-out requests | model top-1 | model top-3 | SQL rule top-1 | SQL rule top-3 | confident coverage | precision when confident |");
    println!("|---|---|---|---|---|---|---|---|");
    let mut all = (vec![], vec![], vec![], vec![]);
    for p in PERSONAS {
        let r = run(p, 28, [true; NC]);
        println!("| {} | {:.0} | {} ± {} | {} ± {} | {} ± {} | {} ± {} | {} | {} |", r.name, mean(&r.n),
            pct(mean(&r.top1)), pct(sd(&r.top1)), pct(mean(&r.top3)), pct(sd(&r.top3)),
            pct(mean(&r.b1)), pct(sd(&r.b1)), pct(mean(&r.b3)), pct(sd(&r.b3)), pct(mean(&r.cover)), pct(mean(&r.prec)));
        all.0.extend(r.top1); all.1.extend(r.top3); all.2.extend(r.b1); all.3.extend(r.b3);
    }
    println!("| **all** | | **{}** | **{}** | **{}** | **{}** | | |", pct(mean(&all.0)), pct(mean(&all.1)), pct(mean(&all.2)), pct(mean(&all.3)));
    assert!(mean(&all.0) > mean(&all.2) + 0.05, "model top-1 {} vs baseline {}", mean(&all.0), mean(&all.2));
    assert!(mean(&all.1) > mean(&all.3) + 0.05, "model top-3 {} vs baseline {}", mean(&all.1), mean(&all.3));

    println!("\n### Ablation (all personas pooled, 28 days): which component earns its place\n");
    println!("| components | top-1 | top-3 |");
    println!("|---|---|---|");
    let variants: [(&str, [bool; NC]); 6] = [
        ("prior only (most frequent request)", [true, false, false, false, false]),
        ("prior + time of day", [true, true, false, false, false]),
        ("prior + previous 1-2 requests", [true, false, true, true, false]),
        ("prior + open apps", [true, false, false, false, true]),
        ("all except open apps", [true, true, true, true, false]),
        ("all five (deployed)", [true; NC]),
    ];
    for (name, mask) in variants {
        let (mut a, mut b) = (vec![], vec![]);
        for p in PERSONAS { let r = run(p, 28, mask); a.extend(r.top1); b.extend(r.top3); }
        println!("| {} | {} | {} |", name, pct(mean(&a)), pct(mean(&b)));
    }

    println!("\n### Learning curve (all personas pooled): how much history it needs\n");
    println!("| days of history | model top-1 | model top-3 | SQL rule top-1 | SQL rule top-3 |");
    println!("|---|---|---|---|---|");
    for days in [7usize, 14, 28, 56] {
        let (mut a, mut b, mut c, mut d) = (vec![], vec![], vec![], vec![]);
        for p in PERSONAS { let r = run(p, days, [true; NC]); a.extend(r.top1); b.extend(r.top3); c.extend(r.b1); d.extend(r.b3); }
        println!("| {} | {} | {} | {} | {} |", days, pct(mean(&a)), pct(mean(&b)), pct(mean(&c)), pct(mean(&d)));
    }
    println!("\nChance level for reference: about 1 in {} candidate commands per persona.", 16);
}

#[test]
fn trains_in_milliseconds() {
    let evs = generate(&PERSONAS[0], 28, 1);
    let n = evs.len();
    let t = std::time::Instant::now();
    let (m, _) = fit(evs.clone());
    let fit_ms = t.elapsed().as_secs_f64() * 1000.0;
    let mut p = Predictor::from_events(evs);
    let t = std::time::Instant::now();
    for i in 0..200 { p.predict(28 * DAY + i * 60, 3, None); }
    let pred_ms = t.elapsed().as_secs_f64() * 1000.0 / 200.0;
    println!("\nfit on {} events ({} commands): {:.1} ms; one prediction: {:.2} ms", n, m.classes(), fit_ms, pred_ms);
    assert!(fit_ms < 5000.0);
}
