//! Automations: OS actions that run later, on a schedule, or on an event.
//!
//!   "shut down when this task finishes"            trigger after_task
//!   "after reboot set brightness to 60%"           trigger on_login (once)
//!   "brightness 40% on weekdays 9 pm to 6 am"      trigger window (restores at the end)
//!   "every day at 9 open the terminal"             trigger daily
//!   "at 23:30 turn the volume down"                trigger at (once)
//!   "remind me to drink water every hour"          trigger every
//!   "when the battery drops below 20%, dim the screen"   trigger battery
//!   "when I unplug the charger, turn on power saving"    trigger power
//!   "when I connect to wifi, open the browser"     trigger network
//!   "when I open the player, turn on do not disturb"     trigger app_open / app_close
//!   "when the download (wget) finishes, shut down" trigger process_exit
//!   "when the computer gets too hot, tell me"      trigger health
//! Any rule may carry only_if conditions: {"power": "battery"|"plugged",
//! "network": "online"|"offline", "app_open": name, "between": ["22:00","06:00"]}.
//! Event triggers fire on the change (not while it stays true) and repeat
//! unless "once" is set; process_exit fires once. With "hold": true the
//! actions last only while the event holds (on battery, app open…) and what
//! they changed is put back afterwards, as for a time window.
//!
//! Rules are approved by the user when created (typed "confirm" when any
//! action is irreversible, e.g. power), saved to ~/.config/osaima/automations.json,
//! and checked every 20 s. Power actions first wait 60 s behind a desktop
//! notification and can be cancelled. Every run is written to the audit log.

use crate::security::AuditLog;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
const POWER_DELAY_SECS: u64 = 60;

static RULES: Mutex<Vec<Value>> = Mutex::new(Vec::new());
/// Power actions counting down: (rule id, when it runs).
static PENDING: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());

fn now_epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Local (weekday 0=Mon, minutes since midnight, yyyy-mm-dd) for an epoch.
fn local(epoch: u64) -> (usize, u32, String) {
    let t = epoch as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    (((tm.tm_wday + 6) % 7) as usize, (tm.tm_hour * 60 + tm.tm_min) as u32,
     format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday))
}

/// "Wed 2026-10-21 21:05", for telling the model what "tonight" means.
pub fn now_text() -> String {
    let (wd, m, date) = local(now_epoch());
    format!("{} {} {:02}:{:02}", ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][wd], date, m / 60, m % 60)
}

/// "2026-10-21T23:30" (local time) to epoch seconds.
fn parse_local(s: &str) -> Option<u64> {
    let (d, t) = s.trim().split_once(['T', ' '])?;
    let mut dp = d.split('-').map(|v| v.parse::<i32>().ok());
    let (y, mo, da) = (dp.next()??, dp.next()??, dp.next()??);
    let mut tp = t.split(':').map(|v| v.parse::<i32>().ok());
    let (h, mi) = (tp.next()??, tp.next()??);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = y - 1900; tm.tm_mon = mo - 1; tm.tm_mday = da; tm.tm_hour = h; tm.tm_min = mi; tm.tm_isdst = -1;
    let e = unsafe { libc::mktime(&mut tm) };
    (e > 0).then_some(e as u64)
}

/// "21:00" to minutes since midnight.
fn hm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

fn days(rule_days: &Value) -> Vec<usize> {
    let list: Vec<String> = rule_days.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_lowercase())).collect()).unwrap_or_default();
    if list.is_empty() || list.iter().any(|d| d == "every" || d == "daily" || d == "all") {
        return (0..7).collect();
    }
    let mut out = Vec::new();
    for d in list {
        match d.as_str() {
            "weekdays" => out.extend(0..5),
            "weekends" => out.extend(5..7),
            _ => if let Some(i) = DAYS.iter().position(|x| d.starts_with(x)) { out.push(i) },
        }
    }
    out
}

/// Whether `now` falls in [from, to) on one of the days; windows may cross midnight
/// (then the day is the day the window started).
fn in_window(epoch: u64, rule_days: &[usize], from: u32, to: u32) -> bool {
    let (wd, mins, _) = local(epoch);
    if from <= to {
        rule_days.contains(&wd) && mins >= from && mins < to
    } else {
        (rule_days.contains(&wd) && mins >= from) || (rule_days.contains(&((wd + 6) % 7)) && mins < to)
    }
}

fn path() -> std::path::PathBuf {
    let dir = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", std::env::var("HOME").unwrap_or_default()));
    std::path::Path::new(&dir).join("osaima/automations.json")
}

