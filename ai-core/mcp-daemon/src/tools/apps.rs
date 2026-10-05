//! Installed applications from freedesktop `.desktop` entries, and launching
//! them. Only programs with a desktop entry can be started; the agent can
//! never run an arbitrary command line through this tool.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Stdio;

use serde::Serialize;
use serde_json::{json, Value};

use super::{read_only, reversible, tool, to_value, Args, ToolContext, ToolError, ToolResult};

/// Terminal emulator used for `Terminal=true` entries when `$TERMINAL` is unset.
const DEFAULT_TERMINAL: &str = "foot";

pub fn definitions() -> Vec<Value> {
    vec![
        tool(
            "list_apps",
            "List installed apps",
            "Installed desktop applications. Optionally filter by a search term matched against name, description and keywords.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 50 }
                },
                "additionalProperties": false
            }),
            read_only(),
        ),
        tool(
            "launch_app",
            "Open an app",
            "Start an installed application by its id (e.g. \"firefox.desktop\") or name (e.g. \"Firefox\").",
            json!({
                "type": "object",
                "properties": { "app": { "type": "string" } },
                "required": ["app"],
                "additionalProperties": false
            }),
            reversible(),
        ),
    ]
}

#[derive(Debug, Clone, Serialize)]
pub struct DesktopApp {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generic_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub categories: Vec<String>,
    #[serde(skip)]
    keywords: Vec<String>,
    #[serde(skip)]
    exec: String,
    #[serde(skip)]
    terminal: bool,
}

impl DesktopApp {
    fn matches(&self, needle: &str) -> bool {
        let needle = needle.to_lowercase();
        self.id.to_lowercase().contains(&needle)
            || self.name.to_lowercase().contains(&needle)
            || self
                .generic_name
                .as_deref()
                .is_some_and(|g| g.to_lowercase().contains(&needle))
            || self
                .comment
                .as_deref()
                .is_some_and(|c| c.to_lowercase().contains(&needle))
            || self.keywords.iter().any(|k| k.to_lowercase().contains(&needle))
    }
}

pub fn list_apps(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    let args = Args::parse(arguments, &["query", "limit"])?;
    let limit = args.u64_in("limit", 1, 200, 50)? as usize;
    let query = args.str("query")?.map(str::trim).filter(|q| !q.is_empty());
    let apps = installed_apps(&ctx.paths.application_dirs);
    let matching: Vec<&DesktopApp> = apps
        .iter()
        .filter(|a| match query {
            Some(q) => a.matches(q),
            None => true,
        })
        .take(limit)
        .collect();
    Ok(json!({ "apps": to_value(&matching), "total_installed": apps.len() }))
}

pub async fn launch_app(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    let args = Args::parse(arguments, &["app"])?;
    let wanted = args.required_str("app")?;
    let apps = installed_apps(&ctx.paths.application_dirs);
    let app = resolve(&apps, wanted)?;
    let mut argv = exec_argv(&app.exec)
        .map_err(|e| ToolError::failed(format!("{} has an invalid Exec line: {e}", app.id)))?;
    if app.terminal {
        let terminal = std::env::var("TERMINAL").unwrap_or_else(|_| DEFAULT_TERMINAL.into());
        argv.splice(0..0, [terminal, "-e".into()]);
    }
    spawn_detached(&argv, &ctx.paths.home).await?;
    tracing::info!(app = %app.id, "launched application");
    Ok(json!({ "launched": app.id, "name": app.name }))
}

