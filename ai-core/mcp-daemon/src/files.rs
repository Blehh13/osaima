//! File tools for the assistant: search, open, show in the file manager,
//! copy, move, rename, new folder, and move to Trash with undo.
//!
//! Typed operations rather than shell text: paths are passed as arguments,
//! never through a shell, so a file name cannot smuggle in a command. Every
//! path must resolve inside $HOME and outside the protected places below.
//! Reached through system.os_action (action "files"), so approval, taint and
//! the audit log apply as for every other action.

use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Never read, changed or listed: keys, OSAIMA's own secrets and audit log.
/// Also hidden (empty) inside every AI-run command's sandbox: keys, tokens,
/// browser profiles, OSAIMA's own key and audit log.
pub const PROTECTED: &[&str] = &[".ssh", ".gnupg", ".config/osaima", ".local/share/osaima", ".local/share/keyrings", ".pki",
    ".password-store", ".mozilla", ".config/google-chrome", ".config/chromium", ".config/BraveSoftware", ".thunderbird",
    ".aws", ".azure", ".kube", ".docker", ".netrc", ".git-credentials", ".config/gh", ".config/gcloud"];

/// Opening these with the default app could run code, so the assistant won't.
pub fn is_launcher(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let exec = fs::metadata(p).map_or(false, |m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    exec || ["desktop", "sh", "bash", "run", "bin", "appimage", "exe", "jar", "deb", "rpm", "msi", "bat", "cmd", "ps1"].contains(&ext.as_str())
}
/// Not worth searching: caches, build output, dependency trees.
const SKIP_DIRS: &[&str] = &["node_modules", ".git", "target", "__pycache__", ".cache", ".venv", "venv", ".cargo", ".rustup", ".npm"];

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
}

/// Resolves `~`, relative paths (against $HOME) and `..` without touching the
/// disk, then checks the result is inside $HOME and not protected.
pub fn resolve(raw: &str) -> Result<PathBuf, String> {
    let home = home();
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("no path given".into());
    }
    let joined = if raw == "~" {
        home.clone()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        home.join(raw)
    };
    let mut clean = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => { clean.pop(); }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    // follow symlinks for the part that exists, so a link cannot point outside
    let real = clean.canonicalize().unwrap_or_else(|_| {
        clean.parent().and_then(|p| p.canonicalize().ok()).map(|p| p.join(clean.file_name().unwrap_or_default())).unwrap_or(clean.clone())
    });
    let rel = real.strip_prefix(&home).map_err(|_| format!("{} is outside your home folder", raw))?;
    let rel_s = rel.to_string_lossy();
    if PROTECTED.iter().any(|p| rel_s == *p || rel_s.starts_with(&format!("{}/", p))) {
        return Err(format!("{} is a protected location", raw));
    }
    Ok(real)
}

fn entry(p: &Path) -> Value {
    let md = fs::metadata(p).ok();
    let modified = md.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs());
    json!({
        "path": p.to_string_lossy(),
        "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "is_dir": md.as_ref().map_or(false, |m| m.is_dir()),
        "size": md.as_ref().map_or(0, |m| m.len()),
        "modified": modified,
    })
}

/// File kinds people ask for by name.
fn kind_matches(kind: &str, name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    let exts: &[&str] = match kind {
        "pdf" => &["pdf"],
        "image" | "images" | "photo" | "photos" | "picture" | "pictures" => &["jpg", "jpeg", "png", "gif", "webp", "heic", "bmp", "svg"],
        "video" | "videos" => &["mp4", "mkv", "webm", "mov", "avi"],
        "audio" | "music" | "song" | "songs" => &["mp3", "flac", "ogg", "wav", "m4a", "opus"],
        "document" | "documents" | "doc" | "docs" => &["pdf", "doc", "docx", "odt", "txt", "md", "rtf", "ppt", "pptx", "odp", "xls", "xlsx", "ods", "csv"],
        "archive" | "archives" | "zip" => &["zip", "tar", "gz", "xz", "bz2", "7z", "rar", "zst"],
        "code" => &["rs", "py", "js", "ts", "c", "h", "cpp", "java", "go", "sh", "html", "css"],
        _ => return true,
    };
    exts.contains(&ext.as_str())
}

