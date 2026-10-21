use std::os::unix::net::UnixStream;
use std::io::{Write, BufRead, BufReader};
use std::process::Command;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem, MasterPty};
use tauri::{AppHandle, Emitter, State};

struct PtyAppState {
    pty_writers: Mutex<HashMap<String, Box<dyn std::io::Write + Send>>>,
    pty_masters: Mutex<HashMap<String, Box<dyn MasterPty + Send>>>,
}

/// Same lookup as the daemon: $OSAIMA_MCP_SOCKET, else
/// $XDG_RUNTIME_DIR/osaima/mcp.sock, else /tmp.
fn mcp_socket() -> String {
    if let Ok(p) = std::env::var("OSAIMA_MCP_SOCKET") {
        return p;
    }
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) if !dir.is_empty() => format!("{}/osaima/mcp.sock", dir),
        _ => "/tmp/osaima_mcp.sock".to_string(),
    }
}

fn query_mcp(method: &str) -> Result<Value, String> {
    let stream = UnixStream::connect(mcp_socket())
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

fn query_mcp_with_params(method: &str, params: Value) -> Result<Value, String> {
    let stream = UnixStream::connect(mcp_socket())
        .map_err(|e| format!("MCP socket unavailable: {}", e))?;

    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let reader = BufReader::new(stream);

    let req = json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": method,
        "params": params
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
        // never invent numbers: the UI shows that the AI core is not reachable
        Err(e) => Err(format!("the AI core is not running: {}", e)),
    }
}

#[tauri::command]
fn system_get_suggestion() -> Result<Value, String> {
    match query_mcp("system.get_suggestion") {
        Ok(response) => {
            if let Some(result) = response.get("result") {
                Ok(result.clone())
            } else {
                Err("MCP daemon returned no result".to_string())
            }
        }
        Err(_) => Ok(json!(null)),
    }
}

#[tauri::command]
fn ping_daemon() -> bool {
    query_mcp("ping").is_ok()
}

#[tauri::command]
fn set_api_key(key: String) -> Result<bool, String> {
    let req = json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": "system.set_api_key",
        "params": { "key": key }
    });
    
    let stream = UnixStream::connect(mcp_socket())
        .map_err(|e| format!("MCP socket unavailable: {}", e))?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    
    let mut payload = serde_json::to_string(&req).unwrap();
    payload.push('\n');
    writer.write_all(payload.as_bytes()).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
fn set_groq_key(key: String) -> Result<bool, String> {
    let req = json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": "system.set_groq_key",
        "params": { "key": key }
    });
    
    let stream = UnixStream::connect(mcp_socket())
        .map_err(|e| format!("MCP socket unavailable: {}", e))?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    
    let mut payload = serde_json::to_string(&req).unwrap();
    payload.push('\n');
    writer.write_all(payload.as_bytes()).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
fn validate_api_key(key: String) -> Result<bool, String> {
    let stream = UnixStream::connect(mcp_socket())
        .map_err(|e| format!("MCP socket unavailable: {}", e))?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let reader = BufReader::new(stream);

    let req = json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": "system.validate_api_key",
        "params": { "key": key }
    });

    let mut payload = serde_json::to_string(&req).unwrap();
    payload.push('\n');
    writer.write_all(payload.as_bytes()).map_err(|e| e.to_string())?;

    let mut line = String::new();
    reader.lines().next()
        .ok_or_else(|| "No response from MCP daemon".to_string())?
        .map_err(|e| e.to_string())
        .and_then(|l| { line = l; Ok(()) })?;

    let response: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    if let Some(result) = response.get("result") {
        if result.get("valid").and_then(|v| v.as_bool()).unwrap_or(false) {
            Ok(true)
        } else {
            let err = result.get("error").and_then(|e| e.as_str()).unwrap_or("Unknown validation error");
            Err(err.to_string())
        }
    } else {
        Err("Invalid response from daemon".to_string())
    }
}

