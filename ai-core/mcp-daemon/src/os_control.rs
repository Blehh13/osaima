//! OS-level actions over D-Bus (PDR §3.6, Interface A): media (MPRIS), brightness,
//! lock and power (logind), Wi-Fi (NetworkManager), Bluetooth (BlueZ),
//! notifications, plus volume through PipeWire's wpctl (PipeWire exposes no
//! standard D-Bus volume API).
//!
//! Each action has a risk tier and says whether the user must approve it, so
//! the RPC layer can gate it the same way as AI-generated shell commands.

use crate::security::Tier;
use serde_json::{json, Value};
use std::collections::HashMap;
use zbus::{proxy, Connection};

#[proxy(interface = "org.mpris.MediaPlayer2.Player", default_path = "/org/mpris/MediaPlayer2")]
trait MprisPlayer {
    fn play_pause(&self) -> zbus::Result<()>;
    fn play(&self) -> zbus::Result<()>;
    fn pause(&self) -> zbus::Result<()>;
    fn next(&self) -> zbus::Result<()>;
    fn previous(&self) -> zbus::Result<()>;
    fn stop(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;
}

#[proxy(interface = "org.freedesktop.login1.Session", default_service = "org.freedesktop.login1",
        default_path = "/org/freedesktop/login1/session/auto")]
trait LoginSession {
    fn lock(&self) -> zbus::Result<()>;
    fn set_brightness(&self, subsystem: &str, name: &str, brightness: u32) -> zbus::Result<()>;
}

#[proxy(interface = "org.freedesktop.login1.Manager", default_service = "org.freedesktop.login1",
        default_path = "/org/freedesktop/login1")]
trait LoginManager {
    fn power_off(&self, interactive: bool) -> zbus::Result<()>;
    fn reboot(&self, interactive: bool) -> zbus::Result<()>;
    fn suspend(&self, interactive: bool) -> zbus::Result<()>;
}

#[proxy(interface = "org.freedesktop.NetworkManager", default_service = "org.freedesktop.NetworkManager",
        default_path = "/org/freedesktop/NetworkManager")]
trait NetworkManager {
    #[zbus(property)]
    fn wireless_enabled(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_wireless_enabled(&self, value: bool) -> zbus::Result<()>;
    /// NMState: 70 = connected (global)
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
}

#[proxy(interface = "org.bluez.Adapter1", default_service = "org.bluez", default_path = "/org/bluez/hci0")]
trait BluezAdapter {
    #[zbus(property)]
    fn powered(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_powered(&self, value: bool) -> zbus::Result<()>;
}

#[proxy(interface = "org.freedesktop.Notifications", default_service = "org.freedesktop.Notifications",
        default_path = "/org/freedesktop/Notifications")]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(&self, app_name: &str, replaces_id: u32, app_icon: &str, summary: &str, body: &str,
              actions: &[&str], hints: &HashMap<&str, zbus::zvariant::Value<'_>>, expire_timeout: i32)
              -> zbus::Result<u32>;
}

/// Risk tier, and whether the user must approve before it runs.
pub fn policy(action: &str, op: &str) -> (Tier, bool) {
    match (action, op) {
        (_, "get") | (_, "status") | ("window", "list") => (Tier::ReadOnly, false),
        // closing every window can lose unsaved work in several apps at once
        ("window", "close_all") => (Tier::Reversible, true),
        ("files", "search") | ("files", "list") | ("files", "open") | ("files", "reveal") => (Tier::ReadOnly, false),
        ("settings", "get") | ("settings", "network") => (Tier::ReadOnly, false),
        // a wrong mode can blank the screen: ask first (the Settings app also reverts after 15 s)
        ("settings", "resolution") => (Tier::Reversible, true),
        // system-wide; the rest are the user's own preferences
        ("settings", "timezone") => (Tier::Reversible, true),
        // new things only; nothing existing changes, and undo removes them
        ("files", "copy") | ("files", "mkdir") | ("files", "undo") => (Tier::Reversible, false),
        // existing files change place: one click, and undo puts them back
        ("files", _) => (Tier::Reversible, true),
        ("power", _) => (Tier::Irreversible, true),
        // Turning radios off can cut the user (or a remote session) off.
        ("wifi", "off") | ("bluetooth", "off") => (Tier::Reversible, true),
        _ => (Tier::Reversible, false),
    }
}

pub struct OsControl {
    session: Option<Connection>,
    system: Option<Connection>,
}

fn op_of(params: &Value) -> &str {
    params["op"].as_str().unwrap_or("")
}

