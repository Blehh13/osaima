use crate::security::Tier;
use serde::Serialize;
use std::error::Error;
use std::process::{Command, Output, Stdio};

#[derive(Debug, Serialize)]
pub struct ExecResult {
    pub output: String,
    pub exit_code: Option<i32>,
    /// Which isolation actually applied, so the UI and audit log never overstate it.
    pub sandbox: String,
}

pub struct SecureExecutor;

impl SecureExecutor {
    /// Runs an AI-generated command in a bubblewrap sandbox sized to its risk tier:
    ///   read-only    → whole filesystem read-only, private /tmp
    ///   reversible   → as above, but $HOME writable
    ///   irreversible → same as reversible; the system itself stays read-only, and
    ///                  sudo cannot work inside a user namespace, so root changes
    ///                  must be done by the user in a real terminal.
    /// Falls back to a systemd transient service when bwrap is unavailable.
    pub fn run_sandboxed(command: &str, tier: Tier) -> Result<ExecResult, Box<dyn Error>> {
        if command.trim().is_empty() {
            return Ok(ExecResult { output: String::new(), exit_code: Some(0), sandbox: "none".to_string() });
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let write = if tier >= Tier::Reversible { vec![home.clone()] } else { vec![] };
        let scope = Scope { write, cwd: home.clone(), network: true, timeout_secs: 60 };
        let out = run(sandbox(&scope, command, &[]))?;
        // exit 1 with a uid-map error means user namespaces are blocked (see apparmor profile)
        if !String::from_utf8_lossy(&out.stderr).starts_with("bwrap:") {
            return Ok(collect(out, "bubblewrap"));
        }
        eprintln!("Executor: bwrap unavailable: {}", String::from_utf8_lossy(&out.stderr).trim());

        let mut sd = with_timeout("systemd-run");
        sd.args(["--user", "--pipe", "--wait", "--collect", "--quiet"])
            .args(["-p", "NoNewPrivileges=yes", "-p", "PrivateTmp=yes", "-p", "ProtectSystem=strict"])
            .arg(format!("--working-directory={}", home));
        if tier == Tier::ReadOnly {
            sd.args(["-p", "ProtectHome=read-only"]);
        }
        sd.args(["--", "bash", "-c", command]);
        Ok(collect(run(sd)?, "systemd-service"))
    }

    /// Runs one step of an agent task: same sandbox and kernel limits, but the
    /// only writable places are the task's approved folders (plus a private
    /// /tmp), network as approved, and a longer time limit for installs/builds.
    pub fn run_scoped(command: &str, scope: &Scope) -> Result<ExecResult, Box<dyn Error>> {
        Ok(collect(run(sandbox(scope, command, &[]))?, "bubblewrap"))
    }

    /// Writes a file for a task from inside the sandbox, so the kernel (not the
    /// daemon's own path checks) decides whether the target is in the scope;
    /// a symlink planted in the folder cannot redirect the write.
    pub fn write_scoped(scope: &Scope, target: &str, content: &[u8]) -> Result<ExecResult, Box<dyn Error>> {
        let dir = std::env::var("XDG_RUNTIME_DIR").map(|d| format!("{}/osaima", d)).unwrap_or_else(|_| "/tmp".into());
        std::fs::create_dir_all(&dir)?;
        let staged = format!("{}/write-{}-{}", dir, std::process::id(), std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        std::fs::write(&staged, content)?;
        let cmd = r#"mkdir -p -- "$(dirname -- "$OSAIMA_TARGET")" && cp -- /tmp/.osaima-input "$OSAIMA_TARGET""#;
        let out = run(sandbox(scope, cmd, &[("--ro-bind", staged.as_str(), "/tmp/.osaima-input"), ("--setenv", "OSAIMA_TARGET", target)]));
        let _ = std::fs::remove_file(&staged);
        Ok(collect(out?, "bubblewrap"))
    }

    /// Run normal command without sandbox (for internal trusted OS operations if needed)
    pub fn run_trusted(command: &str) -> Result<String, Box<dyn Error>> {
        if command.trim().is_empty() {
            return Ok(String::new());
        }
        Ok(collect(Command::new("sh").arg("-c").arg(command).output()?, "none").output)
    }
}

/// What one agent task may touch: written folders, network, time per step.
#[derive(Clone, Debug)]
pub struct Scope {
    pub write: Vec<String>,
    pub cwd: String,
    pub network: bool,
    pub timeout_secs: u64,
}

/// The bubblewrap + osaima-jail command line for one sandboxed command.
/// Inside: the filesystem read-only except the scope's folders; the user's
/// session (/run/user: D-Bus, display, the OSAIMA daemon socket) and the
/// system bus replaced by empty folders; keys, tokens and browser profiles
/// hidden; an empty environment except a short allowlist (so no API key or
/// bus address leaks in); then osaima-jail's kernel limits.
fn sandbox(scope: &Scope, command: &str, extra: &[(&str, &str, &str)]) -> Command {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let uid = unsafe { libc::getuid() };
    let mut c = Command::new("timeout");
    c.args(["--kill-after=5", &scope.timeout_secs.to_string(), "bwrap"]);
    c.args(["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp"]);
    for w in &scope.write {
        c.args(["--bind", w, w]);
    }
    // after the binds, so they also cover a writable $HOME
    for hidden in [format!("/run/user/{}", uid), "/run/dbus".to_string()] {
        if std::path::Path::new(&hidden).is_dir() {
            c.args(["--tmpfs", &hidden]);
        }
    }
    for p in crate::files::PROTECTED {
        let full = format!("{}/{}", home, p);
        match std::fs::metadata(&full) {
            Ok(m) if m.is_dir() => { c.args(["--tmpfs", &full]); }
            Ok(_) => { c.args(["--ro-bind", "/dev/null", &full]); }
            Err(_) => {}
        }
    }
    c.arg("--unshare-all");
    if scope.network {
        c.arg("--share-net");
    }
    c.args(["--die-with-parent", "--new-session", "--chdir", &scope.cwd, "--clearenv"]);
    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
    let lang = std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into());
    let user = std::env::var("USER").unwrap_or_default();
    for (k, v) in [("PATH", path.as_str()), ("HOME", home.as_str()), ("USER", user.as_str()), ("LOGNAME", user.as_str()),
                   ("LANG", lang.as_str()), ("TERM", "dumb"), ("TMPDIR", "/tmp"),
                   // caches live in the private /tmp: tools that cache under ~ still work
                   ("XDG_CACHE_HOME", "/tmp/.cache"), ("PIP_CACHE_DIR", "/tmp/.cache/pip"), ("npm_config_cache", "/tmp/.cache/npm"),
                   ("CARGO_HOME", "/tmp/.cargo"), ("PYTHONDONTWRITEBYTECODE", "1")] {
        c.args(["--setenv", k, v]);
    }
    for (flag, a, b) in extra {
        c.args([*flag, *a, *b]);
    }
    if let Some(jail) = jail_path() {
        c.arg(&jail);
        for w in &scope.write {
            c.args(["--write", w]);
        }
        c.args(["--cpu", &scope.timeout_secs.to_string(), "--"]);
    }
    c.args(["bash", "-c", command]);
    c
}

/// osaima-jail ships next to the daemon binary (or OSAIMA_JAIL points to it).
fn jail_path() -> Option<String> {
    let p = std::env::var("OSAIMA_JAIL").map(std::path::PathBuf::from).ok().or_else(|| {
        std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("osaima-jail")))
    })?;
    p.is_file().then(|| p.to_string_lossy().into_owned())
}

