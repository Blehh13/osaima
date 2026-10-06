//! Bridge to the OSAIMA agent service (`ai-core/agent`).
//!
//! One persistent connection carries newline-delimited JSON-RPC. Replies are
//! matched to requests by id; `agent.event` notifications (streamed text, tool
//! calls, approval requests) are handed to an event sink, which the app wires
//! to a Tauri event for the UI. The connection is re-established on demand.

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::oneshot;

/// Replies are quick (the answer itself arrives as events); this only bounds a hung service.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;
pub type EventSink = Arc<dyn Fn(Value) + Send + Sync>;

/// `$OSAIMA_AGENT_SOCKET`, else `agent.sock` next to the AI Core's socket.
pub fn socket_path() -> PathBuf {
    if let Some(path) = env::var_os("OSAIMA_AGENT_SOCKET").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    crate::mcp::runtime_dir().join("agent.sock")
}

struct Link {
    writer: OwnedWriteHalf,
    pending: Pending,
    alive: Arc<AtomicBool>,
}

struct Inner {
    path: PathBuf,
    sink: EventSink,
    link: tokio::sync::Mutex<Option<Link>>,
    next_id: AtomicU64,
}

#[derive(Clone)]
pub struct AgentBridge(Arc<Inner>);

impl AgentBridge {
    pub fn new(path: PathBuf, sink: EventSink) -> Self {
        Self(Arc::new(Inner {
            path,
            sink,
            link: tokio::sync::Mutex::new(None),
            next_id: AtomicU64::new(1),
        }))
    }

    /// Send one request and wait for its reply (the `result`).
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        let pending = {
            let mut guard = self.0.link.lock().await;
            if guard.as_ref().is_none_or(|l| !l.alive.load(Ordering::Acquire)) {
                *guard = Some(self.connect().await?);
            }
            let link = guard.as_mut().expect("link was just established");
            link.pending
                .lock()
                .expect("pending map poisoned")
                .insert(id, tx);
            let mut line =
                json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
                    .to_string();
            line.push('\n');
            if let Err(err) = link.writer.write_all(line.as_bytes()).await {
                link.alive.store(false, Ordering::Release);
                link.pending.lock().expect("pending map poisoned").remove(&id);
                return Err(format!("lost connection to the assistant service: {err}"));
            }
            Arc::clone(&link.pending)
        };

        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err("the assistant service disconnected".into()),
            Err(_) => {
                pending.lock().expect("pending map poisoned").remove(&id);
                Err("the assistant service did not respond in time".into())
            }
        }
    }

    async fn connect(&self) -> Result<Link, String> {
        let stream = UnixStream::connect(&self.0.path).await.map_err(|err| {
            format!(
                "The assistant service is not running ({}: {err})",
                self.0.path.display()
            )
        })?;
        let (reader, writer) = stream.into_split();
        let pending: Pending = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        tokio::spawn(read_loop(
            reader,
            Arc::clone(&pending),
            Arc::clone(&alive),
            Arc::clone(&self.0.sink),
        ));
        Ok(Link {
            writer,
            pending,
            alive,
        })
    }
}

