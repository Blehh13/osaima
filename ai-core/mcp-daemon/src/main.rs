mod context;
mod protocol;
mod llm;
mod agent;
mod dbus_control;
mod executor;
mod router;
mod nlu;
mod os_control;
mod guard;
mod behavior;
mod security;
mod desktop;
mod files;
mod apps;
mod automations;
mod health;
mod forecast;
mod predictor;
#[cfg(test)]
mod predictor_eval;
mod syssettings;
mod task;
mod models;
mod voice;

use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use protocol::{JsonRpcRequest, JsonRpcResponse};
use context::ContextProvider;
use agent::ProactiveAgent;
use dbus_control::DBusController;
use router::IntentRouter;
use llm::LlmClient;
use security::{AuditLog, Tier, CONFIRM_PHRASE};
use executor::SecureExecutor;
use serde_json::json;
use tokio::sync::RwLock;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = mcp_socket_path();
    if let Some(dir) = std::path::Path::new(&socket_path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    
    // Clean up old socket if it exists
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;
    println!("MCP Daemon listening on {}", socket_path);

    let ctx = Arc::new(ContextProvider::new());
    
    // Start proactive agent
    let agent = ProactiveAgent::new(Arc::clone(&ctx));
    let suggestion_cache = agent.get_suggestion_handle();
    agent.spawn();
    health::spawn();

    let dbus_ctrl = Arc::new(DBusController::new().await?);
    let router = Arc::new(IntentRouter::new());
    let llm_client = Arc::new(LlmClient::new().unwrap_or_else(|_| panic!("Failed to init LlmClient")));
    let audit = Arc::new(AuditLog::open_default());
    let os = Arc::new(os_control::OsControl::new().await);
    let taint = Arc::new(guard::Taint::new());
    let behavior = Arc::new(behavior::Behavior::open());
    task::init(Arc::clone(&llm_client), Arc::clone(&audit));
    {
        // automations run OS actions through the same code as everything else
        let os2 = Arc::clone(&os);
        let runner: automations::Runner = Arc::new(move |a: serde_json::Value| {
            let os = Arc::clone(&os2);
            Box::pin(async move {
                let action = a["action"].as_str().unwrap_or("").to_string();
                os.perform(&action, &a).await
            })
        });
        automations::spawn(runner, Arc::clone(&audit));
    }
    // Fill the LLM's prompt cache in the background so the first request is not the slow one.
    {
        let llm = Arc::clone(&llm_client);
        tokio::spawn(async move {
            if llm.mode().await == llm::Mode::Local {
                tokio::join!(llm.warm_up_intent(), llm.warm_up());
            } else {
                llm.warm_up_intent().await
            }
        });
    }

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let ctx_clone = Arc::clone(&ctx);
                let sugg_clone = Arc::clone(&suggestion_cache);
                let dbus_clone = Arc::clone(&dbus_ctrl);
                let router_clone = Arc::clone(&router);
                let llm_clone = Arc::clone(&llm_client);
                let audit_clone = Arc::clone(&audit);
                let os_clone = Arc::clone(&os);
                let taint_clone = Arc::clone(&taint);
                let behavior_clone = Arc::clone(&behavior);

                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, ctx_clone, sugg_clone, dbus_clone, router_clone, llm_clone, audit_clone, os_clone, taint_clone, behavior_clone).await {
                        eprintln!("Error handling client: {}", e);
                    }
                });
            }
            Err(e) => {
                eprintln!("Accept failed: {}", e);
            }
        }
    }
}

