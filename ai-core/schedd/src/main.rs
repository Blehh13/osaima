//! `osaima-schedd`: sets each process's CPU priority from what it has been doing.
//!
//!   osaima-schedd [run] [options]     measure and set priorities until stopped
//!   osaima-schedd sample --pids 1,2   print the features of some processes (for training)
//!
//! Run as root so priorities can be raised as well as lowered; everything it changes is
//! put back when it stops.

use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use osaima_schedd::actuator::KernelActuator;
use osaima_schedd::bpf::BpfCollector;
use osaima_schedd::collector::{Collector, ProcCollector};
use osaima_schedd::engine::{Config, Engine};
use osaima_schedd::features::Features;
use osaima_schedd::model::{Class, Classifier, Model};
use osaima_schedd::policy::Policy;

const USAGE: &str = "\
Usage: osaima-schedd [run] [options]
       osaima-schedd sample --pids PID[,PID...] [--seconds N] [--interval S]

Options for run:
  --interval S          seconds between measurements (default 2)
  --min-uid N           only touch processes of users from this id up (default 1000)
  --hysteresis N        windows in a row before a change (default 3)
  --min-confidence P    ignore verdicts less sure than P, 0 to 1 (default 0.6)
  --model FILE          a trained model (JSON); without one, built-in rules are used
  --policy FILE         priorities per kind of process (JSON), over the defaults
  --source S            where measurements come from: bpf, proc or auto (default auto:
                        eBPF when its object file is installed, else /proc)
  --bpf-object FILE     the compiled eBPF telemetry (default /usr/lib/osaima/schedd/telemetry.bpf.o)
  --bpftool PATH        the bpftool to load it with (default bpftool)
  --ignore NAME         never touch processes with this name (repeatable)
  --dry-run             decide and report, change nothing
  --once                measure for one window, report, and exit
  -v, --verbose         print a summary every round
";

const DEFAULT_BPF_OBJECT: &str = "/usr/lib/osaima/schedd/telemetry.bpf.o";
const PIN_DIR: &str = "/sys/fs/bpf/osaima-schedd";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

#[derive(Debug)]
struct Opts {
    command: String,
    interval: f64,
    min_uid: u32,
    hysteresis: u32,
    min_confidence: f64,
    model: Option<String>,
    policy: Option<String>,
    source: String,
    bpf_object: String,
    bpftool: String,
    ignore: Vec<String>,
    dry_run: bool,
    once: bool,
    verbose: bool,
    pids: Vec<i32>,
    seconds: f64,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        command: "run".into(),
        interval: 2.0,
        min_uid: Config::default().min_uid,
        hysteresis: Config::default().hysteresis,
        min_confidence: Config::default().min_confidence,
        model: None,
        policy: None,
        source: "auto".into(),
        bpf_object: DEFAULT_BPF_OBJECT.into(),
        bpftool: "bpftool".into(),
        ignore: Vec::new(),
        dry_run: false,
        once: false,
        verbose: false,
        pids: Vec::new(),
        seconds: 10.0,
    };
    let mut it = args.iter().peekable();
    if let Some(first) = it.peek() {
        if !first.starts_with('-') {
            o.command = it.next().expect("peeked").clone();
        }
    }
    fn value<'a>(
        it: &mut impl Iterator<Item = &'a String>,
        flag: &str,
    ) -> Result<&'a String, String> {
        it.next().ok_or_else(|| format!("{flag} needs a value"))
    }
    fn number<T: std::str::FromStr>(text: &str, flag: &str) -> Result<T, String> {
        text.parse()
            .map_err(|_| format!("{flag}: '{text}' is not a valid number"))
    }
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--interval" => o.interval = number(value(&mut it, arg)?, arg)?,
            "--min-uid" => o.min_uid = number(value(&mut it, arg)?, arg)?,
            "--hysteresis" => o.hysteresis = number(value(&mut it, arg)?, arg)?,
            "--min-confidence" => o.min_confidence = number(value(&mut it, arg)?, arg)?,
            "--model" => o.model = Some(value(&mut it, arg)?.clone()),
            "--policy" => o.policy = Some(value(&mut it, arg)?.clone()),
            "--source" => o.source = value(&mut it, arg)?.clone(),
            "--bpf-object" => o.bpf_object = value(&mut it, arg)?.clone(),
            "--bpftool" => o.bpftool = value(&mut it, arg)?.clone(),
            "--ignore" => o.ignore.push(value(&mut it, arg)?.clone()),
            "--pids" => {
                for part in value(&mut it, arg)?.split(',').filter(|p| !p.is_empty()) {
                    o.pids.push(number(part, arg)?);
                }
            }
            "--seconds" => o.seconds = number(value(&mut it, arg)?, arg)?,
            "--dry-run" => o.dry_run = true,
            "--once" => o.once = true,
            "-v" | "--verbose" => o.verbose = true,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if !(o.interval >= 0.05 && o.interval <= 3600.0) {
        return Err("--interval must be between 0.05 and 3600 seconds".into());
    }
    if !["auto", "bpf", "proc"].contains(&o.source.as_str()) {
        return Err("--source must be auto, bpf or proc".into());
    }
    if o.hysteresis == 0 {
        return Err("--hysteresis must be at least 1".into());
    }
    if !(0.0..=1.0).contains(&o.min_confidence) {
        return Err("--min-confidence must be between 0 and 1".into());
    }
    Ok(o)
}

