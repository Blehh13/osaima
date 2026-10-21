use crate::llm::LlmClient;
use crate::context::ContextProvider;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};
use serde_json::json;

pub struct ProactiveAgent {
    ctx: Arc<ContextProvider>,
    latest_suggestion: Arc<RwLock<Option<serde_json::Value>>>,
    memory: Arc<RwLock<Vec<serde_json::Value>>>, // Agentic Memory: store past suggestions
}

impl ProactiveAgent {
    pub fn new(ctx: Arc<ContextProvider>) -> Self {
        Self {
            ctx,
            latest_suggestion: Arc::new(RwLock::new(None)),
            memory: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn get_suggestion_handle(&self) -> Arc<RwLock<Option<serde_json::Value>>> {
        Arc::clone(&self.latest_suggestion)
    }

    pub fn spawn(self) {
        let client_res = LlmClient::new();
        if let Err(e) = client_res {
            eprintln!("Proactive Agent disabled: {}", e);
            return;
        }
        let client = client_res.unwrap();
        
        tokio::spawn(async move {
            println!("Proactive Agent Loop started.");
            // The local model serves one request at a time, so background
            // suggestions must not crowd out the user's own requests.
            let min_gap = Duration::from_secs(300);
            let mut last_llm_call: Option<tokio::time::Instant> = None;
            loop {
                sleep(Duration::from_secs(60)).await;

                let stats = self.ctx.get_system_stats();

                // Only consult the LLM when the stats actually warrant a suggestion
                // (the same thresholds the prompt states), and rarely.
                let cpu = stats["cpu"]["usage_percent"].as_f64().unwrap_or(0.0);
                let used = stats["memory"]["used_bytes"].as_f64().unwrap_or(0.0);
                let total = stats["memory"]["total_bytes"].as_f64().unwrap_or(1.0).max(1.0);
                if cpu <= 70.0 && used / total * 100.0 <= 85.0 {
                    *self.latest_suggestion.write().await = None;
                    continue;
                }
                if last_llm_call.map_or(false, |t| t.elapsed() < min_gap) {
                    continue;
                }
                last_llm_call = Some(tokio::time::Instant::now());
                let recent_memory = {
                    let mem = self.memory.read().await;
                    serde_json::to_string_pretty(&*mem).unwrap_or_default()
                };
                
                let prompt = format!(
                    "You are the AI Core of the OSAIMA desktop environment. 
Your job is to proactively monitor the system and provide helpful, concise, actionable suggestions to the user if needed.
Current System Stats:
{}

Recent suggestions you already made (Agentic Memory):
{}
Do not repeat recent suggestions unless the problem has significantly worsened.

Based on these stats, do you have a UI suggestion? (e.g. 'CPU is very high, want to open Task Manager?', 'Memory is almost full, want to open Monitor?').
Only output a suggestion if it's genuinely useful (e.g. CPU > 70% or Mem > 85%). If everything is fine, output exactly the word 'NONE'.
If you have a suggestion, output it in valid JSON format like:
{{
    \"message\": \"Your message here\",
    \"actionLabel\": \"Button label here\",
    \"actionCommand\": \"monitor\" (or \"terminal\", \"browser\", etc. Must be exact app ID)
}}", 
                    serde_json::to_string_pretty(&stats).unwrap_or_default(),
                    recent_memory
                );

                match client.generate_content(&prompt).await {
                    Ok(response_text) => {
                        let text = response_text.trim();
                        if text != "NONE" {
                            let clean_text = if text.starts_with("```json") {
                                text.trim_start_matches("```json").trim_end_matches("```").trim()
                            } else {
                                text
                            };
                            
                            if let Ok(suggestion) = serde_json::from_str::<serde_json::Value>(clean_text) {
                                println!("AI Core Suggestion generated: {:?}", suggestion);
                                
                                // Update current suggestion
                                let mut cache = self.latest_suggestion.write().await;
                                *cache = Some(suggestion.clone());

                                // Update Agentic Memory
                                let mut mem = self.memory.write().await;
                                mem.push(suggestion);
                                if mem.len() > 5 {
                                    mem.remove(0); // Keep last 5
                                }
                            } else {
                                eprintln!("AI Core failed to parse JSON: {}", clean_text);
                            }
                        } else {
                            // Clear suggestion if everything is fine
                            let mut cache = self.latest_suggestion.write().await;
                            *cache = None;
                        }
                    },
                    Err(e) => {
                        eprintln!("AI Core generation error: {}", e);
                    }
                }
            }
        });
    }
}
