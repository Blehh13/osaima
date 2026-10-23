//! Real window management through sway's IPC (swaymsg): list, focus, close,
//! launch, layout and workspaces, so the assistant acts on the actual desktop
//! rather than on panels inside its own page.
//!
//! Reached through system.os_action (actions "window", "app", "layout",
//! "workspace"), so the same approval policy, taint checks and audit log apply.
//! Apps are launched only from the fixed table below, never from model output.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

/// App name (as the intent router and the model use them) -> command, and the
/// app_id/title fragments that identify its windows.
const APPS: &[(&str, &str, &[&str])] = &[
    ("terminal", "foot", &["foot"]),
    ("browser", "firefox", &["firefox"]),
    ("files", "thunar", &["thunar"]),
    ("editor", "mousepad", &["mousepad"]),
    ("office", "libreoffice-bin", &["libreoffice", "soffice"]),
    // gpu output first, plain software (wlshm) where there is no GPU (VMs)
    ("player", "mpv --player-operation-mode=pseudo-gui --vo=gpu,wlshm", &["mpv"]),
    ("archives", "xarchiver", &["xarchiver"]),
    ("assistant", "osaima-app assistant", &["osaima-assistant"]),
    ("launcher", "osaima-app launcher", &["osaima-launcher"]),
    ("settings", "osaima-app settings", &["osaima-settings"]),
    ("config", "osaima-app settings", &["osaima-settings"]),
    ("monitor", "osaima-app monitor", &["osaima-monitor"]),
    ("taskmanager", "osaima-app taskmanager", &["osaima-taskmanager"]),
    ("notes", "osaima-app notes", &["osaima-notes"]),
    ("calendar", "osaima-app calendar", &["osaima-calendar"]),
    ("knowledge", "osaima-app knowledge", &["osaima-knowledge"]),
    ("behavior", "osaima-app behavior", &["osaima-behavior"]),
    ("about", "osaima-app about", &["osaima-about"]),
    ("history", "osaima-app history", &["osaima-history"]),
    ("security", "osaima-app security", &["osaima-security"]),
    ("onboarding", "osaima-app onboarding", &["osaima-onboarding"]),
];

/// sway's IPC socket. The daemon runs as a user service, so SWAYSOCK is often
/// not in its environment: fall back to the newest socket in the runtime dir.
fn socket() -> Option<PathBuf> {
    if let Ok(s) = std::env::var("SWAYSOCK") {
        return Some(PathBuf::from(s));
    }
    let dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    std::fs::read_dir(dir).ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("sway-ipc."))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

pub fn socket_path() -> Option<PathBuf> {
    socket()
}

pub fn available() -> bool {
    socket().is_some()
}

fn swaymsg(args: &[&str]) -> Result<Value, String> {
    let sock = socket().ok_or("no sway session is running")?;
    let out = Command::new("swaymsg").arg("-s").arg(&sock).arg("-r").args(args).output()
        .map_err(|e| format!("swaymsg: {}", e))?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("swaymsg output: {}", e))?;
    // command replies are [{"success": bool, "error": "..."}]
    if let Some(err) = v.as_array().and_then(|a| a.iter().find(|r| r["success"] == json!(false))) {
        return Err(err["error"].as_str().unwrap_or("sway refused the command").to_string());
    }
    Ok(v)
}

fn command(cmd: &str) -> Result<Value, String> {
    swaymsg(&[cmd])
}

/// Start a program in the desktop session (sway's exec), e.g. a Flatpak app.
pub fn exec(cmdline: &str) -> Result<Value, String> {
    command(&format!("exec {}", cmdline))
}

/// Every application window: id, app, title, workspace, focused.
pub fn windows() -> Result<Vec<Value>, String> {
    fn walk(node: &Value, ws: &str, out: &mut Vec<Value>) {
        let ws = if node["type"] == "workspace" { node["name"].as_str().unwrap_or(ws) } else { ws };
        let is_window = node["pid"].is_number() && (node["type"] == "con" || node["type"] == "floating_con");
        if is_window {
            let app = node["app_id"].as_str()
                .or_else(|| node["window_properties"]["class"].as_str())
                .unwrap_or("").to_string();
            out.push(json!({ "id": node["id"], "app": app, "title": node["name"],
                             "workspace": ws, "focused": node["focused"] }));
        }
        for key in ["nodes", "floating_nodes"] {
            for child in node[key].as_array().into_iter().flatten() {
                walk(child, ws, out);
            }
        }
    }
    let tree = swaymsg(&["-t", "get_tree"])?;
    let mut out = Vec::new();
    walk(&tree, "", &mut out);
    // the scratchpad holds hidden windows; they are not "open" for the user
    out.retain(|w| w["workspace"] != "__i3_scratch");
    Ok(out)
}

