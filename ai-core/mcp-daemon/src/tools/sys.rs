//! System context: stats, processes, disks, and signalling processes.

use serde_json::{json, Value};

use super::{
    destructive, no_args, read_only, to_value, tool, Args, Caller, ToolContext, ToolError,
    ToolResult,
};
use crate::system::{ProcessSignal, ProcessSort};

const DEFAULT_PROCESS_LIMIT: u64 = 15;
const MAX_PROCESS_LIMIT: u64 = 200;

pub fn definitions() -> Vec<Value> {
    vec![
        tool(
            "get_system_stats",
            "System statistics",
            "Current OS, kernel, host, uptime, load average, CPU usage and memory usage.",
            no_args(),
            read_only(),
        ),
        tool(
            "list_processes",
            "List processes",
            "Running processes, highest resource usage first. cpu_percent is relative to one core.",
            json!({
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
            }),
            read_only(),
        ),
        tool(
            "kill_process",
            "End a process",
            "Send a signal (default TERM) to a process owned by the user. Confirm with the user first.",
            json!({
                "type": "object",
                "properties": {
                    "pid": { "type": "integer", "minimum": 2 },
                    "signal": { "type": "string", "enum": ProcessSignal::NAMES, "default": "TERM" }
                },
                "required": ["pid"],
                "additionalProperties": false
            }),
            destructive(),
        ),
        tool(
            "list_disks",
            "List disks",
            "Mounted filesystems with total and available space.",
            no_args(),
            read_only(),
        ),
    ]
}

pub fn get_system_stats(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    Args::parse(arguments, &[])?;
    Ok(to_value(&ctx.monitor.stats()))
}

pub fn list_processes(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    let args = Args::parse(arguments, &["limit", "sort_by"])?;
    let limit = args.u64_in("limit", 1, MAX_PROCESS_LIMIT, DEFAULT_PROCESS_LIMIT)?;
    let sort = match args.str("sort_by")? {
        None | Some("cpu") => ProcessSort::Cpu,
        Some("memory") => ProcessSort::Memory,
        Some(other) => {
            return Err(ToolError::invalid(format!(
                "sort_by must be \"cpu\" or \"memory\", got {other:?}"
            )))
        }
    };
    let processes = ctx.monitor.processes(sort, limit as usize);
    Ok(json!({ "processes": processes }))
}

pub fn kill_process(ctx: &ToolContext, caller: Caller, arguments: Option<Value>) -> ToolResult {
    let args = Args::parse(arguments, &["pid", "signal"])?;
    let pid = args
        .u64("pid")?
        .ok_or_else(|| ToolError::invalid("pid is required"))?;
    let pid = u32::try_from(pid).map_err(|_| ToolError::invalid("pid is too large"))?;
    let signal = match args.str("signal")? {
        None => ProcessSignal::Term,
        Some(name) => ProcessSignal::parse(name).ok_or_else(|| {
            ToolError::invalid(format!(
                "unsupported signal {name:?}; expected one of {:?}",
                ProcessSignal::NAMES
            ))
        })?,
    };
    ctx.monitor
        .signal_process(pid, signal, caller.uid)
        .map_err(|err| ToolError::failed(err.to_string()))?;
    tracing::info!(pid, ?signal, uid = caller.uid, "signalled process");
    Ok(json!({ "pid": pid, "signalled": true }))
}

pub fn list_disks(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    Args::parse(arguments, &[])?;
    Ok(json!({ "disks": ctx.monitor.disks() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::context;

    const ROOT: Caller = Caller { uid: 0 };

    #[test]
    fn stats_have_live_values() {
        let dir = tempfile::tempdir().unwrap();
        let stats = get_system_stats(&context(dir.path()), None).unwrap();
        assert!(stats["cpu"]["cores"].as_u64().unwrap() > 0);
        assert!(stats["memory"]["total_bytes"].as_u64().unwrap() > 0);
    }

    #[test]
    fn list_processes_honours_limit_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let res = list_processes(&ctx, Some(json!({ "limit": 2, "sort_by": "memory" }))).unwrap();
        assert!(res["processes"].as_array().unwrap().len() <= 2);
        for bad in [
            json!({ "limit": 0 }),
            json!({ "sort_by": "io" }),
            json!({ "x": 1 }),
        ] {
            assert!(matches!(
                list_processes(&ctx, Some(bad)),
                Err(ToolError::InvalidParams(_))
            ));
        }
    }

    #[test]
    fn kill_process_refuses_protected_pids() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let err = kill_process(&ctx, ROOT, Some(json!({ "pid": 1 }))).unwrap_err();
        assert!(matches!(err, ToolError::Failed(m) if m.contains("protected")));
    }

    #[test]
    fn kill_process_validates_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        assert!(matches!(
            kill_process(&ctx, ROOT, Some(json!({}))),
            Err(ToolError::InvalidParams(_))
        ));
        assert!(matches!(
            kill_process(
                &ctx,
                ROOT,
                Some(json!({ "pid": 4_000_000, "signal": "SEGV" }))
            ),
            Err(ToolError::InvalidParams(_))
        ));
    }
}
