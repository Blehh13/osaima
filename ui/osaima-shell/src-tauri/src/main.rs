use tauri::Manager;
use std::os::unix::net::UnixStream;
use std::io::{Write, BufRead, BufReader};
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

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_system_stats, ping_daemon])
        .run(tauri::generate_context!())
        .expect("error while running osaima-shell");
}