async fn handle_client(
    mut stream: UnixStream, 
    ctx: Arc<ContextProvider>, 
    suggestion_cache: Arc<RwLock<Option<serde_json::Value>>>,
    dbus: Arc<DBusController>,
    router: Arc<IntentRouter>,
    llm: Arc<LlmClient>,
    audit: Arc<AuditLog>,
    os: Arc<os_control::OsControl>,
    taint: Arc<guard::Taint>,
    behavior: Arc<behavior::Behavior>
) -> Result<(), Box<dyn std::error::Error>> {
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).await?;
        if bytes_read == 0 {
            break; // Connection closed
        }

        let request: Result<JsonRpcRequest, _> = serde_json::from_str(&line);
        match request {
            Ok(req) => {
                let response = match req.method.as_str() {
                    "system.get_stats" => {
                        let stats = ctx.get_system_stats();
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), stats)
                    },
                    "system.get_suggestion" => {
                        let sugg = suggestion_cache.read().await;
                        if let Some(s) = &*sugg {
                            JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), s.clone())
                        } else {
                            JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), json!(null))
                        }
                    },
                    "system.dispatch_intent" => {
                        let utterance = req.params.as_ref()
                            .and_then(|p| p.get("utterance"))
                            .and_then(|u| u.as_str())
                            .unwrap_or("");
                        
                        match router.dispatch(utterance, &dbus, &llm).await {
                            Ok(res) => JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), json!({ "result": res })),
                            Err(e) => JsonRpcResponse::error(req.id.unwrap_or_else(|| "0".to_string()), -32000, e.to_string())
                        }
                    },
                    "system.set_api_key" => {
                        let key = req.params.as_ref()
                            .and_then(|p| p.get("key"))
                            .and_then(|u| u.as_str())
                            .unwrap_or("");
                        llm.set_api_key(key.to_string()).await;
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), json!({ "status": "ok" }))
                    },
                    "system.set_groq_key" => {
                        let key = req.params.as_ref()
                            .and_then(|p| p.get("key"))
                            .and_then(|u| u.as_str())
                            .unwrap_or("");
                        llm.set_groq_key(key.to_string()).await;
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), json!({ "status": "ok" }))
                    },
                    "system.validate_api_key" => {
                        let key = req.params.as_ref()
                            .and_then(|p| p.get("key"))
                            .and_then(|u| u.as_str())
                            .unwrap_or("");
                        match llm.validate_key(key).await {
                            Ok(_) => JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), json!({ "valid": true })),
                            Err(e) => JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), json!({ "valid": false, "error": e }))
                        }
                    },
                    "system.classify_intent" => {
                        let query = req.params.as_ref()
                            .and_then(|p| p.get("query"))
                            .and_then(|u| u.as_str())
                            .unwrap_or("");
                        
                        // Fast path first; the LLM only for what the router can't settle.
                        // Untrusted outside content (clipboard, page, email...): [{source, text}]
                        // The last few turns of this conversation, as the shell showed them:
                        // [{role: "user"|"assistant", text}]. Capped so a long chat stays cheap.
                        let history: Vec<(String, String)> = req.params.as_ref()
                            .and_then(|p| p.get("history")).and_then(|h| h.as_array())
                            .map(|a| a.iter().rev().take(6).rev().filter_map(|t| {
                                let role = match t["role"].as_str()? { "user" => "user", "assistant" => "assistant", _ => return None };
                                let text: String = t["text"].as_str()?.chars().take(1500).collect();
                                Some((role.to_string(), text))
                            }).collect())
                            .unwrap_or_default();
                        let context: Vec<(String, String)> = req.params.as_ref()
                            .and_then(|p| p.get("context")).and_then(|c| c.as_array())
                            .map(|items| items.iter().map(|i| (
                                i["source"].as_str().unwrap_or("untrusted").to_string(),
                                i["text"].as_str().unwrap_or("").to_string())).filter(|(_, t)| !t.is_empty()).collect())
                            .unwrap_or_default();
                        // Files the user selected ("Ask OSAIMA" in the file manager): their
                        // location and readable content, fenced like any outside text.
                        let mut context = context;
                        for p in req.params.as_ref().and_then(|p| p.get("selected")).and_then(|s| s.as_array()).into_iter().flatten().take(5) {
                            if let Some(item) = p.as_str().and_then(files::describe_for_model) {
                                context.push(item);
                            }
                        }
                        let started = std::time::Instant::now();
                        // the Security Center's "try to trick me" runs are tagged so they can be told apart in the log
                        let demo = req.params.as_ref().and_then(|p| p.get("demo")).and_then(|d| d.as_bool()).unwrap_or(false);
                        // Stated preferences are stored, not classified.
                        if let Some(rule) = behavior::stated_preference(query) {
                            behavior.remember(&rule);
                            let r = json!({ "intent": "greeting", "params": {}, "source": "fast", "ms": 0,
                                            "reply": format!("Got it, I'll remember: {}", rule) });
                            return_response(&mut writer, JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), r)).await?;
                            continue;
                        }
                        if behavior::is_forget_request(query) {
                            let n = behavior.forget_preferences();
                            let r = json!({ "intent": "greeting", "params": {}, "source": "fast", "ms": 0,
                                            "reply": format!("Forgotten {} saved preference(s).", n) });
                            return_response(&mut writer, JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), r)).await?;
                            continue;
                        }
                        // A saved routine: its steps go back through the shell's normal plan card
                        // and approvals. Never with outside content in play.
                        if context.is_empty() {
                            if let Some((name, steps)) = behavior.routine_match(query) {
                                let r = json!({ "intent": "orchestrate", "params": { "steps": steps }, "source": "routine",
                                                "ms": started.elapsed().as_millis() as u64,
                                                "reply": format!("Running your {} routine.", name) });
                                return_response(&mut writer, JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), r)).await?;
                                continue;
                            }
                        }
                        // "why did X fail", "what's wrong with my computer": answer from evidence
                        // (health, failed services, recent logs), fenced as outside text.
                        if is_diagnosis(query) {
                            let evidence = health::evidence(query);
                            let ctx = vec![("system:diagnostics".to_string(), evidence)];
                            let ask = format!("{}\n\n(Using the diagnostics provided: say plainly what is wrong or why it failed, the likely cause, and the exact steps to fix it. If nothing is wrong, say so. Safe fixes only: never suggest stopping or killing OSAIMA's own services (osaima-*, mcp-daemon, llama-server), never commands that delete data or need root; prefer closing a program from its window, restarting a service with systemctl --user restart, or freeing space with the Trash.)", query);
                            let id = req.id.clone().unwrap_or_else(|| "0".to_string());
                            let resp = match llm.run_task(llm::Task::Write, &ask, None, &ctx, &[], &history).await {
                                Ok(mut r) => {
                                    r["source"] = json!("llm");
                                    r["diagnosis"] = json!(true);
                                    r["ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
                                    JsonRpcResponse::success(id, r)
                                }
                                Err(e) => JsonRpcResponse::error(id, -32000, e),
                            };
                            return_response(&mut writer, resp).await?;
                            continue;
                        }
                        // Plain arithmetic ("what is 12 plus 30") is computed, not guessed.
                        if context.is_empty() {
                            if let Some((expr, value)) = arithmetic(query) {
                                let shown = if value.fract() == 0.0 && value.abs() < 1e15 { format!("{}", value as i64) } else { format!("{:.6}", value).trim_end_matches('0').trim_end_matches('.').to_string() };
                                let r = json!({ "intent": "calculator", "params": { "expression": expr, "result": value },
                                                "reply": format!("{} = {}", expr, shown), "source": "fast", "ms": started.elapsed().as_secs_f64() * 1000.0 });
                                return_response(&mut writer, JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), r)).await?;
                                continue;
                            }
                        }
                        // With outside content the request needs the LLM (and the guard).
                        let verdict = if context.is_empty() { nlu::route(query).await } else { None };
                        // "how do I ..." wants instructions, not the action itself.
                        if let Some(fast) = verdict.as_ref().filter(|_| !how_to(query) && !is_scheduled(query)).and_then(|v| nlu::fast_intent(v, started)) {
                            println!("Intent (fast path): {:?}", fast);
                            behavior.log_request(query, &fast);
                            JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), fast)
                        } else {
                            let candidates = verdict.as_ref().and_then(nlu::candidates);
                            // outside content (clipboard, page, selected files) needs the full prompt and its guard
                            let outside = !context.is_empty();
                            // RAG (PDR §3.5): for requests that need knowledge rather than a
                            // device action, add the best-matching passages. They are fenced as
                            // untrusted like any outside text, since documents can be poisoned.
                            let asks_question = how_to(query) || {
                                let q = query.trim().to_lowercase();
                                ["how ", "what ", "why ", "explain", "where ", "which ", "can i ", "tell me"]
                                    .iter().any(|w| q.starts_with(w)) || q.ends_with('?')
                            };
                            let needs_knowledge = !about_files(&query.to_lowercase()) && (asks_question || verdict.as_ref()
                                .map_or(false, |v| v["track"] != "SYSTEM_COMMAND"));
                            let mut context = context;
                            let mut sources = Vec::new();
                            if needs_knowledge {
                                // Two short passages: on a CPU every extra (uncached) prompt token costs time.
                                for (source, text) in nlu::search(query, 2).await {
                                    sources.push(source.clone());
                                    let text: String = text.chars().take(900).collect();
                                    context.push((format!("knowledge:{}", source.rsplit('/').next().unwrap_or(&source)), text));
                                }
                                sources.dedup();
                            }
                            // Ground file requests in the real folder contents. File names are
                            // outside text (a name can carry an injection), so they are fenced
                            // as untrusted like any other context.
                            if about_files(&query.to_lowercase()) {
                                for dir in files::folders_mentioned(query) {
                                    context.push((format!("folder:{}", dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                                                  files::listing(&dir)));
                                }
                            }
                            let prefs = behavior.preferences(5);
                            let task = choose_task(query, verdict.as_ref());
                            // Local-first: the distilled intent model, then the full-prompt LLM
                            // only if it is off, unreachable or gave an unusable answer.
                            let local = local_first(&llm, task, query, outside, &context, &prefs, &history).await;
                            let used_intent_model = local.is_some();
                            let result = match local {
                                Some(r) => r,
                                None => llm.run_task(task, query, candidates.as_deref(), &context, &prefs, &history).await,
                            };
                            match result {
                                Ok(mut intent_json) => {
                                    annotate_risk(&mut intent_json);
                                    if intent_json["tainted"].as_bool().unwrap_or(false) {
                                        mark_taint(&taint, &intent_json);
                                        audit.record("tainted_proposal", json!({ "query": query, "intent": intent_json["intent"],
                                            "flags": intent_json["untrusted_flags"], "demo": demo }));
                                    }
                                    if intent_json["injection_detected"].as_bool().unwrap_or(false) {
                                        audit.record("injection_blocked", json!({ "query": query, "why": "canary leaked", "demo": demo }));
                                    }
                                    if let Some(why) = mismatched(query, &intent_json) {
                                        // a disruptive action the words never asked for: ask, don't act
                                        audit.record("mismatch_held", json!({ "query": query, "intent": intent_json["intent"] }));
                                        intent_json = json!({ "intent": "unknown", "params": {}, "reply": why });
                                    }
                                    if let Some(app) = app_install_command(intent_json["params"]["command"].as_str().unwrap_or("")) {
                                        // a package-manager command for an app: the typed, root-free Apps action instead
                                        intent_json = json!({ "intent": "os_action", "params": app, "reply": intent_json["reply"] });
                                    }
                                    if intent_json["intent"] == "automation" {
                                        automations::from_words(query, &mut intent_json["params"]["rule"]);
                                    }
                                    intent_json["source"] = json!("llm");
                                    intent_json["llm_task"] = json!(if used_intent_model { "fast_intent".to_string() } else { format!("{:?}", task).to_lowercase() });
                                    // for the pipeline trace: what the router made of it, and how much outside text was fenced
                                    intent_json["router"] = verdict.as_ref().map(nlu::summary).unwrap_or(json!(null));
                                    intent_json["context_items"] = json!(context.len());
                                    if !sources.is_empty() {
                                        intent_json["sources"] = json!(sources);
                                    }
                                    behavior.log_request(query, &intent_json);
                                    intent_json["ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
                                    println!("Intent classified: {:?}", intent_json);
                                    JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), intent_json)
                                },
                                Err(e) => {
                                    eprintln!("Intent classification failed: {}", e);
                                    JsonRpcResponse::error(req.id.unwrap_or_else(|| "0".to_string()), -32000, e)
                                }
                            }
                        }
                    },
                    "system.assess_command" => {
                        let command = param_str(&req.params, "command");
                        let a = security::assess(command);
                        audit.record("assess", json!({ "command": command, "tier": a.tier, "reasons": a.reasons }));
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()),
                            json!({ "tier": a.tier, "reasons": a.reasons, "confirm_phrase": CONFIRM_PHRASE }))
                    },
                    "system.execute_command" => {
                        // Re-assessed here: the tier the UI showed is never trusted.
                        let command = param_str(&req.params, "command").to_string();
                        let approved = req.params.as_ref().and_then(|p| p.get("approved")).and_then(|v| v.as_bool()).unwrap_or(false);
                        let typed = param_str(&req.params, "typed").trim().to_lowercase();
                        let a = security::assess(&command);
                        // Proposed from untrusted content: never auto-run, and anything
                        // beyond read-only needs the typed phrase.
                        let tainted = taint.is_tainted(&command);
                        let typed_ok = typed == CONFIRM_PHRASE;
                        let refusal = match (a.tier, tainted) {
                            (Tier::ReadOnly, false) => None,
                            (Tier::ReadOnly, true) | (Tier::Reversible, false) if approved => None,
                            (_, _) if approved && typed_ok => None,
                            (Tier::ReadOnly, true) => Some("came from untrusted content and needs approval".to_string()),
                            (Tier::Reversible, false) => Some("needs approval".to_string()),
                            (_, true) => Some(format!("came from untrusted content: needs approval and the typed phrase '{}'", CONFIRM_PHRASE)),
                            (Tier::Irreversible, false) => Some(format!("needs approval and the typed phrase '{}'", CONFIRM_PHRASE)),
                        };
                        let id = req.id.unwrap_or_else(|| "0".to_string());
                        let dry_run = req.params.as_ref().and_then(|p| p.get("dry_run")).and_then(|v| v.as_bool()).unwrap_or(false);
                        if dry_run {
                            // Report the gate's decision without running anything (red-team eval, previews).
                            JsonRpcResponse::success(id, json!({ "would_run": refusal.is_none(), "why": refusal,
                                                                 "tier": a.tier, "tainted": tainted }))
                        } else if let Some(why) = refusal {
                            audit.record("refused", json!({ "command": command, "tier": a.tier, "why": why }));
                            JsonRpcResponse::error(id, -32001, format!("Not executed: {:?} command {}", a.tier, why))
                        } else {
                            let tier = a.tier;
                            let cmd = command.clone();
                            match tokio::task::spawn_blocking(move || SecureExecutor::run_sandboxed(&cmd, tier).map_err(|e| e.to_string())).await {
                                Ok(Ok(res)) => {
                                    let (hash, seq) = audit.record_n("execute", json!({ "command": command, "tier": a.tier, "approved": approved,
                                        "exit_code": res.exit_code, "sandbox": res.sandbox,
                                        "output_bytes": res.output.len() }));
                                    JsonRpcResponse::success(id, json!({ "tier": a.tier, "output": res.output,
                                        "exit_code": res.exit_code, "sandbox": res.sandbox,
                                        "audit": { "seq": seq, "hash": hash } }))
                                }
                                Ok(Err(e)) => {
                                    audit.record("execute_failed", json!({ "command": command, "error": e }));
                                    JsonRpcResponse::error(id, -32000, e)
                                }
                                Err(e) => {
                                    audit.record("execute_failed", json!({ "command": command, "error": e.to_string() }));
                                    JsonRpcResponse::error(id, -32000, e.to_string())
                                }
                            }
                        }
                    },
                    "system.os_action" => {
                        // Same gate as shell commands: the daemon decides, the UI only asks.
                        let params = req.params.clone().unwrap_or(json!({}));
                        let action = param_str(&req.params, "action").to_string();
                        let op = param_str(&req.params, "op").to_string();
                        let approved = params["approved"].as_bool().unwrap_or(false);
                        let typed = param_str(&req.params, "typed").trim().to_lowercase();
                        let (tier, needs_approval) = os_control::policy(&action, &op);
                        // an impossible file change fails now, not after the user approved it
                        let precheck = if action == "files" { files::check(&op, &params).err() } else { None };
                        let needs_approval = needs_approval || taint.is_tainted(&guard::os_key(&action, &op));
                        let id = req.id.unwrap_or_else(|| "0".to_string());
                        let refusal = if !needs_approval || (approved && (tier != Tier::Irreversible || typed == CONFIRM_PHRASE)) {
                            None
                        } else if tier == Tier::Irreversible {
                            Some(format!("needs approval and the typed phrase '{}'", CONFIRM_PHRASE))
                        } else {
                            Some("needs approval".to_string())
                        };
                        if let Some(e) = precheck {
                            JsonRpcResponse::error(id, -32000, e)
                        } else if params["dry_run"].as_bool().unwrap_or(false) {
                            JsonRpcResponse::success(id, json!({ "would_run": refusal.is_none(), "why": refusal, "tier": tier }))
                        } else if let Some(why) = refusal {
                            audit.record("os_refused", json!({ "action": action, "op": op, "tier": tier, "why": why }));
                            JsonRpcResponse::error(id, -32001, format!("Not done: {} {} {}", action, op, why))
                        } else {
                            match os.perform(&action, &params).await {
                                Ok(result) => {
                                    let (hash, seq) = audit.record_n("os_action", json!({ "action": action, "op": op, "app": params["app"], "path": params["path"],
                                                                      "paths": params["paths"], "to": params["to"], "name": params["name"],
                                                                      "tier": tier, "result": result }));
                                    JsonRpcResponse::success(id, json!({ "tier": tier, "result": result, "audit": { "seq": seq, "hash": hash } }))
                                }
                                Err(e) => {
                                    audit.record("os_failed", json!({ "action": action, "op": op, "app": params["app"], "error": e }));
                                    JsonRpcResponse::error(id, -32000, e)
                                }
                            }
                        }
                    },
                    // Local voice (see voice.rs): whisper.cpp in, Piper out
                    "system.voice_status" => JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), voice::status()),
                    "system.voice_cancel" => JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), voice::cancel()),
                    "system.speak_stop" => JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), voice::stop()),
                    "system.voice_listen" => {
                        let rid = req.id.unwrap_or_else(|| "0".to_string());
                        match tokio::task::spawn_blocking(voice::listen).await {
                            Ok(Ok(v)) => { audit.record("voice_listen", json!({ "chars": v["text"].as_str().map(|t| t.len()).unwrap_or(0), "ms": v["ms"] })); JsonRpcResponse::success(rid, v) }
                            Ok(Err(e)) => JsonRpcResponse::error(rid, -32000, e),
                            Err(e) => JsonRpcResponse::error(rid, -32000, e.to_string()),
                        }
                    },
                    "system.speak" => {
                        let t = req.params.as_ref().and_then(|p| p.get("text")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let rid = req.id.unwrap_or_else(|| "0".to_string());
                        match tokio::task::spawn_blocking(move || voice::speak(&t)).await {
                            Ok(Ok(v)) => JsonRpcResponse::success(rid, v),
                            Ok(Err(e)) => JsonRpcResponse::error(rid, -32000, e),
                            Err(e) => JsonRpcResponse::error(rid, -32000, e.to_string()),
                        }
                    },
                    // Local model advice + download (see models.rs)
                    "system.gpu_info" => {
                        let v = tokio::task::spawn_blocking(models::gpu_info).await.unwrap_or(json!({}));
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), v)
                    },
                    "system.model_status" => JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), models::status()),
                    "system.model_cancel" => JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), models::cancel()),
                    "system.model_download" => {
                        let p = req.params.clone().unwrap_or(json!({}));
                        let rid = req.id.unwrap_or_else(|| "0".to_string());
                        match models::start(p["id"].as_str().unwrap_or(""), p["approved"].as_bool() == Some(true), Arc::clone(&audit)) {
                            Ok(v) => JsonRpcResponse::success(rid, v),
                            Err(e) => JsonRpcResponse::error(rid, -32001, e),
                        }
                    },
                    // Agent tasks: start, poll, answer, cancel (see task.rs)
                    "system.task_start" | "system.task_status" | "system.task_decide" | "system.task_cancel" | "system.tasks" => {
                        let p = req.params.clone().unwrap_or(json!({}));
                        let id = p["id"].as_str().unwrap_or("");
                        let out = match req.method.as_str() {
                            "system.task_start" => task::start(p["goal"].as_str().unwrap_or("")),
                            "system.task_status" => task::status(id),
                            "system.task_decide" => task::decide(id, p["approve"].as_bool().unwrap_or(false), p["typed"].as_str().unwrap_or("")),
                            "system.task_cancel" => task::cancel(id),
                            _ => Ok(task::list()),
                        };
                        let rid = req.id.unwrap_or_else(|| "0".to_string());
                        match out {
                            Ok(v) => JsonRpcResponse::success(rid, v),
                            Err(e) => JsonRpcResponse::error(rid, -32000, e),
                        }
                    },
                    "system.share" => {
                        let list: Vec<String> = req.params.as_ref().and_then(|p| p.get("paths")).and_then(|v| v.as_array())
                            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                        let id = req.id.unwrap_or_else(|| "0".to_string());
                        match files::share(&list) {
                            Ok(n) => JsonRpcResponse::success(id, json!({ "shared": n })),
                            Err(e) => JsonRpcResponse::error(id, -32000, e),
                        }
                    },
                    "system.ask" => {
                        let q = req.params.as_ref().and_then(|p| p.get("query")).and_then(|v| v.as_str()).unwrap_or("");
                        let id = req.id.unwrap_or_else(|| "0".to_string());
                        match files::ask(q) {
                            Ok(()) => JsonRpcResponse::success(id, json!({ "ok": true })),
                            Err(e) => JsonRpcResponse::error(id, -32000, e),
                        }
                    },
                    "system.shared_take" => {
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()),
                            json!({ "paths": files::take_shared(), "query": files::take_asked() }))
                    },
                    "system.suggestions" => {
                        let n = req.params.as_ref().and_then(|p| p.get("n")).and_then(|v| v.as_u64()).unwrap_or(5) as usize;
                        let (forecasts, anomalies, _) = forecast::insights();
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()),
                            json!({ "suggestions": behavior.suggestions(n), "preferences": behavior.preferences(10),
                                    "predictions": behavior.predictions(2), "alerts": health::status()["alerts"],
                                    "forecasts": forecasts, "anomalies": anomalies }))
                    },
                    "system.insights" => {
                        let n = req.params.as_ref().and_then(|p| p.get("n")).and_then(|v| v.as_u64()).unwrap_or(3) as usize;
                        let (forecasts, anomalies, health_info) = forecast::insights();
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()),
                            json!({ "predictions": match req.params.as_ref().and_then(|p| p.get("at")).and_then(|v| v.as_i64()) { Some(t) => behavior.predictions_at(n, t), None => behavior.predictions(n) }, "forecasts": forecasts, "anomalies": anomalies,
                                    "model_info": { "predictor": behavior.model_info(), "health": health_info } }))
                    },
                    "system.routine_save" => {
                        let id = req.id.unwrap_or_else(|| "0".to_string());
                        let p = req.params.as_ref();
                        let name = p.and_then(|p| p.get("name")).and_then(|v| v.as_str()).unwrap_or("");
                        match behavior.routine_save(name, p.and_then(|p| p.get("steps")).unwrap_or(&serde_json::Value::Null)) {
                            Ok(rid) => JsonRpcResponse::success(id, json!({ "saved": true, "id": rid, "name": name.trim() })),
                            Err(e) => JsonRpcResponse::error(id, -32602, e),
                        }
                    },
                    "system.routines" => {
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), json!({ "routines": behavior.routines() }))
                    },
                    "system.routine_delete" => {
                        let rid = req.params.as_ref().and_then(|p| p.get("id")).and_then(|v| v.as_i64()).unwrap_or(-1);
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), json!({ "deleted": behavior.routine_delete(rid) }))
                    },
                    // A plan finished: count its shape so repeats can be offered as a routine.
                    "system.routine_note" => {
                        let steps = req.params.as_ref().and_then(|p| p.get("steps")).cloned().unwrap_or(serde_json::Value::Null);
                        let (count, saved) = behavior.routine_note(&steps);
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), json!({ "count": count, "saved": saved }))
                    },
                    "system.automations" => {
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), automations::list())
                    },
                    "system.automation_add" => {
                        // created only with the user's approval; typed "confirm" when an action is irreversible
                        let p = req.params.clone().unwrap_or(json!({}));
                        let id = req.id.unwrap_or_else(|| "0".to_string());
                        match automations::validate(&p["rule"]) {
                            Err(e) => JsonRpcResponse::error(id, -32000, e),
                            Ok((rule, typed_needed)) => {
                                let approved = p["approved"].as_bool().unwrap_or(false);
                                let typed_ok = !typed_needed || p["typed"].as_str().map(|t| t.trim().eq_ignore_ascii_case(CONFIRM_PHRASE)).unwrap_or(false);
                                if p["dry_run"].as_bool().unwrap_or(false) {
                                    JsonRpcResponse::success(id, json!({ "summary": rule["summary"], "typed": typed_needed }))
                                } else if !approved || !typed_ok {
                                    JsonRpcResponse::error(id, -32001, format!("Not done: creating this automation needs approval{}",
                                        if typed_needed { format!(" and the typed phrase '{}'", CONFIRM_PHRASE) } else { String::new() }))
                                } else {
                                    audit.record("automation_add", json!({ "summary": rule["summary"], "rule": rule }));
                                    JsonRpcResponse::success(id, automations::add(rule))
                                }
                            }
                        }
                    },
                    "system.automation_remove" => {
                        let rid = req.params.as_ref().and_then(|p| p.get("id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let id = req.id.unwrap_or_else(|| "0".to_string());
                        match automations::remove(&rid) {
                            Ok(v) => { audit.record("automation_remove", json!({ "id": rid })); JsonRpcResponse::success(id, v) }
                            Err(e) => JsonRpcResponse::error(id, -32000, e),
                        }
                    },
                    "system.automation_cancel" => {
                        let v = automations::cancel_pending();
                        audit.record("automation_cancelled", v.clone());
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), v)
                    },
                    "system.health" => {
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), health::status())
                    },
                    "system.audit_tail" => {
                        let n = req.params.as_ref().and_then(|p| p.get("n")).and_then(|v| v.as_u64()).unwrap_or(8).min(50) as usize;
                        let entries = audit.tail(n, &["execute", "refused", "os_action", "os_refused", "os_failed", "tainted_proposal",
                                                     "task_start", "task_done", "task_refused"]);
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()),
                            json!({ "entries": entries, "verified": audit.verify() }))
                    },
                    "system.audit_query" => {
                        let p = req.params.clone().unwrap_or(serde_json::Value::Null);
                        let offset = p.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                        let limit = p.get("limit").and_then(|v| v.as_u64()).unwrap_or(50).clamp(1, 200) as usize;
                        let events: Vec<String> = p.get("events").and_then(|v| v.as_array())
                            .map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect()).unwrap_or_default();
                        let events: Vec<&str> = events.iter().map(String::as_str).collect();
                        let text = p.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        let (entries, total) = audit.query(offset, limit, &events, text);
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()),
                            json!({ "entries": entries, "total": total, "offset": offset }))
                    },
                    "system.audit_verify" => {
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), audit.verify())
                    },
                    // Security Center: counters, the kernel self-check, and the first-run flag
                    "system.security_stats" => {
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), audit.stats())
                    },
                    "system.audit_chain" => {
                        let n = req.params.as_ref().and_then(|p| p.get("n")).and_then(|v| v.as_u64()).unwrap_or(8).clamp(1, 24) as usize;
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), json!({ "blocks": audit.chain_tail(n) }))
                    },
                    "system.sandbox_check" => {
                        // The same probe test_vm.sh uses: a harmless command through the real sandbox,
                        // reporting what the kernel actually enforced. Nothing user-visible runs.
                        let id = req.id.unwrap_or_else(|| "0".to_string());
                        match tokio::task::spawn_blocking(|| SecureExecutor::run_sandboxed("true", Tier::ReadOnly).map_err(|e| e.to_string())).await {
                            Ok(Ok(res)) => {
                                let s = res.sandbox.clone();
                                JsonRpcResponse::success(id, json!({
                                    "sandbox": s, "exit_code": res.exit_code,
                                    "bubblewrap": s.contains("bubblewrap"),
                                    "landlock": if s.contains("+landlock(partial)") { "partial" } else if s.contains("+landlock") { "full" } else { "off" },
                                    "seccomp": s.contains("+seccomp"),
                                    "fallback": s.contains("systemd") }))
                            }
                            Ok(Err(e)) => JsonRpcResponse::error(id, -32000, e),
                            Err(e) => JsonRpcResponse::error(id, -32000, e.to_string()),
                        }
                    },
                    "system.ui_flag" => {
                        // a tiny per-user flag store (~/.config/osaima/ui.json): {name, value?}; no value = read
                        let p = req.params.clone().unwrap_or(json!({}));
                        let id = req.id.unwrap_or_else(|| "0".to_string());
                        match ui_flag(p["name"].as_str().unwrap_or(""), p.get("value").and_then(|v| v.as_bool())) {
                            Ok(v) => JsonRpcResponse::success(id, json!({ "value": v })),
                            Err(e) => JsonRpcResponse::error(id, -32602, e),
                        }
                    },
                    "system.llm_status" => {
                        let mut status = llm.status().await;
                        status["router"] = json!({ "reachable": nlu::route("hello").await.is_some() });
                        status["desktop"] = json!({ "sway": desktop::available() });
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), status)
                    },
                    "system.set_llm_mode" => {
                        let mode = req.params.as_ref().and_then(|p| p.get("mode")).and_then(|v| v.as_str()).unwrap_or("");
                        let mode = match mode { "cloud" => Some(llm::Mode::Cloud), "local" => Some(llm::Mode::Local), _ => None };
                        match mode {
                            Some(m) => {
                                llm.set_mode(m).await;
                                JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), llm.status().await)
                            }
                            None => JsonRpcResponse::error(req.id.unwrap_or_else(|| "0".to_string()), -32602, "mode must be cloud or local".to_string()),
                        }
                    },
                    "ping" => {
                        JsonRpcResponse::success(req.id.unwrap_or_else(|| "0".to_string()), json!("pong"))
                    },
                    _ => {
                        JsonRpcResponse::error(
                            req.id.unwrap_or_else(|| "0".to_string()),
                            -32601,
                            "Method not found".to_string(),
                        )
                    }
                };

                let mut res_str = serde_json::to_string(&response)?;
                res_str.push('\n');
                writer.write_all(res_str.as_bytes()).await?;
            }
            Err(e) => {
                let err_res = JsonRpcResponse::error(
                    "null".to_string(),
                    -32700,
                    format!("Parse error: {}", e),
                );
                let mut res_str = serde_json::to_string(&err_res)?;
                res_str.push('\n');
                writer.write_all(res_str.as_bytes()).await?;
            }
        }
    }

    Ok(())
}

