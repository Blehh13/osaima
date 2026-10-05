//! Window management through the sway IPC protocol (i3-compatible):
//! list, focus and close real application windows in the session.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use super::{
    destructive, no_args, read_only, reversible, to_value, tool, Args, ToolContext, ToolError,
    ToolResult,
};

const MAGIC: &[u8; 6] = b"i3-ipc";
const RUN_COMMAND: u32 = 0;
const GET_TREE: u32 = 4;
const TIMEOUT: Duration = Duration::from_secs(3);
const MAX_REPLY_BYTES: usize = 32 * 1024 * 1024;

pub fn definitions() -> Vec<Value> {
    let target = json!({
        "type": "object",
        "properties": {
            "id": { "type": "integer", "description": "Window id from list_windows" },
            "match": { "type": "string", "description": "Text matched against window title and app id" }
        },
        "additionalProperties": false
    });
    vec![
        tool(
            "list_windows",
            "List windows",
            "Open application windows with their id, title, app, workspace and which one is focused.",
            no_args(),
            read_only(),
        ),
        tool(
            "focus_window",
            "Focus a window",
            "Bring a window to the front, by id or by matching its title or app.",
            target.clone(),
            reversible(),
        ),
        tool(
            "close_window",
            "Close a window",
            "Close a window, by id or by matching its title or app. Unsaved work may be lost, so confirm with the user first.",
            target,
            destructive(),
        ),
    ]
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Window {
    pub id: u64,
    pub title: String,
    pub app: Option<String>,
    pub pid: Option<u64>,
    pub workspace: Option<String>,
    pub focused: bool,
    pub visible: bool,
}

pub async fn list_windows(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    Args::parse(arguments, &[])?;
    let socket = sway_socket(ctx)?;
    let windows = windows(socket).await?;
    Ok(json!({ "windows": to_value(&windows) }))
}

pub async fn focus_window(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    act_on_window(ctx, arguments, "focus").await
}

pub async fn close_window(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    act_on_window(ctx, arguments, "kill").await
}

async fn act_on_window(ctx: &ToolContext, arguments: Option<Value>, command: &str) -> ToolResult {
    let args = Args::parse(arguments, &["id", "match"])?;
    let id = args.u64("id")?;
    let pattern = args.str("match")?.map(str::trim).filter(|m| !m.is_empty());
    if id.is_some() == pattern.is_some() {
        return Err(ToolError::invalid("give exactly one of id or match"));
    }
    let socket = sway_socket(ctx)?;
    let all = windows(socket).await?;
    let window = match (id, pattern) {
        (Some(id), _) => all
            .into_iter()
            .find(|w| w.id == id)
            .ok_or_else(|| ToolError::failed(format!("no window with id {id}")))?,
        (_, Some(pattern)) => pick(all, pattern)?,
        _ => unreachable!("checked above"),
    };
    let reply = ipc(
        socket,
        RUN_COMMAND,
        &format!("[con_id={}] {command}", window.id),
    )
    .await?;
    let ok = reply
        .as_array()
        .and_then(|r| r.first())
        .and_then(|r| r["success"].as_bool())
        .unwrap_or(false);
    if !ok {
        let reason = reply
            .pointer("/0/error")
            .and_then(Value::as_str)
            .unwrap_or("sway refused the command");
        return Err(ToolError::failed(reason.to_string()));
    }
    Ok(
        json!({ "window": to_value(&window), "action": if command == "kill" { "closed" } else { "focused" } }),
    )
}

/// The single window whose title or app contains `pattern`.
fn pick(windows: Vec<Window>, pattern: &str) -> Result<Window, ToolError> {
    let needle = pattern.to_lowercase();
    let mut found: Vec<Window> = windows
        .into_iter()
        .filter(|w| {
            w.title.to_lowercase().contains(&needle)
                || w.app
                    .as_deref()
                    .is_some_and(|a| a.to_lowercase().contains(&needle))
        })
        .collect();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(ToolError::failed(format!(
            "no open window matches {pattern:?}"
        ))),
        _ => Err(ToolError::failed(format!(
            "{pattern:?} matches several windows: {}. Use an id from list_windows.",
            found
                .iter()
                .map(|w| format!("{} \"{}\"", w.id, w.title))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn sway_socket(ctx: &ToolContext) -> Result<&Path, ToolError> {
    ctx.paths.sway_socket.as_deref().ok_or_else(|| {
        ToolError::failed("window control needs the sway session (SWAYSOCK is not set)")
    })
}

async fn windows(socket: &Path) -> Result<Vec<Window>, ToolError> {
    let tree = ipc(socket, GET_TREE, "").await?;
    let mut out = Vec::new();
    collect(&tree, None, &mut out);
    Ok(out)
}

/// Walk the layout tree; leaf containers that belong to a process are windows.
fn collect(node: &Value, workspace: Option<&str>, out: &mut Vec<Window>) {
    let workspace = if node["type"] == "workspace" {
        node["name"].as_str()
    } else {
        workspace
    };
    let children: Vec<&Value> = ["nodes", "floating_nodes"]
        .iter()
        .filter_map(|k| node[*k].as_array())
        .flatten()
        .collect();
    let is_window = matches!(node["type"].as_str(), Some("con" | "floating_con"))
        && children.is_empty()
        && node["pid"].is_number();
    if is_window {
        let app = node["app_id"]
            .as_str()
            .or_else(|| {
                node.pointer("/window_properties/class")
                    .and_then(Value::as_str)
            })
            .map(str::to_string);
        out.push(Window {
            id: node["id"].as_u64().unwrap_or_default(),
            title: node["name"].as_str().unwrap_or_default().to_string(),
            app,
            pid: node["pid"].as_u64(),
            workspace: workspace.map(str::to_string),
            focused: node["focused"].as_bool().unwrap_or(false),
            visible: node["visible"].as_bool().unwrap_or(false),
        });
    }
    for child in children {
        collect(child, workspace, out);
    }
}

/// One request/reply exchange on the sway socket.
async fn ipc(socket: &Path, kind: u32, payload: &str) -> Result<Value, ToolError> {
    let exchange = async {
        let mut stream = UnixStream::connect(socket).await?;
        let len = u32::try_from(payload.len()).map_err(std::io::Error::other)?;
        let mut message = Vec::with_capacity(14 + payload.len());
        message.extend_from_slice(MAGIC);
        message.extend_from_slice(&len.to_ne_bytes());
        message.extend_from_slice(&kind.to_ne_bytes());
        message.extend_from_slice(payload.as_bytes());
        stream.write_all(&message).await?;

        let mut header = [0u8; 14];
        stream.read_exact(&mut header).await?;
        if &header[..6] != MAGIC {
            return Err(std::io::Error::other("not a sway IPC reply"));
        }
        let len = u32::from_ne_bytes(header[6..10].try_into().expect("4 bytes")) as usize;
        if len > MAX_REPLY_BYTES {
            return Err(std::io::Error::other("sway reply too large"));
        }
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).await?;
        Ok(body)
    };
    let body = tokio::time::timeout(TIMEOUT, exchange)
        .await
        .map_err(|_| ToolError::failed("sway did not respond"))?
        .map_err(|e| ToolError::failed(format!("could not talk to sway: {e}")))?;
    serde_json::from_slice(&body)
        .map_err(|e| ToolError::failed(format!("bad reply from sway: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::context;
    use std::sync::{Arc, Mutex};
    use tokio::net::UnixListener;

    fn tree() -> Value {
        json!({
            "id": 1, "type": "root", "nodes": [{
                "id": 2, "type": "output", "name": "eDP-1", "nodes": [
                    { "id": 3, "type": "workspace", "name": "1", "nodes": [
                        { "id": 10, "type": "con", "name": "Mozilla Firefox", "app_id": "firefox",
                          "pid": 100, "focused": true, "visible": true, "nodes": [], "floating_nodes": [] },
                        { "id": 4, "type": "con", "name": "split", "nodes": [
                            { "id": 11, "type": "con", "name": "~ — foot", "app_id": "foot",
                              "pid": 101, "focused": false, "visible": true, "nodes": [] }
                        ]}
                    ], "floating_nodes": [
                        { "id": 12, "type": "floating_con", "name": "Steam", "app_id": null,
                          "window_properties": { "class": "steam" }, "pid": 102, "nodes": [] }
                    ]},
                    { "id": 5, "type": "workspace", "name": "2", "nodes": [], "floating_nodes": [] }
                ]
            }]
        })
    }

    /// A fake sway that serves `tree()` and records RUN_COMMAND payloads.
    async fn fake_sway(path: &Path) -> Arc<Mutex<Vec<String>>> {
        let listener = UnixListener::bind(path).unwrap();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&commands);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut header = [0u8; 14];
                stream.read_exact(&mut header).await.unwrap();
                let len = u32::from_ne_bytes(header[6..10].try_into().unwrap()) as usize;
                let kind = u32::from_ne_bytes(header[10..14].try_into().unwrap());
                let mut payload = vec![0u8; len];
                stream.read_exact(&mut payload).await.unwrap();
                let reply = if kind == GET_TREE {
                    tree()
                } else {
                    seen.lock()
                        .unwrap()
                        .push(String::from_utf8(payload).unwrap());
                    json!([{ "success": true }])
                };
                let body = reply.to_string();
                let mut msg = MAGIC.to_vec();
                msg.extend_from_slice(&(body.len() as u32).to_ne_bytes());
                msg.extend_from_slice(&kind.to_ne_bytes());
                msg.extend_from_slice(body.as_bytes());
                stream.write_all(&msg).await.unwrap();
            }
        });
        commands
    }

    fn sway_context(root: &Path) -> ToolContext {
        let mut ctx = context(root);
        let mut paths = (*ctx.paths).clone();
        paths.sway_socket = Some(root.join("sway.sock"));
        ctx.paths = Arc::new(paths);
        ctx
    }

    #[test]
    fn collects_leaf_windows_with_workspaces() {
        let mut out = Vec::new();
        collect(&tree(), None, &mut out);
        let ids: Vec<u64> = out.iter().map(|w| w.id).collect();
        assert_eq!(ids, [10, 11, 12]);
        assert!(out.iter().all(|w| w.workspace.as_deref() == Some("1")));
        assert_eq!(out[2].app.as_deref(), Some("steam"));
        assert!(out[0].focused);
    }

    #[tokio::test]
    async fn lists_focuses_and_closes_through_ipc() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = sway_context(dir.path());
        let commands = fake_sway(&dir.path().join("sway.sock")).await;

        let listed = list_windows(&ctx, None).await.unwrap();
        assert_eq!(listed["windows"].as_array().unwrap().len(), 3);

        focus_window(&ctx, Some(json!({ "match": "foot" })))
            .await
            .unwrap();
        let closed = close_window(&ctx, Some(json!({ "id": 12 }))).await.unwrap();
        assert_eq!(closed["action"], "closed");
        assert_eq!(
            *commands.lock().unwrap(),
            ["[con_id=11] focus", "[con_id=12] kill"]
        );
    }

    #[tokio::test]
    async fn ambiguous_or_missing_targets_fail_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = sway_context(dir.path());
        fake_sway(&dir.path().join("sway.sock")).await;

        let err = focus_window(&ctx, Some(json!({ "match": "o" })))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Failed(m) if m.contains("several windows")));
        let err = close_window(&ctx, Some(json!({ "id": 999 })))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Failed(m) if m.contains("no window")));
        let err = close_window(&ctx, Some(json!({}))).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn explains_missing_sway_session() {
        let dir = tempfile::tempdir().unwrap();
        let err = list_windows(&context(dir.path()), None).await.unwrap_err();
        assert!(matches!(err, ToolError::Failed(m) if m.contains("SWAYSOCK")));
    }
}
