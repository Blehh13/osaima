//! Behaviour store: what the user actually does, kept on-device in SQLite
//! (schema: ai-core/behavior/schema.sql).
//!
//!   * every request is logged (utterance, intent, fast/LLM path, action, outcome)
//!   * suggestions(): the user's own frequent commands, weighted by how often,
//!     how recently and at what hour of the day they are used
//!   * stated preferences ("remember that I prefer short emails") are stored as
//!     style rules and given to the LLM whenever it writes something

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use crate::predictor::{self, Predictor};

const SCHEMA: &str = include_str!("../../behavior/schema.sql");

pub struct Behavior {
    db: Mutex<Option<Connection>>,
    /// The next-action model, trained from the interactions table on first use
    /// and updated with every logged request (see predictor.rs).
    predictor: Mutex<Option<Predictor>>,
}

fn db_path() -> PathBuf {
    if let Ok(p) = std::env::var("OSAIMA_BEHAVIOR_DB") {
        return PathBuf::from(p);
    }
    let data = std::env::var("XDG_DATA_HOME").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".local/share"));
    data.join("osaima/behavior.db")
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn now_iso() -> String {
    // seconds since epoch -> RFC 3339 in UTC, without pulling in a date crate
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// "remember that ..." / "from now on ..." -> the preference text, if this is one.
pub fn stated_preference(query: &str) -> Option<String> {
    let q = query.trim();
    let lower = q.to_lowercase();
    for prefix in ["remember that ", "remember i ", "remember: ", "from now on, ", "from now on ", "always "] {
        if lower.starts_with(prefix) {
            let rest = q[prefix.len()..].trim().trim_end_matches('.');
            let rest = if prefix == "remember i " { format!("I {}", rest) } else { rest.to_string() };
            return (rest.len() > 3).then_some(rest);
        }
    }
    None
}

pub fn is_forget_request(query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.starts_with("forget my preferences") || q.starts_with("forget everything you remember") || q == "forget all preferences"
}

const MAX_ROUTINE_STEPS: usize = 20;

/// The shape of a plan: each step's intent plus its operation, e.g. `["files:move","files:move"]`.
fn plan_signature(steps: &Value) -> String {
    let parts: Vec<String> = steps.as_array().map(|a| a.iter().map(|s| {
        let op = s["params"]["op"].as_str().or_else(|| s["params"]["action"].as_str()).unwrap_or("");
        format!("{}:{}", s["intent"].as_str().unwrap_or(""), op)
    }).collect()).unwrap_or_default();
    json!(parts).to_string()
}

impl Behavior {
    pub fn open() -> Self {
        Self::open_at(db_path())
    }

    pub fn open_at(path: PathBuf) -> Self {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(&path).ok().filter(|c| c.execute_batch(SCHEMA).is_ok());
        if conn.is_none() {
            eprintln!("Behaviour store unavailable at {}", path.display());
        }
        Self { db: Mutex::new(conn), predictor: Mutex::new(None) }
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Option<T> {
        let guard = self.db.lock().unwrap();
        guard.as_ref().and_then(|c| f(c).map_err(|e| eprintln!("behaviour store: {}", e)).ok())
    }

    fn load_events(&self) -> Vec<predictor::Event> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT ts, utterance, intent, action_taken, outcome FROM interactions ORDER BY ts, id")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
                                                  r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?)))?;
            let mut out = Vec::new();
            for row in rows.flatten() {
                let Some(t) = predictor::parse_iso(&row.0) else { continue };
                let a: Value = row.3.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
                out.push(predictor::build_event(t, &row.1, &row.2, a["source"].as_str().unwrap_or(""), &a["params"], row.4.as_deref() == Some("success")));
            }
            Ok(out)
        }).unwrap_or_default()
    }

    /// Run `f` on the predictor, training it from the stored history on first use.
    fn with_predictor<T>(&self, f: impl FnOnce(&mut Predictor) -> T) -> T {
        let mut g = self.predictor.lock().unwrap();
        if g.is_none() {
            *g = Some(Predictor::from_events(self.load_events()));
        }
        f(g.as_mut().unwrap())
    }

    /// One user request and how it was handled.
    pub fn log_request(&self, utterance: &str, result: &Value) {
        let track = result["track"].as_str().unwrap_or(match result["source"].as_str() {
            Some("fast") => "SYSTEM_COMMAND",
            _ => "CONTENT_GENERATION",
        });
        let action = json!({ "params": result["params"], "source": result["source"], "ms": result["ms"] }).to_string();
        self.with_predictor(|_| ()); // load the history before this row is added to it
        self.with(|c| c.execute(
            "INSERT INTO interactions (ts, utterance, track, intent, action_taken, risk_tier, outcome) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![now_iso(), utterance, track, result["intent"].as_str().unwrap_or("unknown"), action,
                    result["params"]["risk"]["tier"].as_str(), "success"],
        ));
        let ev = predictor::build_event(now_secs(), utterance, result["intent"].as_str().unwrap_or("unknown"),
                                        result["source"].as_str().unwrap_or(""), &result["params"], true);
        self.with_predictor(|p| p.observe(ev));
        if result["intent"] == "open_app" {
            if let Some(app) = result["params"]["app"].as_str() {
                let hour = now_iso()[11..13].parse::<i64>().unwrap_or(0);
                self.with(|c| c.execute(
                    "INSERT INTO app_frequency (app, launches, last_used, peak_hour) VALUES (?1, 1, ?2, ?3)
                     ON CONFLICT(app) DO UPDATE SET launches = launches + 1, last_used = ?2, peak_hour = ?3",
                    params![app, now_iso(), hour]));
            }
        }
    }

    /// The user's own frequent commands: count, decayed by age, boosted near the current hour.
    pub fn suggestions(&self, n: usize) -> Vec<String> {
        let hour = now_iso()[11..13].parse::<i64>().unwrap_or(0);
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT utterance,
                        SUM( 1.0 / (1.0 + (julianday('now') - julianday(ts)))
                             * (CASE WHEN abs(CAST(substr(ts, 12, 2) AS INTEGER) - ?1) <= 1 THEN 2.0 ELSE 1.0 END) ) AS score
                 FROM interactions
                 WHERE outcome = 'success' AND intent NOT IN ('unknown', 'greeting') AND length(utterance) <= 40
                   -- only commands the fast path understood confidently: never LLM plans,
                   -- shell commands or anything typed with outside content
                   AND json_extract(action_taken, '$.source') = 'fast'
                   AND intent NOT IN ('close_all_windows', 'clean_storage', 'delete_note', 'os_action', 'shutdown')
                 GROUP BY lower(utterance) ORDER BY score DESC LIMIT ?2")?;
            let rows = stmt.query_map(params![hour, n as i64], |r| r.get::<_, String>(0))?;
            rows.collect()
        }).unwrap_or_default()
    }

    /// "You usually do this around now": a short command the user ran on at least
    /// three different days of the last four weeks within 45 minutes of this time
    /// of day, and not in the last two hours. Only fast-path commands, never
    /// power actions, so acting on a prediction is as safe as typing it.
    pub fn predictions(&self, n: usize) -> Vec<Value> {
        self.predictions_at(n, now_secs())
    }

    /// As `predictions`, for another moment (used to inspect the model).
    pub fn predictions_at(&self, n: usize, t: i64) -> Vec<Value> {
        let learned = self.with_predictor(|p| p.ready().then(|| p.predict(t, n, None)));
        match learned {
            Some(v) => v,
            None => self.predictions_heuristic(n), // too little history to train on yet
        }
    }

    /// What the learned model knows, for system.insights.
    pub fn model_info(&self) -> Value {
        self.with_predictor(|p| p.info())
    }

    /// The original SQL rule: kept as the cold-start fallback and as the evaluation baseline.
    pub fn predictions_heuristic(&self, n: usize) -> Vec<Value> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "WITH recent AS (
                   SELECT utterance, ts, date(ts, 'localtime') AS day,
                          CAST(strftime('%H', ts, 'localtime') AS INTEGER) * 60 + CAST(strftime('%M', ts, 'localtime') AS INTEGER) AS m
                   FROM interactions
                   WHERE outcome = 'success' AND length(utterance) <= 60
                     AND json_extract(action_taken, '$.source') = 'fast'
                     AND intent NOT IN ('unknown', 'greeting', 'chat', 'close_all_windows', 'clean_storage', 'delete_note', 'shutdown')
                     AND coalesce(json_extract(action_taken, '$.params.action'), '') <> 'power'
                     AND julianday('now') - julianday(ts) <= 28),
                 nowm AS (SELECT CAST(strftime('%H', 'now', 'localtime') AS INTEGER) * 60 + CAST(strftime('%M', 'now', 'localtime') AS INTEGER) AS m)
                 SELECT lower(utterance), COUNT(DISTINCT day) AS days, CAST(AVG(recent.m) AS INTEGER)
                 FROM recent, nowm
                 WHERE min(abs(recent.m - nowm.m), 1440 - abs(recent.m - nowm.m)) <= 45
                   AND lower(utterance) NOT IN (SELECT lower(utterance) FROM interactions WHERE (julianday('now') - julianday(ts)) * 24 < 2)
                 GROUP BY lower(utterance)
                 HAVING days >= 3
                 ORDER BY days DESC LIMIT ?1")?;
            let rows = stmt.query_map(params![n as i64], |r| {
                let (text, days, m): (String, i64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?);
                let usual = format!("{:02}:{:02}", m / 60, m % 60);
                Ok(json!({ "text": text, "days": days, "usual_time": usual, "source": "heuristic",
                           "reason": format!("around {} you usually \u{201c}{}\u{201d} (on {} of the last 28 days)", usual, text, days) }))
            })?;
            rows.collect()
        }).unwrap_or_default()
    }

    // ---- Routines: finished plans the user chose to keep (rows of `workflows` with a name).
    // Rows without a name only count how often a plan shape has succeeded.

    /// Save or replace (by name, case-insensitive) a routine. Err is a message for the user.
    pub fn routine_save(&self, name: &str, steps: &Value) -> Result<i64, String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            return Err("routine name must be 1-60 characters".into());
        }
        let list = steps.as_array().filter(|a| !a.is_empty() && a.len() <= MAX_ROUTINE_STEPS)
            .ok_or_else(|| format!("a routine needs 1-{} steps", MAX_ROUTINE_STEPS))?;
        if !list.iter().all(|s| s["intent"].as_str().map_or(false, |i| !i.is_empty()) && s["params"].is_object()) {
            return Err("every step needs an intent and a params object".into());
        }
        let seq = steps.to_string();
        let lower = name.to_lowercase();
        let existing = self.routines().into_iter()
            .find(|r| r["name"].as_str().map(str::to_lowercase).as_deref() == Some(lower.as_str()))
            .and_then(|r| r["id"].as_i64());
        self.with(|c| match existing {
            Some(id) => c.execute("UPDATE workflows SET name = ?1, step_sequence = ?2, last_seen = ?3 WHERE id = ?4",
                                  params![name, seq, now_iso(), id]).map(|_| id),
            None => c.execute("INSERT INTO workflows (name, step_sequence, occurrences, last_seen, automatable) VALUES (?1, ?2, 0, ?3, 1)",
                              params![name, seq, now_iso()]).map(|_| c.last_insert_rowid()),
        }).ok_or_else(|| "behaviour store unavailable".to_string())
    }

    /// Saved routines, most recently used first: id, name, steps (count), occurrences, last_seen.
    pub fn routines(&self) -> Vec<Value> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, name, step_sequence, occurrences, last_seen FROM workflows
                 WHERE name IS NOT NULL ORDER BY last_seen DESC, id DESC")?;
            let rows = stmt.query_map([], |r| {
                let steps: Value = serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or(Value::Null);
                Ok(json!({ "id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?,
                           "steps": steps.as_array().map_or(0, Vec::len),
                           "occurrences": r.get::<_, i64>(3)?, "last_seen": r.get::<_, Option<String>>(4)? }))
            })?;
            rows.collect()
        }).unwrap_or_default()
    }

    pub fn routine_delete(&self, id: i64) -> bool {
        self.with(|c| c.execute("DELETE FROM workflows WHERE id = ?1 AND name IS NOT NULL", params![id])).unwrap_or(0) > 0
    }

    /// "run my X routine" / "run the X routine" / "run X" / "X" -> that routine's (name, steps);
    /// the use is counted.
    pub fn routine_match(&self, query: &str) -> Option<(String, Value)> {
        let q = query.trim().trim_end_matches(['.', '!']).trim().to_lowercase();
        let mut forms = vec![q.clone()];
        for pre in ["run my ", "run the ", "run "] {
            if let Some(rest) = q.strip_prefix(pre) {
                forms.push(rest.trim().to_string());
                if let Some(n) = rest.trim().strip_suffix(" routine") { forms.push(n.trim().to_string()); }
            }
        }
        self.with(|c| {
            let mut stmt = c.prepare("SELECT id, name, step_sequence FROM workflows WHERE name IS NOT NULL")?;
            let rows: Vec<(i64, String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            let hit = rows.into_iter().find(|(_, n, _)| forms.contains(&n.trim().to_lowercase()));
            if let Some((id, name, seq)) = hit {
                c.execute("UPDATE workflows SET occurrences = occurrences + 1, last_seen = ?1 WHERE id = ?2", params![now_iso(), id])?;
                return Ok(serde_json::from_str::<Value>(&seq).ok().map(|s| (name, s)));
            }
            Ok(None)
        }).flatten()
    }

    /// A finished plan: count its shape (intent + op per step). Returns (times seen, already saved).
    pub fn routine_note(&self, steps: &Value) -> (i64, bool) {
        let sig = plan_signature(steps);
        let saved = self.with(|c| {
            let mut stmt = c.prepare("SELECT step_sequence FROM workflows WHERE name IS NOT NULL")?;
            let rows: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            Ok(rows.iter().any(|s| serde_json::from_str::<Value>(s).map_or(false, |v| plan_signature(&v) == sig)))
        }).unwrap_or(false);
        let count = self.with(|c| {
            let n = c.execute("UPDATE workflows SET occurrences = occurrences + 1, last_seen = ?2 WHERE name IS NULL AND step_sequence = ?1",
                              params![sig, now_iso()])?;
            if n == 0 {
                c.execute("INSERT INTO workflows (name, step_sequence, occurrences, last_seen) VALUES (NULL, ?1, 1, ?2)", params![sig, now_iso()])?;
            }
            c.query_row("SELECT occurrences FROM workflows WHERE name IS NULL AND step_sequence = ?1", params![sig], |r| r.get(0))
        }).unwrap_or(1);
        (count, saved)
    }

    pub fn remember(&self, rule: &str) {
        self.with(|c| c.execute("INSERT INTO style_rules (ts, category, rule) VALUES (?1, 'general', ?2)",
                                params![now_iso(), rule]));
    }

    pub fn forget_preferences(&self) -> usize {
        self.with(|c| c.execute("DELETE FROM style_rules", [])).unwrap_or(0)
    }

    /// Most recent stated preferences, newest first.
    pub fn preferences(&self, n: usize) -> Vec<String> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT rule FROM style_rules ORDER BY weight DESC, id DESC LIMIT ?1")?;
            let rows = stmt.query_map(params![n as i64], |r| r.get::<_, String>(0))?;
            rows.collect()
        }).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_parsing() {
        assert_eq!(stated_preference("Remember that I prefer short emails."), Some("I prefer short emails".into()));
        assert_eq!(stated_preference("from now on, sign emails as Sunil"), Some("sign emails as Sunil".into()));
        assert_eq!(stated_preference("open the terminal"), None);
        assert!(is_forget_request("Forget my preferences"));
    }

    #[test]
    fn routines_save_list_match_delete() {
        let path = std::env::temp_dir().join(format!("osaima-routines-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let b = Behavior::open_at(path.clone());
        let steps = json!([{ "intent": "files", "params": { "op": "move" } }, { "intent": "open_app", "params": { "app": "files" } }]);
        // validation
        assert!(b.routine_save("", &steps).is_err());
        assert!(b.routine_save(&"x".repeat(61), &steps).is_err());
        assert!(b.routine_save("ok", &json!([])).is_err());
        assert!(b.routine_save("ok", &json!("nope")).is_err());
        assert!(b.routine_save("ok", &json!([{ "intent": "files" }])).is_err());
        assert!(b.routine_save("ok", &Value::Array(vec![steps[0].clone(); 21])).is_err());
        assert!(b.routines().is_empty());
        // save, then upsert by name regardless of case
        let id = b.routine_save("Tidy Up", &steps).unwrap();
        assert_eq!(b.routine_save("tidy up", &json!([steps[0].clone()])).unwrap(), id);
        let list = b.routines();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["steps"], 1);
        // matching
        for q in ["run my tidy up routine", "Run the Tidy Up routine.", "run tidy up", "  TIDY UP "] {
            assert_eq!(b.routine_match(q).map(|m| m.0), Some("tidy up".to_string()), "{}", q);
        }
        assert!(b.routine_match("open the terminal").is_none());
        assert_eq!(b.routines()[0]["occurrences"], 4);
        // repetition counting
        assert_eq!(b.routine_note(&steps), (1, false));
        assert_eq!(b.routine_note(&steps), (2, false));
        assert!(b.routine_note(&json!([steps[0].clone()])).1, "a saved shape is reported as saved");
        // delete
        assert!(b.routine_delete(id));
        assert!(b.routines().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn predicts_what_is_usual_at_this_time() {
        let path = std::env::temp_dir().join(format!("osaima-predict-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let b = Behavior::open_at(path.clone());
        let add = |utt: &str, ts_sql: &str, action: &str| {
            b.with(|c| c.execute(&format!(
                "INSERT INTO interactions (ts, utterance, track, intent, action_taken, outcome) VALUES ({}, ?1, 'SYSTEM_COMMAND', 'os_action', ?2, 'success')", ts_sql),
                params![utt, json!({ "source": "fast", "params": { "action": action } }).to_string()]));
        };
        for d in 1..=3 {
            add("set brightness to 40", &format!("strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-{} days', '-10 minutes')", d), "brightness");
            add("shut down", &format!("strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-{} days')", d), "power");
            add("open the terminal", &format!("strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-{} days', '-5 hours')", d), "app");
        }
        add("mute", "strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-1 days')", "volume");
        let p = b.predictions(5);
        assert_eq!(p.len(), 1, "{:?}", p);
        assert_eq!(p[0]["text"], "set brightness to 40");
        assert_eq!(p[0]["days"], 3);
        // done an hour ago: not suggested again
        add("set brightness to 40", "strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-1 hours')", "brightness");
        assert!(b.predictions(5).is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn log_suggest_remember() {
        let path = std::env::temp_dir().join(format!("osaima-behavior-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::env::set_var("OSAIMA_BEHAVIOR_DB", &path);
        let b = Behavior::open();
        for _ in 0..3 {
            b.log_request("open the terminal", &json!({ "intent": "open_app", "params": { "app": "terminal" }, "source": "fast" }));
        }
        b.log_request("set a timer for 5 minutes", &json!({ "intent": "timer", "params": { "seconds": 300 }, "source": "fast" }));
        b.log_request("uninstall everything", &json!({ "intent": "orchestrate", "params": {}, "source": "llm" }));
        let s = b.suggestions(5);
        assert_eq!(s.first().map(String::as_str), Some("open the terminal"));
        assert!(s.contains(&"set a timer for 5 minutes".to_string()));
        assert!(!s.contains(&"uninstall everything".to_string()), "LLM plans must never become suggestions");
        b.remember("I prefer short emails");
        assert_eq!(b.preferences(5), vec!["I prefer short emails".to_string()]);
        assert_eq!(b.forget_preferences(), 1);
        assert!(now_iso().len() == 20 && now_iso().ends_with('Z'));
        let _ = std::fs::remove_file(&path);
    }
}
