//! Screen brightness through the kernel backlight interface
//! (`/sys/class/backlight`). The `video` group gets write access via the
//! udev rule shipped with the ebuild, so no root is needed.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{no_args, read_only, reversible, tool, Args, ToolContext, ToolError, ToolResult};

pub fn definitions() -> Vec<Value> {
    vec![
        tool(
            "get_brightness",
            "Get screen brightness",
            "Current screen backlight brightness as a percentage.",
            no_args(),
            read_only(),
        ),
        tool(
            "set_brightness",
            "Set screen brightness",
            "Set the screen backlight brightness to a percentage (1-100).",
            json!({
                "type": "object",
                "properties": { "percent": { "type": "integer", "minimum": 1, "maximum": 100 } },
                "required": ["percent"],
                "additionalProperties": false
            }),
            reversible(),
        ),
    ]
}

struct Backlight {
    name: String,
    dir: PathBuf,
    max: u64,
}

impl Backlight {
    fn current(&self) -> Result<u64, ToolError> {
        read_u64(&self.dir.join("brightness"))
    }

    fn report(&self, raw: u64) -> Value {
        json!({ "device": self.name, "percent": percent(raw, self.max), "raw": raw, "max": self.max })
    }
}

pub fn get_brightness(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    Args::parse(arguments, &[])?;
    let backlight = find_backlight(&ctx.paths.backlight_dir)?;
    Ok(backlight.report(backlight.current()?))
}

pub fn set_brightness(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    let args = Args::parse(arguments, &["percent"])?;
    let pct = args
        .u64("percent")?
        .ok_or_else(|| ToolError::invalid("percent is required"))?;
    if !(1..=100).contains(&pct) {
        return Err(ToolError::invalid("percent must be between 1 and 100"));
    }
    let backlight = find_backlight(&ctx.paths.backlight_dir)?;
    // Never write 0: on many panels that turns the backlight fully off.
    let raw = (backlight.max * pct).div_ceil(100).clamp(1, backlight.max);
    fs::write(backlight.dir.join("brightness"), raw.to_string()).map_err(|err| {
        if err.kind() == ErrorKind::PermissionDenied {
            ToolError::failed(
                "permission denied writing the backlight; the user must be in the `video` group",
            )
        } else {
            ToolError::failed(format!("could not set brightness: {err}"))
        }
    })?;
    Ok(backlight.report(raw))
}

/// Pick the best backlight: firmware/platform controls first, raw last,
/// matching what desktop environments do.
fn find_backlight(dir: &Path) -> Result<Backlight, ToolError> {
    let no_device = || {
        ToolError::failed("no adjustable backlight found (virtual machines and desktop monitors usually have none)")
    };
    let mut candidates: Vec<(u8, Backlight)> = fs::read_dir(dir)
        .map_err(|_| no_device())?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let dir = entry.path();
            let max = read_u64(&dir.join("max_brightness"))
                .ok()
                .filter(|m| *m > 0)?;
            let rank = match fs::read_to_string(dir.join("type")).ok()?.trim() {
                "firmware" => 0,
                "platform" => 1,
                _ => 2,
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            Some((rank, Backlight { name, dir, max }))
        })
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    candidates
        .into_iter()
        .next()
        .map(|(_, b)| b)
        .ok_or_else(no_device)
}

fn read_u64(path: &Path) -> Result<u64, ToolError> {
    fs::read_to_string(path)
        .map_err(|e| ToolError::failed(format!("could not read {}: {e}", path.display())))?
        .trim()
        .parse()
        .map_err(|_| ToolError::failed(format!("{} is not a number", path.display())))
}

fn percent(raw: u64, max: u64) -> u64 {
    (raw * 100 + max / 2) / max
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::context;

    fn fake_backlight(root: &Path, name: &str, kind: &str, max: u64, value: u64) {
        let dir = root.join("backlight").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("type"), format!("{kind}\n")).unwrap();
        fs::write(dir.join("max_brightness"), format!("{max}\n")).unwrap();
        fs::write(dir.join("brightness"), format!("{value}\n")).unwrap();
    }

    #[test]
    fn reads_and_sets_percent() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        fake_backlight(dir.path(), "intel_backlight", "raw", 1000, 500);
        assert_eq!(get_brightness(&ctx, None).unwrap()["percent"], 50);

        let res = set_brightness(&ctx, Some(json!({ "percent": 30 }))).unwrap();
        assert_eq!(res["raw"], 300);
        let written = fs::read_to_string(dir.path().join("backlight/intel_backlight/brightness"));
        assert_eq!(written.unwrap(), "300");
    }

    #[test]
    fn prefers_firmware_devices() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        fake_backlight(dir.path(), "intel_backlight", "raw", 1000, 1000);
        fake_backlight(dir.path(), "acpi_video0", "firmware", 10, 5);
        assert_eq!(get_brightness(&ctx, None).unwrap()["device"], "acpi_video0");
    }

    #[test]
    fn never_writes_zero_and_validates_range() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        fake_backlight(dir.path(), "panel", "raw", 7, 7);
        assert_eq!(
            set_brightness(&ctx, Some(json!({ "percent": 1 }))).unwrap()["raw"],
            1
        );
        for bad in [
            json!({ "percent": 0 }),
            json!({ "percent": 101 }),
            json!({}),
        ] {
            assert!(matches!(
                set_brightness(&ctx, Some(bad)),
                Err(ToolError::InvalidParams(_))
            ));
        }
    }

    #[test]
    fn explains_missing_backlight() {
        let dir = tempfile::tempdir().unwrap();
        let err = get_brightness(&context(dir.path()), None).unwrap_err();
        assert!(matches!(err, ToolError::Failed(m) if m.contains("no adjustable backlight")));
    }
}