/// Pick one app for `wanted`: exact id, exact name, then a unique partial match.
fn resolve<'a>(apps: &'a [DesktopApp], wanted: &str) -> Result<&'a DesktopApp, ToolError> {
    let lower = wanted.to_lowercase();
    let id = lower.strip_suffix(".desktop").unwrap_or(&lower);
    if let Some(app) = apps
        .iter()
        .find(|a| a.id.to_lowercase().trim_end_matches(".desktop") == id)
    {
        return Ok(app);
    }
    if let Some(app) = apps.iter().find(|a| a.name.to_lowercase() == lower) {
        return Ok(app);
    }
    let partial: Vec<&DesktopApp> = apps.iter().filter(|a| a.matches(wanted)).collect();
    match partial.as_slice() {
        [one] => Ok(one),
        [] => Err(ToolError::failed(format!(
            "no installed app matches {wanted:?}"
        ))),
        many => Err(ToolError::failed(format!(
            "{wanted:?} matches several apps: {}. Ask the user which one.",
            many.iter()
                .take(8)
                .map(|a| format!("{} ({})", a.name, a.id))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// All visible applications, deduplicated by id; earlier directories win.
pub fn installed_apps(dirs: &[impl AsRef<Path>]) -> Vec<DesktopApp> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        collect_desktop_files(dir.as_ref(), dir.as_ref(), &mut files);
        files.sort();
        for (id, path) in files {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(app) = fs::read_to_string(&path)
                .ok()
                .and_then(|text| parse_desktop_entry(&id, &text))
            {
                apps.push(app);
            }
        }
    }
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}

/// Desktop file ids use `-` for subdirectories (spec: "Desktop File ID").
fn collect_desktop_files(root: &Path, dir: &Path, out: &mut Vec<(String, std::path::PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_desktop_files(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "desktop") {
            if let Ok(rel) = path.strip_prefix(root) {
                let id = rel.to_string_lossy().replace('/', "-");
                out.push((id, path));
            }
        }
    }
}

/// Parse the `[Desktop Entry]` group. Hidden, NoDisplay and non-Application
/// entries return `None`.
fn parse_desktop_entry(id: &str, text: &str) -> Option<DesktopApp> {
    let mut in_entry = false;
    let mut get = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            // Localized keys such as Name[fr] are skipped.
            let key = key.trim();
            if !key.contains('[') {
                get.insert(key.to_string(), unescape_value(value.trim()));
            }
        }
    }
    let flag = |k: &str| get.get(k).is_some_and(|v| v == "true");
    if get.get("Type").map(String::as_str) != Some("Application")
        || flag("NoDisplay")
        || flag("Hidden")
    {
        return None;
    }
    let list = |k: &str| -> Vec<String> {
        get.get(k)
            .map(|v| {
                v.split(';')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(DesktopApp {
        id: id.to_string(),
        name: get.get("Name")?.clone(),
        generic_name: get.get("GenericName").cloned(),
        comment: get.get("Comment").cloned(),
        categories: list("Categories"),
        keywords: list("Keywords"),
        exec: get.get("Exec")?.clone(),
        terminal: flag("Terminal"),
    })
}

/// Undo the string-level escapes of desktop entry values.
fn unescape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => {
                // Keep other escapes (e.g. \" \$) for the Exec tokenizer.
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Split an `Exec` value into argv, honouring the spec's double-quote rules,
/// and drop field codes (%f, %U, ...) since we launch without files.
pub fn exec_argv(exec: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(e @ ('"' | '`' | '$' | '\\')) => current.push(e),
                            Some(other) => {
                                current.push('\\');
                                current.push(other);
                            }
                            None => return Err("unterminated escape".into()),
                        },
                        Some(other) => current.push(other),
                        None => return Err("unterminated quote".into()),
                    }
                }
            }
            c if c.is_whitespace() => {
                if in_token {
                    args.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            '%' => {
                in_token = true;
                match chars.next() {
                    Some('%') => current.push('%'),
                    Some(_) => {} // field code: dropped
                    None => return Err("dangling %".into()),
                }
            }
            other => {
                in_token = true;
                current.push(other);
            }
        }
    }
    if in_token {
        args.push(current);
    }
    // A token that was only a field code leaves an empty argument behind.
    args.retain(|a| !a.is_empty());
    if args.is_empty() {
        return Err("empty command".into());
    }
    Ok(args)
}

