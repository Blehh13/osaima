use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use mcp_daemon::paths;
use mcp_daemon::server::Server;
use mcp_daemon::system::{SystemMonitor, SAMPLE_INTERVAL};
use mcp_daemon::transport::{self, SocketListener};
use tokio::signal::unix::{signal, SignalKind};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
OSAIMA AI Core — Model Context Protocol server

Usage: mcp-daemon [--socket PATH | --stdio]

Options:
  --socket PATH  Listen on PATH (default: $OSAIMA_MCP_SOCKET,
                 $XDG_RUNTIME_DIR/osaima/mcp.sock or /tmp/osaima-<uid>/mcp.sock)
  --stdio        Serve one MCP session on stdin/stdout
  -h, --help     Show this help
  -V, --version  Show the version

Logging goes to stderr; set OSAIMA_LOG (e.g. OSAIMA_LOG=debug) to change the level.";

#[derive(Debug, PartialEq)]
enum Mode {
    Socket(PathBuf),
    Stdio,
}

/// `Ok(None)` means help or version was printed.
fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Mode>, String> {
    let mut mode = None;
    while let Some(arg) = args.next() {
        let next = match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("mcp-daemon {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--stdio" => Mode::Stdio,
            "--socket" => {
                let path = args.next().ok_or("--socket requires a path")?;
                Mode::Socket(PathBuf::from(path))
            }
            other => return Err(format!("unknown argument: {other}")),
        };
        if mode.replace(next).is_some() {
            return Err("--socket and --stdio are mutually exclusive".into());
        }
    }
    Ok(Some(
        mode.unwrap_or_else(|| Mode::Socket(paths::socket_path())),
    ))
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("OSAIMA_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let mode = match parse_args(std::env::args().skip(1)) {
        Ok(Some(mode)) => mode,
        Ok(None) => return ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let monitor = Arc::new(SystemMonitor::new());
    if let Err(err) = monitor.spawn_sampler(SAMPLE_INTERVAL) {
        tracing::error!(error = %err, "could not start the system sampler");
        return ExitCode::FAILURE;
    }
    let server = Server::new(monitor);

    match mode {
        Mode::Stdio => match transport::serve_stdio(server).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                tracing::error!(error = %err, "stdio session failed");
                ExitCode::FAILURE
            }
        },
        Mode::Socket(path) => {
            let listener = match SocketListener::bind(&path) {
                Ok(listener) => listener,
                Err(err) => {
                    tracing::error!(error = %err, path = %path.display(), "could not bind socket");
                    return ExitCode::FAILURE;
                }
            };
            let mut terminate = match signal(SignalKind::terminate()) {
                Ok(stream) => stream,
                Err(err) => {
                    tracing::error!(error = %err, "could not install SIGTERM handler");
                    return ExitCode::FAILURE;
                }
            };
            tracing::info!(path = %listener.path().display(), "listening");
            tokio::select! {
                _ = listener.serve(server) => {}
                _ = tokio::signal::ctrl_c() => tracing::info!("interrupted, shutting down"),
                _ = terminate.recv() => tracing::info!("terminated, shutting down"),
            }
            // Dropping the listener removes the socket file.
            ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Mode>, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_modes() {
        assert_eq!(parse(&["--stdio"]), Ok(Some(Mode::Stdio)));
        assert_eq!(
            parse(&["--socket", "/run/x.sock"]),
            Ok(Some(Mode::Socket("/run/x.sock".into())))
        );
        assert!(matches!(parse(&[]), Ok(Some(Mode::Socket(_)))));
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse(&["--socket"]).is_err());
        assert!(parse(&["--stdio", "--socket", "/x"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }
}
