//! Bridge to the voice service (`osaima-voice`).
//!
//! The service owns the microphone and the speech models; the shell only relays
//! calls to it. One request and one reply per call over the per-user Unix socket
//! (newline-delimited JSON-RPC 2.0), the same style as the daemon bridge.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::{json, Value};

/// A hung service must not hang the shell.
const TIMEOUT: Duration = Duration::from_secs(10);

/// `$OSAIMA_VOICE_SOCKET`, else `$XDG_RUNTIME_DIR/osaima/voice.sock`, else /tmp
/// (the same lookup as the service).
pub fn socket_path() -> String {
    if let Ok(p) = std::env::var("OSAIMA_VOICE_SOCKET") {
        if !p.is_empty() {
            return p;
        }
    }
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) if !dir.is_empty() => format!("{}/osaima/voice.sock", dir),
        _ => "/tmp/osaima_voice.sock".to_string(),
    }
}

/// Send one request to the service at `path` and return its `result`.
pub fn call(path: &str, method: &str, params: Value) -> Result<Value, String> {
    let stream = UnixStream::connect(path)
        .map_err(|e| format!("The voice service is not running ({}: {})", path, e))?;
    stream.set_read_timeout(Some(TIMEOUT)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(TIMEOUT)).map_err(|e| e.to_string())?;

    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    request.push('\n');
    writer.write_all(request.as_bytes()).map_err(|e| e.to_string())?;

    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).map_err(|e| format!("The voice service did not answer: {}", e))?;
    if reply.trim().is_empty() {
        return Err("The voice service closed the connection".to_string());
    }
    let message: Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
    if let Some(error) = message.get("error") {
        return Err(error["message"].as_str().unwrap_or("The voice service reported an error").to_string());
    }
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
}

/// Relay a `voice.*` call from the UI. Nothing else may go through here.
#[tauri::command]
pub async fn voice_call(method: String, params: Option<Value>) -> Result<Value, String> {
    if !method.starts_with("voice.") {
        return Err(format!("not a voice method: {}", method));
    }
    let params = params.unwrap_or_else(|| json!({}));
    tauri::async_runtime::spawn_blocking(move || call(&socket_path(), &method, params))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// Serve one connection: read a request line, answer with `reply`.
    fn serve_once(name: &str, reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let path = std::env::temp_dir().join(format!("osaima-voice-{}-{}.sock", std::process::id(), name));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            let mut writer = stream;
            writer.write_all(reply.as_bytes()).unwrap();
            line
        });
        (path.to_string_lossy().into_owned(), handle)
    }

    #[test]
    fn a_call_is_relayed_and_the_result_returned() {
        let (path, server) = serve_once("ok", "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"state\":\"idle\"}}\n");
        let result = call(&path, "voice.status", json!({})).unwrap();
        assert_eq!(result["state"], "idle");
        let seen: Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(seen["method"], "voice.status");
        assert_eq!(seen["jsonrpc"], "2.0");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parameters_reach_the_service() {
        let (path, server) = serve_once("params", "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n");
        call(&path, "voice.speak", json!({ "text": "hello" })).unwrap();
        let seen: Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(seen["params"]["text"], "hello");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_error_from_the_service_becomes_the_error_message() {
        let (path, server) = serve_once("err", "{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32002,\"message\":\"not listening\"}}\n");
        let err = call(&path, "voice.stop", json!({})).unwrap_err();
        assert_eq!(err, "not listening");
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_service_is_a_clear_error() {
        let err = call("/nonexistent/voice.sock", "voice.status", json!({})).unwrap_err();
        assert!(err.contains("not running"), "{err}");
    }

    #[test]
    fn a_service_that_hangs_up_is_reported() {
        let (path, server) = serve_once("empty", "");
        let err = call(&path, "voice.status", json!({})).unwrap_err();
        assert!(err.contains("closed the connection"), "{err}");
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn only_voice_methods_are_relayed() {
        let err = tauri::async_runtime::block_on(voice_call("system.audit_tail".to_string(), None)).unwrap_err();
        assert!(err.contains("not a voice method"), "{err}");
    }
}