/// Searches $HOME (or `folder`) by words in the name, kind, size and age.
fn search(params: &Value) -> Result<Value, String> {
    let root = match params["folder"].as_str().filter(|s| !s.is_empty()) {
        Some(f) => resolve(f)?,
        None => home(),
    };
    let words: Vec<String> = params["query"].as_str().unwrap_or("").to_lowercase()
        .split_whitespace().map(str::to_string).collect();
    let kind = params["kind"].as_str().unwrap_or("").to_lowercase();
    let min_size = params["min_size_mb"].as_f64().map(|m| (m * 1_048_576.0) as u64).unwrap_or(0);
    let newer = params["modified_days"].as_f64()
        .map(|d| SystemTime::now() - Duration::from_secs((d * 86400.0) as u64));
    let limit = params["limit"].as_u64().unwrap_or(30).min(100) as usize;
    let deadline = SystemTime::now() + Duration::from_secs(4);

    let mut found = Vec::new();
    let mut stack = vec![root.clone()];
    let mut scanned = 0usize;
    while let Some(dir) = stack.pop() {
        if SystemTime::now() > deadline || scanned > 200_000 {
            break;
        }
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            scanned += 1;
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            let path = e.path();
            if ft.is_dir() {
                if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) && resolve(&path.to_string_lossy()).is_ok() {
                    stack.push(path.clone());
                }
            }
            if name.starts_with('.') || !ft.is_file() && !(ft.is_dir() && kind.is_empty() && !words.is_empty()) {
                continue;
            }
            let lname = name.to_lowercase();
            if !words.iter().all(|w| lname.contains(w.as_str())) || !kind_matches(&kind, &name) {
                continue;
            }
            let Ok(md) = e.metadata() else { continue };
            if md.len() < min_size || newer.map_or(false, |t| md.modified().map_or(true, |m| m < t)) {
                continue;
            }
            found.push((md.modified().ok(), entry(&path)));
        }
    }
    // newest first (what people usually look for), or largest first on request
    if params["sort"].as_str() == Some("largest") {
        found.sort_by_key(|f| std::cmp::Reverse(f.1["size"].as_u64().unwrap_or(0)));
    } else {
        found.sort_by(|a, b| b.0.cmp(&a.0));
    }
    let total = found.len();
    Ok(json!({ "files": found.into_iter().take(limit).map(|f| f.1).collect::<Vec<_>>(), "total": total,
               "folder": root.to_string_lossy() }))
}

/// The Wayland display of the user's session, for programs the daemon starts.
fn session_env(cmd: &mut std::process::Command) {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        if let Ok(run) = std::env::var("XDG_RUNTIME_DIR") {
            if let Some(d) = fs::read_dir(&run).ok().and_then(|rd| rd.flatten()
                .filter(|e| { let n = e.file_name().to_string_lossy().into_owned(); n.starts_with("wayland-") && !n.ends_with(".lock") })
                .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())) {
                cmd.env("WAYLAND_DISPLAY", d.file_name());
            }
        }
    }
}

fn spawn(program: &str, args: &[&str]) -> Result<(), String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    session_env(&mut cmd);
    cmd.spawn().map(|_| ()).map_err(|e| format!("{}: {}", program, e))
}

/// A desktop notification (mako on the OSAIMA session), best effort.
pub fn notify_user(summary: &str, body: &str) {
    let _ = spawn("notify-send", &["-a", "OSAIMA", "-i", "dialog-information", summary, body]);
}