/// Windows that belong to `app` (an APPS name, else matched against app_id and title).
fn matching(app: &str) -> Result<Vec<Value>, String> {
    let app = app.trim().to_lowercase();
    let keys: Vec<String> = APPS.iter().find(|(name, _, _)| *name == app)
        .map(|(_, _, ids)| ids.iter().map(|s| s.to_string()).collect())
        .unwrap_or_else(|| vec![app.clone()]);
    Ok(windows()?.into_iter().filter(|w| {
        let hay = norm(&format!("{} {}", w["app"].as_str().unwrap_or(""), w["title"].as_str().unwrap_or("")));
        keys.iter().any(|k| hay.contains(norm(k).as_str()))
    }).collect())
}

/// Lower-case, hyphens as spaces: OSAIMA windows are found by app_id or by
/// their title ("OSAIMA Assistant"), whichever the toolkit exposes.
fn norm(s: &str) -> String {
    s.to_lowercase().replace('-', " ")
}

fn is_osaima_assistant(w: &Value) -> bool {
    norm(&format!("{} {}", w["app"].as_str().unwrap_or(""), w["title"].as_str().unwrap_or(""))).contains("osaima assistant")
}

/// Percent-encodes everything except ASCII letters and digits, so the result
/// can sit inside a shell word without any quoting concerns.
fn url_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{:02X}", b) }).collect()
}

fn by_id(id: &Value, cmd: &str) -> Result<Value, String> {
    command(&format!("[con_id={}] {}", id, cmd))
}

/// The names people and models use for an app ("archive manager",
/// "spreadsheet", "web browser") mapped onto the launch table's names.
/// Whether a window of this app (any name the assistant understands) is open.
pub fn is_open(app: &str) -> bool {
    let name = canonical(app);
    let frags: Vec<String> = APPS.iter().find(|(n, _, _)| *n == name)
        .map(|(_, _, f)| f.iter().map(|s| s.to_string()).collect())
        .unwrap_or_else(|| vec![app.trim().to_lowercase()]);
    windows().map(|ws| ws.iter().any(|w| {
        let (id, title) = (w["app"].as_str().unwrap_or("").to_lowercase(), w["title"].as_str().unwrap_or("").to_lowercase());
        frags.iter().any(|f| id.contains(f.as_str()) || (f.starts_with("osaima-") && title == f.replacen('-', " ", 1)))
    })).unwrap_or(false)
}

fn canonical(app: &str) -> String {
    let a = app.trim().to_lowercase();
    if APPS.iter().any(|(n, _, _)| *n == a) {
        return a;
    }
    const ALIASES: &[(&str, &[&str])] = &[
        ("files", &["file manager", "file explorer", "thunar", "folders", "explorer"]),
        ("terminal", &["foot", "console", "shell", "command line"]),
        ("browser", &["firefox", "web browser", "internet", "web"]),
        ("editor", &["text editor", "mousepad", "notepad"]),
        ("office", &["libreoffice", "writer", "word processor", "spreadsheet", "calc", "impress", "presentation", "documents"]),
        ("player", &["mpv", "video player", "media player", "music player", "video", "music"]),
        ("archives", &["archive manager", "archive", "xarchiver", "zip", "zip tool"]),
        ("monitor", &["system monitor", "task manager", "htop", "resource monitor"]),
    ];
    ALIASES.iter().find(|(_, names)| names.iter().any(|n| a == *n || a.contains(n)))
        .map(|(c, _)| c.to_string()).unwrap_or(a)
}