/// Sleep for `seconds`, waking early when asked to stop.
fn nap(seconds: f64) {
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    while !STOP.load(Ordering::SeqCst) && Instant::now() < end {
        thread::sleep(Duration::from_millis(50).min(end.saturating_duration_since(Instant::now())));
    }
}

fn install_signal_handlers() {
    unsafe {
        let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

fn load_classifier(path: &Option<String>) -> Result<Classifier, String> {
    match path {
        None => Ok(Classifier::Rules),
        Some(p) => {
            let text = std::fs::read_to_string(p)
                .map_err(|e| format!("cannot read the model {p}: {e}"))?;
            Model::from_json(&text)
                .map(Classifier::Trained)
                .map_err(|e| format!("{p}: {e}"))
        }
    }
}

fn run(o: &Opts) -> Result<(), String> {
    let classifier = load_classifier(&o.model)?;
    let policy = match &o.policy {
        None => Policy::default(),
        Some(p) => Policy::from_json(
            &std::fs::read_to_string(p).map_err(|e| format!("cannot read the policy {p}: {e}"))?,
        )?,
    };
    let mut cfg = Config {
        min_uid: o.min_uid,
        hysteresis: o.hysteresis,
        min_confidence: o.min_confidence,
        dry_run: o.dry_run,
        ..Config::default()
    };
    cfg.ignore.extend(o.ignore.iter().cloned());
    let use_bpf = match o.source.as_str() {
        "bpf" => true,
        "proc" => false,
        _ => std::path::Path::new(&o.bpf_object).exists(),
    };
    if use_bpf {
        let collector = BpfCollector::attach(
            &o.bpftool,
            std::path::Path::new(&o.bpf_object),
            std::path::Path::new(PIN_DIR),
        )
        .map_err(|e| {
            format!("cannot start the eBPF telemetry: {e} (use --source proc to go without)")
        })?;
        drive(
            Engine::new(collector, KernelActuator, classifier, policy, cfg),
            o,
            "eBPF",
        )
    } else {
        drive(
            Engine::new(
                ProcCollector::new(),
                KernelActuator,
                classifier,
                policy,
                cfg,
            ),
            o,
            "/proc",
        )
    }
}

/// Measure, judge and set priorities until told to stop, then put everything back.
fn drive<C: Collector>(
    mut engine: Engine<C, KernelActuator>,
    o: &Opts,
    source: &str,
) -> Result<(), String> {
    println!(
        "osaima-schedd: {} on {source} data every {} s, processes of users from {} up{}",
        engine.classifier_name(),
        o.interval,
        o.min_uid,
        if o.dry_run {
            " (dry run: nothing is changed)"
        } else {
            ""
        }
    );
    install_signal_handlers();

    engine
        .tick(o.interval)
        .map_err(|e| format!("cannot read /proc: {e}"))?; // the baseline
    let mut last = Instant::now();
    while !STOP.load(Ordering::SeqCst) {
        nap(o.interval);
        if STOP.load(Ordering::SeqCst) {
            break;
        }
        let now = Instant::now();
        let seconds = now.duration_since(last).as_secs_f64();
        last = now;
        let report = engine
            .tick(seconds)
            .map_err(|e| format!("cannot read /proc: {e}"))?;
        for c in &report.changes {
            let what = match (&c.error, c.applied) {
                (Some(e), _) => format!("refused: {e}"),
                (None, true) => "set".to_string(),
                (None, false) => "would set".to_string(),
            };
            println!(
                "{} {} -> {} (p={:.2}): {what} nice {} {:?}",
                c.pid, c.comm, c.class, c.confidence, c.setting.nice, c.setting.sched
            );
        }
        if o.verbose || o.once {
            let by_class: Vec<String> = Class::ALL
                .iter()
                .map(|k| format!("{k} {}", report.counts.get(k).copied().unwrap_or(0)))
                .collect();
            let (tracked, adjusted) = engine.counts();
            println!(
                "judged {} ({}); {tracked} tracked, {adjusted} adjusted",
                report.judged,
                by_class.join(", ")
            );
        }
        if o.once {
            break;
        }
    }
    engine.shutdown();
    println!("osaima-schedd: stopped; priorities put back");
    Ok(())
}

/// Print the features of the given processes, one CSV row per process per window.
fn sample(o: &Opts) -> Result<(), String> {
    if o.pids.is_empty() {
        return Err("sample needs --pids".into());
    }
    let mut collector = ProcCollector::new();
    let read =
        |c: &mut ProcCollector| -> Result<HashMap<i32, osaima_schedd::collector::Sample>, String> {
            Ok(c.sample()
                .map_err(|e| format!("cannot read /proc: {e}"))?
                .into_iter()
                .filter(|s| o.pids.contains(&s.pid))
                .map(|s| (s.pid, s))
                .collect())
        };
    println!("pid,comm,cpu_util,vol_rate,invol_rate,slice_ms,wait_ratio");
    let mut prev = read(&mut collector)?;
    let mut last = Instant::now();
    let end = last + Duration::from_secs_f64(o.seconds);
    install_signal_handlers();
    while !STOP.load(Ordering::SeqCst) && Instant::now() < end {
        nap(o.interval);
        let now = Instant::now();
        let seconds = now.duration_since(last).as_secs_f64();
        last = now;
        let current = read(&mut collector)?;
        let mut pids: Vec<&i32> = current.keys().collect();
        pids.sort();
        for pid in pids {
            let (s, before) = (&current[pid], prev.get(pid));
            if let Some(f) =
                before.and_then(|b| Features::between(&b.counters, &s.counters, seconds))
            {
                println!(
                    "{},{},{:.6},{:.4},{:.4},{:.4},{:.4}",
                    s.pid,
                    s.comm.replace(',', ";"),
                    f.cpu_util,
                    f.vol_rate,
                    f.invol_rate,
                    f.slice_ms,
                    f.wait_ratio
                );
            }
        }
        prev = current;
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match parse(&args) {
        Ok(o) => o,
        Err(message) => {
            if !message.is_empty() {
                eprintln!("error: {message}\n");
            }
            eprint!("{USAGE}");
            return ExitCode::from(if message.is_empty() { 0 } else { 2 });
        }
    };
    let result = match opts.command.as_str() {
        "run" => run(&opts),
        "sample" => sample(&opts),
        other => Err(format!("unknown command: {other}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn options_are_parsed() {
        let o = parse(&args("run --interval 0.5 --min-uid 500 --hysteresis 2 --ignore firefox --ignore foot --dry-run -v")).unwrap();
        assert_eq!(
            (o.command.as_str(), o.interval, o.min_uid, o.hysteresis),
            ("run", 0.5, 500, 2)
        );
        assert_eq!(o.ignore, vec!["firefox", "foot"]);
        assert!(o.dry_run && o.verbose && !o.once);
        let s = parse(&args("sample --pids 10,20,30 --seconds 5")).unwrap();
        assert_eq!(
            (s.command.as_str(), s.pids.clone(), s.seconds),
            ("sample", vec![10, 20, 30], 5.0)
        );
        assert_eq!(parse(&[]).unwrap().command, "run");
    }

    #[test]
    fn bad_options_are_explained() {
        assert!(parse(&args("--interval fast"))
            .unwrap_err()
            .contains("not a valid number"));
        assert!(parse(&args("--interval"))
            .unwrap_err()
            .contains("needs a value"));
        assert!(parse(&args("--interval 0"))
            .unwrap_err()
            .contains("between"));
        assert!(parse(&args("--hysteresis 0"))
            .unwrap_err()
            .contains("at least 1"));
        assert!(parse(&args("--min-confidence 2"))
            .unwrap_err()
            .contains("between 0 and 1"));
        assert!(parse(&args("--bogus"))
            .unwrap_err()
            .contains("unknown option"));
        assert_eq!(parse(&args("--help")).unwrap_err(), "");
    }

    #[test]
    fn a_missing_model_file_is_a_clear_error() {
        let err = load_classifier(&Some("/nonexistent/model.json".into())).unwrap_err();
        assert!(err.contains("cannot read the model"), "{err}");
        assert!(matches!(load_classifier(&None), Ok(Classifier::Rules)));
    }
}