/// Freedesktop Trash, so Thunar shows trashed files and can restore them too.
fn trash_one(p: &Path) -> Result<(PathBuf, PathBuf), String> {
    let base = home().join(".local/share/Trash");
    fs::create_dir_all(base.join("files")).and_then(|_| fs::create_dir_all(base.join("info"))).map_err(|e| e.to_string())?;
    let name = p.file_name().ok_or("cannot trash this")?.to_string_lossy().into_owned();
    let mut target = name.clone();
    let mut n = 1;
    while base.join("files").join(&target).exists() || base.join("info").join(format!("{}.trashinfo", target)).exists() {
        n += 1;
        target = format!("{}.{}", name, n);
    }
    let encoded: String = p.to_string_lossy().bytes().map(|b| {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) { (b as char).to_string() } else { format!("%{:02X}", b) }
    }).collect();
    let date = std::process::Command::new("date").arg("+%Y-%m-%dT%H:%M:%S").output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let info = base.join("info").join(format!("{}.trashinfo", target));
    fs::write(&info, format!("[Trash Info]\nPath={}\nDeletionDate={}\n", encoded, date)).map_err(|e| e.to_string())?;
    let dest = base.join("files").join(&target);
    if let Err(e) = fs::rename(p, &dest) {
        let _ = fs::remove_file(&info);
        return Err(format!("could not move {} to the Trash: {}", p.display(), e));
    }
    Ok((dest, info))
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)?.flatten() {
            copy_recursive(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

/// `to` must be an existing folder; the file keeps its name (renaming is its
/// own action). A folder that doesn't exist is an error that lists the
/// folders that do, rather than silently becoming a new file name.
fn destination(from: &Path, to: &str) -> Result<PathBuf, String> {
    let dir = resolve(to)?;
    if !dir.is_dir() {
        let parent = dir.parent().unwrap_or(&dir);
        let mut near: Vec<String> = fs::read_dir(parent).map(|rd| rd.flatten()
            .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        near.sort();
        near.truncate(12);
        return Err(format!("there is no folder {} (folders in {}: {})", dir.display(), parent.display(),
                           if near.is_empty() { "none".into() } else { near.join(", ") }));
    }
    let dest = dir.join(from.file_name().ok_or("bad source")?);
    if dest.exists() {
        return Err(format!("{} already exists; I won't overwrite it", dest.display()));
    }
    Ok(dest)
}

/// What each change did, newest last, so "undo" can reverse it.
enum Undo {
    Moved(Vec<(PathBuf, PathBuf)>),          // (now at, originally at)
    Trashed(Vec<(PathBuf, PathBuf, PathBuf)>), // (in trash, info file, originally at)
    Created(Vec<PathBuf>),                    // copies and new folders
}
static UNDO: Mutex<Vec<Undo>> = Mutex::new(Vec::new());

fn paths(params: &Value) -> Result<Vec<PathBuf>, String> {
    let list: Vec<String> = match &params["paths"] {
        Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        _ => params["path"].as_str().map(|s| vec![s.to_string()]).unwrap_or_default(),
    };
    if list.is_empty() {
        return Err("no file given".into());
    }
    list.iter().map(|p| {
        let r = resolve(p)?;
        if r.exists() { Ok(r) } else { Err(format!("{} doesn't exist", p)) }
    }).collect()
}

/// Folders in $HOME (two levels deep) whose name the request mentions, e.g.
/// "my Downloads" or "the osaima-plantest folder", so the model sees the real
/// file names instead of guessing them.
pub fn folders_mentioned(query: &str) -> Vec<PathBuf> {
    let q = query.to_lowercase();
    let words: Vec<&str> = q.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_' && c != '.').filter(|w| w.len() >= 3).collect();
    let mut out = Vec::new();
    let mut level = vec![home()];
    for _ in 0..2 {
        let mut next = Vec::new();
        for dir in &level {
            for e in fs::read_dir(dir).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) || !e.path().is_dir() {
                    continue;
                }
                if words.iter().any(|w| *w == name) && resolve(&e.path().to_string_lossy()).is_ok() {
                    out.push(e.path());
                }
                next.push(e.path());
            }
        }
        level = next;
    }
    out.truncate(3);
    out
}

/// A short listing of a folder for the model: names, folders marked with "/".
pub fn listing(dir: &Path) -> String {
    let mut items: Vec<(bool, String, u64)> = fs::read_dir(dir).into_iter().flatten().flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| { let md = e.metadata().ok(); (md.as_ref().map_or(false, |m| m.is_dir()), e.file_name().to_string_lossy().into_owned(), md.map_or(0, |m| m.len())) })
        .collect();
    items.sort_by(|a, b| (!a.0, a.1.to_lowercase()).cmp(&(!b.0, b.1.to_lowercase())));
    let total = items.len();
    let shown: Vec<String> = items.into_iter().take(60).map(|(d, n, s)| if d { format!("{}/", n) } else { format!("{} ({} KB)", n, s / 1024) }).collect();
    let shown_n = shown.len();
    let tilde = dir.to_string_lossy().replacen(&*home().to_string_lossy(), "~", 1);
    format!("Contents of {} ({} items{}):\n{}", tilde, total, if total > shown_n { ", first 60 shown" } else { "" }, shown.join("\n"))
}

/// Files the user handed to the assistant from the file manager ("Ask OSAIMA"),
/// waiting for the assistant window to pick them up.
static SHARED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn share(list: &[String]) -> Result<usize, String> {
    let ok: Vec<String> = list.iter().filter_map(|p| resolve(p).ok()).filter(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned()).collect();
    if ok.is_empty() {
        return Err("none of those files can be shared with the assistant".into());
    }
    let n = ok.len();
    *SHARED.lock().unwrap() = ok;
    Ok(n)
}