async fn read_loop(reader: OwnedReadHalf, pending: Pending, alive: Arc<AtomicBool>, sink: EventSink) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(mut message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(id) = message.get("id").and_then(Value::as_u64) {
            let reply = match message.get_mut("error") {
                Some(error) => Err(error["message"]
                    .as_str()
                    .unwrap_or("the assistant service returned an error")
                    .to_string()),
                None => Ok(message
                    .get_mut("result")
                    .map(Value::take)
                    .unwrap_or(Value::Null)),
            };
            if let Some(tx) = pending.lock().expect("pending map poisoned").remove(&id) {
                let _ = tx.send(reply);
            }
        } else if message["method"] == "agent.event" {
            sink(message["params"].take());
        }
    }
    alive.store(false, Ordering::Release);
    // Dropping the senders fails every waiting request with "disconnected".
    pending.lock().expect("pending map poisoned").clear();
    sink(json!({ "type": "disconnected" }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    fn collector() -> (EventSink, Arc<Mutex<Vec<Value>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        let sink: EventSink = Arc::new(move |event| seen.lock().unwrap().push(event));
        (sink, events)
    }

    /// Serves one connection: echoes `ping`, emits an event for `chat`, and
    /// hangs up after `drop_after` replies if set.
    async fn serve_once(listener: &UnixListener, drop_after: Option<usize>) {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let mut replies = 0;
        while let Some(line) = lines.next_line().await.unwrap() {
            let request: Value = serde_json::from_str(&line).unwrap();
            let id = request["id"].clone();
            let mut out = String::new();
            match request["method"].as_str().unwrap() {
                "agent.chat" => {
                    out += &json!({"jsonrpc":"2.0","method":"agent.event",
                        "params":{"type":"text","delta":"hi","turn_id":"t1"}})
                    .to_string();
                    out += "\n";
                    out += &json!({"jsonrpc":"2.0","id":id,"result":{"turn_id":"t1"}}).to_string();
                }
                "agent.bad" => {
                    out += &json!({"jsonrpc":"2.0","id":id,
                        "error":{"code":-32602,"message":"nope"}})
                    .to_string();
                }
                _ => {
                    out += &json!({"jsonrpc":"2.0","id":id,"result":request["params"]}).to_string();
                }
            }
            out += "\n";
            writer.write_all(out.as_bytes()).await.unwrap();
            replies += 1;
            if drop_after == Some(replies) {
                return;
            }
        }
    }

    #[tokio::test]
    async fn replies_and_events_are_routed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.sock");
        let listener = UnixListener::bind(&path).unwrap();
        tokio::spawn(async move { serve_once(&listener, None).await });
        let (sink, events) = collector();
        let bridge = AgentBridge::new(path, sink);

        let echoed = bridge.call("agent.status", json!({"a": 1})).await.unwrap();
        assert_eq!(echoed, json!({"a": 1}));

        let started = bridge.call("agent.chat", json!({"message": "x"})).await.unwrap();
        assert_eq!(started["turn_id"], "t1");
        // The event is delivered by the reader task; give it a moment.
        for _ in 0..50 {
            if !events.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(events.lock().unwrap()[0]["delta"], "hi");

        let err = bridge.call("agent.bad", json!({})).await.unwrap_err();
        assert_eq!(err, "nope");
    }

    #[tokio::test]
    async fn reconnects_after_the_service_drops() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.sock");
        let listener = UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            serve_once(&listener, Some(1)).await;
            serve_once(&listener, None).await;
        });
        let (sink, events) = collector();
        let bridge = AgentBridge::new(path, sink);

        assert!(bridge.call("agent.status", json!(1)).await.is_ok());
        // The server hung up after one reply; wait for the reader to notice.
        for _ in 0..100 {
            if events.lock().unwrap().iter().any(|e| e["type"] == "disconnected") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(events.lock().unwrap().iter().any(|e| e["type"] == "disconnected"));
        assert_eq!(bridge.call("agent.status", json!(2)).await.unwrap(), json!(2));
    }

    #[tokio::test]
    async fn a_missing_service_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let (sink, _) = collector();
        let bridge = AgentBridge::new(dir.path().join("none.sock"), sink);
        let err = bridge.call("agent.status", json!({})).await.unwrap_err();
        assert!(err.contains("not running"), "{err}");
    }

    #[test]
    fn socket_path_honours_the_override() {
        env::set_var("OSAIMA_AGENT_SOCKET", "/run/custom.sock");
        assert_eq!(socket_path(), PathBuf::from("/run/custom.sock"));
        env::remove_var("OSAIMA_AGENT_SOCKET");
        assert!(socket_path().ends_with("agent.sock"));
    }
}
