//! osaima-mcp — OSAIMA's AI core as a Model Context Protocol server.
//!
//! Any MCP client (Claude Desktop, MCP Inspector, an Ollama agent…) launches
//! this program and talks MCP over stdin/stdout: newline-delimited JSON-RPC 2.0,
//! revisions 2025-06-18, 2025-03-26 and 2024-11-05 (the lifecycle and protocol
//! handling follow Banu's spec-compliant server on feat/m1-foundation).
//!
//! It holds no power of its own: every tool call is forwarded to the running
//! mcp-daemon over its socket, so the same risk tiers, approvals, kernel
//! sandbox and hash-chained audit log apply as for the OSAIMA apps.
//! State-changing tools need `confirmed_by_user: true`, which the client may
//! only send after asking its user (the tools are annotated destructive); the
//! tools that can't be undone also need `typed: "confirm"`. Each one shows a
//! notification on the OSAIMA desktop, so nothing happens out of sight.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_LINE: usize = 1 << 20;

const INSTRUCTIONS: &str = "OSAIMA AI core: live context about this computer (health, windows, files, \
settings, automations, recent AI activity) and safe actions on it. Tools marked destructive change \
the computer: ask the user first, then pass confirmed_by_user: true. Actions that can't be undone \
(shutdown, restart) also need typed: \"confirm\", typed by the user.";

fn socket() -> PathBuf {
    if let Ok(p) = std::env::var("OSAIMA_MCP_SOCKET") {
        return PathBuf::from(p);
    }
    let run = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }));
    PathBuf::from(run).join("osaima/mcp.sock")
}

/// One call to the running daemon. Err is a message for the client.
fn daemon(method: &str, params: Value) -> Result<Value, String> {
    let mut s = UnixStream::connect(socket()).map_err(|e| format!("OSAIMA's AI core isn't running ({})", e))?;
    s.set_read_timeout(Some(Duration::from_secs(120))).ok();
    let req = json!({ "jsonrpc": "2.0", "id": "mcp", "method": method, "params": params });
    s.write_all(format!("{}\n", req).as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    let r: Value = serde_json::from_str(&line).map_err(|e| format!("bad reply from the AI core: {}", e))?;
    match r.get("error").filter(|e| !e.is_null()) {
        Some(e) => Err(e["message"].as_str().unwrap_or("the AI core refused").to_string()),
        None => Ok(r["result"].clone()),
    }
}

fn tool(name: &str, description: &str, props: Value, required: &[&str], destructive: bool) -> Value {
    let mut props = props;
    if destructive {
        props["confirmed_by_user"] = json!({ "type": "boolean", "description": "true only after the user agreed to this exact action" });
        props["typed"] = json!({ "type": "string", "description": "the word 'confirm' typed by the user, for actions that can't be undone" });
    }
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": props, "required": required, "additionalProperties": false },
        "annotations": { "readOnlyHint": !destructive, "destructiveHint": destructive, "openWorldHint": false },
    })
}

