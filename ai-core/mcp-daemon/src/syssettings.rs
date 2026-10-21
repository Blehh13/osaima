//! System settings for the Settings app and the assistant: wallpaper, display
//! resolution and scale, keyboard layout, network status. Applied through
//! sway, saved to ~/.config/osaima/desktop.json and reapplied at login by
//! osaima-apply-prefs. Reached through system.os_action (action "settings"),
//! so approval and the audit log apply as for every other action.

use serde_json::{json, Value};

use std::process::Command;

/// The night light process this daemon started, kept so it can be reaped.
static NIGHT: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

/// Wallpapers that ship with OSAIMA (desktop/wallpaper/osaima-<id>.png).
const WALLPAPERS: &[(&str, &str)] = &[("contours", "Contours"), ("dawn", "Dawn"), ("deep", "Deep")];

/// Keyboard layouts offered (xkb layout[:variant], label).
const LAYOUTS: &[(&str, &str)] = &[
    ("us", "English (US)"), ("gb", "English (UK)"), ("in:kan", "Kannada"), ("in", "Hindi"),
    ("in:tel", "Telugu"), ("in:tam", "Tamil"), ("in:mal", "Malayalam"), ("de", "German"), ("fr", "French"),
];

const SCALES: &[f64] = &[1.0, 1.25, 1.5, 1.75, 2.0];

fn desktop_dir() -> String {
    std::env::var("OSAIMA_DESKTOP").unwrap_or_else(|_| "/usr/share/osaima/desktop".into())
}

fn prefs_path() -> std::path::PathBuf {
    let dir = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", std::env::var("HOME").unwrap_or_default()));
    std::path::Path::new(&dir).join("osaima/desktop.json")
}

fn load() -> Value {
    std::fs::read_to_string(prefs_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}))
}

fn save(key: &str, value: Value) -> Result<(), String> {
    let mut p = load();
    p[key] = value;
    let path = prefs_path();
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&p).unwrap_or_default()).map_err(|e| e.to_string())
}

fn swaymsg_json(args: &[&str]) -> Result<Value, String> {
    let sock = crate::desktop::socket_path().ok_or("no sway session is running")?;
    let out = Command::new("swaymsg").arg("-s").arg(&sock).arg("-r").args(args).output().map_err(|e| e.to_string())?;
    serde_json::from_slice(&out.stdout).map_err(|e| format!("swaymsg: {}", e))
}

fn sway(cmd: &str) -> Result<(), String> {
    let v = swaymsg_json(&[cmd])?;
    if let Some(err) = v.as_array().and_then(|a| a.iter().find(|r| r["success"] == json!(false))) {
        return Err(err["error"].as_str().unwrap_or("sway refused the setting").to_string());
    }
    Ok(())
}

/// The path of a shipped wallpaper id, or a user image checked like any file
/// the assistant touches (inside $HOME, not protected, an image).
fn wallpaper_path(value: &str) -> Result<String, String> {
    if let Some((id, _)) = WALLPAPERS.iter().find(|(id, name)| *id == value || name.eq_ignore_ascii_case(value)) {
        return Ok(format!("{}/wallpaper/osaima-{}.png", desktop_dir(), id));
    }
    let p = crate::files::resolve(value)?;
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !p.is_file() || !["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()) {
        return Err("choose a PNG, JPEG or WebP image".into());
    }
    let s = p.to_string_lossy().into_owned();
    if s.contains('"') || s.contains('\n') {
        return Err("that file name can't be used as a wallpaper".into());
    }
    Ok(s)
}

fn outputs() -> Result<Vec<Value>, String> {
    let v = swaymsg_json(&["-t", "get_outputs"])?;
    Ok(v.as_array().cloned().unwrap_or_default().into_iter().filter(|o| o["active"] == json!(true)).map(|o| {
        let mut modes: Vec<String> = o["modes"].as_array().into_iter().flatten()
            .map(|m| format!("{}x{}", m["width"], m["height"])).collect();
        modes.dedup();
        modes.sort_by_key(|m| std::cmp::Reverse(m.split('x').filter_map(|n| n.parse::<u64>().ok()).product::<u64>()));
        modes.dedup();
        json!({ "name": o["name"], "make": o["make"], "model": o["model"], "scale": o["scale"],
                "current": format!("{}x{}", o["current_mode"]["width"], o["current_mode"]["height"]), "modes": modes })
    }).collect())
}