/// Start `argv` without waiting for it, detached from the daemon: a short-lived
/// `sh` backgrounds the program and exits, so it is reparented to init.
async fn spawn_detached(argv: &[String], cwd: &Path) -> Result<(), ToolError> {
    let status = tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("\"$@\" </dev/null >/dev/null 2>&1 &")
        .arg("sh")
        .args(argv)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .status()
        .await
        .map_err(|e| ToolError::failed(format!("could not launch {}: {e}", argv[0])))?;
    if status.success() {
        Ok(())
    } else {
        Err(ToolError::failed(format!("could not launch {}", argv[0])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::context;

    fn write_entry(dir: &Path, file: &str, body: &str) {
        fs::create_dir_all(dir.join(file).parent().unwrap()).unwrap();
        fs::write(dir.join(file), body).unwrap();
    }

    const FIREFOX: &str = "[Desktop Entry]\nType=Application\nName=Firefox\nName[fr]=Navigateur\n\
        GenericName=Web Browser\nExec=firefox %u\nKeywords=internet;www;\nCategories=Network;WebBrowser;\n\
        [Desktop Action new-window]\nName=New Window\nExec=firefox --new-window\n";

    #[test]
    fn tokenizes_exec_lines() {
        assert_eq!(exec_argv("firefox %u").unwrap(), ["firefox"]);
        assert_eq!(
            exec_argv(r#""/opt/My App/run" --flag "a \"q\"" 100%%"#).unwrap(),
            ["/opt/My App/run", "--flag", "a \"q\"", "100%"]
        );
        assert_eq!(exec_argv("app --file=%f").unwrap(), ["app", "--file="]);
        assert!(exec_argv("\"open").is_err());
        assert!(exec_argv("%F").is_err());
    }

    #[test]
    fn parses_entries_and_skips_hidden_ones() {
        let app = parse_desktop_entry("firefox.desktop", FIREFOX).unwrap();
        assert_eq!(app.name, "Firefox");
        assert_eq!(app.exec, "firefox %u");
        assert_eq!(app.keywords, ["internet", "www"]);
        let hidden = "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n";
        assert!(parse_desktop_entry("x.desktop", hidden).is_none());
        let link = "[Desktop Entry]\nType=Link\nName=Docs\nURL=https://example.com\n";
        assert!(parse_desktop_entry("docs.desktop", link).is_none());
    }

    #[test]
    fn user_entries_override_system_ones_and_subdirs_form_ids() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        write_entry(&dir.path().join("apps-system"), "firefox.desktop", FIREFOX);
        write_entry(
            &dir.path().join("apps-user"),
            "firefox.desktop",
            "[Desktop Entry]\nType=Application\nName=Firefox (custom)\nExec=firefox -P work\n",
        );
        write_entry(
            &dir.path().join("apps-system"),
            "kde/konsole.desktop",
            "[Desktop Entry]\nType=Application\nName=Konsole\nExec=konsole\nTerminal=false\n",
        );
        let apps = installed_apps(&ctx.paths.application_dirs);
        let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Firefox (custom)", "Konsole"]);
        assert!(apps.iter().any(|a| a.id == "kde-konsole.desktop"));

        let listed = list_apps(&ctx, Some(json!({ "query": "konsole" }))).unwrap();
        assert_eq!(listed["apps"].as_array().unwrap().len(), 1);
        assert_eq!(listed["total_installed"], 2);
    }

    #[test]
    fn resolves_by_id_name_and_unique_partial_match() {
        let apps = vec![
            parse_desktop_entry("firefox.desktop", FIREFOX).unwrap(),
            parse_desktop_entry(
                "org.gnome.Calculator.desktop",
                "[Desktop Entry]\nType=Application\nName=Calculator\nExec=gnome-calculator\n",
            )
            .unwrap(),
        ];
        assert_eq!(resolve(&apps, "firefox").unwrap().name, "Firefox");
        assert_eq!(resolve(&apps, "FIREFOX.desktop").unwrap().name, "Firefox");
        assert_eq!(resolve(&apps, "calculator").unwrap().id, "org.gnome.Calculator.desktop");
        assert_eq!(resolve(&apps, "web browser").unwrap().name, "Firefox");
        assert!(matches!(resolve(&apps, "photoshop"), Err(ToolError::Failed(_))));
    }

    #[tokio::test]
    async fn launches_detached_without_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        fs::create_dir_all(&ctx.paths.home).unwrap();
        let marker = dir.path().join("launched");
        write_entry(
            &dir.path().join("apps-user"),
            "touch.desktop",
            &format!(
                "[Desktop Entry]\nType=Application\nName=Toucher\nExec=sh -c \"sleep 1; touch '{}'\"\n",
                marker.display()
            ),
        );
        let started = std::time::Instant::now();
        let res = launch_app(&ctx, Some(json!({ "app": "Toucher" }))).await.unwrap();
        assert_eq!(res["launched"], "touch.desktop");
        assert!(started.elapsed() < std::time::Duration::from_millis(800), "must not wait");
        for _ in 0..100 {
            if marker.exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("launched program never ran");
    }
}
