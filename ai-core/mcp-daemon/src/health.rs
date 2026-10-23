//! Continuous health watch: CPU temperature and thermal throttling, CPU,
//! memory and disk pressure (the kernel's PSI counters), low memory, low disk,
//! battery, and failed services. Every 20 s; each problem becomes an alert
//! with what is wrong, the likely cause and what to do, shown in the
//! assistant and sent once (per 30 min) as a desktop notification.
//!
//! `evidence()` gathers the same facts plus recent error logs, so the
//! assistant can explain why something failed and how to fix it.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct State {
    readings: Value,
    alerts: Vec<Value>,
    notified: HashMap<String, Instant>,
    high_cpu_rounds: u32,
    last_throttle: Option<u64>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Hottest CPU sensor in °C (hwmon k10temp/coretemp/zenpower, else thermal zones).
fn cpu_temp() -> Option<f64> {
    let mut best: Option<f64> = None;
    for e in std::fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        let p = e.path();
        let name = read(&format!("{}/name", p.display())).unwrap_or_default();
        if !["k10temp", "coretemp", "zenpower", "cpu_thermal"].contains(&name.as_str()) {
            continue;
        }
        for f in std::fs::read_dir(&p).ok()?.flatten() {
            let n = f.file_name().to_string_lossy().into_owned();
            if n.starts_with("temp") && n.ends_with("_input") {
                if let Some(v) = read(&f.path().to_string_lossy()).and_then(|v| v.parse::<f64>().ok()) {
                    best = Some(best.map_or(v / 1000.0, |b: f64| b.max(v / 1000.0)));
                }
            }
        }
    }
    best.or_else(|| {
        std::fs::read_dir("/sys/class/thermal").ok()?.flatten()
            .filter_map(|e| read(&format!("{}/temp", e.path().display())).and_then(|v| v.parse::<f64>().ok()))
            .map(|v| v / 1000.0).fold(None, |a: Option<f64>, v| Some(a.map_or(v, |a| a.max(v))))
    })
}

/// Intel's thermal-throttle counter (sum over CPUs); None where it doesn't exist.
fn throttle_count() -> Option<u64> {
    let mut total = 0u64;
    let mut seen = false;
    for e in std::fs::read_dir("/sys/devices/system/cpu").ok()?.flatten() {
        if let Some(v) = read(&format!("{}/thermal_throttle/package_throttle_count", e.path().display())).and_then(|v| v.parse::<u64>().ok()) {
            total += v;
            seen = true;
        }
    }
    seen.then_some(total)
}

/// Current speed as a share of the maximum, averaged over CPUs.
fn freq_ratio() -> Option<f64> {
    let mut sum = 0.0;
    let mut n = 0.0;
    for e in std::fs::read_dir("/sys/devices/system/cpu").ok()?.flatten() {
        let p = e.path().display().to_string();
        if let (Some(cur), Some(max)) = (read(&format!("{}/cpufreq/scaling_cur_freq", p)).and_then(|v| v.parse::<f64>().ok()),
                                         read(&format!("{}/cpufreq/cpuinfo_max_freq", p)).and_then(|v| v.parse::<f64>().ok())) {
            if max > 0.0 { sum += cur / max; n += 1.0; }
        }
    }
    (n > 0.0).then(|| sum / n)
}

/// PSI "some avg10": % of the last 10 s that tasks waited on this resource.
fn pressure(kind: &str) -> Option<f64> {
    read(&format!("/proc/pressure/{}", kind))?.lines().next()?.split_whitespace()
        .find_map(|f| f.strip_prefix("avg10=")).and_then(|v| v.parse().ok())
}

fn meminfo() -> (u64, u64) {
    let text = read("/proc/meminfo").unwrap_or_default();
    let get = |k: &str| text.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * 1024;
    (get("MemTotal:"), get("MemAvailable:"))
}

fn disk_free() -> Option<(u64, u64)> {
    let home = std::ffi::CString::new(std::env::var("HOME").unwrap_or_else(|_| "/".into())).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(home.as_ptr(), &mut st) } == 0)
        .then(|| (st.f_blocks as u64 * st.f_frsize as u64, st.f_bavail as u64 * st.f_frsize as u64))
}