fn persist(rules: &[Value]) {
    if let Some(d) = path().parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(path(), serde_json::to_string_pretty(rules).unwrap_or_default());
}

/// Plain-language summary of a rule ("Weekdays 21:00–06:00: brightness set 40").
// ---------- what the machine is doing right now (cheap /sys and /proc reads) ----------

/// (battery percent, on mains power) — None without a battery.
fn battery() -> Option<(u32, bool)> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    let mut pct = None;
    let mut mains = false;
    for e in dir.flatten() {
        let p = e.path();
        let kind = std::fs::read_to_string(p.join("type")).unwrap_or_default();
        match kind.trim() {
            "Battery" => if pct.is_none() {
                pct = std::fs::read_to_string(p.join("capacity")).ok().and_then(|c| c.trim().parse().ok());
            },
            "Mains" | "USB" => mains |= std::fs::read_to_string(p.join("online")).map(|o| o.trim() == "1").unwrap_or(false),
            _ => {}
        }
    }
    pct.map(|p| (p, mains))
}

fn on_mains() -> bool {
    battery().map(|(_, m)| m).unwrap_or(true) // a desktop without a battery is always plugged in
}

/// Online = a default route on a real interface.
fn online() -> bool {
    std::fs::read_to_string("/proc/net/route").map(|t| t.lines().skip(1).any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        f.len() > 1 && f[0] != "lo" && f[1] == "00000000"
    })).unwrap_or(false)
}

fn wifi_name() -> Option<String> {
    let o = std::process::Command::new("nmcli").args(["-t", "-f", "active,ssid", "dev", "wifi"]).output().ok()?;
    String::from_utf8_lossy(&o.stdout).lines().find_map(|l| l.strip_prefix("yes:").map(str::to_string))
}

/// A process with this exact name (or command line containing it) is running.
fn process_running(name: &str) -> bool {
    let name = name.trim().to_lowercase();
    if name.is_empty() { return false; }
    let me = std::process::id().to_string();
    std::fs::read_dir("/proc").map(|d| d.flatten().any(|e| {
        let pid = e.file_name().to_string_lossy().to_string();
        if !pid.chars().all(|c| c.is_ascii_digit()) || pid == me { return false; }
        let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default().trim().to_lowercase();
        if comm == name { return true; }
        let cmd = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
        let cmd = String::from_utf8_lossy(&cmd).replace('\0', " ").to_lowercase();
        !cmd.is_empty() && name.len() >= 4 && cmd.split_whitespace().next().map_or(false, |a| a.ends_with(&format!("/{}", name)) || a == name)
    })).unwrap_or(false)
}

fn health_alert(kind: &str) -> bool {
    crate::health::status()["alerts"].as_array().into_iter().flatten()
        .any(|a| kind.is_empty() || kind == "any" || a["id"].as_str().map_or(false, |id| id.contains(kind)))
}

/// Whether an event trigger's condition holds now.
fn event_state(t: &Value) -> Option<bool> {
    Some(match t["type"].as_str()? {
        "battery" => {
            let (pct, _) = battery()?;
            match (t["below"].as_u64(), t["above"].as_u64()) {
                (Some(b), _) => (pct as u64) < b,
                (_, Some(a)) => (pct as u64) > a,
                _ => return None,
            }
        }
        "power" => on_mains() == (t["state"] == "plugged"),
        "network" => {
            let up = online();
            match t["ssid"].as_str().filter(|s| !s.is_empty()) {
                Some(want) if t["state"] != "disconnected" => up && wifi_name().map_or(false, |n| n.eq_ignore_ascii_case(want)),
                _ => up == (t["state"] != "disconnected"),
            }
        }
        "app_open" => crate::desktop::is_open(t["app"].as_str().unwrap_or("")),
        "app_close" => !crate::desktop::is_open(t["app"].as_str().unwrap_or("")),
        "process_exit" => !process_running(t["process"].as_str().unwrap_or("")),
        "health" => health_alert(t["alert"].as_str().unwrap_or("any")),
        _ => return None,
    })
}

