//! Session and power actions through elogind's `loginctl`. Every action here
//! is marked destructive, so clients must confirm with the user first.

use serde_json::{json, Value};

use super::{destructive, run_program, tool, Args, ToolContext, ToolError, ToolResult};

const ACTIONS: [&str; 4] = ["lock", "suspend", "reboot", "poweroff"];

pub fn definitions() -> Vec<Value> {
    vec![tool(
        "power_action",
        "Power and session",
        "Lock the screen, suspend, reboot or power off the computer. Always confirm with the user first.",
        json!({
            "type": "object",
            "properties": { "action": { "type": "string", "enum": ACTIONS } },
            "required": ["action"],
            "additionalProperties": false
        }),
        destructive(),
    )]
}

pub async fn power_action(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    let args = Args::parse(arguments, &["action"])?;
    let action = args.required_str("action")?;
    let command = loginctl_args(action)?;
    tracing::warn!(action, "performing power action");
    let out = run_program("loginctl", &command, ctx.settings.helper_timeout).await?;
    if out.success {
        Ok(json!({ "action": action, "done": true }))
    } else {
        Err(ToolError::failed(format!(
            "{action} failed: {}",
            out.stderr
        )))
    }
}

fn loginctl_args(action: &str) -> Result<Vec<&'static str>, ToolError> {
    match action {
        "lock" => Ok(vec!["lock-session"]),
        "suspend" => Ok(vec!["suspend"]),
        "reboot" => Ok(vec!["reboot"]),
        "poweroff" => Ok(vec!["poweroff"]),
        other => Err(ToolError::invalid(format!(
            "unknown action {other:?}; expected one of {ACTIONS:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_actions_to_loginctl() {
        assert_eq!(loginctl_args("lock").unwrap(), ["lock-session"]);
        assert_eq!(loginctl_args("poweroff").unwrap(), ["poweroff"]);
        assert!(loginctl_args("hibernate; rm -rf /").is_err());
    }

    #[tokio::test]
    async fn rejects_unknown_actions_without_running_anything() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = crate::tools::test_support::context(dir.path());
        for bad in [json!({}), json!({ "action": "shutdown -h now" })] {
            assert!(matches!(
                power_action(&ctx, Some(bad)).await,
                Err(ToolError::InvalidParams(_))
            ));
        }
    }
}
