//! MCP tools exposed through `tools/list` and `tools/call`.
//!
//! Each tool returns a `CallToolResult`: a JSON text block for any MCP client
//! plus `structuredContent` for programmatic callers such as the shell.
//! Failures while running a tool are reported in-band with `isError: true`;
//! unknown tools and malformed arguments are JSON-RPC errors.

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::protocol::RpcError;
use crate::system::{ProcessSignal, ProcessSort, SystemMonitor};

const DEFAULT_PROCESS_LIMIT: u64 = 15;
const MAX_PROCESS_LIMIT: u64 = 200;

/// Who is calling, as established by the transport.
#[derive(Debug, Clone, Copy)]
pub struct Caller {
    pub uid: u32,
}

/// Tool definitions for `tools/list`.
pub fn definitions() -> Value {
    json!([
        {
            "name": "get_system_stats",
            "title": "System statistics",
            "description": "Current OS, kernel, host, uptime, load average, CPU usage and memory usage.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "list_processes",
            "title": "List processes",
            "description": "Running processes, highest resource usage first. cpu_percent is relative to one core.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_PROCESS_LIMIT,
                        "default": DEFAULT_PROCESS_LIMIT
                    },
                    "sort_by": { "type": "string", "enum": ["cpu", "memory"], "default": "cpu" }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "kill_process",
            "title": "Signal a process",
            "description": "Send a signal (default TERM) to a process owned by the caller. Clients must confirm with the user first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pid": { "type": "integer", "minimum": 2 },
                    "signal": { "type": "string", "enum": ProcessSignal::NAMES, "default": "TERM" }
                },
                "required": ["pid"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": true,
                "idempotentHint": false,
                "openWorldHint": false
            }
        },
        {
            "name": "list_disks",
            "title": "List disks",
            "description": "Mounted filesystems with total and available space.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        }
    ])
}

/// Run tool `name` with `arguments`.
pub fn call(
    monitor: &SystemMonitor,
    caller: Caller,
    name: &str,
    arguments: Option<Value>,
) -> Result<Value, RpcError> {
    let args = match arguments {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map,
        Some(_) => return Err(RpcError::invalid_params("arguments must be an object")),
    };
    match name {
        "get_system_stats" => {
            reject_unknown(&args, &[])?;
            Ok(structured(&monitor.stats()))
        }
        "list_processes" => {
            reject_unknown(&args, &["limit", "sort_by"])?;
            let limit = optional_u64(&args, "limit")?.unwrap_or(DEFAULT_PROCESS_LIMIT);
            if !(1..=MAX_PROCESS_LIMIT).contains(&limit) {
                return Err(RpcError::invalid_params(format!(
                    "limit must be between 1 and {MAX_PROCESS_LIMIT}"
                )));
            }
            let sort = match optional_str(&args, "sort_by")? {
                None | Some("cpu") => ProcessSort::Cpu,
                Some("memory") => ProcessSort::Memory,
                Some(other) => {
                    return Err(RpcError::invalid_params(format!(
                        "sort_by must be \"cpu\" or \"memory\", got {other:?}"
                    )))
                }
            };
            let processes = monitor.processes(sort, limit as usize);
            Ok(structured(&json!({ "processes": processes })))
        }
        "kill_process" => {
            reject_unknown(&args, &["pid", "signal"])?;
            let pid = optional_u64(&args, "pid")?
                .ok_or_else(|| RpcError::invalid_params("pid is required"))?;
            let pid = u32::try_from(pid).map_err(|_| RpcError::invalid_params("pid is too large"))?;
            let signal = match optional_str(&args, "signal")? {
                None => ProcessSignal::Term,
                Some(name) => ProcessSignal::parse(name).ok_or_else(|| {
                    RpcError::invalid_params(format!(
                        "unsupported signal {name:?}; expected one of {:?}",
                        ProcessSignal::NAMES
                    ))
                })?,
            };
            match monitor.signal_process(pid, signal, caller.uid) {
                Ok(()) => {
                    tracing::info!(pid, ?signal, uid = caller.uid, "signalled process");
                    Ok(structured(&json!({ "pid": pid, "signalled": true })))
                }
                Err(err) => Ok(tool_error(err.to_string())),
            }
        }
        "list_disks" => {
            reject_unknown(&args, &[])?;
            Ok(structured(&json!({ "disks": monitor.disks() })))
        }
        _ => Err(RpcError::invalid_params(format!("unknown tool: {name}"))),
    }
}

fn structured<T: Serialize>(value: &T) -> Value {
    let value = serde_json::to_value(value).unwrap_or(Value::Null);
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

fn reject_unknown(args: &Map<String, Value>, allowed: &[&str]) -> Result<(), RpcError> {
    match args.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(key) => Err(RpcError::invalid_params(format!("unexpected argument: {key}"))),
        None => Ok(()),
    }
}

fn optional_u64(args: &Map<String, Value>, key: &str) -> Result<Option<u64>, RpcError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| RpcError::invalid_params(format!("{key} must be a non-negative integer"))),
    }
}

fn optional_str<'a>(args: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, RpcError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_str()
            .map(Some)
            .ok_or_else(|| RpcError::invalid_params(format!("{key} must be a string"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::INVALID_PARAMS;

    const ROOT: Caller = Caller { uid: 0 };

    #[test]
    fn every_tool_has_a_schema() {
        let defs = definitions();
        let tools = defs.as_array().unwrap();
        assert_eq!(tools.len(), 4);
        for tool in tools {
            assert!(tool["name"].is_string());
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn stats_tool_returns_structured_content() {
        let monitor = SystemMonitor::new();
        let res = call(&monitor, ROOT, "get_system_stats", None).unwrap();
        assert_eq!(res["isError"], false);
        assert!(res["structuredContent"]["cpu"]["cores"].as_u64().unwrap() > 0);
        assert!(res["content"][0]["text"].as_str().unwrap().contains("memory"));
    }

    #[test]
    fn list_processes_honours_limit_and_validates() {
        let monitor = SystemMonitor::new();
        let res = call(
            &monitor,
            ROOT,
            "list_processes",
            Some(json!({ "limit": 2, "sort_by": "memory" })),
        )
        .unwrap();
        assert!(res["structuredContent"]["processes"].as_array().unwrap().len() <= 2);

        for bad in [json!({ "limit": 0 }), json!({ "sort_by": "io" }), json!({ "x": 1 })] {
            let err = call(&monitor, ROOT, "list_processes", Some(bad)).unwrap_err();
            assert_eq!(err.code, INVALID_PARAMS);
        }
    }

    #[test]
    fn kill_process_reports_refusal_in_band() {
        let monitor = SystemMonitor::new();
        let res = call(&monitor, ROOT, "kill_process", Some(json!({ "pid": 1 }))).unwrap();
        assert_eq!(res["isError"], true);
        assert!(res["content"][0]["text"].as_str().unwrap().contains("protected"));
    }

    #[test]
    fn kill_process_requires_pid_and_valid_signal() {
        let monitor = SystemMonitor::new();
        let err = call(&monitor, ROOT, "kill_process", Some(json!({}))).unwrap_err();
        assert_eq!(err.code, INVALID_PARAMS);
        let err = call(
            &monitor,
            ROOT,
            "kill_process",
            Some(json!({ "pid": 4_000_000, "signal": "SEGV" })),
        )
        .unwrap_err();
        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[test]
    fn unknown_tool_is_invalid_params() {
        let monitor = SystemMonitor::new();
        let err = call(&monitor, ROOT, "format_disk", None).unwrap_err();
        assert_eq!(err.code, INVALID_PARAMS);
    }
}