/// only_if conditions; all must hold.
fn conditions_hold(c: &Value, now: u64) -> bool {
    if !c.is_object() { return true; }
    if let Some(p) = c["power"].as_str() { if on_mains() != (p == "plugged") { return false; } }
    if let Some(n) = c["network"].as_str() { if online() != (n == "online") { return false; } }
    if let Some(a) = c["app_open"].as_str() { if !crate::desktop::is_open(a) { return false; } }
    if let Some(b) = c["between"].as_array() {
        let (f, t) = (b.first().and_then(Value::as_str).and_then(hm), b.get(1).and_then(Value::as_str).and_then(hm));
        if let (Some(f), Some(t)) = (f, t) { if !in_window(now, &[0, 1, 2, 3, 4, 5, 6], f, t) { return false; } }
    }
    if let Some(d) = c.get("days").filter(|d| d.is_array()) {
        let (wd, _, _) = local(now);
        if !days(d).contains(&wd) { return false; }
    }
    true
}

const EVENT_TRIGGERS: [&str; 7] = ["battery", "power", "network", "app_open", "app_close", "process_exit", "health"];

/// Corrections from the user's own words that the model may miss: "while /
/// as long as / until" on an event means hold it only while it lasts.
pub fn from_words(query: &str, rule: &mut Value) {
    let q = query.to_lowercase();
    let t = rule["trigger"]["type"].as_str().unwrap_or("").to_string();
    let lasting = ["while ", "as long as", "until ", "only when ", "whilst "].iter().any(|w| q.contains(w));
    if lasting && ["battery", "power", "network", "app_open", "app_close", "health"].contains(&t.as_str())
        && rule["end_actions"].as_array().map_or(true, |a| a.is_empty()) {
        rule["trigger"]["hold"] = json!(true);
    }
}

fn app_words(app: &str) -> String {
    match app {
        "player" => "the video player".into(), "browser" => "the browser".into(), "files" => "the file manager".into(),
        "terminal" => "the terminal".into(), "editor" => "the text editor".into(), "office" => "LibreOffice".into(),
        "archives" => "the archive manager".into(), "" => "the app".into(), a => a.to_string(),
    }
}

fn setting_words(op: &str) -> String {
    match op {
        "dnd" => "Do Not Disturb".into(), "night_light" => "night light".into(), "lock_minutes" => "screen lock time".into(),
        "keyboard" => "keyboard layout".into(), "sound_output" => "sound output".into(), o => o.replace('_', " "),
    }
}

/// One action in plain words: "set brightness to 40%", "turn night light on".
fn say(a: &Value) -> String {
    let action = a["action"].as_str().unwrap_or("").replace('_', " ");
    let op = a["op"].as_str().unwrap_or("").replace('_', " ");
    let num = a["value"].as_f64();
    match (a["action"].as_str().unwrap_or(""), a["op"].as_str().unwrap_or("")) {
        ("volume" | "brightness", "set") => format!("set {} to {}%", action, num.unwrap_or(0.0)),
        ("power", _) => match op.as_str() { "reboot" => "restart the computer".into(), "shutdown" => "shut down the computer".into(), o => format!("{} the computer", o) },
        ("lock_screen", _) => "lock the screen".into(),
        ("notify", _) => format!("show \"{}\"", a["body"].as_str().or(a["title"].as_str()).or(a["text"].as_str()).unwrap_or("a reminder")),
        ("settings", o) => match &a["value"] {
            Value::Bool(b) => format!("turn {} {}", setting_words(o), if *b { "on" } else { "off" }),
            Value::Null => setting_words(o),
            v => format!("set {} to {}", setting_words(o), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())),
        },
        ("app", _) => format!("open {}", app_words(a["app"].as_str().unwrap_or(""))),
        ("window", "close") => format!("close {}", app_words(a["app"].as_str().unwrap_or(""))),
        ("browser", _) => match a["url"].as_str().or(a["query"].as_str()) { Some(u) => format!("open {} in the browser", u), None => "open the browser".into() },
        ("media", o) => format!("{} the music", o.replace("play_pause", "play/pause")),
        ("files", o) => {
            let what = a["paths"].as_array().map(|p| p.iter().filter_map(Value::as_str)
                .map(|x| x.rsplit('/').next().unwrap_or(x).to_string()).collect::<Vec<_>>().join(", "))
                .filter(|w| !w.is_empty()).or(a["path"].as_str().map(str::to_string)).unwrap_or_else(|| "files".into());
            match (o, a["to"].as_str(), a["name"].as_str()) {
                ("move", Some(to), _) => format!("move {} to {}", what, to),
                ("copy", Some(to), _) => format!("copy {} to {}", what, to),
                ("rename", _, Some(n)) => format!("rename {} to {}", what, n),
                ("mkdir", _, _) => format!("make the folder {}", a["path"].as_str().unwrap_or("?")),
                _ => format!("{} {}", o, what),
            }
        }
        ("wifi" | "bluetooth", "on" | "off") => format!("turn {} {}", if action == "wifi" { "Wi-Fi" } else { "Bluetooth" }, op),
        _ => format!("{} {}{}", action, op, num.map(|v| format!(" {}", v)).unwrap_or_default()),
    }
}

