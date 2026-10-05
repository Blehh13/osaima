//! Find files by name inside the user's home directory. The search is
//! bounded in depth, entries visited and time, and never leaves `$HOME`.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};

use super::{read_only, tool, to_value, Args, ToolContext, ToolError, ToolResult};

const MAX_DEPTH: usize = 8;
const MAX_VISITED: usize = 100_000;
const TIME_BUDGET: Duration = Duration::from_secs(3);

pub fn definitions() -> Vec<Value> {
    vec![tool(
        "search_files",
        "Find files",
        "Find files and folders in the user's home whose name contains the query (case-insensitive). Most recently modified first.",
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "folder": { "type": "string", "description": "Folder inside home to search, e.g. \"Documents\"" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 },
                "include_hidden": { "type": "boolean", "default": false }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        read_only(),
    )]
}

#[derive(Debug, Serialize)]
struct Hit {
    path: String,
    is_dir: bool,
    size_bytes: u64,
    modified_unix: Option<u64>,
}

pub async fn search_files(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    let args = Args::parse(arguments, &["query", "folder", "limit", "include_hidden"])?;
    let query = args.required_str("query")?.to_lowercase();
    let limit = args.u64_in("limit", 1, 100, 20)? as usize;
    let include_hidden = args.bool("include_hidden")?.unwrap_or(false);
    let home = ctx.paths.home.clone();
    let start = resolve_folder(&home, args.str("folder")?)?;

    tokio::task::spawn_blocking(move || {
        let (mut hits, complete) = walk(&start, &query, include_hidden);
        hits.sort_by(|a, b| b.modified_unix.cmp(&a.modified_unix));
        let total = hits.len();
        hits.truncate(limit);
        Ok(json!({
            "results": to_value(&hits),
            "total_matches": total,
            "search_complete": complete,
        }))
    })
    .await
    .map_err(|e| ToolError::failed(format!("search failed: {e}")))?
}

/// `folder` relative to home (or absolute but inside it). Anything resolving
/// outside home, including through symlinks or `..`, is refused.
fn resolve_folder(home: &Path, folder: Option<&str>) -> Result<PathBuf, ToolError> {
    let home = fs::canonicalize(home)
        .map_err(|e| ToolError::failed(format!("home directory unavailable: {e}")))?;
    let Some(folder) = folder.map(str::trim).filter(|f| !f.is_empty()) else {
        return Ok(home);
    };
    let folder = folder.strip_prefix("~/").unwrap_or(folder);
    let candidate = fs::canonicalize(home.join(folder))
        .map_err(|_| ToolError::failed(format!("folder {folder:?} does not exist")))?;
    if candidate.starts_with(&home) {
        Ok(candidate)
    } else {
        Err(ToolError::invalid("folder must be inside the home directory"))
    }
}

/// Breadth-first name search. Returns the hits and whether the walk finished
/// within its limits. Symlinked directories are not followed.
fn walk(start: &Path, query: &str, include_hidden: bool) -> (Vec<Hit>, bool) {
    let started = Instant::now();
    let mut queue = VecDeque::from([(start.to_path_buf(), 0usize)]);
    let mut hits = Vec::new();
    let mut visited = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.filter_map(Result::ok) {
            visited += 1;
            if visited > MAX_VISITED || started.elapsed() > TIME_BUDGET {
                return (hits, false);
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !include_hidden && name.starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else { continue };
            let path = entry.path();
            if name.to_lowercase().contains(query) {
                let meta = entry.metadata().ok();
                hits.push(Hit {
                    path: path.display().to_string(),
                    is_dir: file_type.is_dir(),
                    size_bytes: meta.as_ref().map_or(0, |m| m.len()),
                    modified_unix: meta
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs()),
                });
            }
            if file_type.is_dir() && depth < MAX_DEPTH {
                queue.push_back((path, depth + 1));
            }
        }
    }
    (hits, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::context;

    fn setup() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let home = &ctx.paths.home;
        fs::create_dir_all(home.join("Documents/Reports")).unwrap();
        fs::create_dir_all(home.join(".config")).unwrap();
        fs::write(home.join("Documents/Reports/Final-Report.pdf"), b"pdf").unwrap();
        fs::write(home.join("Documents/notes.txt"), b"n").unwrap();
        fs::write(home.join(".config/report.conf"), b"c").unwrap();
        (dir, ctx)
    }

    #[tokio::test]
    async fn finds_case_insensitively_and_skips_hidden() {
        let (_dir, ctx) = setup();
        let res = search_files(&ctx, Some(json!({ "query": "REPORT" }))).await.unwrap();
        let paths: Vec<&str> = res["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths.len(), 2, "{paths:?}");
        assert!(paths.iter().any(|p| p.ends_with("Final-Report.pdf")));
        assert!(paths.iter().all(|p| !p.contains(".config")));
        assert_eq!(res["search_complete"], true);

        let res = search_files(&ctx, Some(json!({ "query": "report", "include_hidden": true })))
            .await
            .unwrap();
        assert_eq!(res["total_matches"], 3);
    }

    #[tokio::test]
    async fn stays_inside_home() {
        let (_dir, ctx) = setup();
        let res = search_files(&ctx, Some(json!({ "query": "notes", "folder": "Documents" })))
            .await
            .unwrap();
        assert_eq!(res["total_matches"], 1);
        for escape in ["..", "../..", "/etc"] {
            let err = search_files(&ctx, Some(json!({ "query": "x", "folder": escape })))
                .await
                .unwrap_err();
            assert!(
                matches!(err, ToolError::InvalidParams(_) | ToolError::Failed(_)),
                "{escape}"
            );
        }
    }

    #[tokio::test]
    async fn requires_a_query() {
        let (_dir, ctx) = setup();
        assert!(matches!(
            search_files(&ctx, Some(json!({}))).await,
            Err(ToolError::InvalidParams(_))
        ));
    }
}