fn network() -> Value {
    let route = Command::new("ip").args(["-j", "route", "get", "1.1.1.1"]).output().ok()
        .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok()).and_then(|v| v.get(0).cloned());
    let ssid = Command::new("iwgetid").arg("-r").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty());
    match route {
        Some(r) => json!({ "online": true, "interface": r["dev"], "address": r["prefsrc"], "wifi": ssid }),
        None => json!({ "online": false }),
    }
}

/// Default apps by kind: the MIME types each kind covers, and the apps offered.
const DEFAULT_APPS: &[(&str, &[&str], &[(&str, &str)])] = &[
    ("browser", &["x-scheme-handler/http", "x-scheme-handler/https", "text/html"], &[("firefox-bin.desktop", "Firefox"), ("firefox.desktop", "Firefox")]),
    ("files", &["inode/directory"], &[("thunar.desktop", "Thunar")]),
    ("pdf", &["application/pdf"], &[("org.pwmt.zathura.desktop", "Zathura"), ("firefox-bin.desktop", "Firefox"), ("libreoffice-draw.desktop", "LibreOffice Draw")]),
    ("images", &["image/png", "image/jpeg", "image/webp", "image/gif"], &[("imv.desktop", "imv"), ("firefox-bin.desktop", "Firefox")]),
    ("video", &["video/mp4", "video/x-matroska", "video/webm", "audio/mpeg", "audio/flac"], &[("mpv.desktop", "mpv")]),
    ("text", &["text/plain"], &[("org.xfce.mousepad.desktop", "Mousepad"), ("libreoffice-writer.desktop", "LibreOffice Writer")]),
];

fn run_ok(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd).args(args).output().map_err(|e| format!("{}: {}", cmd, e))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().lines().last().unwrap_or("failed").to_string())
    }
}

/// Audio outputs from `wpctl status` (PipeWire): id, name, default.
fn sinks() -> Vec<Value> {
    let Ok(text) = run_ok("wpctl", &["status"]) else { return vec![] };
    let mut out = Vec::new();
    let mut in_sinks = false;
    for line in text.lines() {
        let t = line.trim_start_matches(|c: char| " │├└─".contains(c)).trim();
        if t.starts_with("Sinks:") { in_sinks = true; continue; }
        if in_sinks && (t.starts_with("Sources:") || t.starts_with("Filters:") || t.is_empty()) { break; }
        if in_sinks {
            let default = t.starts_with('*');
            let t = t.trim_start_matches('*').trim();
            if let Some((id, rest)) = t.split_once('.') {
                if let Ok(id) = id.trim().parse::<u32>() {
                    let name = rest.split('[').next().unwrap_or(rest).trim();
                    out.push(json!({ "id": id, "name": name, "default": default }));
                }
            }
        }
    }
    out
}

fn bool_value(params: &Value) -> Result<bool, String> {
    match &params["value"] {
        Value::Bool(b) => Ok(*b),
        Value::String(s) if ["on", "true", "yes", "enabled", "1"].contains(&s.to_lowercase().as_str()) => Ok(true),
        Value::String(s) if ["off", "false", "no", "disabled", "0"].contains(&s.to_lowercase().as_str()) => Ok(false),
        _ => Err("expected on or off".into()),
    }
}