/// First-run style flags the shell keeps (onboarding shown, ...). Names are
/// restricted so this can never be used as a general file write.
fn ui_flag(name: &str, set: Option<bool>) -> Result<bool, String> {
    if name.is_empty() || name.len() > 32 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("bad flag name".to_string());
    }
    let dir = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", std::env::var("HOME").unwrap_or_default()));
    let dir = std::path::PathBuf::from(dir).join("osaima");
    let path = dir.join("ui.json");
    let mut all: serde_json::Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    if !all.is_object() { all = json!({}); }
    if let Some(v) = set {
        all[name] = json!(v);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, all.to_string()).map_err(|e| e.to_string())?;
    }
    Ok(all[name].as_bool().unwrap_or(false))
}

fn param_str<'a>(params: &'a Option<serde_json::Value>, key: &str) -> &'a str {
    params.as_ref().and_then(|p| p.get(key)).and_then(|v| v.as_str()).unwrap_or("")
}

/// Attaches the daemon's own risk assessment to any shell command in an intent
/// (including orchestrate steps) so the UI can show the right confirmation.
fn annotate_risk(intent: &mut serde_json::Value) {
    if intent["intent"] == "execute_command" {
        if let Some(cmd) = intent["params"]["command"].as_str() {
            let a = security::assess(cmd);
            intent["params"]["risk"] = json!({ "tier": a.tier, "reasons": a.reasons });
        }
    }
    if let Some(steps) = intent["params"]["steps"].as_array_mut() {
        for step in steps {
            annotate_risk(step);
        }
    }
}

