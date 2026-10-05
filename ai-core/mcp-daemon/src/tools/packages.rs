//! Installed Gentoo packages, read straight from Portage's database
//! (`/var/db/pkg/<category>/<name>-<version>/`). Read-only.

use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};

use super::{read_only, tool, to_value, Args, ToolContext, ToolError, ToolResult};

pub fn definitions() -> Vec<Value> {
    vec![tool(
        "package_info",
        "Installed packages",
        "Search installed Portage packages by name (e.g. \"firefox\" or \"dev-lang/rust\"), with version, slot and description.",
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 20 }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        read_only(),
    )]
}

#[derive(Debug, Serialize, PartialEq)]
struct Package {
    atom: String,
    category: String,
    name: String,
    version: String,
    slot: Option<String>,
    description: Option<String>,
}

pub async fn package_info(ctx: &ToolContext, arguments: Option<Value>) -> ToolResult {
    let args = Args::parse(arguments, &["query", "limit"])?;
    let query = args.required_str("query")?.to_lowercase();
    let limit = args.u64_in("limit", 1, 200, 20)? as usize;
    let db = ctx.paths.package_db.clone();
    tokio::task::spawn_blocking(move || {
        if !db.is_dir() {
            return Err(ToolError::failed(
                "Portage package database not found; package_info only works on Interstellar OS (Gentoo)",
            ));
        }
        let mut found = search(&db, &query);
        found.sort_by(|a, b| a.atom.cmp(&b.atom));
        let total = found.len();
        found.truncate(limit);
        Ok(json!({ "packages": to_value(&found), "total_matches": total }))
    })
    .await
    .map_err(|e| ToolError::failed(format!("package search failed: {e}")))?
}

fn search(db: &Path, query: &str) -> Vec<Package> {
    let mut out = Vec::new();
    let Ok(categories) = fs::read_dir(db) else { return out };
    for category in categories.filter_map(Result::ok) {
        let category_name = category.file_name().to_string_lossy().into_owned();
        let Ok(packages) = fs::read_dir(category.path()) else { continue };
        for pkg in packages.filter_map(Result::ok) {
            let dir_name = pkg.file_name().to_string_lossy().into_owned();
            let Some((name, version)) = split_name_version(&dir_name) else { continue };
            let atom = format!("{category_name}/{name}");
            if !atom.to_lowercase().contains(query) {
                continue;
            }
            let read = |file: &str| {
                fs::read_to_string(pkg.path().join(file))
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            };
            out.push(Package {
                atom,
                category: category_name.clone(),
                name: name.to_string(),
                version: version.to_string(),
                slot: read("SLOT"),
                description: read("DESCRIPTION"),
            });
        }
    }
    out
}

/// Split `gtk+-3.24.41-r1` into (`gtk+`, `3.24.41-r1`). The version is the
/// first `-`-separated suffix that starts with a digit and, apart from an
/// optional `-rN` revision, contains no further `-`.
fn split_name_version(dir: &str) -> Option<(&str, &str)> {
    dir.match_indices('-').find_map(|(i, _)| {
        let (name, rest) = (&dir[..i], &dir[i + 1..]);
        if name.is_empty() || !rest.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        let core = match rest.rsplit_once("-r") {
            Some((core, rev)) if !rev.is_empty() && rev.chars().all(|c| c.is_ascii_digit()) => core,
            _ => rest,
        };
        (!core.contains('-')).then_some((name, rest))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::context;

    #[test]
    fn splits_portage_directory_names() {
        assert_eq!(split_name_version("firefox-128.3.0"), Some(("firefox", "128.3.0")));
        assert_eq!(split_name_version("gtk+-3.24.41-r1"), Some(("gtk+", "3.24.41-r1")));
        assert_eq!(split_name_version("font-misc-misc-1.1.3"), Some(("font-misc-misc", "1.1.3")));
        assert_eq!(split_name_version("python-3.12.4_p1"), Some(("python", "3.12.4_p1")));
        assert_eq!(split_name_version("no-version"), None);
    }

    #[tokio::test]
    async fn searches_the_package_database() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        for (cat, pkg, slot, desc) in [
            ("www-client", "firefox-128.3.0", "rapid", "Firefox Web Browser"),
            ("dev-lang", "rust-bin-1.82.0", "1.82.0", "Systems programming language"),
            ("dev-lang", "python-3.12.4_p1", "3.12", "Python"),
        ] {
            let p = dir.path().join("pkg").join(cat).join(pkg);
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("SLOT"), format!("{slot}\n")).unwrap();
            fs::write(p.join("DESCRIPTION"), format!("{desc}\n")).unwrap();
        }
        let res = package_info(&ctx, Some(json!({ "query": "dev-lang/" }))).await.unwrap();
        assert_eq!(res["total_matches"], 2);
        let res = package_info(&ctx, Some(json!({ "query": "Firefox" }))).await.unwrap();
        let pkg = &res["packages"][0];
        assert_eq!(pkg["atom"], "www-client/firefox");
        assert_eq!(pkg["version"], "128.3.0");
        assert_eq!(pkg["slot"], "rapid");
    }

    #[tokio::test]
    async fn explains_non_gentoo_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let err = package_info(&context(dir.path()), Some(json!({ "query": "x" })))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Failed(m) if m.contains("Gentoo")));
    }
}