impl OsControl {
    pub async fn new() -> Self {
        // Either bus may be missing (e.g. a headless VM); actions on it then report an error.
        Self { session: Connection::session().await.ok(), system: Connection::system().await.ok() }
    }

    fn session(&self) -> Result<&Connection, String> {
        self.session.as_ref().ok_or_else(|| "no D-Bus session bus".to_string())
    }

    fn system(&self) -> Result<&Connection, String> {
        self.system.as_ref().ok_or_else(|| "no D-Bus system bus".to_string())
    }

    pub async fn perform(&self, action: &str, params: &Value) -> Result<Value, String> {
        let op = op_of(params);
        match action {
            "media" => self.media(op).await,
            "volume" => volume(op, params["value"].as_f64()),
            "brightness" => self.brightness(op, params["value"].as_f64()).await,
            "lock_screen" => {
                LoginSessionProxy::new(self.system()?).await.map_err(|e| e.to_string())?
                    .lock().await.map_err(|e| e.to_string())?;
                Ok(json!({ "locked": true }))
            }
            "power" => self.power(op).await,
            "wifi" => self.wifi(op).await,
            "bluetooth" => self.bluetooth(op).await,
            "notify" => {
                let p = NotificationsProxy::new(self.session()?).await.map_err(|e| e.to_string())?;
                let id = p.notify("OSAIMA", 0, "dialog-information", params["title"].as_str().unwrap_or("OSAIMA"),
                                  params["body"].as_str().unwrap_or(""), &[], &HashMap::new(), 5000)
                    .await.map_err(|e| e.to_string())?;
                Ok(json!({ "id": id }))
            }
            "settings" => {
                let params = params.clone();
                tokio::task::spawn_blocking(move || crate::syssettings::perform(op_of(&params), &params))
                    .await.map_err(|e| e.to_string())?
            }
            "files" => {
                let params = params.clone();
                tokio::task::spawn_blocking(move || crate::files::perform(op_of(&params), &params))
                    .await.map_err(|e| e.to_string())?
            }
            "window" | "app" | "layout" | "workspace" | "browser" => {
                let (action, params) = (action.to_string(), params.clone());
                tokio::task::spawn_blocking(move || crate::desktop::perform(&action, op_of(&params), &params))
                    .await.map_err(|e| e.to_string())?
            }
            _ => Err(format!("unknown OS action '{}'", action)),
        }
    }