/// $OSAIMA_MCP_SOCKET, else $XDG_RUNTIME_DIR/osaima/mcp.sock (PDR §3.2's
/// /run/user/$UID/osaima/), else /tmp. Must match the shell's lookup.
fn mcp_socket_path() -> String {
    if let Ok(p) = std::env::var("OSAIMA_MCP_SOCKET") {
        return p;
    }
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) if !dir.is_empty() => format!("{}/osaima/mcp.sock", dir),
        _ => "/tmp/osaima_mcp.sock".to_string(),
    }
}

/// Remembers every action a tainted LLM response proposed, so executing it later
/// always needs the user's approval.
fn mark_taint(taint: &guard::Taint, intent: &serde_json::Value) {
    if let Some(cmd) = intent["params"]["command"].as_str() {
        taint.mark(cmd);
    }
    if intent["intent"] == "os_action" {
        taint.mark(&guard::os_key(intent["params"]["action"].as_str().unwrap_or(""), intent["params"]["op"].as_str().unwrap_or("")));
    }
    if let Some(steps) = intent["params"]["steps"].as_array() {
        for step in steps {
            mark_taint(taint, step);
        }
    }
}

async fn return_response(writer: &mut tokio::net::unix::WriteHalf<'_>, response: JsonRpcResponse) -> std::io::Result<()> {
    let mut res_str = serde_json::to_string(&response)?;
    res_str.push('\n');
    writer.write_all(res_str.as_bytes()).await
}

