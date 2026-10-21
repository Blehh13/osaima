use regex::Regex;
use crate::dbus_control::DBusController;
use crate::llm::LlmClient;
use crate::executor::SecureExecutor;
use crate::security;
use std::error::Error;

pub enum RoutingDecision {
    FastPathDBus(String),
    SlowPathLLM(String),
}

pub struct IntentRouter {
    play_regex: Regex,
    pause_regex: Regex,
    next_regex: Regex,
    notify_regex: Regex,
}

impl IntentRouter {
    pub fn new() -> Self {
        Self {
            play_regex: Regex::new(r"(?i)^(play|resume)\b").unwrap(),
            pause_regex: Regex::new(r"(?i)^(pause|stop)\b").unwrap(),
            next_regex: Regex::new(r"(?i)^(next|skip)\b").unwrap(),
            notify_regex: Regex::new(r"(?i)^notify\s+(.*)").unwrap(),
        }
    }

    pub fn classify(&self, utterance: &str) -> RoutingDecision {
        let text = utterance.trim();
        if self.play_regex.is_match(text) || self.pause_regex.is_match(text) || self.next_regex.is_match(text) || self.notify_regex.is_match(text) {
            RoutingDecision::FastPathDBus(text.to_string())
        } else {
            RoutingDecision::SlowPathLLM(text.to_string())
        }
    }

    pub async fn dispatch(&self, utterance: &str, dbus: &DBusController, llm: &LlmClient) -> Result<String, Box<dyn Error>> {
        match self.classify(utterance) {
            RoutingDecision::FastPathDBus(cmd) => {
                println!("Router: Fast Path for command '{}'", cmd);
                if self.play_regex.is_match(&cmd) || self.pause_regex.is_match(&cmd) {
                    dbus.media_play_pause(None).await?;
                    Ok("Toggled media playback".to_string())
                } else if self.next_regex.is_match(&cmd) {
                    dbus.media_next(None).await?;
                    Ok("Skipped to next track".to_string())
                } else if let Some(caps) = self.notify_regex.captures(&cmd) {
                    let msg = caps.get(1).map_or("", |m| m.as_str());
                    dbus.send_notification("OSAIMA", msg).await?;
                    Ok("Notification sent".to_string())
                } else {
                    Ok("Fast path matched but no action taken".to_string())
                }
            }
            RoutingDecision::SlowPathLLM(cmd) => {
                println!("Router: Slow Path (LLM) for command '{}'", cmd);
                
                // Fetch RAG context
                let mut rag_context = String::new();
                let osaima_root = std::env::var("OSAIMA_ROOT").unwrap_or_else(|_| ".".to_string());
                let rag_path = std::path::PathBuf::from(&osaima_root).join("ai-core/rag-engine/retriever.py");
                let fallback_path = std::path::PathBuf::from("../../ai-core/rag-engine/retriever.py");
                let target_rag = if rag_path.exists() { rag_path } else { fallback_path };
                if target_rag.exists() {
                    println!("Router: Fetching RAG context...");
                    if let Ok(output) = std::process::Command::new("python3")
                        .arg(&target_rag)
                        .arg(&cmd)
                        .output() {
                            let stdout = String::from_utf8_lossy(&output.stdout);
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&stdout) {
                                if let Some(context_array) = parsed.get("context").and_then(|c| c.as_array()) {
                                    for item in context_array {
                                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                            rag_context.push_str(text);
                                            rag_context.push_str("\n---\n");
                                        }
                                    }
                                }
                            }
                        }
                }

                // Call LLM
                let prompt = format!(
                    "You are OSAIMA, a Linux command generator. The user says: '{}'.
Relevant contextual information (if any):
{}

Output ONLY a valid bash command that accomplishes this. Do not output markdown code blocks. If you cannot do it, output 'echo \"Cannot execute\"'.", 
                    cmd, rag_context
                );
                
                let generated_cmd = llm.generate_content(&prompt).await?;
                println!("LLM Generated Command: {}", generated_cmd);
                
                // Only read-only commands run without the user; anything else
                // goes back for confirmation through system.execute_command.
                let assessment = security::assess(&generated_cmd);
                if assessment.tier != security::Tier::ReadOnly {
                    return Ok(format!("Needs confirmation ({:?}): {}\nReasons: {}",
                        assessment.tier, generated_cmd, assessment.reasons.join("; ")));
                }
                let result = SecureExecutor::run_sandboxed(&generated_cmd, assessment.tier)?;
                Ok(format!("Executed: {}\nOutput:\n{}", generated_cmd, result.output))
            }
        }
    }
}
