use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use mcp_daemon::paths;
use mcp_daemon::server::Server;
use mcp_daemon::settings::Settings;
use mcp_daemon::system::SystemMonitor;
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

Logging goes to stderr; set OSAIMA_LOG (e.g. OSAIMA_LOG=debug) to change the level.

Settings (environment variables, defaults in brackets; docs/configuration.md has details):
  OSAIMA_SAMPLE_INTERVAL_MS     how often system data is re-sampled        [2000]
  OSAIMA_MAX_MESSAGE_BYTES      longest accepted message                   [1048576]
  OSAIMA_HELPER_TIMEOUT_MS      limit for wpctl, loginctl and sway calls   [5000]
  OSAIMA_CONNECT_TIMEOUT_MS     limit per step of check_connectivity       [4000]
  OSAIMA_CONNECTIVITY_HOST      host tested when none is given             [example.com]
  OSAIMA_PROCESS_LIST_DEFAULT   processes listed by default                [15]
  OSAIMA_PROCESS_LIST_MAX       most processes one call may list           [200]
  OSAIMA_SEARCH_MAX_DEPTH       directory levels file search descends      [8]
  OSAIMA_SEARCH_MAX_ENTRIES     entries file search examines at most       [100000]
  OSAIMA_SEARCH_TIME_BUDGET_MS  time budget for one file search            [3000]
  OSAIMA_MAX_VOLUME_PERCENT     highest volume set_volume accepts          [150]
  OSAIMA_TERMINAL               terminal for Terminal=true apps ($TERMINAL) [foot]";

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

    let settings = match Settings::from_env() {
        Ok(settings) => settings,
        Err(err) => {
            eprintln!("error: invalid setting {err}");
            return ExitCode::from(2);
        }
    };
    let monitor = Arc::new(SystemMonitor::new());
    if let Err(err) = monitor.spawn_sampler(settings.sample_interval) {
        tracing::error!(error = %err, "could not start the system sampler");
        return ExitCode::FAILURE;
    }
    let server = Server::new(monitor, settings);

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
