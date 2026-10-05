//! MCP tools exposed through `tools/list` and `tools/call`.
//!
//! Each tool returns a `CallToolResult`: a JSON text block for any MCP client
//! plus `structuredContent` for programmatic callers such as the shell and the
//! agent. Failures while running a tool are reported in-band with
//! `isError: true`; unknown tools and malformed arguments are JSON-RPC errors.
//!
//! Every definition carries MCP annotations. Clients use them to decide what
//! needs the user's approval: `readOnlyHint` tools only observe, and
//! `destructiveHint` tools must be confirmed before they run.

mod apps;
mod args;
mod audio;
mod display;
mod files;
mod network;
mod packages;
mod power;
mod sys;
mod windows;

use std::env;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::protocol::RpcError;
use crate::system::SystemMonitor;

pub use args::Args;

/// Who is calling, as established by the transport.
#[derive(Debug, Clone, Copy)]
pub struct Caller {
    pub uid: u32,
}

/// Host locations the tools read. Tests point these at temporary directories.
#[derive(Debug, Clone)]
pub struct HostPaths {
    pub home: PathBuf,
    /// `.desktop` search path, highest priority first.
    pub application_dirs: Vec<PathBuf>,
    pub backlight_dir: PathBuf,
    pub net_class_dir: PathBuf,
    pub proc_net_route: PathBuf,
    pub resolv_conf: PathBuf,
    pub package_db: PathBuf,
    /// The sway IPC socket, when running inside a sway session.
    pub sway_socket: Option<PathBuf>,
}

impl HostPaths {
    /// Locations for the running system, following the XDG base-directory spec.
    pub fn from_env() -> Self {
        let home = env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let data_home = env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        let data_dirs = env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        let application_dirs = std::iter::once(data_home)
            .chain(env::split_paths(&data_dirs))
            .map(|d| d.join("applications"))
            .collect();
        Self {
            home,
            application_dirs,
            backlight_dir: "/sys/class/backlight".into(),
            net_class_dir: "/sys/class/net".into(),
            proc_net_route: "/proc/net/route".into(),
            resolv_conf: "/etc/resolv.conf".into(),
            package_db: "/var/db/pkg".into(),
            sway_socket: env::var_os("SWAYSOCK")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
        }
    }
}

/// Everything a tool may use.
#[derive(Clone)]
pub struct ToolContext {
    pub monitor: Arc<SystemMonitor>,
    pub paths: Arc<HostPaths>,
}

#[derive(Debug, PartialEq)]
pub enum ToolError {
    /// The request itself is wrong; reported as JSON-RPC `-32602`.
    InvalidParams(String),
    /// The tool ran but could not do its job; reported in-band (`isError`).
    Failed(String),
}

impl ToolError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidParams(message.into())
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// Structured content on success.
pub type ToolResult = Result<Value, ToolError>;

/// Tool definitions for `tools/list`.
pub fn definitions() -> Value {
    let all: Vec<Value> = [
        sys::definitions(),
        network::definitions(),
        display::definitions(),
        audio::definitions(),
        apps::definitions(),
        windows::definitions(),
        power::definitions(),
        files::definitions(),
        packages::definitions(),
    ]
    .into_iter()
    .flatten()
    .collect();
    Value::Array(all)
}

/// Run tool `name` and wrap the outcome as an MCP `CallToolResult`.
pub async fn call(
    ctx: &ToolContext,
    caller: Caller,
    name: &str,
    arguments: Option<Value>,
) -> Result<Value, RpcError> {
    let outcome = match name {
        "get_system_stats" => sys::get_system_stats(ctx, arguments),
        "list_processes" => sys::list_processes(ctx, arguments),
        "kill_process" => sys::kill_process(ctx, caller, arguments),
        "list_disks" => sys::list_disks(ctx, arguments),
        "network_status" => network::network_status(ctx, arguments),
        "check_connectivity" => network::check_connectivity(arguments).await,
        "get_brightness" => display::get_brightness(ctx, arguments),
        "set_brightness" => display::set_brightness(ctx, arguments),
        "get_volume" => audio::get_volume(arguments).await,
        "set_volume" => audio::set_volume(arguments).await,
        "list_apps" => apps::list_apps(ctx, arguments),
        "launch_app" => apps::launch_app(ctx, arguments).await,
        "list_windows" => windows::list_windows(ctx, arguments).await,
        "focus_window" => windows::focus_window(ctx, arguments).await,
        "close_window" => windows::close_window(ctx, arguments).await,
        "power_action" => power::power_action(arguments).await,
        "search_files" => files::search_files(ctx, arguments).await,
        "package_info" => packages::package_info(ctx, arguments).await,
        _ => return Err(RpcError::invalid_params(format!("unknown tool: {name}"))),
    };
    match outcome {
        Ok(value) => {
            tracing::debug!(tool = name, "tool succeeded");
            Ok(structured(value))
        }
        Err(ToolError::Failed(message)) => {
            tracing::info!(tool = name, %message, "tool failed");
            Ok(tool_error(message))
        }
        Err(ToolError::InvalidParams(message)) => Err(RpcError::invalid_params(message)),
    }
}

