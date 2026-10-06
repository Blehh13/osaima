mod agent;
mod host;
mod mcp;
mod settings;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{Emitter, Manager, State};

use agent::AgentBridge;

/// Live system statistics from the AI Core.
#[tauri::command]
async fn get_system_stats() -> Result<Value, String> {
    mcp::call_tool("get_system_stats", json!({})).await
}

/// Whether the AI Core daemon is reachable.
#[tauri::command]
async fn ping_daemon() -> bool {
    mcp::request("ping", json!({})).await.is_ok()
}

/// Top processes by CPU: `{ processes: [{ pid, name, cpu_percent, memory_bytes, uid, status }] }`.
#[tauri::command]
async fn list_processes(limit: Option<u32>) -> Result<Value, String> {
    mcp::call_tool("list_processes", json!({ "limit": limit.unwrap_or(40) })).await
}

/// Ask the AI Core to terminate a process the user owns.
#[tauri::command]
async fn kill_process(pid: u32, signal: Option<String>) -> Result<(), String> {
    let signal = signal.unwrap_or_else(|| "TERM".into());
    mcp::call_tool("kill_process", json!({ "pid": pid, "signal": signal }))
        .await
        .map(|_| ())
}

/// Mounted filesystems.
#[tauri::command]
async fn list_disks() -> Result<Value, String> {
    mcp::call_tool("list_disks", json!({})).await
}

/// Run a command for the Terminal app.
#[tauri::command]
async fn run_command(cmd: String, cwd: Option<String>) -> Result<host::CommandOutput, String> {
    let cwd = cwd.filter(|c| !c.is_empty()).map(PathBuf::from);
    let limits = settings::get();
    host::run_command(
        &cmd,
        cwd.as_deref(),
        limits.command_timeout,
        limits.command_output_limit,
    )
    .await
}

/// List a directory for the Files app.
#[tauri::command]
async fn read_dir(path: Option<String>) -> Result<Value, String> {
    let path = path.filter(|p| !p.is_empty()).map(PathBuf::from);
    host::read_dir(path.as_deref())
}

/// Talk to the assistant service. Only `agent.*` methods are allowed through.
/// Progress arrives separately as `agent-event` events.
#[tauri::command]
async fn agent_call(
    bridge: State<'_, AgentBridge>,
    method: String,
    params: Option<Value>,
) -> Result<Value, String> {
    if !method.starts_with("agent.") {
        return Err(format!("{method} is not an assistant method"));
    }
    bridge
        .call(&method, params.unwrap_or_else(|| json!({})))
        .await
}

/// The user's home directory, so apps can start there.
#[tauri::command]
fn home_dir() -> String {
    host::home_dir().display().to_string()
}

fn main() {
    let settings = match settings::init() {
        Ok(settings) => settings,
        Err(err) => {
            eprintln!("osaima-shell: invalid setting {err}");
            std::process::exit(2);
        }
    };
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let sink: agent::EventSink = Arc::new(move |event| {
                let _ = handle.emit("agent-event", event);
            });
            app.manage(AgentBridge::new(
                agent::socket_path(),
                settings.agent_timeout,
                sink,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_system_stats,
            ping_daemon,
            list_processes,
            kill_process,
            list_disks,
            agent_call,
            run_command,
            read_dir,
            home_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running osaima-shell");
}