fn mins_text(m: u64) -> String {
    if m % 60 == 0 { let h = m / 60; if h == 1 { "hour".into() } else { format!("{} hours", h) } } else { format!("{} minutes", m) }
}

pub fn describe(rule: &Value) -> String {
    let acts = |key: &str| rule[key].as_array().into_iter().flatten().map(say).collect::<Vec<_>>().join(", ");
    let t = &rule["trigger"];
    let day_names = |v: &Value| {
        let d = days(v);
        if d.len() == 7 { "Every day".to_string() } else if d == vec![0, 1, 2, 3, 4] { "Weekdays".into() } else if d == vec![5, 6] { "Weekends".into() }
        else { d.iter().map(|i| { let s = DAYS[*i]; format!("{}{}", s[..1].to_uppercase(), &s[1..]) }).collect::<Vec<_>>().join(", ") }
    };
    let when = match t["type"].as_str().unwrap_or("") {
        "at" => format!("At {}", t["at"].as_str().unwrap_or("?").replace('T', " ")),
        "daily" => format!("{} at {}", day_names(&t["days"]), t["time"].as_str().unwrap_or("?")),
        "window" => format!("{} {}–{}", day_names(&t["days"]), t["from"].as_str().unwrap_or("?"), t["to"].as_str().unwrap_or("?")),
        "after_task" => "When the current task finishes".into(),
        "on_login" => if t["every"].as_bool().unwrap_or(false) { "At every login".into() } else { "At the next login".into() },
        "every" => format!("Every {}", mins_text(t["minutes"].as_u64().unwrap_or(60))),
        "battery" => match (t["below"].as_u64(), t["above"].as_u64()) {
            (Some(b), _) => format!("When the battery drops below {}%", b),
            (_, Some(a)) => format!("When the battery charges above {}%", a),
            _ => "When the battery changes".into(),
        },
        "power" => match (t["state"] == "plugged", t["hold"] == json!(true)) {
            (true, true) => "While plugged in".into(), (false, true) => "While on battery".into(),
            (true, false) => "When the charger is plugged in".into(), (false, false) => "When the charger is unplugged".into(),
        },
        "network" => match (t["state"].as_str(), t["ssid"].as_str()) {
            (Some("disconnected"), _) => "When the internet disconnects".into(),
            (_, Some(n)) if !n.is_empty() => format!("When connected to Wi-Fi \"{}\"", n),
            _ => "When the internet connects".into(),
        },
        "app_open" => format!("{} {} is open", if t["hold"] == json!(true) { "While" } else { "When" }, app_words(t["app"].as_str().unwrap_or(""))),
        "app_close" => format!("When {} closes", app_words(t["app"].as_str().unwrap_or(""))),
        "process_exit" => format!("When {} finishes", t["process"].as_str().unwrap_or("the program")),
        "health" => match t["alert"].as_str().unwrap_or("any") {
            "any" | "" => "When the health watch warns about something".into(),
            k if k.contains("temp") || k.contains("heat") || k.contains("hot") => "When the computer gets too hot".into(),
            k if k.contains("cpu") || k.contains("throttl") => "When the processor is overloaded or slowed down".into(),
            k if k.contains("mem") => "When memory runs low".into(),
            k if k.contains("disk") => "When the disk is nearly full".into(),
            k => format!("When the health watch warns about {}", k.replace('_', " ")),
        },
        other => other.to_string(),
    };
    let c = &rule["only_if"];
    let mut cond = vec![];
    if let Some(p) = c["power"].as_str() { cond.push(if p == "plugged" { "plugged in".to_string() } else { "on battery".into() }); }
    if let Some(n) = c["network"].as_str() { cond.push(if n == "online" { "online".to_string() } else { "offline".into() }); }
    if let Some(a) = c["app_open"].as_str() { cond.push(format!("{} is open", a)); }
    if let Some(b) = c["between"].as_array() { cond.push(format!("between {} and {}", b.first().and_then(Value::as_str).unwrap_or("?"), b.get(1).and_then(Value::as_str).unwrap_or("?"))); }
    if c.get("days").map_or(false, |d| d.is_array()) { cond.push(day_names(&c["days"]).to_lowercase()); }
    let when = if cond.is_empty() { when } else { format!("{} (only when {})", when, cond.join(", ")) };
    let mut s = format!("{}: {}", when, acts("actions"));
    if t["type"] == "window" || t["hold"] == json!(true) {
        s.push_str(if rule["end_actions"].as_array().map_or(false, |a| !a.is_empty()) { "; at the end: " } else { "; back to before at the end" });
        s.push_str(&acts("end_actions"));
    }
    s
}