pub fn take_shared() -> Vec<String> {
    std::mem::take(&mut *SHARED.lock().unwrap())
}

/// A request typed in the Ask bar (Super+Space), waiting for the assistant.
static ASKED: Mutex<Option<String>> = Mutex::new(None);

pub fn ask(query: &str) -> Result<(), String> {
    let q = query.trim();
    if q.is_empty() || q.len() > 2000 {
        return Err("type a request first".into());
    }
    *ASKED.lock().unwrap() = Some(q.to_string());
    Ok(())
}

pub fn take_asked() -> Option<String> {
    ASKED.lock().unwrap().take()
}

/// What the model sees for a selected file: where it is, its size, and the
/// start of its text for documents it can read (PDFs via pdftotext).
pub fn describe_for_model(raw: &str) -> Option<(String, String)> {
    let p = resolve(raw).ok()?;
    let md = fs::metadata(&p).ok()?;
    let name = p.file_name()?.to_string_lossy().into_owned();
    let tilde = p.to_string_lossy().replacen(&*home().to_string_lossy(), "~", 1);
    let mut text = format!("Selected {}: {} ({} KB)", if md.is_dir() { "folder" } else { "file" }, tilde, md.len() / 1024);
    if md.is_dir() {
        text.push_str("\n");
        text.push_str(&listing(&p));
    } else {
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        let body = if ext == "pdf" {
            std::process::Command::new("pdftotext").args(["-l", "5", "-q"]).arg(&p).arg("-").output().ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        } else if ["txt", "md", "csv", "log", "json", "py", "rs", "js", "ts", "sh", "html", "css", "toml", "yaml", "yml", "c", "h", "cpp", "java"].contains(&ext.as_str()) && md.len() < 2_000_000 {
            fs::read_to_string(&p).ok()
        } else {
            None
        };
        if let Some(b) = body.filter(|b| !b.trim().is_empty()) {
            text.push_str("\nContent (start):\n");
            text.extend(b.chars().take(6000));
        }
    }
    Some((format!("file:{}", name), text))
}

/// Checks a change can be done (files exist, destination is a real folder,
/// the new name is valid) before the user is asked to approve it.
pub fn check(op: &str, params: &Value) -> Result<(), String> {
    match op {
        "copy" | "move" => {
            let to = params["to"].as_str().ok_or("no destination given")?;
            for from in paths(params)? {
                destination(&from, to)?;
            }
            Ok(())
        }
        "rename" | "trash" | "open" | "reveal" => paths(params).map(|_| ()),
        _ => Ok(()),
    }
}

