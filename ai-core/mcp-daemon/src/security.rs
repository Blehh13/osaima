//! Risk tiers and the tamper-evident audit log for LLM-generated commands (PDR §4).
//!
//! Every command the AI proposes is assessed here, never in the UI, so a
//! compromised or buggy frontend cannot skip confirmation.

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Only inspects the system: runs without asking.
    ReadOnly,
    /// Changes state that can be undone: one-click confirmation.
    Reversible,
    /// Destroys data, escalates privilege or leaves the machine: type-to-confirm.
    Irreversible,
}

#[derive(Debug, Clone, Serialize)]
pub struct Assessment {
    pub tier: Tier,
    pub reasons: Vec<String>,
}

/// What the user must type to approve an irreversible command.
pub const CONFIRM_PHRASE: &str = "confirm";

const READ_ONLY: &[&str] = &[
    "ls", "cat", "head", "tail", "less", "more", "grep", "egrep", "rg", "find", "fd", "du", "df",
    "free", "ps", "pgrep", "uptime", "uname", "whoami", "id", "pwd", "echo", "printf", "date", "cal",
    "which", "whereis", "type", "file", "stat", "wc", "sort", "uniq", "cut", "tr", "lsblk", "lscpu",
    "lsusb", "lspci", "lsmod", "nproc", "hostname", "env", "printenv", "journalctl", "sensors",
    "ping", "ip", "ifconfig", "nmcli", "iwgetid", "neofetch", "fastfetch", "tree", "basename",
    "dirname", "realpath", "readlink", "md5sum", "sha256sum", "diff", "true", "test", "[",
];

const DESTRUCTIVE: &[&str] = &[
    "rm", "rmdir", "dd", "shred", "wipefs", "fdisk", "sfdisk", "parted", "mkswap", "shutdown",
    "reboot", "poweroff", "halt", "killall", "pkill", "truncate", "userdel", "groupdel", "passwd",
    "chpasswd", "visudo", "mount", "umount", "swapoff", "iptables", "nft", "insmod", "rmmod",
    "modprobe", "crontab", "at",
];

const PRIVILEGE: &[&str] = &["sudo", "su", "doas", "pkexec", "run0"];

const SYSTEM_PATHS: &[&str] = &["/etc", "/boot", "/dev", "/sys", "/proc", "/usr", "/bin", "/sbin", "/lib", "/var/lib"];

