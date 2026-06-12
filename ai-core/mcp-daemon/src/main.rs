mod context;
mod protocol;

use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use protocol::{JsonRpcRequest, JsonRpcResponse};
use context::ContextProvider;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = "/tmp/interstellar_mcp.sock";
    
    // Clean up old socket if it exists
    let _ = std::fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path)?;
    println!("MCP Daemon listening on {}", socket_path);

    let ctx = Arc::new(ContextProvider::new());

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let ctx_clone = Arc::clone(&ctx);
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, ctx_clone).await {
                        eprintln!("Error handling client: {}", e);
                    }
                });
            }
            Err(e) => {
                eprintln!("Accept failed: {}", e);
            }
        }
    }
}

async fn handle_client(mut stream: UnixStream, ctx: Arc<ContextProvider>) -> Result<(), Box<dyn std::error::Error>> {
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).await?;
        if bytes_read == 0 {
            break; // Connection closed
        }

        let request: Result<JsonRpcRequest, _> = serde_json::from_str(&line);
        match request {
            Ok(req) => {
                let response = match req.method.as_str() {
                    "system.get_stats" => {
                        let stats = ctx.get_system_stats();
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), stats)
                    },
                    "ping" => {
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), json!("pong"))
                    },
                    _ => {
                        JsonRpcResponse::error(
                            req.id.unwrap_or_else(|| "0".to_string()),
                            -32601,
                            "Method not found".to_string(),
                        )
                    }
                };

                let mut res_str = serde_json::to_string(&response)?;
                res_str.push('\n');
                writer.write_all(res_str.as_bytes()).await?;
            }
            Err(e) => {
                let err_res = JsonRpcResponse::error(
                    "null".to_string(),
                    -32700,
                    format!("Parse error: {}", e),
                );
                let mut res_str = serde_json::to_string(&err_res)?;
                res_str.push('\n');
                writer.write_all(res_str.as_bytes()).await?;
            }
        }
    }

    Ok(())
}