pub fn perform(action: &str, op: &str, params: &Value) -> Result<Value, String> {
    let canon = canonical(params["app"].as_str().unwrap_or(""));
    let app = canon.as_str();
    let n = params["value"].as_f64().map(|v| v as i64);
    match (action, op) {
        ("window", "list") => Ok(json!({ "windows": windows()? })),
        ("window", "focus") => {
            let w = matching(app)?.into_iter().next().ok_or(format!("no {} window is open", app))?;
            by_id(&w["id"], "focus")?;
            Ok(json!({ "focused": w }))
        }
        ("window", "close") => {
            // no app: the focused window, unless that is the assistant itself
            let targets = if app.is_empty() {
                windows()?.into_iter().filter(|w| w["focused"] == json!(true) && !is_osaima_assistant(w)).collect()
            } else {
                matching(app)?
            };
            for w in &targets {
                by_id(&w["id"], "kill")?;
            }
            Ok(json!({ "closed": targets.len() }))
        }
        ("window", "close_all") => {
            // keep the assistant: closing it would leave the user without the shell
            let targets: Vec<Value> = windows()?.into_iter().filter(|w| !is_osaima_assistant(w)).collect();
            for w in &targets {
                by_id(&w["id"], "kill")?;
            }
            Ok(json!({ "closed": targets.len() }))
        }
        ("window", "fullscreen") => { command("fullscreen toggle")?; Ok(json!({ "fullscreen": "toggled" })) }
        ("window", "float") => { command("floating toggle")?; Ok(json!({ "floating": "toggled" })) }
        ("app", "launch") => {
            // an already-open window is brought forward instead of starting a second copy
            if let Some(w) = matching(app)?.into_iter().next() {
                by_id(&w["id"], "focus")?;
                return Ok(json!({ "focused": w }));
            }
            let (_, cmd, _) = APPS.iter().find(|(name, _, _)| *name == app)
                .ok_or(format!("I don't know how to open '{}'", app))?;
            let bin = cmd.split_whitespace().next().unwrap_or("");
            if !Command::new("sh").args(["-c", &format!("command -v {}", bin)]).output().map_or(false, |o| o.status.success()) {
                return Err(format!("{} is not installed", bin));
            }
            // quoted: sway would otherwise split a command at commas (mpv --vo=gpu,wlshm)
            command(&format!("exec '{}'", cmd))?;
            Ok(json!({ "launched": app }))
        }
        ("browser", "search") => {
            let q = params["query"].as_str().unwrap_or("").trim();
            if q.is_empty() {
                return Err("nothing to search for".into());
            }
            command(&format!("exec firefox --new-tab https://duckduckgo.com/?q={}", url_encode(q)))?;
            Ok(json!({ "searching": q }))
        }
        ("layout", layout) => {
            let cmd = match layout {
                "tile" | "split" => "layout splith",
                "split_v" | "grid" => "layout splitv",
                "monocle" | "tabbed" => "layout tabbed",
                "stacked" | "spiral" => "layout stacking",
                "float" => "floating toggle",
                other => return Err(format!("unknown layout '{}'", other)),
            };
            command(cmd)?;
            Ok(json!({ "layout": layout }))
        }
        ("workspace", "switch") => {
            let n = n.filter(|n| (1..=10).contains(n)).ok_or("workspace must be 1-10")?;
            command(&format!("workspace number {}", n))?;
            Ok(json!({ "workspace": n }))
        }
        ("workspace", "move") => {
            let n = n.filter(|n| (1..=10).contains(n)).ok_or("workspace must be 1-10")?;
            let id = if app.is_empty() { None } else { matching(app)?.into_iter().next().map(|w| w["id"].clone()) };
            match id {
                Some(id) => by_id(&id, &format!("move container to workspace number {}", n))?,
                None => command(&format!("move container to workspace number {}", n))?,
            };
            Ok(json!({ "moved_to": n }))
        }
        _ => Err(format!("unknown desktop action '{} {}'", action, op)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_table_has_no_shell_metacharacters() {
        for (_, cmd, _) in APPS {
            assert!(!cmd.contains([';', '&', '|', '$', '`', '>', '<', '\'', '"']), "{}", cmd);
        }
    }

    #[test]
    fn search_queries_cannot_reach_the_shell() {
        let e = url_encode("x'; rm -rf ~ #$(id)`");
        assert!(e.chars().all(|c| c.is_ascii_alphanumeric() || c == '%'), "{}", e);
    }

    #[test]
    fn natural_app_names() {
        assert_eq!(canonical("Archive Manager"), "archives");
        assert_eq!(canonical("spreadsheet"), "office");
        assert_eq!(canonical("the video player"), "player");
        assert_eq!(canonical("files"), "files");
        assert_eq!(canonical("something else"), "something else");
    }

    #[test]
    fn unknown_actions_are_errors() {
        assert!(perform("window", "explode", &json!({})).is_err());
        assert!(perform("layout", "rm -rf", &json!({})).is_err());
    }
}