/// Splits a shell line into simple commands on ; && || | and newlines.
fn segments(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut sq, mut dq) = (false, false);
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' if !dq => sq = !sq,
            '"' if !sq => dq = !dq,
            ';' | '|' | '&' | '\n' if !sq && !dq => {
                // `&>` and `>&` are redirections, not separators
                let redirect = c == '&' && (chars.get(i + 1) == Some(&'>') || (i > 0 && chars[i - 1] == '>'));
                if !redirect {
                    if !cur.trim().is_empty() {
                        out.push(cur.trim().to_string());
                    }
                    cur.clear();
                    i += 1;
                    continue;
                }
            }
            _ => {}
        }
        cur.push(c);
        i += 1;
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn base(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn assess_segment(seg: &str, piped_input: bool) -> Assessment {
    let mut tier = Tier::ReadOnly;
    let mut reasons: Vec<String> = Vec::new();
    fn raise(t: Tier, why: String, tier: &mut Tier, reasons: &mut Vec<String>) {
        if t > *tier {
            *tier = t;
        }
        reasons.push(why);
    }
    macro_rules! bump {
        ($t:expr, $why:expr) => { raise($t, $why, &mut tier, &mut reasons) };
    }

    let words: Vec<&str> = seg.split_whitespace().collect();
    // skip leading VAR=value assignments
    let mut idx = words.iter().position(|w| !(w.contains('=') && !w.starts_with('-'))).unwrap_or(words.len());
    if idx < words.len() && PRIVILEGE.contains(&base(words[idx])) {
        bump!(Tier::Irreversible, format!("runs as root via `{}`", words[idx]));
        idx += 1;
        while idx < words.len() && words[idx].starts_with('-') {
            idx += 1;
        }
    }
    let Some(&first) = words.get(idx) else {
        return Assessment { tier, reasons };
    };
    let prog = base(first);
    let args = &words[idx + 1..];
    let has = |flag: &str| args.iter().any(|a| *a == flag);

    if matches!(prog, "eval" | "exec" | "source" | ".") {
        bump!(Tier::Irreversible, format!("`{}` runs arbitrary, possibly hidden code", prog));
    } else if DESTRUCTIVE.contains(&prog) || prog.starts_with("mkfs") {
        bump!(Tier::Irreversible, format!("`{}` can destroy data or stop the system", prog));
    } else if matches!(prog, "sh" | "bash" | "zsh" | "dash" | "python" | "python3" | "perl" | "node") && piped_input {
        bump!(Tier::Irreversible, "pipes downloaded or generated code into an interpreter".into());
    } else if prog == "git" && args.iter().any(|a| matches!(*a, "push" | "clean")) || (prog == "git" && has("--hard")) {
        bump!(Tier::Irreversible, "git operation that publishes or discards work".into());
    } else if matches!(prog, "chmod" | "chown" | "chgrp") && (has("-R") || has("--recursive")) {
        bump!(Tier::Irreversible, format!("recursive `{}`", prog));
    } else if prog == "find" && args.iter().any(|a| matches!(*a, "-delete" | "-ok" | "-okdir" | "-fprint" | "-fprint0" | "-fprintf" | "-fls")) {
        bump!(Tier::Irreversible, "`find` that deletes or writes files".into());
    } else if prog == "find" && args.iter().any(|a| matches!(*a, "-exec" | "-execdir")) {
        // like xargs: judge each command find runs (up to `;` or `+`)
        let ends = |a: &str| matches!(a, ";" | r"\;" | "+" | "';'");
        let mut i = 0;
        while i < args.len() {
            if matches!(args[i], "-exec" | "-execdir") {
                let start = i + 1;
                let mut end = start;
                while end < args.len() && !ends(args[end]) {
                    end += 1;
                }
                let inner = assess_segment(&args[start..end].join(" "), false);
                if start == end {
                    bump!(Tier::Irreversible, "`find -exec` with no command".into());
                } else if inner.tier > Tier::ReadOnly {
                    bump!(inner.tier, format!("find runs: {}", inner.reasons.join("; ")));
                }
                i = end;
            }
            i += 1;
        }
    } else if matches!(prog, "apt" | "apt-get" | "dnf" | "pacman" | "emerge" | "pip" | "pip3" | "npm" | "snap" | "flatpak")
        && args.iter().any(|a| matches!(*a, "remove" | "purge" | "uninstall" | "-C" | "--unmerge" | "-R" | "-Rns" | "autoremove"))
    {
        bump!(Tier::Irreversible, "removes installed software".into());
    } else if prog == "sed" && !args.iter().any(|a| a.starts_with("-i")) || matches!(prog, "awk" | "gawk" | "jq") {
        // text filters: read-only unless they edit in place
    } else if prog == "systemctl" && args.first().map_or(false, |a| matches!(*a, "status" | "list-units" | "is-active" | "show")) {
        // inspection only
    } else if prog == "git" && args.first().map_or(false, |a| matches!(*a, "status" | "log" | "diff" | "show" | "branch" | "remote")) {
        // inspection only
    } else if prog == "xargs" {
        // xargs runs the command after its own options: judge that command
        let cmd: Vec<&str> = args.iter().skip_while(|a| a.starts_with('-')).copied().collect();
        let inner = assess_segment(&cmd.join(" "), false);
        if cmd.is_empty() {
            // bare xargs runs echo
        } else if inner.tier > Tier::ReadOnly {
            bump!(inner.tier, format!("xargs runs: {}", inner.reasons.join("; ")));
        }
    } else if !READ_ONLY.contains(&prog) {
        bump!(Tier::Reversible, format!("`{}` may change system state", prog));
    }

    // Command substitution: judge the commands inside it rather than assuming the worst.
    for inner in substitutions(seg) {
        let a = assess(&inner);
        if a.tier > Tier::ReadOnly {
            bump!(a.tier, format!("command substitution `{}`: {}", inner, a.reasons.join("; ")));
        }
    }
    if seg.contains('`') {
        bump!(Tier::Reversible, "uses backtick command substitution".into());
    }

    // Output redirection writes files; into system paths it is irreversible.
    for (i, w) in words.iter().enumerate() {
        if let Some(pos) = w.find('>') {
            let target = if w[pos..].trim_start_matches(&['>', '&'][..]).is_empty() {
                words.get(i + 1).copied().unwrap_or("")
            } else {
                w[pos..].trim_start_matches(&['>', '&'][..])
            };
            if target == "/dev/null" || target.is_empty() {
                continue;
            }
            if SYSTEM_PATHS.iter().any(|p| target.starts_with(p)) {
                bump!(Tier::Irreversible, format!("writes to system path {}", target));
            } else {
                bump!(Tier::Reversible, format!("writes to {}", target));
            }
        }
    }
    for w in args {
        if SYSTEM_PATHS.iter().any(|p| w.starts_with(p)) && tier >= Tier::Reversible && !reasons.iter().any(|r| r.contains("system path")) {
            bump!(Tier::Irreversible, format!("modifies system path {}", w));
        }
    }

    Assessment { tier, reasons }
}

/// Contents of each $( ... ) in a segment, handling nesting.
fn substitutions(seg: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = seg.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'$' && b[i + 1] == b'(' {
            let (start, mut depth, mut j) = (i + 2, 1, i + 2);
            while j < b.len() && depth > 0 {
                match b[j] { b'(' => depth += 1, b')' => depth -= 1, _ => {} }
                j += 1;
            }
            out.push(seg[start..j.saturating_sub(1).max(start)].to_string());
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Classifies a whole command line; the riskiest segment decides the tier.
pub fn assess(cmd: &str) -> Assessment {
    let mut tier = Tier::ReadOnly;
    let mut reasons = Vec::new();
    if cmd.trim().is_empty() {
        return Assessment { tier, reasons };
    }
    let piped = cmd.contains('|');
    for (i, seg) in segments(cmd).iter().enumerate() {
        let a = assess_segment(seg, piped && i > 0);
        tier = tier.max(a.tier);
        reasons.extend(a.reasons);
    }
    reasons.dedup();
    Assessment { tier, reasons }
}

/// Append-only JSONL log where each record carries the SHA-256 of the previous
/// one, so editing or deleting any line breaks the chain (PDR §4.4).
pub struct AuditLog {
    path: PathBuf,
    last_hash: Mutex<String>,
}

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{:02x}", b)).collect()
}

impl AuditLog {
    pub fn open_default() -> Self {
        let path = std::env::var("OSAIMA_AUDIT_LOG").map(PathBuf::from).unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".local/share/osaima/audit.log")
        });
        Self::open(path)
    }

    pub fn open(path: PathBuf) -> Self {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let last = fs::File::open(&path)
            .ok()
            .and_then(|f| BufReader::new(f).lines().filter_map(Result::ok).filter(|l| !l.trim().is_empty()).last())
            .and_then(|l| serde_json::from_str::<Value>(&l).ok())
            .and_then(|v| v["hash"].as_str().map(str::to_string))
            .unwrap_or_else(|| GENESIS.to_string());
        Self { path, last_hash: Mutex::new(last) }
    }

    /// Appends one record and returns its hash.
    pub fn record(&self, event: &str, data: Value) -> String {
        let mut last = self.last_hash.lock().unwrap();
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let body = json!({ "ts": ts, "event": event, "data": data, "prev": *last });
        let hash = sha256_hex(body.to_string().as_bytes());
        let mut rec = body;
        rec["hash"] = json!(hash);
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{}", rec);
        }
        *last = hash.clone();
        hash
    }

    /// The newest `n` records (oldest first) whose event is in `events`,
    /// without their hashes: what the activity view shows.
    pub fn tail(&self, n: usize, events: &[&str]) -> Vec<Value> {
        let Ok(f) = fs::File::open(&self.path) else { return Vec::new() };
        let mut out: std::collections::VecDeque<Value> = std::collections::VecDeque::with_capacity(n + 1);
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(rec) = serde_json::from_str::<Value>(&line) else { continue };
            if events.contains(&rec["event"].as_str().unwrap_or("")) {
                out.push_back(json!({ "ts": rec["ts"], "event": rec["event"], "data": rec["data"] }));
                if out.len() > n {
                    out.pop_front();
                }
            }
        }
        out.into()
    }

    /// A page of records, newest first, without their hashes. `events` limits
    /// the kinds (empty = all); `text` keeps records whose event or data
    /// contain it (case-insensitive). Returns the page and how many records
    /// match in all, so the history view can offer "load more".
    pub fn query(&self, offset: usize, limit: usize, events: &[&str], text: &str) -> (Vec<Value>, usize) {
        let Ok(f) = fs::File::open(&self.path) else { return (Vec::new(), 0) };
        let needle = text.trim().to_lowercase();
        let mut hits: Vec<Value> = Vec::new();
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(rec) = serde_json::from_str::<Value>(&line) else { continue };
            let event = rec["event"].as_str().unwrap_or("");
            if !events.is_empty() && !events.contains(&event) {
                continue;
            }
            if !needle.is_empty()
                && !event.to_lowercase().contains(&needle)
                && !rec["data"].to_string().to_lowercase().contains(&needle)
            {
                continue;
            }
            hits.push(json!({ "ts": rec["ts"], "event": rec["event"], "data": rec["data"] }));
        }
        let total = hits.len();
        hits.reverse();
        (hits.into_iter().skip(offset).take(limit).collect(), total)
    }

    /// Re-hashes every record; reports the first line where the chain breaks.
    pub fn verify(&self) -> Value {
        let Ok(f) = fs::File::open(&self.path) else {
            return json!({ "ok": true, "records": 0 });
        };
        let mut prev = GENESIS.to_string();
        let mut n = 0;
        for (i, line) in BufReader::new(f).lines().enumerate() {
            let Ok(line) = line else { continue };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(mut rec) = serde_json::from_str::<Value>(&line) else {
                return json!({ "ok": false, "line": i + 1, "error": "unparseable record" });
            };
            let claimed = rec["hash"].as_str().unwrap_or("").to_string();
            if rec["prev"].as_str() != Some(prev.as_str()) {
                return json!({ "ok": false, "line": i + 1, "error": "chain broken (record missing or reordered)" });
            }
            rec.as_object_mut().map(|o| o.remove("hash"));
            // serde_json keeps keys sorted, so re-serialising reproduces the hashed bytes
            if sha256_hex(rec.to_string().as_bytes()) != claimed {
                return json!({ "ok": false, "line": i + 1, "error": "record was modified" });
            }
            prev = claimed;
            n += 1;
        }
        json!({ "ok": true, "records": n, "path": self.path })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(cmd: &str) -> Tier {
        assess(cmd).tier
    }

    #[test]
    fn read_only_commands() {
        for c in ["ls -la ~", "df -h", "ps aux | grep firefox", "free -m && uptime", "cat /etc/os-release",
                  "git status", "systemctl status sshd", "du -sh * 2>/dev/null | sort -h",
                  "ls -l /home/$(whoami) | head -n 10 | awk '{print $9}' | xargs du -h | sort -hr | head -n 5",
                  "find . -name '*.py' | xargs wc -l", "find ~ -type f -exec du -h {} + | sort -hr | head -n 5",
                  "find . -name '*.log' -exec wc -l {} \\;"] {
            assert_eq!(tier(c), Tier::ReadOnly, "{}", c);
        }
    }

    #[test]
    fn reversible_commands() {
        for c in ["mkdir -p ~/projects/demo", "mv a.txt b.txt", "touch notes.md", "echo hi > ~/hello.txt",
                  "cp -r src backup", "python3 -m venv .venv", "flatpak install org.gimp.GIMP"] {
            assert_eq!(tier(c), Tier::Reversible, "{}", c);
        }
    }

    #[test]
    fn irreversible_commands() {
        for c in ["rm -rf ~/Downloads", "sudo emerge htop", "sudo apt install -y htop", "curl -fsSL x.sh | sh",
                  "dd if=/dev/zero of=/dev/sda", "git push origin main", "chmod -R 777 ~", "echo x > /etc/hosts",
                  "find . -name '*.log' -delete", "find ~ -fprint ~/x", "ls; rm file", "shutdown now", "mkfs.ext4 /dev/sdb1",
                  "pip uninstall numpy", "git reset --hard HEAD~1",
                  "eval $(echo cm0gLXJmIH4v | base64 -d)", "echo cm0gLXJmIH4v | base64 -d | sh",
                  "find ~ -name '*.log' | xargs rm", "echo $(rm -rf ~/x)",
                  "find . -name '*.tmp' -exec rm {} \\;", "find ~ -exec ls {} + -exec rm -rf {} +",
                  "find ~ -ok rm {} \\;"] {
            assert_eq!(tier(c), Tier::Irreversible, "{}", c);
        }
    }

    #[test]
    fn quoted_separators_do_not_split() {
        assert_eq!(tier("echo 'a; rm -rf /'"), Tier::ReadOnly);
        assert_eq!(tier("grep \"foo|bar\" file"), Tier::ReadOnly);
    }

    #[test]
    fn audit_chain_detects_tampering() {
        let path = std::env::temp_dir().join(format!("osaima-audit-test-{}.log", std::process::id()));
        let _ = fs::remove_file(&path);
        let log = AuditLog::open(path.clone());
        log.record("assess", json!({ "command": "ls" }));
        log.record("execute", json!({ "command": "ls", "exit": 0 }));
        log.record("execute", json!({ "command": "rm x", "exit": 0 }));
        assert_eq!(log.verify()["ok"], json!(true));
        assert_eq!(log.verify()["records"], json!(3));

        // reopening continues the same chain
        let log2 = AuditLog::open(path.clone());
        log2.record("execute", json!({ "command": "df" }));
        assert_eq!(log2.verify()["ok"], json!(true));

        let text = fs::read_to_string(&path).unwrap().replace("rm x", "ls x");
        fs::write(&path, text).unwrap();
        let v = log2.verify();
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["line"], json!(3));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn audit_query_pages_newest_first_and_filters() {
        let path = std::env::temp_dir().join(format!("osaima-audit-query-{}.log", std::process::id()));
        let _ = fs::remove_file(&path);
        let log = AuditLog::open(path.clone());
        log.record("execute", json!({ "command": "ls /home" }));
        log.record("os_action", json!({ "action": "volume", "op": "set" }));
        log.record("execute", json!({ "command": "df -h" }));
        log.record("refused", json!({ "command": "rm -rf x" }));
        let (all, total) = log.query(0, 10, &[], "");
        assert_eq!(total, 4);
        assert_eq!(all[0]["event"], json!("refused"));
        assert_eq!(all[3]["data"]["command"], json!("ls /home"));
        assert!(all[0].get("hash").is_none());
        // paging: total stays the match count, offset skips the newest
        let (page, total) = log.query(1, 2, &[], "");
        assert_eq!((page.len(), total), (2, 4));
        assert_eq!(page[0]["data"]["command"], json!("df -h"));
        let (past, _) = log.query(10, 5, &[], "");
        assert!(past.is_empty());
        // event filter and case-insensitive text search
        let (ex, total) = log.query(0, 10, &["execute"], "");
        assert_eq!((ex.len(), total), (2, 2));
        let (found, total) = log.query(0, 10, &[], "DF -H");
        assert_eq!((found.len(), total), (1, 1));
        let (none, total) = log.query(0, 10, &["os_action"], "df");
        assert!((none.is_empty(), total) == (true, 0));
        // a missing log is empty, not an error
        let missing = AuditLog::open(std::env::temp_dir().join("osaima-audit-query-none.log"));
        assert_eq!(missing.query(0, 10, &[], "").1, 0);
        let _ = fs::remove_file(&path);
    }
}