pub fn perform(op: &str, params: &Value) -> Result<Value, String> {
    match op {
        "search" => search(params),
        "list" => {
            let dir = resolve(params["path"].as_str().unwrap_or("~"))?;
            let mut items: Vec<Value> = fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten()
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .map(|e| entry(&e.path())).collect();
            items.sort_by_key(|v| (!v["is_dir"].as_bool().unwrap_or(false), v["name"].as_str().unwrap_or("").to_lowercase()));
            Ok(json!({ "folder": dir.to_string_lossy(), "files": items }))
        }
        "open" => {
            let p = paths(params)?;
            if let Some(l) = p.iter().find(|f| is_launcher(f)) {
                return Err(format!("{} is a program or launcher; I won't open it for you (use Show in Files instead)", l.display()));
            }
            for f in &p {
                spawn("xdg-open", &[&f.to_string_lossy()])?;
            }
            Ok(json!({ "opened": p.len() }))
        }
        "reveal" => {
            let p = paths(params)?.remove(0);
            let (dir, name) = if p.is_dir() { (p.clone(), String::new()) } else {
                (p.parent().unwrap_or(&p).to_path_buf(), p.file_name().unwrap_or_default().to_string_lossy().into_owned())
            };
            // Thunar's D-Bus interface opens the folder with the file selected
            let uri = format!("file://{}", dir.to_string_lossy());
            let ok = std::process::Command::new("gdbus").args(["call", "--session", "--dest", "org.xfce.FileManager",
                "--object-path", "/org/xfce/FileManager", "--method", "org.xfce.FileManager.DisplayFolderAndSelect",
                &uri, &name, "", ""]).output().map_or(false, |o| o.status.success());
            if !ok {
                spawn("thunar", &[&dir.to_string_lossy()])?;
            }
            Ok(json!({ "shown": p.to_string_lossy() }))
        }
        "mkdir" => {
            let p = resolve(params["path"].as_str().unwrap_or(""))?;
            // a folder that is already there is the goal reached (routines re-run this)
            if p.is_dir() {
                return Ok(json!({ "created": p.to_string_lossy(), "existed": true }));
            }
            if p.exists() {
                return Err(format!("{} already exists and is not a folder", p.display()));
            }
            fs::create_dir_all(&p).map_err(|e| e.to_string())?;
            UNDO.lock().unwrap().push(Undo::Created(vec![p.clone()]));
            Ok(json!({ "created": p.to_string_lossy(), "undoable": true }))
        }
        "copy" | "move" => {
            let to = params["to"].as_str().ok_or("no destination given")?;
            let mut done = Vec::new();
            for from in paths(params)? {
                let dest = destination(&from, to)?;
                if op == "copy" {
                    copy_recursive(&from, &dest).map_err(|e| format!("copying {}: {}", from.display(), e))?;
                } else {
                    fs::rename(&from, &dest).or_else(|_| copy_recursive(&from, &dest).and_then(|_|
                        if from.is_dir() { fs::remove_dir_all(&from) } else { fs::remove_file(&from) }))
                        .map_err(|e| format!("moving {}: {}", from.display(), e))?;
                }
                done.push((dest, from));
            }
            let out: Vec<String> = done.iter().map(|(d, _)| d.to_string_lossy().into_owned()).collect();
            UNDO.lock().unwrap().push(if op == "copy" { Undo::Created(done.into_iter().map(|d| d.0).collect()) } else { Undo::Moved(done) });
            Ok(json!({ op: out, "undoable": true }))
        }
        "rename" => {
            let from = paths(params)?.remove(0);
            let name = params["name"].as_str().unwrap_or("").trim();
            if name.is_empty() || name.contains('/') || name == "." || name == ".." {
                return Err("the new name must be a plain file name".into());
            }
            let dest = from.with_file_name(name);
            if dest.exists() {
                return Err(format!("{} already exists", dest.display()));
            }
            fs::rename(&from, &dest).map_err(|e| e.to_string())?;
            UNDO.lock().unwrap().push(Undo::Moved(vec![(dest.clone(), from)]));
            Ok(json!({ "renamed": dest.to_string_lossy(), "undoable": true }))
        }
        "trash" => {
            let mut done = Vec::new();
            for p in paths(params)? {
                if p == home() {
                    return Err("I won't trash your whole home folder".into());
                }
                let (dest, info) = trash_one(&p)?;
                done.push((dest, info, p));
            }
            let n = done.len();
            UNDO.lock().unwrap().push(Undo::Trashed(done));
            Ok(json!({ "trashed": n, "undoable": true }))
        }
        "undo" => {
            let last = UNDO.lock().unwrap().pop().ok_or("there is nothing to undo")?;
            match last {
                Undo::Moved(v) => {
                    for (now, orig) in &v {
                        fs::rename(now, orig).map_err(|e| e.to_string())?;
                    }
                    Ok(json!({ "undone": "move", "count": v.len() }))
                }
                Undo::Trashed(v) => {
                    for (t, info, orig) in &v {
                        if orig.exists() {
                            return Err(format!("{} exists again, so I can't put the old one back", orig.display()));
                        }
                        fs::rename(t, orig).map_err(|e| e.to_string())?;
                        let _ = fs::remove_file(info);
                    }
                    Ok(json!({ "undone": "trash", "count": v.len() }))
                }
                Undo::Created(v) => {
                    // a copy or new folder goes to the Trash rather than being deleted outright
                    for p in &v {
                        if p.exists() {
                            trash_one(p)?;
                        }
                    }
                    Ok(json!({ "undone": "create", "count": v.len() }))
                }
            }
        }
        _ => Err(format!("unknown file action '{}'", op)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_must_stay_in_home() {
        assert!(resolve("/etc/passwd").is_err());
        assert!(resolve("~/../../etc/passwd").is_err());
        assert!(resolve("../").is_err());
        assert!(resolve("~/.ssh/id_ed25519").is_err());
        assert!(resolve("~/.config/osaima/llm.env").is_err());
        assert!(resolve("~/.local/share/osaima/audit.log").is_err());
        assert!(resolve("~/Documents/report.pdf").is_ok());
        assert!(resolve("Downloads").is_ok());
    }

    #[test]
    fn kinds() {
        assert!(kind_matches("pdf", "Report.PDF"));
        assert!(kind_matches("photos", "a.jpeg"));
        assert!(!kind_matches("pdf", "a.txt"));
        assert!(kind_matches("", "anything"));
    }
}