/// Checks a proposed rule: known trigger, sane times, only actions the
/// daemon's OS policy knows. Returns (normalised rule, needs typed confirm).
pub fn validate(rule: &Value) -> Result<(Value, bool), String> {
    let t = &rule["trigger"];
    match t["type"].as_str().unwrap_or("") {
        "at" => { parse_local(t["at"].as_str().unwrap_or("")).ok_or("the time should look like 2026-10-21T23:30")?; }
        "daily" => { hm(t["time"].as_str().unwrap_or("")).ok_or("the time should look like 21:00")?; }
        "window" => {
            hm(t["from"].as_str().unwrap_or("")).ok_or("the start should look like 21:00")?;
            hm(t["to"].as_str().unwrap_or("")).ok_or("the end should look like 06:00")?;
        }
        "after_task" | "on_login" => {}
        "every" => { t["minutes"].as_u64().filter(|m| (5..=1440).contains(m)).ok_or("repeat every 5 minutes to 24 hours")?; }
        "battery" => { (t["below"].as_u64().or(t["above"].as_u64())).filter(|p| (1..=99).contains(p)).ok_or("give a battery level between 1 and 99%")?; }
        "power" => { ["plugged", "unplugged"].contains(&t["state"].as_str().unwrap_or("")).then_some(()).ok_or("power state is plugged or unplugged")?; }
        "network" => { ["connected", "disconnected"].contains(&t["state"].as_str().unwrap_or("connected")).then_some(()).ok_or("network state is connected or disconnected")?; }
        "app_open" | "app_close" => { t["app"].as_str().filter(|a| !a.trim().is_empty()).ok_or("say which app")?; }
        "process_exit" => {
            let p = t["process"].as_str().filter(|p| !p.trim().is_empty()).ok_or("say which program to wait for")?;
            if !process_running(p) { return Err(format!("'{}' isn't running now, so there is nothing to wait for", p)); }
        }
        "health" => {}
        other => return Err(format!("unknown trigger '{}'", other)),
    }
    let actions = rule["actions"].as_array().filter(|a| !a.is_empty() && a.len() <= 10).ok_or("an automation needs 1 to 10 actions")?;
    let allowed = ["volume", "brightness", "media", "lock_screen", "power", "wifi", "bluetooth", "notify", "settings", "app", "window",
                   "workspace", "layout", "browser", "files"];
    let mut typed = false;
    for a in actions.iter().chain(rule["end_actions"].as_array().into_iter().flatten()) {
        let action = a["action"].as_str().unwrap_or("");
        if !allowed.contains(&action) {
            return Err(format!("automations can't do '{}'", action));
        }
        // file changes later, unattended: only the undoable ones
        if action == "files" && !["move", "copy", "mkdir", "rename"].contains(&a["op"].as_str().unwrap_or("")) {
            return Err("automations can move, copy or rename files and make folders, but never delete".into());
        }
        let (tier, _) = crate::os_control::policy(action, a["op"].as_str().unwrap_or(""));
        typed |= tier == crate::security::Tier::Irreversible;
    }
    let mut r = rule.clone();
    r["id"] = json!(format!("a{}", SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)));
    r["enabled"] = json!(true);
    r["created"] = json!(now_epoch());
    // event triggers start from what is true now, so they fire on the next change
    if EVENT_TRIGGERS.contains(&t["type"].as_str().unwrap_or("")) {
        r["was"] = json!(event_state(t).unwrap_or(false));
    }
    if t["type"] == "every" { r["next"] = json!(now_epoch() + t["minutes"].as_u64().unwrap_or(60) * 60); }
    r["summary"] = json!(describe(&r));
    Ok((r, typed))
}

pub fn add(rule: Value) -> Value {
    let mut rules = RULES.lock().unwrap();
    rules.push(rule.clone());
    persist(&rules);
    rule
}

pub fn list() -> Value {
    let pending: Vec<Value> = PENDING.lock().unwrap().iter().map(|(id, at)| json!({ "id": id, "runs_in": at.saturating_sub(now_epoch()) })).collect();
    json!({ "automations": *RULES.lock().unwrap(), "pending": pending })
}