/// The distilled on-device intent model answers; only long-form text then goes
/// to the writer (Gemini in Fast mode, else the on-device writer model). None
/// when the intent model is off, unreachable or unsure, or the request carries
/// outside content: the caller then uses the full-prompt LLM as before.
async fn local_first(llm: &llm::LlmClient, task: llm::Task, query: &str, outside: bool, context: &[(String, String)],
                     prefs: &[String], history: &[(String, String)]) -> Option<Result<serde_json::Value, String>> {
    if !llm.fast_intent_enabled() || outside || task == llm::Task::Write {
        return None;
    }
    let mut intent = match llm.fast_intent(query).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Intent model: {}; using the full prompt", e);
            return None;
        }
    };
    if llm::LlmClient::needs_writing(&intent) {
        let written = match llm.run_task(llm::Task::Write, query, None, context, prefs, history).await {
            Ok(w) => w,
            Err(e) => return Some(Err(e)),
        };
        let classified_by = intent["backend"].clone();
        if intent["intent"] == "generate_text" {
            intent = written;
        } else {
            intent["params"]["content"] = written["params"]["content"].clone();
            if let Some(p) = intent["params"].as_object_mut() {
                p.remove("write");
            }
            for k in ["backend", "context_from", "tainted", "untrusted_flags"] {
                if !written[k].is_null() {
                    intent[k] = written[k].clone();
                }
            }
        }
        intent["classified_by"] = classified_by;
    }
    Some(Ok(intent))
}

