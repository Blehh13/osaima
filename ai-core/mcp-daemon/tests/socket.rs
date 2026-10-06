//! End-to-end tests over a real Unix socket.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use mcp_daemon::server::Server;
use mcp_daemon::settings::Settings;
use mcp_daemon::system::SystemMonitor;
use mcp_daemon::transport::SocketListener;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

struct Client {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl Client {
    async fn connect(path: &std::path::Path) -> Self {
        let (reader, writer) = UnixStream::connect(path).await.unwrap().into_split();
        Self {
            reader: BufReader::new(reader),
            writer,
        }
    }

    async fn send(&mut self, raw: &str) {
        self.writer.write_all(raw.as_bytes()).await.unwrap();
        self.writer.write_all(b"\n").await.unwrap();
    }

    async fn recv(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line).await.unwrap() {
            0 => None,
            _ => Some(serde_json::from_str(&line).unwrap()),
        }
    }

    async fn request(&mut self, msg: Value) -> Value {
        self.send(&msg.to_string()).await;
        self.recv().await.expect("connection closed")
    }
}

#[tokio::test]
async fn full_mcp_session_over_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("mcp.sock");
    let listener = SocketListener::bind(&path).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "socket must be owner-only");

    let server = Server::new(Arc::new(SystemMonitor::new()), Settings::default());
    let task = tokio::spawn(async move { listener.serve(server).await });

    let mut client = Client::connect(&path).await;
    let init = client
        .request(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": { "name": "test", "version": "0" } }
        }))
        .await;
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

    // The notification must not produce a reply: the next line read belongs to the ping.
    client
        .send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .await;
    let pong = client
        .request(json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }))
        .await;
    assert_eq!(pong["id"], 2);
    assert_eq!(pong["result"], json!({}));

    let stats = client
        .request(json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "get_system_stats", "arguments": {} }
        }))
        .await;
    assert!(
        stats["result"]["structuredContent"]["memory"]["total_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );

    // A second concurrent client is served independently.
    let mut other = Client::connect(&path).await;
    let res = other
        .request(json!({ "jsonrpc": "2.0", "id": "x", "method": "tools/list" }))
        .await;
    assert_eq!(res["id"], "x");

    task.abort();
    let _ = task.await;
    assert!(!path.exists(), "socket file should be removed on shutdown");
}

#[tokio::test]
async fn oversized_message_closes_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.sock");
    let listener = SocketListener::bind(&path).unwrap();
    let server = Server::new(Arc::new(SystemMonitor::new()), Settings::default());
    let task = tokio::spawn(async move { listener.serve(server).await });

    let mut client = Client::connect(&path).await;
    let huge = "x".repeat(Settings::default().max_message_bytes + 10);
    client.send(&huge).await;
    let res = client.recv().await.expect("expected an error reply");
    assert_eq!(res["error"]["code"], -32600);
    assert!(client.recv().await.is_none(), "connection should be closed");

    task.abort();
}

#[tokio::test]
async fn the_message_limit_is_a_setting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.sock");
    let listener = SocketListener::bind(&path).unwrap();
    let settings = Settings {
        max_message_bytes: 2048,
        ..Settings::default()
    };
    let server = Server::new(Arc::new(SystemMonitor::new()), settings);
    let task = tokio::spawn(async move { listener.serve(server).await });

    let mut client = Client::connect(&path).await;
    // A normal message still works.
    let pong = client
        .request(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .await;
    assert_eq!(pong["id"], 1);
    // Larger than the configured limit, though far below the default.
    client.send(&"x".repeat(3000)).await;
    let res = client.recv().await.expect("expected an error reply");
    assert_eq!(res["error"]["code"], -32600);
    assert!(res["error"]["message"]
        .as_str()
        .unwrap()
        .contains("2048-byte"));
    assert!(client.recv().await.is_none(), "connection should be closed");

    task.abort();
}

#[tokio::test]
async fn refuses_to_steal_a_live_socket_but_replaces_stale_ones() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.sock");

    let first = SocketListener::bind(&path).unwrap();
    let err = SocketListener::bind(&path)
        .err()
        .expect("second bind must fail");
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);

    drop(first);
    assert!(!path.exists());

    // Simulate a crash: the socket file stays behind but nothing listens on it.
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    SocketListener::bind(&path).expect("stale socket should be replaced");
}