/// Wraps a program in coreutils `timeout` so runaway commands are killed.
fn with_timeout(program: &str) -> Command {
    let mut c = Command::new("timeout");
    c.args(["--kill-after=5", "60", program]);
    c
}

fn run(mut cmd: Command) -> std::io::Result<Output> {
    cmd.stdin(Stdio::null()).output()
}

fn collect(out: Output, sandbox: &'static str) -> ExecResult {
    let mut output = String::from_utf8_lossy(&out.stdout).to_string();
    let mut err = String::from_utf8_lossy(&out.stderr).to_string();
    let mut sandbox = sandbox.to_string();
    // osaima-jail's first stderr line says what the kernel enforced
    if let Some(rest) = err.strip_prefix("osaima-jail: ") {
        let (status, tail) = rest.split_once('\n').unwrap_or((rest, ""));
        if status.contains("landlock=full") { sandbox.push_str("+landlock"); }
        else if status.contains("landlock=partial") { sandbox.push_str("+landlock(partial)"); }
        if status.contains("seccomp=on") { sandbox.push_str("+seccomp"); }
        err = tail.to_string();
    }
    if !err.is_empty() {
        output.push_str("\n[STDERR]\n");
        output.push_str(&err);
    }
    ExecResult { output, exit_code: out.status.code(), sandbox }
}