fn how_to(query: &str) -> bool {
    let q = query.trim().to_lowercase();
    ["how do i ", "how can i ", "how to ", "how would i ", "how should i "].iter().any(|p| q.starts_with(p))
}

/// Gives the LLM the job the router says this is: write text, propose one
/// command, or (when unsure) classify among all intents.
/// Questions about failures or the computer's condition, answered from evidence.
fn is_diagnosis(query: &str) -> bool {
    let q = query.to_lowercase();
    let re = regex::Regex::new(r"\b(why (did|does|is|was|won't|can't|isn't)\b.*\b(fail|failed|failing|crash|crashed|slow|hot|stuck|not working|broken|error)|what'?s wrong|what is wrong|diagnose|troubleshoot|is (my|the) (computer|system|laptop) (ok|okay|healthy)|why is (my|the) (computer|system|laptop|pc) (slow|hot|lagging))").unwrap();
    re.is_match(&q)
}

/// "what is 12 plus 30", "calculate (3+4)*2", "7 x 8": the expression and its
/// value, when the whole request is arithmetic. A small recursive-descent
/// parser over + - * / % and parentheses; never `eval`.
fn arithmetic(query: &str) -> Option<(String, f64)> {
    let mut q = query.trim().to_lowercase();
    for p in ["what is ", "what's ", "whats ", "calculate ", "compute ", "how much is ", "solve "] {
        if let Some(rest) = q.strip_prefix(p) { q = rest.to_string(); }
    }
    let q = q.trim_end_matches(|c: char| c == '?' || c == '.' || c == '=' || c.is_whitespace())
        .replace("multiplied by", "*").replace("divided by", "/").replace(" plus ", " + ").replace(" minus ", " - ")
        .replace(" times ", " * ").replace(" over ", " / ").replace(" x ", " * ").replace('×', "*").replace('÷', "/");
    if q.is_empty() || !q.chars().all(|c| c.is_ascii_digit() || " .+-*/()".contains(c)) { return None; }
    let toks: Vec<char> = q.chars().filter(|c| !c.is_whitespace()).collect();
    if !toks.iter().any(|c| "+-*/".contains(*c)) || !toks.iter().any(|c| c.is_ascii_digit()) { return None; }
    struct P<'a> { t: &'a [char], i: usize }
    impl P<'_> {
        fn num(&mut self) -> Option<f64> {
            if self.t.get(self.i) == Some(&'(') { self.i += 1; let v = self.sum()?; if self.t.get(self.i) != Some(&')') { return None; } self.i += 1; return Some(v); }
            if self.t.get(self.i) == Some(&'-') { self.i += 1; return self.num().map(|v| -v); }
            let s = self.i;
            while self.i < self.t.len() && (self.t[self.i].is_ascii_digit() || self.t[self.i] == '.') { self.i += 1; }
            self.t[s..self.i].iter().collect::<String>().parse().ok()
        }
        fn prod(&mut self) -> Option<f64> {
            let mut v = self.num()?;
            while let Some(&op) = self.t.get(self.i).filter(|c| "*/".contains(**c)) {
                self.i += 1; let r = self.num()?;
                v = if op == '*' { v * r } else { if r == 0.0 { return None; } v / r };
            }
            Some(v)
        }
        fn sum(&mut self) -> Option<f64> {
            let mut v = self.prod()?;
            while let Some(&op) = self.t.get(self.i).filter(|c| "+-".contains(**c)) {
                self.i += 1; let r = self.prod()?;
                v = if op == '+' { v + r } else { v - r };
            }
            Some(v)
        }
    }
    let mut p = P { t: &toks, i: 0 };
    let v = p.sum()?;
    (p.i == toks.len() && v.is_finite()).then(|| (toks.iter().collect::<String>().replace('*', " × ").replace('/', " ÷ ").replace('+', " + ").replace('-', " - ").split_whitespace().collect::<Vec<_>>().join(" "), v))
}

