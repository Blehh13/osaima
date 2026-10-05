//! Output volume through PipeWire's `wpctl`.

use std::time::Duration;

use serde_json::{json, Value};

use super::{no_args, read_only, reversible, run_program, tool, Args, ToolError, ToolResult};

const WPCTL: &str = "wpctl";
const SINK: &str = "@DEFAULT_AUDIO_SINK@";
const TIMEOUT: Duration = Duration::from_secs(5);
/// wpctl allows boosting above 100%; cap it to protect speakers and ears.
const MAX_PERCENT: u64 = 150;

pub fn definitions() -> Vec<Value> {
    vec![
        tool(
            "get_volume",
            "Get volume",
            "Current output volume (percent) and whether it is muted.",
            no_args(),
            read_only(),
        ),
        tool(
            "set_volume",
            "Set volume",
            "Change the output volume. Give `percent` for an absolute level, `change` for a relative step (e.g. -10), and/or `muted`.",
            json!({
                "type": "object",
                "properties": {
                    "percent": { "type": "integer", "minimum": 0, "maximum": MAX_PERCENT },
                    "change": { "type": "integer", "minimum": -100, "maximum": 100 },
                    "muted": { "type": "boolean" }
                },
                "additionalProperties": false
            }),
            reversible(),
        ),
    ]
}

pub async fn get_volume(arguments: Option<Value>) -> ToolResult {
    Args::parse(arguments, &[])?;
    read_volume().await
}

pub async fn set_volume(arguments: Option<Value>) -> ToolResult {
    let args = Args::parse(arguments, &["percent", "change", "muted"])?;
    let percent = args.u64("percent")?;
    let change = args.i64("change")?;
    let muted = args.bool("muted")?;
    if percent.is_some() && change.is_some() {
        return Err(ToolError::invalid(
            "give either percent or change, not both",
        ));
    }
    if percent.is_none() && change.is_none() && muted.is_none() {
        return Err(ToolError::invalid("give percent, change or muted"));
    }
    if percent.is_some_and(|p| p > MAX_PERCENT) {
        return Err(ToolError::invalid(format!(
            "percent must be between 0 and {MAX_PERCENT}"
        )));
    }
    if change.is_some_and(|c| !(-100..=100).contains(&c)) {
        return Err(ToolError::invalid("change must be between -100 and 100"));
    }

    let limit = format!("{:.2}", MAX_PERCENT as f64 / 100.0);
    let level = match (percent, change) {
        (Some(p), _) => Some(format!("{p}%")),
        (_, Some(c)) if c >= 0 => Some(format!("{c}%+")),
        (_, Some(c)) => Some(format!("{}%-", c.unsigned_abs())),
        _ => None,
    };
    if let Some(level) = level {
        wpctl(&["set-volume", "-l", &limit, SINK, &level]).await?;
    }
    if let Some(muted) = muted {
        wpctl(&["set-mute", SINK, if muted { "1" } else { "0" }]).await?;
    }
    read_volume().await
}

async fn read_volume() -> ToolResult {
    let out = wpctl(&["get-volume", SINK]).await?;
    parse_volume(&out)
        .map(|(percent, muted)| json!({ "percent": percent, "muted": muted }))
        .ok_or_else(|| ToolError::failed(format!("unexpected wpctl output: {}", out.trim())))
}

async fn wpctl(args: &[&str]) -> Result<String, ToolError> {
    let out = run_program(WPCTL, args, TIMEOUT).await?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(ToolError::failed(format!(
            "wpctl failed (is PipeWire running?): {}",
            out.stderr
        )))
    }
}

/// Parse `Volume: 0.40` or `Volume: 0.40 [MUTED]`.
fn parse_volume(output: &str) -> Option<(u64, bool)> {
    let rest = output.trim().strip_prefix("Volume:")?.trim();
    let level: f64 = rest.split_whitespace().next()?.parse().ok()?;
    if !level.is_finite() || level < 0.0 {
        return None;
    }
    Some(((level * 100.0).round() as u64, rest.contains("[MUTED]")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wpctl_output() {
        assert_eq!(parse_volume("Volume: 0.40\n"), Some((40, false)));
        assert_eq!(parse_volume("Volume: 1.25 [MUTED]\n"), Some((125, true)));
        assert_eq!(parse_volume("Volume: 0.004"), Some((0, false)));
        assert_eq!(parse_volume("error"), None);
        assert_eq!(parse_volume("Volume: abc"), None);
    }

    #[tokio::test]
    async fn validates_before_running_anything() {
        for bad in [
            json!({}),
            json!({ "percent": 50, "change": 5 }),
            json!({ "percent": 151 }),
            json!({ "change": -101 }),
            json!({ "muted": "yes" }),
        ] {
            assert!(
                matches!(
                    set_volume(Some(bad.clone())).await,
                    Err(ToolError::InvalidParams(_))
                ),
                "{bad}"
            );
        }
    }
}