fn battery() -> Option<(u64, bool)> {
    for e in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let p = e.path().display().to_string();
        if read(&format!("{}/type", p)).as_deref() == Some("Battery") {
            let cap = read(&format!("{}/capacity", p))?.parse().ok()?;
            let discharging = read(&format!("{}/status", p)).as_deref() == Some("Discharging");
            return Some((cap, discharging));
        }
    }
    None
}

/// CPU ticks used so far and resident memory (MB) for every process.
fn proc_sample() -> HashMap<u32, (String, u64, f64)> {
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as f64;
    let mut out = HashMap::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Some(stat) = read(&format!("/proc/{}/stat", pid)) else { continue };
        // comm is in parentheses and may contain spaces: split after the last ')'
        let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
        let name = stat[open + 1..close].to_string();
        let f: Vec<&str> = stat[close + 2..].split_whitespace().collect();
        let ticks = f.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) + f.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let rss = read(&format!("/proc/{}/statm", pid)).and_then(|m| m.split_whitespace().nth(1).and_then(|v| v.parse::<f64>().ok())).unwrap_or(0.0);
        out.insert(pid, (name, ticks, rss * page / 1048576.0));
    }
    out
}

/// Busiest program over the last second (name, % of one core) and the
/// largest by memory (name, MB): current use, not the lifetime average ps shows.
fn top_processes() -> (Option<(String, f64)>, Option<(String, f64)>) {
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let a = proc_sample();
    std::thread::sleep(Duration::from_millis(1000));
    let b = proc_sample();
    let cpu = b.iter().filter_map(|(pid, (name, t, _))| a.get(pid).map(|(_, t0, _)| (name.clone(), (t.saturating_sub(*t0)) as f64 / hz * 100.0)))
        .max_by(|x, y| x.1.partial_cmp(&y.1).unwrap());
    let mem = b.values().max_by(|x, y| x.2.partial_cmp(&y.2).unwrap()).map(|(n, _, m)| (n.clone(), *m));
    (cpu, mem)
}

/// "app-google\x2dchrome@2bee4a8a….service (user)" → "Google Chrome": the
/// name a person knows, for titles. systemd escapes '-' as \x2d and adds an
/// instance id after '@' for apps started from the desktop.
fn friendly_unit(unit: &str) -> String {
    let u = unit.trim_end_matches(" (user)");
    let u = u.rsplit_once('.').filter(|(_, ext)| ["service", "scope", "socket", "timer", "mount"].contains(ext)).map_or(u, |(n, _)| n);
    let u = u.split('@').next().unwrap_or(u);
    let u = u.strip_prefix("app-").unwrap_or(u).replace("\\x2d", "-");
    // a trailing random id ("-2bee4a8a") from the launcher
    let u = match u.rsplit_once('-') { Some((head, tail)) if tail.len() >= 6 && tail.chars().all(|c| c.is_ascii_hexdigit()) => head.to_string(), _ => u };
    let words: Vec<String> = u.split(['-', '_', '.']).filter(|w| !w.is_empty())
        .map(|w| { let mut c = w.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }).collect();
    if words.is_empty() { unit.to_string() } else { words.join(" ") }
}

fn failed_units() -> Vec<String> {
    let mut out = Vec::new();
    for user in [true, false] {
        let mut c = Command::new("systemctl");
        if user { c.arg("--user"); }
        if let Ok(o) = c.args(["--failed", "--no-legend", "--plain"]).output() {
            for l in String::from_utf8_lossy(&o.stdout).lines() {
                if let Some(u) = l.split_whitespace().next() {
                    out.push(format!("{}{}", u, if user { " (user)" } else { "" }));
                }
            }
        }
    }
    out
}

fn alert(id: &str, level: &str, title: String, detail: String, advice: String) -> Value {
    json!({ "id": id, "level": level, "title": title, "detail": detail, "advice": advice })
}

