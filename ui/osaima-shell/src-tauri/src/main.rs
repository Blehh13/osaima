use std::os::unix::net::UnixStream;
use std::io::{Write, BufRead, BufReader};
use std::process::Command;
use serde_json::{json, Value};

const MCP_SOCKET: &str = "/tmp/interstellar_mcp.sock";

fn query_mcp(method: &str) -> Result<Value, String> {
    let stream = UnixStream::connect(MCP_SOCKET)
        .map_err(|e| format!("MCP socket unavailable: {}", e))?;

    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let reader = BufReader::new(stream);

    let req = json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": method
    });

    let mut payload = serde_json::to_string(&req).unwrap();
    payload.push('\n');
    writer.write_all(payload.as_bytes()).map_err(|e| e.to_string())?;

    let mut line = String::new();
    reader.lines().next()
        .ok_or_else(|| "No response from MCP daemon".to_string())?
        .map_err(|e| e.to_string())
        .and_then(|l| { line = l; Ok(()) })?;

    serde_json::from_str(&line).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_system_stats() -> Result<Value, String> {
    match query_mcp("system.get_stats") {
        Ok(response) => {
            if let Some(result) = response.get("result") {
                Ok(result.clone())
            } else {
                Err("MCP daemon returned no result".to_string())
            }
        }
        // If MCP daemon isn't running yet, return mock data for development
        Err(_) => Ok(json!({
            "os": "Interstellar OS",
            "kernel": "6.x.x-interstellar",
            "host": "interstellar",
            "memory": { "total_bytes": 17179869184u64, "used_bytes": 5368709120u64 },
            "cpu": { "usage_percent": 12.5, "cores": 8 }
        })),
    }
}

#[tauri::command]
fn ping_daemon() -> bool {
    query_mcp("ping").is_ok()
}

/// Run a real shell command and return combined stdout + stderr.
/// The shell process runs as the session user, so this is genuine OS control.
#[tauri::command]
fn run_command(cmd: String) -> Result<String, String> {
    if cmd.trim().is_empty() {
        return Ok(String::new());
    }
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(&cmd)
        .output()
        .map_err(|e| e.to_string())?;
    let mut out = String::from_utf8_lossy(&output.stdout).to_string();
    let err = String::from_utf8_lossy(&output.stderr);
    if !err.is_empty() {
        out.push_str(&err);
    }
    Ok(out)
}

/// List a real directory. Returns { path, entries: [{ name, is_dir }] }.
#[tauri::command]
fn read_dir(path: String) -> Result<Value, String> {
    let p = if path.trim().is_empty() { "/".to_string() } else { path };
    let mut entries: Vec<Value> = Vec::new();
    for entry in std::fs::read_dir(&p).map_err(|e| format!("{}", e))? {
        if let Ok(entry) = entry {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push(json!({ "name": name, "is_dir": is_dir }));
        }
    }
    entries.sort_by(|a, b| {
        let ad = a["is_dir"].as_bool().unwrap_or(false);
        let bd = b["is_dir"].as_bool().unwrap_or(false);
        bd.cmp(&ad).then_with(|| {
            a["name"].as_str().unwrap_or("").to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
        })
    });
    Ok(json!({ "path": p, "entries": entries }))
}

/// Real process list (top by CPU) via `ps`.
#[tauri::command]
fn list_processes() -> Result<Value, String> {
    let output = Command::new("ps")
        .args(["-eo", "pid,comm,pcpu,rss", "--sort=-pcpu"])
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut procs: Vec<Value> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 { continue; } // skip header
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            procs.push(json!({
                "pid": parts[0].parse::<u32>().unwrap_or(0),
                "name": parts[1],
                "cpu": parts[2].parse::<f64>().unwrap_or(0.0),
                "mem": parts[3].parse::<f64>().unwrap_or(0.0) / 1024.0, // KiB -> MiB
            }));
        }
        if procs.len() >= 40 { break; }
    }
    Ok(json!({ "processes": procs }))
}

/// Kill a real process by PID.
#[tauri::command]
fn kill_process(pid: u32) -> Result<bool, String> {
    let status = Command::new("kill")
        .arg(pid.to_string())
        .status()
        .map_err(|e| e.to_string())?;
    Ok(status.success())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_system_stats,
            ping_daemon,
            run_command,
            read_dir,
            list_processes,
            kill_process
        ])
        .run(tauri::generate_context!())
        .expect("error while running osaima-shell");
}