pub fn remove(id: &str) -> Result<Value, String> {
    let mut rules = RULES.lock().unwrap();
    let before = rules.len();
    rules.retain(|r| r["id"] != json!(id));
    PENDING.lock().unwrap().retain(|(p, _)| p != id);
    if rules.len() == before {
        return Err("no such automation".into());
    }
    persist(&rules);
    Ok(json!({ "removed": id }))
}

/// Cancels power actions that are counting down (the notification's "Cancel").
pub fn cancel_pending() -> Value {
    let n = std::mem::take(&mut *PENDING.lock().unwrap()).len();
    json!({ "cancelled": n })
}

/// Called by the task engine when a task ends successfully.
pub fn task_finished() {
    let mut rules = RULES.lock().unwrap();
    for r in rules.iter_mut().filter(|r| r["enabled"] == json!(true) && r["trigger"]["type"] == "after_task") {
        r["fire"] = json!(true);
    }
}

/// The action that puts back what `a` is about to change, read from the
/// current state: volume/brightness levels, Wi-Fi/Bluetooth on or off, and any
/// desktop setting (night light, do not disturb, wallpaper, keyboard, lock time…).
async fn undo_for(a: &Value, run: &Runner) -> Option<Value> {
    let action = a["action"].as_str()?;
    let op = a["op"].as_str().unwrap_or("");
    match action {
        "volume" | "brightness" if op == "set" || op == "up" || op == "down" || op == "mute" => {
            let v = run(json!({ "action": action, "op": "get" })).await.ok()?;
            let p = v["percent"].as_f64().or(v["result"]["percent"].as_f64())?;
            let mut back = json!({ "action": action, "op": "set", "value": p });
            if action == "volume" && op == "mute" { back = json!({ "action": "volume", "op": "unmute" }); }
            Some(back)
        }
        "wifi" | "bluetooth" if op == "on" || op == "off" => {
            let v = run(json!({ "action": action, "op": "status" })).await.ok()?;
            let on = v["wifi_enabled"].as_bool().or(v["bluetooth_on"].as_bool())
                .or(v["result"]["wifi_enabled"].as_bool()).or(v["result"]["bluetooth_on"].as_bool())?;
            Some(json!({ "action": action, "op": if on { "on" } else { "off" } }))
        }
        "settings" => {
            let v = run(json!({ "action": "settings", "op": "get" })).await.ok()?;
            let v = if v["result"].is_object() { v["result"].clone() } else { v };
            let field = match op { "keyboard" => "layout", o => o };
            let cur = v.get(field).filter(|c| !c.is_null() && !c.is_object() && !c.is_array())?;
            Some(json!({ "action": "settings", "op": op, "value": cur }))
        }
        _ => None,
    }
}

pub type Runner = Arc<dyn Fn(Value) -> futures::future::BoxFuture<'static, Result<Value, String>> + Send + Sync>;

async fn run_actions(rule: &Value, key: &str, run: &Runner, audit: &Arc<AuditLog>) -> Vec<String> {
    let mut problems = Vec::new();
    for a in rule[key].as_array().cloned().unwrap_or_default() {
        let res = run(a.clone()).await;
        audit.record("automation_run", json!({ "automation": rule["summary"], "action": a, "ok": res.is_ok(),
                                                 "error": res.as_ref().err() }));
        if let Err(e) = res {
            problems.push(e);
        }
    }
    problems
}