/// One look at the machine: readings, and the problems they show.
fn check(st: &mut State) -> (Value, Vec<Value>) {
    let temp = cpu_temp();
    let freq = freq_ratio();
    let (cpu_p, mem_p, io_p) = (pressure("cpu"), pressure("memory"), pressure("io"));
    let (total, avail) = meminfo();
    let disk = disk_free();
    let bat = battery();
    let (top_cpu, top_mem) = top_processes();
    let failed = failed_units();
    let throttle = throttle_count();
    let throttled = match (throttle, st.last_throttle) { (Some(now), Some(before)) => now > before, _ => false };
    st.last_throttle = throttle;
    let busy = cpu_p.unwrap_or(0.0) > 40.0 || top_cpu.as_ref().map_or(false, |t| t.1 > 85.0);
    st.high_cpu_rounds = if busy { st.high_cpu_rounds + 1 } else { 0 };

    let mut alerts = Vec::new();
    let culprit = top_cpu.as_ref().filter(|t| t.1 >= 20.0).map(|t| format!(" The busiest program is {} ({:.0}% of the processor).", t.0, t.1)).unwrap_or_default();
    if let Some(t) = temp {
        // hot, and either the throttle counter moved or the clock dropped well below its maximum
        let slowed = throttled || (t >= 85.0 && freq.map_or(false, |f| f < 0.6));
        if t >= 95.0 || slowed {
            alerts.push(alert("thermal", "bad", format!("The processor is very hot ({:.0}°C){}", t, if slowed { " and slowing itself down" } else { "" }),
                format!("Heat makes the processor lower its speed to protect itself (thermal throttling), so everything feels slower.{}", culprit),
                "Close heavy programs you don't need, make sure the vents aren't blocked, and plug in the charger if it's on battery.".into()));
        } else if t >= 85.0 {
            alerts.push(alert("thermal", "warn", format!("The processor is running hot ({:.0}°C)", t),
                format!("It may start slowing down soon to cool off.{}", culprit),
                "Give it a moment of lighter work, or close what's using the processor.".into()));
        }
    }
    if st.high_cpu_rounds >= 3 {
        alerts.push(alert("cpu", "warn", "The processor has been busy for a while".into(),
            format!("Programs have been waiting for the processor for over a minute.{}", culprit),
            "If that program isn't doing something you need, close it. Ask me \"what's using the processor?\" for the full list.".into()));
    }
    if total > 0 && (avail as f64) < total as f64 * 0.10 || mem_p.unwrap_or(0.0) > 20.0 {
        let big = top_mem.as_ref().map(|m| format!(" The largest program is {} ({:.1} GB).", m.0, m.1 / 1024.0)).unwrap_or_default();
        alerts.push(alert("memory", if (avail as f64) < total as f64 * 0.05 { "bad" } else { "warn" },
            format!("Memory is running low ({:.1} GB free)", avail as f64 / 1073741824.0),
            format!("When memory runs out, programs slow down or get closed by the system.{}", big),
            "Close programs or browser tabs you aren't using.".into()));
    }
    if let Some((t, free)) = disk {
        if t > 0 && ((free as f64) < t as f64 * 0.05 || free < 2 * 1073741824) {
            alerts.push(alert("disk", if free < 1073741824 { "bad" } else { "warn" }, format!("Disk space is low ({:.1} GB free)", free as f64 / 1073741824.0),
                "Downloads, saving files and updates can fail when the disk is full.".into(),
                "Ask me to \"find large files\" or empty the Trash.".into()));
        }
    }
    if io_p.unwrap_or(0.0) > 40.0 {
        alerts.push(alert("io", "warn", "The disk is very busy".into(),
            "Programs are waiting on the disk, so opening files and apps is slower.".into(),
            "This usually passes once a big copy or install finishes.".into()));
    }
    if let Some((cap, true)) = bat {
        if cap <= 15 {
            alerts.push(alert("battery", if cap <= 7 { "bad" } else { "warn" }, format!("Battery low ({}%)", cap),
                "The computer will turn off when it runs out.".into(), "Plug in the charger and save your work.".into()));
        }
    }
    for u in &failed {
        let name = friendly_unit(u);
        alerts.push(alert(&format!("failed:{}", u), "bad", format!("{} stopped working", name),
            format!("Service {} failed.", u.trim_end_matches(" (user)")), format!("Ask me \"why did {} fail?\" and I'll look at its log.", name)));
    }
    let readings = json!({
        "cpu_temp_c": temp, "cpu_speed_ratio": freq, "throttled": throttled,
        "pressure": { "cpu": cpu_p, "memory": mem_p, "io": io_p },
        "memory": { "total_bytes": total, "available_bytes": avail },
        "disk": disk.map(|(t, f)| json!({ "total_bytes": t, "free_bytes": f })),
        "battery": bat.map(|(c, d)| json!({ "percent": c, "discharging": d })),
        "top_cpu": top_cpu.map(|t| json!({ "name": t.0, "cpu_percent": t.1 })),
        "top_memory": top_mem.map(|m| json!({ "name": m.0, "mb": m.1 })),
        "failed_units": failed,
    });
    (readings, alerts)
}

