//! Transports: a per-user Unix socket (the shell and local agents) and stdio
//! (MCP clients that launch the daemon as a subprocess). Both carry
//! newline-delimited JSON-RPC messages.

use std::fs::{self, DirBuilder, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::paths::current_uid;
use crate::protocol::{Response, RpcError, INVALID_REQUEST, PARSE_ERROR};
use crate::server::{Server, Session};
use crate::tools::Caller;

/// Largest accepted message; longer lines close the connection.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// A bound daemon socket. The socket file is removed when this is dropped.
pub struct SocketListener {
    listener: UnixListener,
    path: PathBuf,
}

impl SocketListener {
    /// Bind `path` with owner-only permissions, creating its directory (0700)
    /// if needed. A stale socket left by a crashed daemon is replaced; a live
    /// one is an error.
    pub fn bind(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            if !parent.exists() {
                DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)?;
            }
        }
        if path.exists() {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("a daemon is already listening on {}", path.display()),
                ));
            }
            fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        fs::set_permissions(path, Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Accept connections forever, one task per client.
    pub async fn serve(&self, server: Server) {
        let own_uid = current_uid();
        loop {
            match self.listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(handle_unix_client(stream, server.clone(), own_uid));
                }
                Err(err) => {
                    // Usually fd exhaustion; back off instead of spinning.
                    tracing::warn!(error = %err, "accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
}

impl Drop for SocketListener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

async fn handle_unix_client(stream: UnixStream, server: Server, own_uid: u32) {
    let uid = match stream.peer_cred() {
        Ok(cred) => cred.uid(),
        Err(err) => {
            tracing::warn!(error = %err, "could not read peer credentials");
            return;
        }
    };
    if uid != own_uid && uid != 0 {
        tracing::warn!(uid, "rejected connection from another user");
        return;
    }
    tracing::debug!(uid, "client connected");
    let (reader, writer) = stream.into_split();
    if let Err(err) = serve_connection(reader, writer, &server, Caller { uid }).await {
        tracing::debug!(error = %err, "client connection ended with an error");
    }
}

/// Serve the MCP session on stdin/stdout until stdin closes.
pub async fn serve_stdio(server: Server) -> io::Result<()> {
    let caller = Caller { uid: current_uid() };
    serve_connection(tokio::io::stdin(), tokio::io::stdout(), &server, caller).await
}

/// Run one session over any byte stream.
pub async fn serve_connection<R, W>(
    reader: R,
    mut writer: W,
    server: &Server,
    caller: Caller,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut session = Session::new(caller);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let limit = MAX_MESSAGE_BYTES as u64 + 1;
        if (&mut reader)
            .take(limit)
            .read_until(b'\n', &mut buf)
            .await?
            == 0
        {
            return Ok(());
        }
        if buf.len() > MAX_MESSAGE_BYTES {
            let err = RpcError::new(INVALID_REQUEST, "message exceeds the 1 MiB limit");
            write_line(&mut writer, &Response::failure(Value::Null, err)).await?;
            return Ok(());
        }
        let text = match std::str::from_utf8(&buf) {
            Ok(text) => text.trim(),
            Err(_) => {
                let err = RpcError::new(PARSE_ERROR, "message is not valid UTF-8");
                write_line(&mut writer, &Response::failure(Value::Null, err)).await?;
                continue;
            }
        };
        if text.is_empty() {
            continue;
        }
        if let Some(reply) = server.handle_message(text, &mut session).await {
            writer.write_all(reply.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
    }
}

async fn write_line<W: AsyncWrite + Unpin>(writer: &mut W, response: &Response) -> io::Result<()> {
    let mut line = serde_json::to_vec(response)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}