/// Requests about the user's files go to the classifier, which has typed file
/// tools, rather than to the command writer, which would write find/mv/rm.
fn about_files(q: &str) -> bool {
    let re = regex::Regex::new(r"\b(files?|folders?|director(y|ies)|pdfs?|photos?|pictures?|images?|videos?|music|songs?|documents?|downloads|desktop|zips?|archives?|trash|rename|move|copy)\b").unwrap();
    re.is_match(q) && !how_to(q)
}

/// Jobs that need several commands with results checked go to the classifier,
/// which can hand them to the task agent, not to the one-command writer.
/// "at 11 pm", "every weekday", "after this finishes", "when I log in": the
/// action is for later, so it must not run now through the fast path.
fn is_scheduled(q: &str) -> bool {
    let re = regex::Regex::new(r"(?i)\b(at \d{1,2}([:.]\d{2})?\s*(am|pm|o'?clock)?\b|tonight|tomorrow|every\b|daily|weekdays?|weekends?|mondays?|tuesdays?|wednesdays?|thursdays?|fridays?|saturdays?|sundays?|from \d{1,2}.* to \d{1,2}|between \d{1,2}|in \d+ ?(min|minutes|hours?|h|m)\b|after (this|that|the|it|reboot|restart|login|log ?in|i log|startup|boot)|once (this|that|it|the|my)|when (this|that|it|the|i log|i start|my|i open|i close|i unplug|i plug|i connect|i disconnect|i'm on|i am on|i get|battery|wifi|the computer|my computer|it's|it gets)|whenever|if (the |my )?(battery|charger|wifi|internet|computer|cpu|laptop)|while (on battery|charging|plugged|unplugged)|on battery|on (login|startup|boot)|at (login|startup|boot)|schedule|automatically)").unwrap();
    re.is_match(q)
}