fn definitions() -> Vec<Value> {
    vec![
        tool("get_system_stats", "CPU, memory, disk and uptime of this computer.", json!({}), &[], false),
        tool("get_health", "The health watch: temperature, throttling, pressure, memory, disk, battery, busiest programs, failed services and current warnings with advice.", json!({}), &[], false),
        tool("list_windows", "Open windows on the desktop, with app, title and workspace.", json!({}), &[], false),
        tool("search_files", "Find files in the user's home folder by name words, kind, size or age.",
             json!({ "query": { "type": "string" }, "kind": { "type": "string", "enum": ["pdf", "images", "videos", "music", "documents", "archives", "code"] },
                     "min_size_mb": { "type": "number" }, "modified_days": { "type": "number" }, "folder": { "type": "string" },
                     "limit": { "type": "integer", "minimum": 1, "maximum": 100 } }), &[], false),
        tool("get_settings", "Current desktop settings: wallpaper, display, keyboard, night light, Do Not Disturb, sound outputs, time zone, default apps.", json!({}), &[], false),
        tool("list_automations", "Automations the user set up, and any power action counting down.", json!({}), &[], false),
        tool("recent_activity", "The latest entries of OSAIMA's tamper-evident activity log.", json!({ "n": { "type": "integer", "minimum": 1, "maximum": 100 } }), &[], false),
        tool("understand_request", "How OSAIMA understands a plain-language request (intent and parameters, and its risk), without doing anything.",
             json!({ "request": { "type": "string" } }), &["request"], false),
        tool("control_device", "Volume, brightness, music, Wi-Fi, Bluetooth, lock screen, or power (shutdown/reboot/suspend, which can't be undone).",
             json!({ "action": { "type": "string", "enum": ["volume", "brightness", "media", "wifi", "bluetooth", "lock_screen", "power"] },
                     "op": { "type": "string", "description": "volume/brightness: up|down|set|mute|unmute|get; media: play|pause|next|previous; wifi/bluetooth: on|off|status; lock_screen: lock; power: shutdown|reboot|suspend" },
                     "value": { "type": "number", "description": "percent, for set" } }), &["action", "op"], true),
        tool("change_setting", "Change a desktop setting: wallpaper, night_light, dnd, scale, resolution, keyboard, pointer_speed, lock_minutes, timezone, sound_output.",
             json!({ "setting": { "type": "string" }, "value": {} }), &["setting", "value"], true),
        tool("open_app", "Open an app: terminal, browser, files, editor, office, player, archives, settings, history, notes, calendar, monitor.",
             json!({ "app": { "type": "string" } }), &["app"], true),
        tool("manage_files", "Copy, move, rename, make a folder, send to the Trash, or undo the last change. Everything can be undone.",
             json!({ "op": { "type": "string", "enum": ["copy", "move", "rename", "mkdir", "trash", "undo"] },
                     "paths": { "type": "array", "items": { "type": "string" } }, "path": { "type": "string" },
                     "to": { "type": "string" }, "name": { "type": "string" } }), &["op"], true),
        tool("create_automation", "Set up an automation: a trigger (time, schedule, battery, charger, network, app opened, program finished, health warning…) and OS actions. Same rule format as OSAIMA's assistant.",
             json!({ "rule": { "type": "object" } }), &["rule"], true),
    ]
}

/// Result of one tool, or an error message the client should show.
fn call(name: &str, args: &Value) -> Result<Value, String> {
    let approved = args["confirmed_by_user"].as_bool().unwrap_or(false);
    let typed = args["typed"].as_str().unwrap_or("").to_string();
    let os = |mut p: Value| -> Result<Value, String> {
        p["approved"] = json!(approved);
        p["typed"] = json!(typed);
        p["client"] = json!("mcp");
        let r = daemon("system.os_action", p)?;
        Ok(r.get("result").cloned().unwrap_or(r))
    };
    let pick = |keys: &[&str]| -> Value {
        let mut o = json!({});
        for k in keys { if !args[*k].is_null() { o[*k] = args[*k].clone(); } }
        o
    };
    let changes = matches!(name, "control_device" | "change_setting" | "open_app" | "manage_files" | "create_automation");
    // read-only ops of a "destructive" tool don't need the user's OK
    let reading = name == "control_device" && matches!(args["op"].as_str(), Some("get") | Some("status"));
    if changes && !reading && !approved {
        // stricter than the apps: the daemon lets small changes like volume run
        // unasked, but an outside client must always show its user first
        return Err(format!("Not done: {} changes this computer, so it needs the user's approval. Ask them, then call again with confirmed_by_user: true.", name.replace('_', " ")));
    }
    if changes && !reading {
        let what = format!("{} {}", name.replace('_', " "), serde_json::to_string(&pick(&["action", "op", "value", "setting", "app", "paths", "path", "to", "name"])).unwrap_or_default());
        let _ = daemon("system.os_action", json!({ "action": "notify", "title": "An MCP client is using OSAIMA", "body": what, "approved": true }));
    }
    match name {
        "get_system_stats" => daemon("system.get_stats", json!({})),
        "get_health" => daemon("system.health", json!({})),
        "list_windows" => os(json!({ "action": "window", "op": "list" })),
        "search_files" => {
            let mut p = pick(&["query", "kind", "min_size_mb", "modified_days", "folder", "limit"]);
            p["action"] = json!("files"); p["op"] = json!("search");
            if p.get("query").is_none() { p["query"] = json!(""); }
            os(p)
        }
        "get_settings" => os(json!({ "action": "settings", "op": "get" })),
        "list_automations" => daemon("system.automations", json!({})),
        "recent_activity" => daemon("system.audit_tail", json!({ "n": args["n"].as_u64().unwrap_or(20).min(100) })),
        "understand_request" => {
            let r = daemon("system.classify_intent", json!({ "query": args["request"].as_str().unwrap_or("") }))?;
            Ok(json!({ "intent": r["intent"], "params": r["params"], "reply": r["reply"], "source": r["source"] }))
        }
        "control_device" => os(pick(&["action", "op", "value"])),
        "change_setting" => os(json!({ "action": "settings", "op": args["setting"], "value": args["value"] })),
        "open_app" => os(json!({ "action": "app", "op": "launch", "app": args["app"] })),
        "manage_files" => { let mut p = pick(&["op", "paths", "path", "to", "name"]); p["action"] = json!("files"); os(p) }
        "create_automation" => daemon("system.automation_add", json!({ "rule": args["rule"], "approved": approved, "typed": typed })),
        _ => Err(format!("unknown tool: {}", name)),
    }
}

fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg["method"].as_str().unwrap_or("");
    let ok = |r: Value| json!({ "jsonrpc": "2.0", "id": id.clone().unwrap_or(Value::Null), "result": r });
    let err = |code: i64, m: String| json!({ "jsonrpc": "2.0", "id": id.clone().unwrap_or(Value::Null), "error": { "code": code, "message": m } });
    if msg["jsonrpc"] != "2.0" || method.is_empty() {
        return Some(err(-32600, "invalid request".into()));
    }
    let reply = match method {
        "initialize" => {
            let asked = msg["params"]["protocolVersion"].as_str().unwrap_or(VERSIONS[0]);
            let v = if VERSIONS.contains(&asked) { asked } else { VERSIONS[0] };
            ok(json!({
                "protocolVersion": v,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "osaima", "title": "OSAIMA AI core", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({ "tools": definitions() })),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            if !definitions().iter().any(|t| t["name"] == name) {
                err(-32602, format!("unknown tool: {}", name))
            } else {
                let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
                // tool failures are results with isError, so the model can read and react
                match call(name, &args) {
                    Ok(v) => ok(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }],
                                        "structuredContent": if v.is_object() { v.clone() } else { json!({ "result": v }) }, "isError": false })),
                    Err(e) => ok(json!({ "content": [{ "type": "text", "text": e }], "isError": true })),
                }
            }
        }
        m if m.starts_with("notifications/") => return None,
        m => err(-32601, format!("method not found: {}", m)),
    };
    id.map(|_| reply)
}

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() { continue; }
        let reply = if line.len() > MAX_LINE {
            Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32600, "message": "message too large" } }))
        } else {
            match serde_json::from_str::<Value>(&line) {
                Ok(msg) => handle(&msg),
                Err(e) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("parse error: {}", e) } })),
            }
        };
        if let Some(r) = reply {
            let _ = writeln!(out, "{}", r);
            let _ = out.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_and_errors() {
        let init = handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } })).unwrap();
        assert_eq!(init["id"], 1);
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        let newer = handle(&json!({ "jsonrpc": "2.0", "id": "a", "method": "initialize", "params": { "protocolVersion": "2099-01-01" } })).unwrap();
        assert_eq!(newer["result"]["protocolVersion"], VERSIONS[0]);
        assert!(handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none());
        assert_eq!(handle(&json!({ "jsonrpc": "2.0", "id": 2, "method": "nope" })).unwrap()["error"]["code"], -32601);
        assert_eq!(handle(&json!({ "jsonrpc": "1.0", "id": 3, "method": "ping" })).unwrap()["error"]["code"], -32600);
        let unknown = handle(&json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "rm_rf" } })).unwrap();
        assert_eq!(unknown["error"]["code"], -32602);
    }

    #[test]
    fn tools_are_annotated() {
        for t in definitions() {
            let destructive = t["annotations"]["destructiveHint"].as_bool().unwrap();
            assert_eq!(destructive, t["inputSchema"]["properties"].get("confirmed_by_user").is_some(), "{}", t["name"]);
            assert_eq!(t["annotations"]["readOnlyHint"].as_bool().unwrap(), !destructive);
        }
    }
}

#[cfg(test)]
mod approval {
    use super::*;

    #[test]
    fn changes_need_confirmation_before_reaching_the_daemon() {
        // no daemon is reachable here: a refusal must come before any call
        std::env::set_var("OSAIMA_MCP_SOCKET", "/nonexistent/osaima.sock");
        for (name, args) in [("control_device", json!({ "action": "volume", "op": "set", "value": 5 })),
                             ("change_setting", json!({ "setting": "dnd", "value": true })),
                             ("open_app", json!({ "app": "terminal" })),
                             ("manage_files", json!({ "op": "undo" })),
                             ("create_automation", json!({ "rule": {} }))] {
            let e = call(name, &args).unwrap_err();
            assert!(e.contains("needs the user's approval"), "{}: {}", name, e);
        }
        // reading a level isn't a change
        assert!(!call("control_device", &json!({ "action": "volume", "op": "get" })).unwrap_err().contains("approval"));
    }
}