pub fn perform(op: &str, params: &Value) -> Result<Value, String> {
    let value = params["value"].as_str().map(str::to_string)
        .or_else(|| params["value"].as_f64().map(|v| v.to_string())).unwrap_or_default();
    match op {
        "get" => {
            let prefs = load();
            Ok(json!({
                "wallpaper": prefs["wallpaper"].as_str().unwrap_or("contours"),
                "wallpapers": WALLPAPERS.iter().map(|(id, name)| json!({ "id": id, "name": name,
                    "path": format!("{}/wallpaper/osaima-{}.png", desktop_dir(), id) })).collect::<Vec<_>>(),
                "layout": prefs["layout"].as_str().unwrap_or("us"),
                "layouts": LAYOUTS.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
                "scales": SCALES,
                "outputs": outputs().unwrap_or_default(),
                "network": network(),
                "natural_scroll": prefs["natural_scroll"].as_bool().unwrap_or(true),
                "tap": prefs["tap"].as_bool().unwrap_or(true),
                "pointer_speed": prefs["pointer_speed"].as_f64().unwrap_or(0.0),
                "repeat_rate": prefs["repeat_rate"].as_u64().unwrap_or(25),
                "dnd": prefs["dnd"].as_bool().unwrap_or(false),
                "night_light": prefs["night_light"].as_bool().unwrap_or(false),
                "lock_minutes": prefs["lock_minutes"].as_u64().unwrap_or(15),
                "sinks": sinks(),
                "timezone": run_ok("timedatectl", &["show", "-p", "Timezone", "--value"]).unwrap_or_default(),
                "default_apps": DEFAULT_APPS.iter().map(|(kind, mimes, apps)| json!({ "kind": kind,
                    "current": run_ok("xdg-mime", &["query", "default", mimes[0]]).unwrap_or_default(),
                    "options": apps.iter().map(|(d, n)| json!({ "id": d, "name": n })).collect::<Vec<_>>() })).collect::<Vec<_>>(),
            }))
        }
        "network" => Ok(network()),
        "wallpaper" => {
            let path = wallpaper_path(&value)?;
            sway(&format!("output * bg \"{}\" fill", path))?;
            let id = WALLPAPERS.iter().find(|(id, _)| path.ends_with(&format!("osaima-{}.png", id))).map(|(id, _)| id.to_string()).unwrap_or(path.clone());
            save("wallpaper", json!(id))?;
            Ok(json!({ "wallpaper": id }))
        }
        "scale" => {
            let s: f64 = value.parse().map_err(|_| "scale must be a number")?;
            let s = *SCALES.iter().min_by(|a, b| (*a - s).abs().partial_cmp(&(*b - s).abs()).unwrap()).unwrap();
            sway(&format!("output * scale {}", s))?;
            save("scale", json!(s))?;
            Ok(json!({ "scale": s }))
        }
        "resolution" => {
            let (w, h) = value.split_once('x').and_then(|(w, h)| Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?)))
                .ok_or("resolution looks like 1920x1080")?;
            let outs = outputs()?;
            let name = params["output"].as_str().map(str::to_string)
                .or_else(|| outs.first().and_then(|o| o["name"].as_str().map(str::to_string))).ok_or("no screen found")?;
            let supported = outs.iter().any(|o| o["name"] == json!(name) && o["modes"].as_array().map_or(false, |m| m.contains(&json!(format!("{}x{}", w, h)))));
            // a VM's virtual screen accepts any size; a real screen only its own modes
            let mode = if supported { format!("{}x{}", w, h) } else { format!("--custom {}x{}", w, h) };
            if !supported && std::process::Command::new("systemd-detect-virt").arg("-q").status().map_or(true, |s| !s.success()) {
                return Err(format!("this screen doesn't offer {}x{}", w, h));
            }
            sway(&format!("output \"{}\" mode {}", name.replace('"', ""), mode))?;
            save("resolution", json!(format!("{}x{}", w, h)))?;
            Ok(json!({ "resolution": format!("{}x{}", w, h), "output": name }))
        }
        "keyboard" => {
            let (id, label) = LAYOUTS.iter().find(|(id, name)| *id == value || name.eq_ignore_ascii_case(&value))
                .ok_or(format!("unknown keyboard layout '{}'", value))?;
            let (layout, variant) = id.split_once(':').unwrap_or((id, ""));
            sway(&format!("input type:keyboard xkb_layout {}", layout))?;
            sway(&format!("input type:keyboard xkb_variant \"{}\"", variant))?;
            save("layout", json!(id))?;
            Ok(json!({ "layout": id, "name": label }))
        }
        "natural_scroll" | "tap" => {
            let on = bool_value(params)?;
            sway(&format!("input type:touchpad {} {}", op, if on { "enabled" } else { "disabled" }))?;
            save(op, json!(on))?;
            Ok(json!({ op: on }))
        }
        "pointer_speed" => {
            let v: f64 = value.parse().map_err(|_| "speed is a number from -1 (slow) to 1 (fast)")?;
            let v = v.clamp(-1.0, 1.0);
            sway(&format!("input type:pointer pointer_accel {}", v))?;
            let _ = sway(&format!("input type:touchpad pointer_accel {}", v));
            save("pointer_speed", json!(v))?;
            Ok(json!({ "pointer_speed": v }))
        }
        "repeat_rate" => {
            let v: u64 = value.parse().map_err(|_| "repeat rate is keys per second")?;
            let v = v.clamp(10, 60);
            sway(&format!("input type:keyboard repeat_rate {}", v))?;
            save("repeat_rate", json!(v))?;
            Ok(json!({ "repeat_rate": v }))
        }
        "night_light" => {
            // a warm screen tint through Wayland gamma control (gammastep, or wlsunset
            // where installed); the process holds the tint, so off stops it
            let on = bool_value(params)?;
            let _ = Command::new("pkill").args(["-x", "gammastep"]).status();
            let _ = Command::new("pkill").args(["-x", "wlsunset"]).status();
            // reap the one we started, so it doesn't linger as a zombie
            if let Some(mut old) = NIGHT.lock().unwrap().take() { let _ = old.kill(); let _ = old.wait(); }
            if on {
                let temp = params["temperature"].as_f64().unwrap_or(4000.0).clamp(2500.0, 5500.0) as u32;
                let t = temp.to_string();
                let gammastep = Command::new("sh").args(["-c", "command -v gammastep"]).output().map(|o| o.status.success()).unwrap_or(false);
                let mut c = if gammastep { Command::new("gammastep") } else { Command::new("wlsunset") };
                if gammastep { c.args(["-m", "wayland", "-P", "-l", "0:0", "-t", &format!("{}:{}", t, t)]); } else { c.args(["-T", "6500", "-t", &t, "-S", "00:00", "-s", "23:59"]); }
                c
                    .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
                if std::env::var_os("WAYLAND_DISPLAY").is_none() {
                    if let Some(d) = std::fs::read_dir(std::env::var("XDG_RUNTIME_DIR").unwrap_or_default()).ok().and_then(|rd| rd.flatten()
                        .filter(|e| { let n = e.file_name().to_string_lossy().into_owned(); n.starts_with("wayland-") && !n.ends_with(".lock") })
                        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())) {
                        c.env("WAYLAND_DISPLAY", d.file_name());
                    }
                }
                let child = c.spawn().map_err(|e| format!("night light needs gammastep or wlsunset: {}", e))?;
                *NIGHT.lock().unwrap() = Some(child);
            }
            save("night_light", json!(on))?;
            Ok(json!({ "night_light": on }))
        }
        "dnd" => {
            let on = bool_value(params)?;
            run_ok("makoctl", &["mode", if on { "-a" } else { "-r" }, "do-not-disturb"])?;
            save("dnd", json!(on))?;
            Ok(json!({ "dnd": on }))
        }
        "lock_minutes" => {
            let m: u64 = value.parse().map_err(|_| "minutes, or 0 for never")?;
            save("lock_minutes", json!(m.min(240)))?;
            // osaima-idle reads it; restart it so the change applies now
            let _ = Command::new("pkill").args(["-x", "swayidle"]).status();
            let _ = sway("exec osaima-idle");
            Ok(json!({ "lock_minutes": m.min(240) }))
        }
        "default_app" => {
            let kind = params["kind"].as_str().unwrap_or("");
            let (_, mimes, apps) = DEFAULT_APPS.iter().find(|(k, _, _)| *k == kind).ok_or(format!("unknown kind '{}'", kind))?;
            let (desktop, name) = apps.iter().find(|(d, n)| *d == value || n.eq_ignore_ascii_case(&value))
                .ok_or(format!("'{}' isn't offered for {}", value, kind))?;
            let mut args = vec!["default", *desktop];
            args.extend(mimes.iter());
            run_ok("xdg-mime", &args)?;
            Ok(json!({ "kind": kind, "app": name }))
        }
        "sound_output" => {
            let id: u32 = value.parse().map_err(|_| "choose an output from the list")?;
            if !sinks().iter().any(|s| s["id"] == json!(id)) {
                return Err("that sound output isn't available".into());
            }
            run_ok("wpctl", &["set-default", &id.to_string()])?;
            Ok(json!({ "sound_output": id }))
        }
        "timezone" => {
            if value.is_empty() || !value.chars().all(|c| c.is_ascii_alphanumeric() || "/_-+".contains(c)) {
                return Err("time zone looks like Asia/Kolkata".into());
            }
            // systemd asks polkit; on the live ISO that may need the administrator
            run_ok("timedatectl", &["set-timezone", &value]).map_err(|e| format!("couldn't change the time zone: {}", e))?;
            Ok(json!({ "timezone": value }))
        }
        _ => Err(format!("unknown setting '{}'", op)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallpapers_and_bad_values() {
        assert!(wallpaper_path("dawn").unwrap().ends_with("wallpaper/osaima-dawn.png"));
        assert!(wallpaper_path("Deep").unwrap().ends_with("osaima-deep.png"));
        assert!(wallpaper_path("/etc/passwd").is_err());
        assert!(perform("keyboard", &json!({ "value": "klingon" })).is_err());
        assert!(perform("resolution", &json!({ "value": "big" })).is_err());
        assert!(perform("explode", &json!({})).is_err());
    }
}