/// Background loop: refresh every 20 s and notify about new problems.
pub fn spawn() {
    *STATE.lock().unwrap() = Some(State { readings: json!({}), alerts: vec![], notified: HashMap::new(), high_cpu_rounds: 0, last_throttle: None });
    std::thread::spawn(|| loop {
        let mut guard = STATE.lock().unwrap();
        let st = guard.as_mut().unwrap();
        let (readings, alerts) = check(st);
        for a in &alerts {
            let id = a["id"].as_str().unwrap_or("").to_string();
            let due = st.notified.get(&id).map_or(true, |t| t.elapsed() > Duration::from_secs(30 * 60));
            if due {
                crate::files::notify_user(a["title"].as_str().unwrap_or(""), a["advice"].as_str().unwrap_or(""));
                st.notified.insert(id, Instant::now());
            }
        }
        crate::forecast::record(&readings);
        st.readings = readings;
        st.alerts = alerts;
        drop(guard);
        std::thread::sleep(Duration::from_secs(20));
    });
}

pub fn status() -> Value {
    match STATE.lock().unwrap().as_ref() {
        Some(st) => json!({ "readings": st.readings, "alerts": st.alerts }),
        None => json!({ "readings": {}, "alerts": [] }),
    }
}

fn cmd_text(cmd: &str, args: &[&str], max_lines: usize) -> String {
    Command::new(cmd).args(args).output().ok().map(|o| {
        let t = String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr);
        t.lines().rev().take(max_lines).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
    }).unwrap_or_default()
}

/// What the assistant needs to explain a failure: current health, failed
/// services with their recent log, and recent warnings/errors. Logs are
/// outside text, so the caller fences them as untrusted context.
pub fn evidence(about: &str) -> String {
    let st = status();
    let mut out = format!("Current health: {}\nActive alerts: {}\n", st["readings"], st["alerts"]);
    for u in st["readings"]["failed_units"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).take(3) {
        let (unit, user) = (u.trim_end_matches(" (user)"), u.ends_with("(user)"));
        let mut args = vec![];
        if user { args.push("--user"); }
        args.extend(["status", "--no-pager", "-n", "15", unit]);
        out.push_str(&format!("\n--- systemctl status {} ---\n{}\n", unit, cmd_text("systemctl", &args, 25)));
    }
    // the named service, if the question mentions one
    if let Some(unit) = about.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '.' && c != '@'))
        .find(|w| w.ends_with(".service") || w.starts_with("osaima-")) {
        let unit = if unit.contains('.') { unit.to_string() } else { format!("{}.service", unit) };
        out.push_str(&format!("\n--- journal of {} ---\n{}\n", unit, cmd_text("journalctl", &["--user", "-u", &unit, "-n", "30", "--no-pager", "-o", "short"], 30)));
    }
    out.push_str(&format!("\n--- recent warnings and errors (this session) ---\n{}\n",
        cmd_text("journalctl", &["--user", "-p", "warning", "-n", "30", "--no-pager", "-o", "short", "--since", "-30min"], 30)));
    out.push_str(&format!("\n--- recent system errors ---\n{}\n",
        cmd_text("journalctl", &["-p", "err", "-b", "-n", "25", "--no-pager", "-o", "short"], 25)));
    out.chars().take(9000).collect()
}

#[cfg(test)]
mod unit_names {
    #[test]
    fn friendly_unit_names() {
        assert_eq!(super::friendly_unit("app-google\\x2dchrome@2bee4a8ae0f1470e9e.service (user)"), "Google Chrome");
        assert_eq!(super::friendly_unit("NetworkManager.service"), "NetworkManager");
        assert_eq!(super::friendly_unit("osaima-router.service (user)"), "Osaima Router");
    }
}