/// Serialize a tool's output for `structuredContent`.
pub(crate) fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn structured(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_default();
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": value,
        "isError": false
    })
}

fn tool_error(message: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true
    })
}

/// Shorthand for a tool definition.
pub(crate) fn tool(
    name: &str,
    title: &str,
    description: &str,
    input_schema: Value,
    annotations: Value,
) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": input_schema,
        "annotations": annotations
    })
}

/// Annotations for tools that only observe.
pub(crate) fn read_only() -> Value {
    json!({ "readOnlyHint": true, "openWorldHint": false })
}

/// Annotations for tools that change state in an easily reversible way.
pub(crate) fn reversible() -> Value {
    json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false })
}

/// Annotations for tools that must be confirmed by the user.
pub(crate) fn destructive() -> Value {
    json!({
        "readOnlyHint": false,
        "destructiveHint": true,
        "idempotentHint": false,
        "openWorldHint": false
    })
}

/// Input schema with no arguments.
pub(crate) fn no_args() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// Output of a helper program.
pub(crate) struct ProgramOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run `program` with `args` (no shell), stdin closed, killed after `timeout`.
pub(crate) async fn run_program(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<ProgramOutput, ToolError> {
    let child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => {
                ToolError::failed(format!("{program} is not installed on this system"))
            }
            _ => ToolError::failed(format!("could not run {program}: {err}")),
        })?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => Ok(ProgramOutput {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        }),
        Ok(Err(err)) => Err(ToolError::failed(format!("{program} failed: {err}"))),
        Err(_) => Err(ToolError::failed(format!(
            "{program} did not finish within {}s",
            timeout.as_secs()
        ))),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::path::Path;

    /// A context whose host paths all live under `root`.
    pub fn context(root: &Path) -> ToolContext {
        let paths = HostPaths {
            home: root.join("home"),
            application_dirs: vec![root.join("apps-user"), root.join("apps-system")],
            backlight_dir: root.join("backlight"),
            net_class_dir: root.join("net"),
            proc_net_route: root.join("route"),
            resolv_conf: root.join("resolv.conf"),
            package_db: root.join("pkg"),
            sway_socket: None,
        };
        ToolContext {
            monitor: Arc::new(SystemMonitor::new()),
            paths: Arc::new(paths),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::INVALID_PARAMS;

    #[test]
    fn definitions_are_unique_and_annotated() {
        let defs = definitions();
        let tools = defs.as_array().unwrap();
        assert_eq!(tools.len(), 18);
        let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), tools.len(), "tool names must be unique");
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
            assert!(tool["annotations"]["readOnlyHint"].is_boolean(), "{tool}");
            assert!(tool["description"].as_str().unwrap().len() > 10, "{tool}");
        }
    }

    #[tokio::test]
    async fn unknown_tool_is_invalid_params() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_support::context(dir.path());
        let err = call(&ctx, Caller { uid: 0 }, "format_disk", None)
            .await
            .unwrap_err();
        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn failures_are_reported_in_band() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_support::context(dir.path());
        // No backlight directory exists under the temp root.
        let res = call(&ctx, Caller { uid: 0 }, "get_brightness", None)
            .await
            .unwrap();
        assert_eq!(res["isError"], true);
        assert!(res.get("structuredContent").is_none());
    }

    #[tokio::test]
    async fn run_program_reports_missing_binaries() {
        let err = run_program("osaima-no-such-binary", &[], Duration::from_secs(1))
            .await
            .err()
            .unwrap();
        assert!(matches!(err, ToolError::Failed(m) if m.contains("not installed")));
    }
}