/// Loads saved rules and checks them every 20 s. `run` performs one OS action
/// (already approved when the rule was created); `audit` records each run.
pub fn spawn(run: Runner, audit: Arc<AuditLog>) {
    if let Ok(text) = std::fs::read_to_string(path()) {
        if let Ok(Value::Array(saved)) = serde_json::from_str::<Value>(&text) {
            *RULES.lock().unwrap() = saved;
        }
    }
    // this start of the daemon is a login: on_login rules fire once the desktop is up
    for r in RULES.lock().unwrap().iter_mut().filter(|r| r["trigger"]["type"] == "on_login" && r["enabled"] == json!(true)) {
        r["fire"] = json!(true);
    }
    tokio::spawn(async move {
        loop {
            let now = now_epoch();
            let (_, _, today) = local(now);
            let mut due: Vec<(Value, &'static str)> = Vec::new();
            {
                let mut rules = RULES.lock().unwrap();
                for r in rules.iter_mut().filter(|r| r["enabled"] == json!(true)) {
                    let t = r["trigger"].clone();
                    match t["type"].as_str().unwrap_or("") {
                        "at" => if parse_local(t["at"].as_str().unwrap_or("")).map_or(false, |at| now >= at) {
                            r["enabled"] = json!(false);
                            due.push((r.clone(), "actions"));
                        },
                        "daily" => {
                            let (wd, mins, _) = local(now);
                            let at = hm(t["time"].as_str().unwrap_or("")).unwrap_or(0);
                            if days(&t["days"]).contains(&wd) && mins >= at && mins < at + 5 && r["last_day"] != json!(today) {
                                r["last_day"] = json!(today.clone());
                                due.push((r.clone(), "actions"));
                            }
                        }
                        "window" => {
                            let inside = in_window(now, &days(&t["days"]), hm(t["from"].as_str().unwrap_or("")).unwrap_or(0), hm(t["to"].as_str().unwrap_or("")).unwrap_or(0));
                            let active = r["active"].as_bool().unwrap_or(false);
                            if inside && !active { r["active"] = json!(true); due.push((r.clone(), "actions")); }
                            if !inside && active { r["active"] = json!(false); due.push((r.clone(), "end_actions")); }
                        }
                        "after_task" | "on_login" => if r["fire"] == json!(true) && (t["type"] != "on_login" || crate::desktop::available()) {
                            r["fire"] = json!(false);
                            if !t["every"].as_bool().unwrap_or(false) { r["enabled"] = json!(false); }
                            due.push((r.clone(), "actions"));
                        },
                        "every" => if r["next"].as_u64().map_or(true, |n| now >= n) {
                            r["next"] = json!(now + t["minutes"].as_u64().unwrap_or(60) * 60);
                            due.push((r.clone(), "actions"));
                        },
                        k if EVENT_TRIGGERS.contains(&k) => if let Some(state) = event_state(&t) {
                            let was = r["was"].as_bool().unwrap_or(false);
                            r["was"] = json!(state);
                            if state && !was {
                                if k == "process_exit" || t["once"].as_bool().unwrap_or(false) { r["enabled"] = json!(false); }
                                due.push((r.clone(), "actions"));
                            }
                            // "hold": only while it lasts, then put things back (like a time window)
                            if !state && was && t["hold"] == json!(true) {
                                due.push((r.clone(), "end_actions"));
                            }
                        },
                        _ => {}
                    }
                }
                // only_if: a rule whose conditions don't hold skips this time (a window
                // still ends, so it always puts things back)
                due.retain(|(r, key)| *key == "end_actions" || conditions_hold(&r["only_if"], now));
                persist(&rules);
            }
            for (rule, key) in due {
                let power = rule[key].as_array().into_iter().flatten().any(|a| a["action"] == "power");
                if power {
                    // a visible, cancellable countdown before anything turns off
                    let id = rule["id"].as_str().unwrap_or("").to_string();
                    PENDING.lock().unwrap().push((id.clone(), now + POWER_DELAY_SECS));
                    crate::files::notify_user("Automation: power action in 60 seconds",
                        &format!("{}. Say \"cancel\" to the assistant or remove it under Automations.", rule["summary"].as_str().unwrap_or("")));
                    let (run, audit) = (run.clone(), audit.clone());
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(POWER_DELAY_SECS)).await;
                        let still = PENDING.lock().unwrap().iter().any(|(p, _)| *p == id);
                        PENDING.lock().unwrap().retain(|(p, _)| *p != id);
                        if still {
                            run_actions(&rule, key, &run, &audit).await;
                        } else {
                            audit.record("automation_cancelled", json!({ "automation": rule["summary"] }));
                        }
                    });
                    continue;
                }
                // a window without its own end actions puts back what it changed
                let mut rule = rule;
                let mut key = key;
                let is_window = rule["trigger"]["type"] == "window" || rule["trigger"]["hold"] == json!(true);
                let no_end = rule["end_actions"].as_array().map_or(true, |a| a.is_empty());
                if is_window && no_end && key == "actions" {
                    let mut restore = Vec::new();
                    for a in rule["actions"].as_array().cloned().unwrap_or_default() {
                        if let Some(back) = undo_for(&a, &run).await {
                            restore.push(back);
                        }
                    }
                    let id = rule["id"].clone();
                    let mut rules = RULES.lock().unwrap();
                    if let Some(r) = rules.iter_mut().find(|r| r["id"] == id) {
                        r["restore"] = json!(restore);
                    }
                    persist(&rules);
                }
                if is_window && no_end && key == "end_actions" {
                    let id = rule["id"].clone();
                    if let Some(saved) = RULES.lock().unwrap().iter().find(|r| r["id"] == id) {
                        rule["end_actions"] = saved["restore"].clone();
                    }
                    key = "end_actions";
                }
                let problems = run_actions(&rule, key, &run, &audit).await;
                let summary = rule["summary"].as_str().unwrap_or("Automation");
                if problems.is_empty() {
                    crate::files::notify_user("Automation ran", summary);
                } else {
                    crate::files::notify_user("Automation couldn't finish", &format!("{}: {}", summary, problems.join("; ")));
                }
            }
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_and_windows() {
        assert_eq!(days(&json!(["weekdays"])), vec![0, 1, 2, 3, 4]);
        assert_eq!(days(&json!([])).len(), 7);
        assert_eq!(hm("21:30"), Some(1290));
        assert_eq!(hm("25:00"), None);
    }

    #[test]
    fn event_rules_validate_and_describe() {
        let r = json!({ "trigger": { "type": "battery", "below": 20 }, "actions": [{ "action": "brightness", "op": "set", "value": 30 }],
                        "only_if": { "power": "battery" } });
        let (v, typed) = validate(&r).unwrap();
        assert!(!typed);
        assert_eq!(v["summary"], "When the battery drops below 20% (only when on battery): set brightness to 30%");
        let e = json!({ "trigger": { "type": "every", "minutes": 60 }, "actions": [{ "action": "notify", "title": "Break", "body": "Drink water" }] });
        assert_eq!(validate(&e).unwrap().0["summary"], "Every hour: show \"Drink water\"");
        let h = json!({ "trigger": { "type": "power", "state": "unplugged", "hold": true }, "actions": [{ "action": "brightness", "op": "set", "value": 50 }] });
        assert_eq!(validate(&h).unwrap().0["summary"], "While on battery: set brightness to 50%; back to before at the end");
        let mut w = json!({ "trigger": { "type": "power", "state": "unplugged" }, "actions": [] });
        from_words("while I'm on battery keep brightness at 50%", &mut w);
        assert_eq!(w["trigger"]["hold"], true);
        let f = json!({ "trigger": { "type": "daily", "time": "18:00", "days": ["fri"] },
                        "actions": [{ "action": "files", "op": "move", "paths": ["~/Downloads/report.pdf"], "to": "~/Documents" }] });
        assert_eq!(validate(&f).unwrap().0["summary"], "Fri at 18:00: move report.pdf to ~/Documents");
        assert!(validate(&json!({ "trigger": { "type": "every", "minutes": 1 }, "actions": [{ "action": "notify" }] })).is_err());
        assert!(validate(&json!({ "trigger": { "type": "process_exit", "process": "no-such-program-xyz" }, "actions": [{ "action": "notify" }] })).is_err());
        assert!(validate(&json!({ "trigger": { "type": "app_open", "app": "player" }, "actions": [{ "action": "settings", "op": "dnd", "value": true }] })).is_ok());
        assert!(validate(&json!({ "trigger": { "type": "power", "state": "unplugged" }, "actions": [{ "action": "files", "op": "search" }] })).is_err());
        assert!(validate(&json!({ "trigger": { "type": "health", "alert": "temp" }, "actions": [{ "action": "execute_command" }] })).is_err());
    }

    #[test]
    fn conditions() {
        let now = now_epoch();
        assert!(conditions_hold(&json!(null), now));
        assert!(conditions_hold(&json!({ "between": ["00:00", "23:59"] }), now) || local(now).1 >= 23 * 60 + 59);
        let other_day = DAYS[(local(now).0 + 1) % 7];
        assert!(!conditions_hold(&json!({ "days": [other_day] }), now));
    }

    #[test]
    fn validation() {
        let ok = json!({ "trigger": { "type": "window", "days": ["weekdays"], "from": "21:00", "to": "06:00" },
                         "actions": [{ "action": "brightness", "op": "set", "value": 40 }] });
        let (r, typed) = validate(&ok).unwrap();
        assert!(!typed);
        assert!(r["summary"].as_str().unwrap().starts_with("Weekdays 21:00–06:00"));
        let power = json!({ "trigger": { "type": "after_task" }, "actions": [{ "action": "power", "op": "shutdown" }] });
        assert!(validate(&power).unwrap().1, "shutdown needs the typed confirm");
        assert!(validate(&json!({ "trigger": { "type": "at", "at": "soon" }, "actions": [{ "action": "volume", "op": "mute" }] })).is_err());
        assert!(validate(&json!({ "trigger": { "type": "daily", "time": "09:00" }, "actions": [{ "action": "files", "op": "trash" }] })).is_err());
    }
}