/// Disruptive intents need the user's own words to point at them. A small
/// model sometimes maps "delete my Downloads folder" to close_all_windows; that
/// would lose unsaved work, so such a mismatch becomes a question instead.
fn mismatched(query: &str, intent: &serde_json::Value) -> Option<String> {
    let q = query.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| q.contains(w));
    let ok = match intent["intent"].as_str().unwrap_or("") {
        "close_all_windows" => has(&["close", "quit", "exit", "kill", "clean the desktop", "clear the desktop", "windows"]),
        "minimize_all" => has(&["minimi", "hide", "show desktop", "show the desktop", "windows"]),
        "shutdown" => has(&["shut", "power off", "turn off", "switch off", "exit", "log out", "logout", "goodbye"]),
        "clean_storage" => has(&["clean", "clear", "free", "empty", "trash", "cache", "space", "storage"]),
        "optimize_cpu" => has(&["cpu", "processor", "optimi", "lag", "slow", "freeze", "sleep", "memory", "speed"]),
        _ => true,
    };
    (!ok).then(|| format!("I want to be sure before doing anything: what should I do with \"{}\"? For files, say for example \"move ~/Downloads to the Trash\".", query.trim()))
}

/// "flatpak uninstall org.videolan.VLC", "sudo emerge gimp", "apt install vlc"
/// -> the apps action (install/remove) for that app. Apps never come from a
/// root package manager; that needs an administrator, not the AI.
fn app_install_command(cmd: &str) -> Option<serde_json::Value> {
    let words: Vec<&str> = cmd.split_whitespace().filter(|w| !w.starts_with('-') && *w != "sudo").collect();
    let (tool, rest) = words.split_first()?;
    if !["flatpak", "emerge", "apt", "apt-get", "dnf", "pacman", "zypper", "snap"].contains(tool) { return None; }
    let (verb, names) = match rest.split_first() {
        Some((v, n)) if ["install", "uninstall", "remove", "erase"].contains(v) => (*v, n.to_vec()),
        _ if *tool == "emerge" && !rest.is_empty() => ("install", rest.to_vec()),
        _ => return None,
    };
    let app = names.iter().find(|w| !["flathub", "--user", "-y"].contains(w))?;
    let op = if verb == "install" { "install" } else { "remove" };
    Some(serde_json::json!({ "action": "apps", "op": op, "value": app.rsplit('/').next().unwrap_or(app) }))
}

fn is_job(q: &str) -> bool {
    let re = regex::Regex::new(r"\b(set ?up|install|configure|scaffold|initiali[sz]e|bootstrap|build|compile|fix|repair|diagnose|troubleshoot|figure out|why is|why does|clean ?up|free up|back ?up|convert|migrate|deploy|create (a|an|my|new) [\w-]+ (project|app|environment|repo))\b").unwrap();
    re.is_match(q) && !how_to(q)
}

fn choose_task(query: &str, verdict: Option<&serde_json::Value>) -> llm::Task {
    let q = query.trim().to_lowercase();
    if about_files(&q) || is_job(&q) || is_scheduled(&q) {
        return llm::Task::Classify;
    }
    let question = ["what is ", "what are ", "why ", "explain", "tell me about ", "who ", "describe "]
        .iter().any(|p| q.starts_with(p));
    let Some(v) = verdict else {
        return if how_to(query) || question { llm::Task::Write } else { llm::Task::Classify };
    };
    let tc = |t: &str| v["track_confidence"][t].as_f64().unwrap_or(0.0);
    let intent = v["intent"].as_str().unwrap_or("");
    if how_to(query) {
        llm::Task::Write
    } else if tc("WORKFLOW_GENERATION") >= 0.6 && intent != "orchestrate" {
        llm::Task::Command
    } else if (tc("CONTENT_GENERATION") >= 0.6 && intent != "edit_note" && intent != "create_note") || question {
        llm::Task::Write
    } else {
        llm::Task::Classify
    }
}

#[cfg(test)]
mod arithmetic_tests {
    use super::arithmetic;
    #[test]
    fn computes_plain_arithmetic_only() {
        assert_eq!(arithmetic("what is 12 plus 30").map(|r| r.1), Some(42.0));
        assert_eq!(arithmetic("calculate (3+4)*2?").map(|r| r.1), Some(14.0));
        assert_eq!(arithmetic("7 x 8").map(|r| r.1), Some(56.0));
        assert_eq!(arithmetic("10 divided by 4").map(|r| r.1), Some(2.5));
        assert_eq!(arithmetic("1/0"), None);
        assert_eq!(arithmetic("what is linux"), None);
        assert_eq!(arithmetic("2024"), None);
        assert_eq!(arithmetic("rm -rf /"), None);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn scheduled_requests_skip_the_fast_path() {
        for q in ["keep my brightness at 40% on weekdays from 9pm to 6am", "at 11:30 tonight lock the screen",
                  "after reboot set my brightness to 70", "shut down after this is completed", "in 10 minutes mute",
                  "when the battery drops below 20% dim the screen", "when I unplug the charger turn on power saving",
                  "whenever I open the video player turn on do not disturb", "once the download finishes shut down",
                  "remind me to drink water every hour", "if the computer gets too hot tell me"] {
            assert!(super::is_scheduled(q), "{}", q);
        }
        for q in ["set brightness to 40", "turn on night light", "mute", "lock the screen", "what is 12 plus 30"] {
            assert!(!super::is_scheduled(q), "{}", q);
        }
    }

    #[test]
    fn package_commands_become_the_apps_action() {
        let a = super::app_install_command("flatpak uninstall org.videolan.VLC").unwrap();
        assert_eq!((a["op"].as_str(), a["value"].as_str()), (Some("remove"), Some("org.videolan.VLC")));
        let b = super::app_install_command("sudo emerge --ask media-gfx/gimp").unwrap();
        assert_eq!((b["op"].as_str(), b["value"].as_str()), (Some("install"), Some("gimp")));
        assert_eq!(super::app_install_command("sudo apt install -y vlc").unwrap()["value"], "vlc");
        assert!(super::app_install_command("uname -r").is_none());
        assert!(super::app_install_command("flatpak list").is_none());
    }

    #[test]
    fn disruptive_intents_need_matching_words() {
        use serde_json::json;
        assert!(super::mismatched("delete my Downloads folder", &json!({ "intent": "close_all_windows" })).is_some());
        assert!(super::mismatched("close everything", &json!({ "intent": "close_all_windows" })).is_none());
        assert!(super::mismatched("open the terminal", &json!({ "intent": "shutdown" })).is_some());
        assert!(super::mismatched("shut down the pc", &json!({ "intent": "shutdown" })).is_none());
        assert!(super::mismatched("delete my Downloads folder", &json!({ "intent": "files" })).is_none());
    }
}
