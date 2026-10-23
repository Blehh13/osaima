//! Apps from Flathub through Flatpak, installed for this user only.
//!
//! No root is involved: `flatpak --user` writes to ~/.local/share/flatpak, and
//! every Flatpak app runs in its own sandbox, so the AI never needs admin
//! rights to give the user new software. System packages stay with Gentoo's
//! emerge, run by a person. Reached through system.os_action (action "apps"),
//! so approval and the audit log apply as for everything else.
//!
//!   search {value: "image editor"}   ->  {apps: [{id, name, summary, version}]}
//!   install {value: "gimp"}          ->  starts in the background, notifies when done
//!   remove {value: "gimp"}  list  open {value: "gimp"}

use serde_json::{json, Value};
use std::process::Command;

const REMOTE: &str = "flathub";
const REMOTE_URL: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";

fn flatpak(args: &[&str]) -> Result<String, String> {
    let o = Command::new("flatpak").args(args).output()
        .map_err(|e| format!("Flatpak isn't installed on this system ({})", e))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&o.stderr);
        Err(err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("flatpak failed").trim().to_string())
    }
}

/// Flathub for this user, added once.
fn ensure_remote() -> Result<(), String> {
    flatpak(&["remote-add", "--user", "--if-not-exists", REMOTE, REMOTE_URL]).map(|_| ())
}

/// Tab-separated flatpak output into objects with the given column names.
fn rows(text: &str, cols: &[&str]) -> Vec<Value> {
    text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| {
        let f: Vec<&str> = l.split('\t').collect();
        (f.len() >= cols.len()).then(|| {
            let mut o = json!({});
            for (c, v) in cols.iter().zip(f) { o[*c] = json!(v.trim()); }
            o
        })
    }).collect()
}

fn search(query: &str) -> Result<Vec<Value>, String> {
    ensure_remote()?;
    let cols = ["id", "name", "summary", "version"];
    let run = || flatpak(&["search", "--user", "--columns=application,name,description,version", query]);
    let mut found = rows(&run()?, &cols);
    if found.is_empty() {
        // the app catalogue (appstream) arrives lazily; fetch it once and retry
        let _ = flatpak(&["update", "--user", "--appstream", REMOTE]);
        found = rows(&run()?, &cols);
    }
    found.retain(|a| a["id"].as_str().map_or(false, |id| id.contains('.')));
    found.truncate(8);
    Ok(found)
}

fn installed() -> Result<Vec<Value>, String> {
    Ok(rows(&flatpak(&["list", "--user", "--app", "--columns=application,name,version"])?, &["id", "name", "version"]))
}

/// A name ("gimp", "VLC") or an app id to one app id: an exact id, a name match,
/// then the best search result.
fn resolve(want: &str, among_installed: bool) -> Result<(String, String), String> {
    let w = want.trim();
    if w.is_empty() { return Err("say which app".into()); }
    let pool = if among_installed { installed()? } else { search(w)? };
    best(&pool, w, among_installed).map(|a| (a["id"].as_str().unwrap_or("").to_string(), a["name"].as_str().unwrap_or(w).to_string()))
        .ok_or_else(|| if among_installed { format!("'{}' isn't installed", w) } else { format!("no app called '{}' on Flathub", w) })
}

/// The app itself before its add-ons: an exact id, then an id whose last part is
/// the name ("gimp" -> org.gimp.GIMP, not org.gimp.GIMP.Manual or a plugin),
/// then an exact name, then a name or id containing it; search order otherwise.
fn best<'a>(pool: &'a [Value], w: &str, strict: bool) -> Option<&'a Value> {
    let lw = w.to_lowercase();
    let id = |a: &Value| a["id"].as_str().unwrap_or("").to_string();
    let name = |a: &Value| a["name"].as_str().unwrap_or("").to_lowercase();
    // add-ons and docs have ids under the app's (four or more parts)
    let main = |a: &Value| id(a).split('.').count() <= 3;
    pool.iter().find(|a| id(a).eq_ignore_ascii_case(w))
        .or_else(|| pool.iter().find(|a| main(a) && id(a).rsplit('.').next().map_or(false, |l| l.eq_ignore_ascii_case(w))))
        .or_else(|| pool.iter().find(|a| main(a) && name(a) == lw))
        .or_else(|| pool.iter().find(|a| main(a) && (name(a).contains(&lw) || id(a).to_lowercase().contains(&lw))))
        .or_else(|| if strict { None } else { pool.iter().find(|a| main(a)).or(pool.first()) })
}

pub fn perform(op: &str, params: &Value) -> Result<Value, String> {
    let value = params["value"].as_str().or(params["app"].as_str()).or(params["query"].as_str()).unwrap_or("").to_string();
    match op {
        "search" => Ok(json!({ "apps": search(&value)? })),
        "list" => Ok(json!({ "apps": installed()? })),
        "install" => {
            let (id, name) = resolve(&value, false)?;
            if installed()?.iter().any(|a| a["id"] == json!(id)) {
                return Ok(json!({ "installed": name, "id": id, "already": true }));
            }
            // apps and their runtimes can be hundreds of MB: don't hold the request
            let (id2, name2) = (id.clone(), name.clone());
            std::thread::spawn(move || {
                match flatpak(&["install", "--user", "-y", "--noninteractive", REMOTE, &id2]) {
                    Ok(_) => crate::files::notify_user(&format!("{} is installed", name2), "Open it from the assistant or ask me to open it."),
                    Err(e) => crate::files::notify_user(&format!("Couldn't install {}", name2), &e),
                }
            });
            Ok(json!({ "installing": name, "id": id, "note": "downloading in the background; a notification follows" }))
        }
        "remove" => {
            let (id, name) = resolve(&value, true)?;
            flatpak(&["uninstall", "--user", "-y", "--noninteractive", &id])?;
            Ok(json!({ "removed": name, "id": id }))
        }
        "open" => {
            let (id, name) = resolve(&value, true)?;
            crate::desktop::exec(&format!("flatpak run {}", id))?;
            Ok(json!({ "opened": name, "id": id }))
        }
        other => Err(format!("unknown apps operation '{}'", other)),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn picks_the_app_not_its_manual() {
        use serde_json::json;
        let pool = vec![
            json!({ "id": "org.gimp.GIMP.Manual", "name": "GIMP User Manual" }),
            json!({ "id": "org.gimp.GIMP", "name": "GNU Image Manipulation Program" }),
            json!({ "id": "org.gimp.GIMP.Plugin.Resynthesizer", "name": "Resynthesizer" }),
            json!({ "id": "org.videolan.VLC", "name": "VLC" }),
        ];
        assert_eq!(super::best(&pool, "gimp", false).unwrap()["id"], "org.gimp.GIMP");
        assert_eq!(super::best(&pool, "vlc", false).unwrap()["id"], "org.videolan.VLC");
        assert_eq!(super::best(&pool, "org.gimp.GIMP.Manual", false).unwrap()["id"], "org.gimp.GIMP.Manual");
        assert!(super::best(&pool, "blender", true).is_none());
    }

    #[test]
    fn parses_flatpak_columns() {
        let out = "org.gimp.GIMP\tGNU Image Manipulation Program\tCreate images and edit photographs\t2.10.38\n\nbroken line\n";
        let r = super::rows(out, &["id", "name", "summary", "version"]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["id"], "org.gimp.GIMP");
        assert_eq!(r[0]["version"], "2.10.38");
    }
}