    /// The MPRIS player that is playing, else the first one found.
    async fn active_player(&self) -> Result<MprisPlayerProxy<'_>, String> {
        let conn = self.session()?;
        let dbus = zbus::fdo::DBusProxy::new(conn).await.map_err(|e| e.to_string())?;
        let names: Vec<String> = dbus.list_names().await.map_err(|e| e.to_string())?
            .into_iter().map(|n| n.to_string()).filter(|n| n.starts_with("org.mpris.MediaPlayer2.")).collect();
        let mut fallback = None;
        for name in names {
            let p = MprisPlayerProxy::builder(conn).destination(name).map_err(|e| e.to_string())?
                .build().await.map_err(|e| e.to_string())?;
            if p.playback_status().await.map(|s| s == "Playing").unwrap_or(false) {
                return Ok(p);
            }
            fallback.get_or_insert(p);
        }
        fallback.ok_or_else(|| "no media player is running".to_string())
    }

    async fn media(&self, op: &str) -> Result<Value, String> {
        let p = self.active_player().await?;
        let r = match op {
            "play_pause" | "toggle" | "" => p.play_pause().await,
            "play" => p.play().await,
            "pause" => p.pause().await,
            "next" => p.next().await,
            "previous" => p.previous().await,
            "stop" => p.stop().await,
            "status" => return Ok(json!({ "status": p.playback_status().await.map_err(|e| e.to_string())? })),
            other => return Err(format!("unknown media op '{}'", other)),
        };
        r.map_err(|e| e.to_string())?;
        Ok(json!({ "player": p.inner().destination().to_string(), "op": op }))
    }

    async fn brightness(&self, op: &str, value: Option<f64>) -> Result<Value, String> {
        let dev = std::fs::read_dir("/sys/class/backlight").map_err(|_| "no backlight device".to_string())?
            .filter_map(Result::ok).next().ok_or("no backlight device")?.path();
        let read = |f: &str| std::fs::read_to_string(dev.join(f)).ok().and_then(|s| s.trim().parse::<f64>().ok());
        let (cur, max) = (read("brightness").ok_or("unreadable brightness")?, read("max_brightness").ok_or("unreadable max")?);
        let pct = cur / max * 100.0;
        let target = match op {
            "get" => return Ok(json!({ "percent": pct.round() })),
            "set" => value.ok_or("brightness set needs a value")?,
            "up" => pct + value.unwrap_or(10.0),
            "down" => pct - value.unwrap_or(10.0),
            other => return Err(format!("unknown brightness op '{}'", other)),
        }.clamp(5.0, 100.0); // never fully black: the user could not see to undo it
        let name = dev.file_name().unwrap().to_string_lossy().to_string();
        LoginSessionProxy::new(self.system()?).await.map_err(|e| e.to_string())?
            .set_brightness("backlight", &name, (target / 100.0 * max).round() as u32).await.map_err(|e| e.to_string())?;
        Ok(json!({ "percent": target.round(), "previous": pct.round() }))
    }

    async fn power(&self, op: &str) -> Result<Value, String> {
        let m = LoginManagerProxy::new(self.system()?).await.map_err(|e| e.to_string())?;
        match op {
            "shutdown" | "poweroff" => m.power_off(true).await,
            "reboot" | "restart" => m.reboot(true).await,
            "suspend" | "sleep" => m.suspend(true).await,
            other => return Err(format!("unknown power op '{}'", other)),
        }.map_err(|e| e.to_string())?;
        Ok(json!({ "op": op }))
    }

    async fn wifi(&self, op: &str) -> Result<Value, String> {
        let nm = NetworkManagerProxy::new(self.system()?).await.map_err(|e| e.to_string())?;
        match op {
            "on" => nm.set_wireless_enabled(true).await.map_err(|e| e.to_string())?,
            "off" => nm.set_wireless_enabled(false).await.map_err(|e| e.to_string())?,
            "status" | "get" => {}
            other => return Err(format!("unknown wifi op '{}'", other)),
        }
        Ok(json!({ "wifi_enabled": nm.wireless_enabled().await.map_err(|e| e.to_string())?,
                   "connected": nm.state().await.map(|s| s == 70).unwrap_or(false) }))
    }

    async fn bluetooth(&self, op: &str) -> Result<Value, String> {
        let bt = BluezAdapterProxy::new(self.system()?).await.map_err(|e| e.to_string())?;
        match op {
            "on" => bt.set_powered(true).await.map_err(|e| e.to_string())?,
            "off" => bt.set_powered(false).await.map_err(|e| e.to_string())?,
            "status" | "get" => {}
            other => return Err(format!("unknown bluetooth op '{}'", other)),
        }
        Ok(json!({ "bluetooth_on": bt.powered().await.map_err(|e| e.to_string())? }))
    }
}

/// Volume through wpctl on the default PipeWire sink.
fn volume(op: &str, value: Option<f64>) -> Result<Value, String> {
    let sink = "@DEFAULT_AUDIO_SINK@";
    let step = format!("{}%", value.unwrap_or(10.0).clamp(1.0, 100.0));
    let args: Vec<String> = match op {
        "get" => vec![],
        "up" => vec!["set-volume".into(), "-l".into(), "1.0".into(), sink.into(), format!("{}+", step)],
        "down" => vec!["set-volume".into(), sink.into(), format!("{}-", step)],
        "set" => vec!["set-volume".into(), "-l".into(), "1.0".into(), sink.into(),
                      format!("{}%", value.ok_or("volume set needs a value")?.clamp(0.0, 100.0))],
        "mute" => vec!["set-mute".into(), sink.into(), "1".into()],
        "unmute" => vec!["set-mute".into(), sink.into(), "0".into()],
        "toggle_mute" => vec!["set-mute".into(), sink.into(), "toggle".into()],
        other => return Err(format!("unknown volume op '{}'", other)),
    };
    if !args.is_empty() {
        let st = std::process::Command::new("wpctl").args(&args).status().map_err(|e| format!("wpctl: {}", e))?;
        if !st.success() {
            return Err("wpctl failed".to_string());
        }
    }
    let out = std::process::Command::new("wpctl").args(["get-volume", sink]).output().map_err(|e| format!("wpctl: {}", e))?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "Volume: 0.40 [MUTED]"
    let level = text.split_whitespace().nth(1).and_then(|v| v.parse::<f64>().ok()).map(|v| (v * 100.0).round());
    Ok(json!({ "percent": level, "muted": text.contains("MUTED") }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_tiers() {
        assert_eq!(policy("wifi", "status"), (Tier::ReadOnly, false));
        assert_eq!(policy("volume", "up"), (Tier::Reversible, false));
        assert_eq!(policy("wifi", "off"), (Tier::Reversible, true));
        assert_eq!(policy("power", "reboot"), (Tier::Irreversible, true));
    }
}
