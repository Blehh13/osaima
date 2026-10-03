//! Client for the OSAIMA AI Core (the MCP daemon in `ai-core/mcp-daemon`).

use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Same lookup order as the daemon: `$OSAIMA_MCP_SOCKET`, then
/// `$XDG_RUNTIME_DIR/osaima/mcp.sock`, then `/tmp/osaima-<uid>/mcp.sock`.
pub fn socket_path() -> PathBuf {
    if let Some(path) = env::var_os("OSAIMA_MCP_SOCKET").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR").filter(|p| !p.is_empty()) {
        return PathBuf::from(dir).join("osaima").join("mcp.sock");
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    PathBuf::from(format!("/tmp/osaima-{uid}")).join("mcp.sock")
}

/// Send one JSON-RPC request and return its `result`.
pub async fn request(method: &str, params: Value) -> Result<Value, String> {
    tokio::time::timeout(REQUEST_TIMEOUT, send(method, params))
        .await
        .map_err(|_| "AI Core did not respond in time".to_string())?
}

/// Call an MCP tool and return its `structuredContent`. In-band tool errors
/// (`isError: true`) become `Err` with the tool's message.
pub async fn call_tool(name: &str, arguments: Value) -> Result<Value, String> {
    let result = request(
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    )
    .await?;
    if result["isError"].as_bool().unwrap_or(false) {
        let message = result
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or("tool failed");
        return Err(message.to_string());
    }
    Ok(result
        .get("structuredContent")
        .cloned()
        .unwrap_or(Value::Null))
}

async fn send(method: &str, params: Value) -> Result<Value, String> {
    let path = socket_path();
    let stream = UnixStream::connect(&path)
        .await
        .map_err(|e| format!("AI Core unavailable at {}: {e}", path.display()))?;
    let (reader, mut writer) = stream.into_split();

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    let mut line = message.to_string();
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())?;

    let mut reply = String::new();
    BufReader::new(reader)
        .read_line(&mut reply)
        .await
        .map_err(|e| e.to_string())?;
    parse_reply(&reply)
}

fn parse_reply(reply: &str) -> Result<Value, String> {
    let mut response: Value =
        serde_json::from_str(reply).map_err(|e| format!("invalid reply from AI Core: {e}"))?;
    if let Some(error) = response.get("error") {
        return Err(error["message"]
            .as_str()
            .unwrap_or("AI Core returned an error")
            .to_string());
    }
    response
        .get_mut("result")
        .map(Value::take)
        .ok_or_else(|| "AI Core reply had no result".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_results_and_errors() {
        assert_eq!(
            parse_reply(r#"{"jsonrpc":"2.0","id":1,"result":{"a":1}}"#),
            Ok(json!({ "a": 1 }))
        );
        assert_eq!(
            parse_reply(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}}"#),
            Err("nope".to_string())
        );
        assert!(parse_reply("garbage").is_err());
    }

    #[test]
    fn env_override_wins() {
        env::set_var("OSAIMA_MCP_SOCKET", "/run/test.sock");
        assert_eq!(socket_path(), PathBuf::from("/run/test.sock"));
        env::remove_var("OSAIMA_MCP_SOCKET");
    }
}