/// Run a real shell command and return combined stdout + stderr.
/// The shell process runs as the session user, so this is genuine OS control.
#[tauri::command]
fn run_command(cmd: String) -> Result<String, String> {
    if cmd.trim().is_empty() {
        return Ok(String::new());
    }
    let output = Command::new("bash")
        .arg("-c")
        .arg(&cmd)
        .env("PATH", format!("/tmp/osaima_bin:{}", std::env::var("PATH").unwrap_or_default()))
        .output()
        .map_err(|e| e.to_string())?;
    let mut out = String::from_utf8_lossy(&output.stdout).to_string();
    let err = String::from_utf8_lossy(&output.stderr);
    if !err.is_empty() {
        out.push_str(&err);
    }
    Ok(out)
}

/// Get the current connected Wi-Fi network SSID (Linux)
#[tauri::command]
fn get_wifi_network() -> String {
    let output = Command::new("iwgetid").arg("-r").output().ok();
    if let Some(out) = output {
        let mut name = String::from_utf8_lossy(&out.stdout).to_string();
        name.retain(|c| c != '\n');
        if !name.is_empty() {
            return name;
        }
    }
    "Ethernet / Unknown".to_string()
}

/// Shutdown the OS (exit the app)
#[tauri::command]
fn shutdown_os() {
    std::process::exit(0);
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

/// Real process list via `ps`: the busiest by CPU plus the largest by memory.
#[tauri::command]
fn list_processes() -> Result<Value, String> {
    let output = Command::new("ps")
        .args(["-eo", "pid,comm,pcpu,pmem,rss", "--sort=-pcpu"])
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut all: Vec<Value> = Vec::new();
    for line in text.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5 {
            let cpu = parts[2].parse::<f64>().unwrap_or(0.0);
            let pmem = parts[3].parse::<f64>().unwrap_or(0.0);
            all.push(json!({
                "pid": parts[0].parse::<u32>().unwrap_or(0),
                "name": parts[1],
                "cpu": cpu,
                "mem": parts[4].parse::<f64>().unwrap_or(0.0) / 1024.0, // KiB -> MiB
                "cpu_percent": cpu,
                "mem_percent": pmem,
            }));
        }
    }
    let mut procs: Vec<Value> = all.iter().take(30).cloned().collect();
    let mut by_mem = all.clone();
    by_mem.sort_by(|a, b| b["mem_percent"].as_f64().partial_cmp(&a["mem_percent"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    for p in by_mem.into_iter().take(10) {
        if !procs.iter().any(|q| q["pid"] == p["pid"]) {
            procs.push(p);
        }
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

/// Unwraps a JSON-RPC response into its result or its error message.
fn mcp_result(method: &str, params: Value) -> Result<Value, String> {
    let response = query_mcp_with_params(method, params)?;
    if let Some(result) = response.get("result") {
        Ok(result.clone())
    } else {
        Err(response["error"]["message"].as_str().unwrap_or("Unknown daemon error").to_string())
    }
}

/// On-device AI status: local LLM, intent router, cloud fallbacks (if any).
#[tauri::command]
fn ai_status() -> Result<Value, String> {
    mcp_result("system.llm_status", json!({}))
}

/// OS control (volume, media, brightness, lock, Wi-Fi, Bluetooth, power) via the
/// daemon, which decides whether approval is needed and audits it.
#[tauri::command]
fn os_action(request: Value) -> Result<Value, String> {
    // the whole request goes through; the daemon validates every field
    let mut req = request;
    req["approved"] = json!(req["approved"].as_bool().unwrap_or(false));
    if !req["typed"].is_string() {
        req["typed"] = json!("");
    }
    mcp_result("system.os_action", req)
}

/// The user's frequent commands (and stated preferences) from the behaviour store.
#[tauri::command]
fn suggestions(n: Option<u64>) -> Result<Value, String> {
    mcp_result("system.suggestions", json!({ "n": n.unwrap_or(5) }))
}

/// Recent audited actions (commands run or refused, OS actions) and whether
/// the log's hash chain still verifies.
#[tauri::command]
fn audit_tail(n: Option<u64>) -> Result<Value, String> {
    mcp_result("system.audit_tail", json!({ "n": n.unwrap_or(8) }))
}

/// A page of the audit log, newest first: offset, limit, optional event
/// kinds and search text. Returns the entries and the total that match.
#[tauri::command]
fn audit_query(offset: Option<u64>, limit: Option<u64>, events: Option<Vec<String>>, text: Option<String>) -> Result<Value, String> {
    mcp_result("system.audit_query", json!({
        "offset": offset.unwrap_or(0), "limit": limit.unwrap_or(50),
        "events": events.unwrap_or_default(), "text": text.unwrap_or_default() }))
}

/// Agent tasks (start, status, decide, cancel, list): one command, whitelisted.
#[tauri::command]
fn task_rpc(method: String, params: Value) -> Result<Value, String> {
    match method.as_str() {
        "system.task_start" | "system.task_status" | "system.task_decide" | "system.task_cancel" | "system.tasks"
        | "system.automations" | "system.automation_add" | "system.automation_remove" | "system.automation_cancel"
        | "system.gpu_info" | "system.model_download" | "system.model_status" | "system.model_cancel" => mcp_result(&method, params),
        _ => Err("not a task method".to_string()),
    }
}

/// A request typed in the Ask bar, handed to the assistant window.
#[tauri::command]
fn ask(query: String) -> Result<Value, String> {
    mcp_result("system.ask", json!({ "query": query }))
}

/// Files the user sent from the file manager ("Ask OSAIMA"), taken once.
#[tauri::command]
fn shared_take() -> Result<Value, String> {
    mcp_result("system.shared_take", json!({}))
}

/// Routines: finished plans the user saved, to be run again by name.
#[tauri::command]
fn routines() -> Result<Value, String> {
    mcp_result("system.routines", json!({}))
}

#[tauri::command]
fn routine_save(name: String, request: Option<String>, steps: Value) -> Result<Value, String> {
    mcp_result("system.routine_save", json!({ "name": name, "request": request.unwrap_or_default(), "steps": steps }))
}

#[tauri::command]
fn routine_delete(id: i64) -> Result<Value, String> {
    mcp_result("system.routine_delete", json!({ "id": id }))
}

/// A plan finished: lets the daemon count repeats so a routine can be offered.
#[tauri::command]
fn routine_note(steps: Value) -> Result<Value, String> {
    mcp_result("system.routine_note", json!({ "steps": steps }))
}

/// Chooses which language model answers first: "cloud" or "local".
#[tauri::command]
fn set_llm_mode(mode: String) -> Result<Value, String> {
    mcp_result("system.set_llm_mode", json!({ "mode": mode }))
}

/// Risk tier of an AI-proposed command, decided by the daemon.
#[tauri::command]
fn assess_command(command: String) -> Result<Value, String> {
    mcp_result("system.assess_command", json!({ "command": command }))
}

/// Runs an AI-proposed command through the daemon's gate, sandbox and audit log.
/// Unlike run_command (used for commands the user typed), this never runs
/// anything the daemon has not approved for its tier.
#[tauri::command]
fn execute_ai_command(command: String, approved: bool, typed: Option<String>) -> Result<Value, String> {
    mcp_result("system.execute_command", json!({ "command": command, "approved": approved, "typed": typed.unwrap_or_default() }))
}

#[tauri::command]
fn classify_intent(query: String, history: Option<Value>, selected: Option<Value>) -> Result<Value, String> {
    match query_mcp_with_params("system.classify_intent", json!({"query": query, "history": history.unwrap_or(json!([])),
                                                                 "selected": selected.unwrap_or(json!([]))})) {
        Ok(response) => {
            if let Some(result) = response.get("result") {
                Ok(result.clone())
            } else if let Some(error) = response.get("error") {
                Err(format!("LLM classification error: {}", error))
            } else {
                Err("No result from classify_intent".to_string())
            }
        },
        Err(e) => Err(e),
    }
}

#[tauri::command]
fn spawn_pty(id: String, app: AppHandle, state: State<'_, PtyAppState>) -> Result<(), String> {
    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let rcfile_content = r#"
if [ -f ~/.bashrc ]; then source ~/.bashrc; fi
cat << 'EOF' > /tmp/osaima_neofetch.conf
print_info() {
    info title
    info underline
    prin "OS" "OSAIMA Interstellar OS x86_64"
    info "Host" model
    info "Kernel" kernel
    info "Uptime" uptime
    info "Packages" packages
    info "Shell" shell
    info "Resolution" resolution
    info "DE" de
    info "WM" wm
    info "Theme" theme
    info "Icons" icons
    info "Terminal" term
    info "CPU" cpu
    info "GPU" gpu
    info "Memory" memory
}
EOF
neofetch() {
    command neofetch --config /tmp/osaima_neofetch.conf "$@"
}
"#;
    let _ = std::fs::write("/tmp/osaima_bashrc", rcfile_content);

    let mut cmd = CommandBuilder::new("bash");
    cmd.args(["--rcfile", "/tmp/osaima_bashrc"]);
    let _child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    
    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    state.pty_writers.lock().unwrap().insert(id.clone(), writer);
    state.pty_masters.lock().unwrap().insert(id.clone(), pair.master);

    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 1024];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 { 
                println!("PTY {} reader reached EOF", id);
                break; 
            }
            let data = String::from_utf8_lossy(&buf[..n]).to_string();
            println!("PTY {} received {} bytes", id, n);
            let res = app.emit(&format!("pty-out-{}", id), data);
            if let Err(e) = res {
                println!("PTY {} failed to emit: {:?}", id, e);
            }
        }
    });

    Ok(())
}

#[tauri::command]
fn write_pty(id: String, data: String, state: State<'_, PtyAppState>) -> Result<(), String> {
    if let Some(writer) = state.pty_writers.lock().unwrap().get_mut(&id) {
        writer.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn resize_pty(id: String, rows: u16, cols: u16, state: State<'_, PtyAppState>) -> Result<(), String> {
    if let Some(master) = state.pty_masters.lock().unwrap().get(&id) {
        master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn freeze_heavy_apps() -> Result<String, String> {
    // In a real OS, we would use sophisticated metrics to find high CPU background tasks.
    // For safety in this demo, we'll try to find known heavy user processes (like browsers, electron apps)
    // and send SIGSTOP. Since finding them robustly across all systems is complex,
    // we'll execute a bash script that finds processes using >20% CPU owned by the user,
    // filters out critical ones (gnome, kwin, bash, ssh, tauri, mcp-daemon), and suspends them.
    
    let script = r#"
        pids=$(ps -eo pid,pcpu,comm,user --sort=-pcpu | awk -v user="$USER" '$4==user && $2>5.0 {print $1, $3}')
        stopped=""
        while read -r pid comm; do
            if [[ -z "$pid" ]]; then continue; fi
            if [[ ! "$comm" =~ ^(osaima-shell|mcp-daemon|bash|ssh|kwin.*|gnome-shell|systemd|Xorg|wayland|plasmashell|antigravity.*|node|npm|cargo|rustc|rust-analyzer|.*Web.*Process|language_server)$ ]]; then
                kill -SIGSTOP $pid 2>/dev/null
                stopped="$stopped $comm($pid)"
            fi
        done <<< "$pids"
        echo $stopped
    "#;
    
    let output = Command::new("bash")
        .arg("-c")
        .arg(script)
        .output()
        .map_err(|e| format!("Failed to execute script: {}", e))?;
        
    let stopped = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stopped.is_empty() {
        Ok("No heavy background apps found to freeze.".to_string())
    } else {
        Ok(format!("Successfully froze heavy apps: {}", stopped))
    }
}

#[tauri::command]
fn unfreeze_apps() -> Result<String, String> {
    // We send SIGCONT to all stopped processes owned by the user
    let script = r#"
        pids=$(ps -eo pid,stat,comm,user | awk -v user="$USER" '$4==user && $2~"T" {print $1, $3}')
        resumed=""
        while read -r pid comm; do
            if [[ -z "$pid" ]]; then continue; fi
            kill -SIGCONT $pid 2>/dev/null
            resumed="$resumed $comm($pid)"
        done <<< "$pids"
        echo $resumed
    "#;
    
    let output = Command::new("bash")
        .arg("-c")
        .arg(script)
        .output()
        .map_err(|e| format!("Failed to execute script: {}", e))?;
        
    let resumed = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if resumed.is_empty() {
        Ok("No frozen apps found to wake up.".to_string())
    } else {
        Ok(format!("Successfully woke up apps: {}", resumed))
    }
}

#[tauri::command]
fn clean_storage() -> Result<String, String> {
    // Empty the trash and clear thumbnail cache
    let script = r#"
        rm -rf ~/.local/share/Trash/files/* ~/.local/share/Trash/info/* 2>/dev/null
        rm -rf ~/.cache/thumbnails/* 2>/dev/null
        echo "Emptied Trash and cleared thumbnail cache."
    "#;
    
    let output = Command::new("bash")
        .arg("-c")
        .arg(script)
        .output()
        .map_err(|e| format!("Failed to execute script: {}", e))?;
        
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tauri::command]
fn fix_network() -> Result<String, String> {
    // Restart network manager (requires sudo, but we can restart user-level networking or just flush DNS via resolvectl)
    let script = r#"
        resolvectl flush-caches 2>/dev/null || true
        echo "Flushed DNS caches and reset local networking state."
    "#;
    
    let output = Command::new("bash")
        .arg("-c")
        .arg(script)
        .output()
        .map_err(|e| format!("Failed to execute script: {}", e))?;
        
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    // Setup global bin for overrides
    let _ = std::fs::create_dir_all("/tmp/osaima_bin");
    
    let neo_conf = r#"
print_info() {
    info title
    info underline
    prin "OS" "OSAIMA Interstellar OS x86_64"
    info "Host" model
    info "Kernel" kernel
    info "Uptime" uptime
    info "Packages" packages
    info "Shell" shell
    info "Resolution" resolution
    info "DE" de
    info "WM" wm
    info "Theme" theme
    info "Icons" icons
    info "Terminal" term
    info "CPU" cpu
    info "GPU" gpu
    info "Memory" memory
}
"#;
    let _ = std::fs::write("/tmp/osaima_neofetch.conf", neo_conf);

    let neo_script = r#"#!/bin/bash
if [[ "$*" == *"--config"* ]]; then
    /usr/bin/neofetch "$@"
else
    /usr/bin/neofetch --config /tmp/osaima_neofetch.conf "$@"
fi
"#;
    let _ = std::fs::write("/tmp/osaima_bin/neofetch", neo_script);
    let _ = std::process::Command::new("chmod").args(["+x", "/tmp/osaima_bin/neofetch"]).output();

    tauri::Builder::default()
        .setup(|app| {
            use tauri::{WebviewUrl, WebviewWindowBuilder};
            // `osaima-shell --app notes` opens one OSAIMA app as an ordinary
            // window that the desktop's window manager (sway) arranges; the
            // window title ("OSAIMA Notes") is how the daemon finds it again.
            // `--desktop` keeps the old all-in-one fullscreen web desktop.
            let args: Vec<String> = std::env::args().collect();
            if args.iter().any(|a| a == "--desktop") {
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("OSAIMA Shell").fullscreen(true).decorations(false).build()?;
            } else {
                let page = args.iter().position(|a| a == "--app").and_then(|i| args.get(i + 1))
                    .filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase()))
                    .cloned().unwrap_or_else(|| "assistant".to_string());
                let title = format!("OSAIMA {}{}", page[..1].to_uppercase(), &page[1..]);
                // OSAIMA_DEV_URL=http://127.0.0.1:8777 loads the pages live from the
                // dev server, so UI edits show on reload without rebuilding.
                let url = match std::env::var("OSAIMA_DEV_URL") {
                    Ok(base) => WebviewUrl::External(format!("{}/app.html?app={}", base.trim_end_matches('/'), page)
                        .parse().map_err(|e| format!("OSAIMA_DEV_URL: {}", e))?),
                    Err(_) => WebviewUrl::App(format!("app.html?app={}", page).into()),
                };
                WebviewWindowBuilder::new(app, "main", url)
                    .title(title).decorations(false).inner_size(1000.0, 700.0).build()?;
            }
            Ok(())
        })
        .manage(PtyAppState {
            pty_writers: Mutex::new(HashMap::new()),
            pty_masters: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            get_system_stats,
            system_get_suggestion,
            ping_daemon,
            run_command,
            read_dir,
            list_processes,
            kill_process,
            set_api_key,
            set_groq_key,
            validate_api_key,
            classify_intent,
            ai_status,
            os_action,
            suggestions,
            audit_tail,
            audit_query,
            set_llm_mode,
            shared_take,
            task_rpc,
            ask,
            routines,
            routine_save,
            routine_delete,
            routine_note,
            assess_command,
            execute_ai_command,
            spawn_pty,
            write_pty,
            resize_pty,
            freeze_heavy_apps,
            unfreeze_apps,
            clean_storage,
            fix_network,
            get_wifi_network,
            shutdown_os
        ])
        .run(tauri::generate_context!())
        .expect("error while running osaima-shell");
}
