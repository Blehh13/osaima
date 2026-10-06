//! Direct host access for the built-in Terminal and Files apps. These run as
//! the session user, so they can do exactly what that user could in a shell.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct CommandOutput {
    pub output: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub truncated: bool,
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Run `cmd` through `/bin/sh -c` in `cwd` (default: home), with stdin closed.
/// The command and everything it started are killed after `timeout`, and output
/// beyond `max_output` bytes is dropped. Returns combined stdout + stderr.
pub async fn run_command(
    cmd: &str,
    cwd: Option<&Path>,
    timeout: Duration,
    max_output: usize,
) -> Result<CommandOutput, String> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return Ok(CommandOutput::default());
    }
    let dir = cwd.map(Path::to_path_buf).unwrap_or_else(home_dir);
    let child = tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Own process group so a timeout can kill everything the command started.
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start command in {}: {e}", dir.display()))?;
    let pid = child.id();

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => {
            let mut bytes = out.stdout;
            bytes.extend_from_slice(&out.stderr);
            let truncated = bytes.len() > max_output;
            bytes.truncate(max_output);
            Ok(CommandOutput {
                output: String::from_utf8_lossy(&bytes).into_owned(),
                exit_code: out.status.code(),
                timed_out: false,
                truncated,
            })
        }
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => {
            if let Some(pid) = pid.and_then(|p| libc::pid_t::try_from(p).ok()) {
                // SAFETY: kill(2) has no memory-safety preconditions; a negative
                // pid targets the process group created above.
                unsafe { libc::kill(-pid, libc::SIGKILL) };
            }
            Ok(CommandOutput {
                output: format!(
                    "command timed out after {}s and was stopped",
                    timeout.as_secs()
                ),
                exit_code: None,
                timed_out: true,
                truncated: false,
            })
        }
    }
}

/// List `path` (default: home). Directories first, then case-insensitive by name.
pub fn read_dir(path: Option<&Path>) -> Result<Value, String> {
    let dir = path.map(Path::to_path_buf).unwrap_or_else(home_dir);
    let mut entries: Vec<(bool, String)> = std::fs::read_dir(&dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| {
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            (is_dir, entry.file_name().to_string_lossy().into_owned())
        })
        .collect();
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    let entries: Vec<Value> = entries
        .into_iter()
        .map(|(is_dir, name)| json!({ "name": name, "is_dir": is_dir }))
        .collect();
    Ok(json!({ "path": dir.display().to_string(), "entries": entries }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(30);
    const LIMIT: usize = 256 * 1024;

    #[tokio::test]
    async fn captures_output_and_exit_code() {
        let out = run_command(
            "echo hi; echo err >&2; exit 3",
            Some(Path::new("/")),
            TIMEOUT,
            LIMIT,
        )
        .await
        .unwrap();
        assert_eq!(out.output, "hi\nerr\n");
        assert_eq!(out.exit_code, Some(3));
        assert!(!out.timed_out);
    }

    #[tokio::test]
    async fn runs_in_requested_directory() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_command("pwd", Some(dir.path()), TIMEOUT, LIMIT)
            .await
            .unwrap();
        let reported = std::fs::canonicalize(out.output.trim()).unwrap();
        assert_eq!(reported, std::fs::canonicalize(dir.path()).unwrap());
    }

    #[tokio::test]
    async fn times_out_and_kills_the_group() {
        let start = std::time::Instant::now();
        let out = run_command("sleep 30 & sleep 30", None, Duration::from_millis(300), LIMIT)
            .await
            .unwrap();
        assert!(out.timed_out);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn truncates_huge_output() {
        let out = run_command(
            "head -c 400000 /dev/zero | tr '\\0' a",
            None,
            TIMEOUT,
            1000, // a small, configured limit
        )
        .await
        .unwrap();
        assert!(out.truncated);
        assert_eq!(out.output.len(), 1000);
    }

    #[test]
    fn lists_directories_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("zdir")).unwrap();
        let listing = read_dir(Some(dir.path())).unwrap();
        let names: Vec<&str> = listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["zdir", "a.txt"]);
    }
}
