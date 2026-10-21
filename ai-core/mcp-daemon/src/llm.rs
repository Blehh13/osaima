use reqwest::Client;
use serde_json::{json, Value};
use std::env;
use std::time::Duration;
use tokio::sync::RwLock;

/// Tried after the main Gemini model, in order (same key; each model has its own
/// free-tier rate limit, and a busy model answers 503 or slowly).
const GEMINI_FALLBACKS: [&str; 2] = ["gemini-3.5-flash", "gemini-flash-lite-latest"];
/// Seconds to wait for a cloud model before also asking the next one.
const HEDGE_SECS: u64 = 5;
const INTENT_PROMPT: &str = include_str!("prompts/intent.txt");
const WRITER_PROMPT: &str = include_str!("prompts/writer.txt");
const COMMAND_PROMPT: &str = include_str!("prompts/command.txt");

/// The job the LLM is given. The router decides the kind of request, so the
/// LLM is only asked to do what it is good at: write text, or propose one
/// command. Classify is the fallback when the router is unsure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Task {
    Classify,
    Write,
    Command,
}
/// Tells the model which package manager this system uses, so it does not
/// suggest apt on a Gentoo-based Interstellar install.
fn system_hint() -> String {
    let name = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string())))
        .unwrap_or_else(|| "Linux".to_string());
    let pm = [("emerge", "emerge (Gentoo; use sudo emerge)"), ("apt", "apt"), ("dnf", "dnf"), ("pacman", "pacman"), ("zypper", "zypper")]
        .iter()
        .find(|(bin, _)| std::path::Path::new("/usr/bin").join(bin).exists())
        .map(|(_, desc)| *desc)
        .unwrap_or("unknown");
    format!("\n\nThis system is {}. Its package manager is {}; use it in any install command.", name, pm)
}

/// One OpenAI-compatible chat endpoint. llama-server, Ollama, vLLM, llamafile,
/// Groq and Gemini's `/openai` route all speak this protocol, so the backend is
/// just configuration.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl Endpoint {
    /// The on-device model. Defaults to llama-server on its standard port.
    ///   OSAIMA_LLM_URL   e.g. http://127.0.0.1:11434/v1 for Ollama
    ///   OSAIMA_LLM_MODEL required by Ollama, ignored by llama-server
    fn local_from_env() -> Self {
        Self {
            name: "local".to_string(),
            base_url: env::var("OSAIMA_LLM_URL").unwrap_or_else(|_| "http://127.0.0.1:8080/v1".to_string()),
            model: env::var("OSAIMA_LLM_MODEL").unwrap_or_else(|_| "default".to_string()),
            api_key: env::var("OSAIMA_LLM_API_KEY").ok().filter(|k| !k.is_empty()),
        }
    }

    fn groq(key: String) -> Self {
        Self {
            name: "groq".to_string(),
            base_url: "https://api.groq.com/openai/v1".to_string(),
            model: "openai/gpt-oss-20b".to_string(),
            api_key: Some(key),
        }
    }

    /// Flash-Lite answers in ~1-2 s; the larger Flash models spend the token
    /// budget on hidden reasoning and are often overloaded (HTTP 503).
    fn gemini(key: String) -> Self {
        Self::gemini_model(key, env::var("OSAIMA_GEMINI_MODEL").unwrap_or_else(|_| "gemini-3.5-flash-lite".to_string()))
    }

    fn gemini_model(key: String, model: String) -> Self {
        Self {
            name: format!("gemini:{}", model),
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai".to_string(),
            model,
            api_key: Some(key),
        }
    }

    fn is_local(&self) -> bool {
        self.name == "local"
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), path)
    }
}

fn env_file() -> std::path::PathBuf {
    let dir = env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", env::var("HOME").unwrap_or_default()));
    std::path::Path::new(&dir).join("osaima/llm.env")
}

/// Reads KEY=value lines from ~/.config/osaima/llm.env (mode 600, never in the
/// repo or the image) into the environment, without overriding real env vars.
fn load_env_file() {
    let Ok(text) = std::fs::read_to_string(env_file()) else { return };
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        if let Some((k, v)) = line.split_once('=') {
            if env::var_os(k.trim()).is_none() {
                env::set_var(k.trim(), v.trim().trim_matches('"'));
            }
        }
    }
}

/// Sets (or, with an empty value, removes) one KEY=value line in llm.env,
/// creating the file readable by its owner only.
pub(crate) fn save_env_key(name: &str, value: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = env_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut lines: Vec<String> = std::fs::read_to_string(&path).unwrap_or_default().lines()
        .filter(|l| !l.trim_start().starts_with(&format!("{}=", name)))
        .map(str::to_string).collect();
    if !value.is_empty() {
        lines.push(format!("{}={}", name, value));
    }
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    std::io::Write::write_all(&mut f, (lines.join("\n") + "\n").as_bytes())
}

/// Which backend is asked first. `cloud` (the default once a key is set) puts
/// the user's cloud endpoints first and keeps the local model as the offline
/// fallback; `local` keeps everything on the device unless it fails.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Cloud,
    Local,
}

/// Chat client over the local model and any cloud endpoints the user has
/// explicitly opted into by supplying a key. No keys are built in.
pub struct LlmClient {
    client: Client,
    local: Endpoint,
    cloud: RwLock<Vec<Endpoint>>,
    mode: RwLock<Mode>,
}

impl LlmClient {
    pub fn new() -> Result<Self, String> {
        let mut builder = Client::builder();
        // Many home and campus networks (and QEMU's user network) advertise IPv6
        // but cannot route it; reqwest then waits 3-9 s on the IPv6 address
        // before trying IPv4. Bind to IPv4 unless OSAIMA_IPV6=1.
        if env::var("OSAIMA_IPV6").as_deref() != Ok("1") {
            builder = builder.local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        }
        let client = builder
            // fail fast to the next endpoint when the local server is down,
            // but give CPU inference time to finish once connected
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(240))
            .build()
            .map_err(|e| e.to_string())?;

        load_env_file();
        let mut cloud = Vec::new();
        if let Some(k) = env::var("GEMINI_API_KEY").ok().filter(|k| !k.is_empty()) {
            cloud.push(Endpoint::gemini(k.clone()));
            // same key, more models: the free tier limits requests per minute
            // per model (15), and a busy model returns 503
            for m in GEMINI_FALLBACKS {
                cloud.push(Endpoint::gemini_model(k.clone(), m.to_string()));
            }
        }
        if let Some(k) = env::var("GROQ_API_KEY").ok().filter(|k| !k.is_empty()) {
            cloud.push(Endpoint::groq(k));
        }
        let mode = match env::var("OSAIMA_LLM_MODE").as_deref() {
            Ok("local") => Mode::Local,
            Ok("cloud") => Mode::Cloud,
            _ if cloud.is_empty() => Mode::Local,
            _ => Mode::Cloud,
        };

        Ok(Self { client, local: Endpoint::local_from_env(), cloud: RwLock::new(cloud), mode: RwLock::new(mode) })
    }

    pub async fn mode(&self) -> Mode {
        *self.mode.read().await
    }

    pub async fn set_mode(&self, mode: Mode) {
        *self.mode.write().await = mode;
    }

    /// Endpoints in the order they are tried for the current mode.
    async fn endpoints(&self) -> Vec<Endpoint> {
        let cloud = self.cloud.read().await.clone();
        match self.mode().await {
            Mode::Cloud => cloud.into_iter().chain(std::iter::once(self.local.clone())).collect(),
            Mode::Local => std::iter::once(self.local.clone()).chain(cloud).collect(),
        }
    }

    async fn set_cloud(&self, ep: Endpoint) {
        let mut cloud = self.cloud.write().await;
        let family = ep.name.split(':').next().unwrap_or("").to_string();
        cloud.retain(|e| !e.name.starts_with(&family));
        if ep.api_key.as_deref().map_or(false, |k| !k.is_empty()) {
            cloud.push(ep);
        }
    }

    /// Opt in to (or, with an empty key, out of) Gemini, and remember the
    /// choice in llm.env so it survives a restart.
    pub async fn set_api_key(&self, new_key: String) {
        if let Err(e) = save_env_key("GEMINI_API_KEY", &new_key) {
            eprintln!("LLM: could not save the key: {}", e);
        }
        let mut cloud = self.cloud.write().await;
        cloud.retain(|e| !e.name.starts_with("gemini"));
        if !new_key.is_empty() {
            cloud.insert(0, Endpoint::gemini(new_key.clone()));
            for (i, m) in GEMINI_FALLBACKS.iter().enumerate() {
                cloud.insert(1 + i, Endpoint::gemini_model(new_key.clone(), m.to_string()));
            }
        }
        drop(cloud);
        let has_cloud = !self.cloud.read().await.is_empty();
        self.set_mode(if has_cloud { Mode::Cloud } else { Mode::Local }).await;
    }

    /// Opt in to (or, with an empty key, out of) Groq as a fallback.
    pub async fn set_groq_key(&self, new_key: String) {
        self.set_cloud(Endpoint::groq(new_key)).await;
    }

    pub async fn validate_key(&self, test_key: &str) -> Result<bool, String> {
        let ep = Endpoint::gemini(test_key.to_string());
        let response = self.client
            .get(ep.url("models"))
            .bearer_auth(test_key)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        if response.status().is_success() {
            Ok(true)
        } else {
            let err = response.text().await.unwrap_or_default();
            Err(format!("Validation failed: {}", err))
        }
    }

    /// Reports whether the local model server is up and which models it serves.
    pub async fn status(&self) -> Value {
        let cloud: Vec<String> = self.cloud.read().await.iter().map(|e| e.name.clone()).collect();
        let local = match self.client.get(self.local.url("models")).send().await {
            Ok(r) if r.status().is_success() => {
                let body: Value = r.json().await.unwrap_or(Value::Null);
                let models: Vec<Value> = body["data"].as_array().cloned().unwrap_or_default()
                    .into_iter().map(|m| m["id"].clone()).collect();
                json!({ "reachable": true, "models": models })
            }
            Ok(r) => json!({ "reachable": false, "error": format!("HTTP {}", r.status()) }),
            Err(e) => json!({ "reachable": false, "error": e.to_string() }),
        };
        let mode = match self.mode().await { Mode::Cloud => "cloud", Mode::Local => "local" };
        json!({ "url": self.local.base_url, "model": self.local.model, "local": local, "cloud_fallbacks": cloud, "mode": mode })
    }

    /// One chat completion. Returns the assistant message object, which holds
    /// either `content` or `tool_calls`.
    pub async fn chat(&self, messages: &Value, tools: Option<&Value>, json_mode: bool) -> Result<Value, String> {
        // 600 tokens fits intent JSON plus an email or how-to; at CPU speeds
        // (~5 tok/s) it also keeps the worst case inside the request timeout.
        self.chat_with(messages, tools, json_mode, 600).await
    }

    /// One completion with a chosen output budget (an agent step may write a file).
    pub async fn chat_with(&self, messages: &Value, tools: Option<&Value>, json_mode: bool, max_tokens: u32) -> Result<Value, String> {
        use futures::stream::{FuturesUnordered, StreamExt};
        let mut errors = Vec::new();
        let (cloud, local): (Vec<Endpoint>, Vec<Endpoint>) = self.endpoints().await.into_iter().partition(|e| !e.is_local());
        // Cloud models race: when one hasn't answered after HEDGE_SECS (a busy or
        // degraded model), the next starts too and the first good answer wins, so
        // a slow day at the provider costs seconds, not a failed request.
        let start = |ep: Endpoint| async move {
            let r = self.call(&ep, messages, tools, json_mode, max_tokens).await;
            (ep, r)
        };
        let mut queue = cloud.into_iter();
        let mut running = FuturesUnordered::new();
        loop {
            if running.is_empty() {
                match queue.next() { Some(ep) => running.push(start(ep)), None => break }
            }
            match tokio::time::timeout(Duration::from_secs(HEDGE_SECS), running.next()).await {
                Ok(Some((ep, Ok(mut msg)))) => {
                    msg["_backend"] = json!(ep.name);
                    return Ok(msg);
                }
                Ok(Some((ep, Err(e)))) => {
                    eprintln!("LLM: {} failed: {}", ep.name, e);
                    errors.push(format!("{}: {}", ep.name, e));
                    if let Some(next) = queue.next() { running.push(start(next)); }
                }
                Ok(None) => {}
                Err(_) => if let Some(next) = queue.next() {
                    eprintln!("LLM: no answer after {} s, also asking {}", HEDGE_SECS, next.name);
                    running.push(start(next));
                },
            }
        }
        for ep in local {
            match self.call(&ep, messages, tools, json_mode, max_tokens).await {
                Ok(mut msg) => {
                    msg["_backend"] = json!(ep.name);
                    return Ok(msg);
                }
                Err(e) => {
                    eprintln!("LLM: {} failed: {}", ep.name, e);
                    errors.push(format!("{}: {}", ep.name, e));
                }
            }
        }
        Err(format!("No LLM backend answered ({})", errors.join("; ")))
    }

    async fn call(&self, ep: &Endpoint, messages: &Value, tools: Option<&Value>, json_mode: bool, max_tokens: u32) -> Result<Value, String> {
        let mut body = json!({
            "model": ep.model,
            "messages": messages,
            "temperature": 0.1,
            "max_tokens": max_tokens
        });
        if let Some(t) = tools {
            body["tools"] = t.clone();
        }
        if json_mode {
            body["response_format"] = json!({ "type": "json_object" });
        }

        let mut req = self.client.post(ep.url("chat/completions")).json(&body);
        if !ep.is_local() {
            // a cloud call that stalls should fall through to the next backend quickly
            // generous: a slow model is raced by the next one rather than cut off
            req = req.timeout(Duration::from_secs(30 + max_tokens as u64 / 100));
        }
        if let Some(k) = &ep.api_key {
            req = req.bearer_auth(k);
        }
        let response = req.send().await.map_err(|e| format!("HTTP error: {}", e))?;
        if !response.status().is_success() {
            let err = response.text().await.unwrap_or_default();
            return Err(format!("API error: {}", err));
        }

        let resp_json: Value = response.json().await.map_err(|e| format!("JSON parse error: {}", e))?;
        let msg = &resp_json["choices"][0]["message"];
        if msg.is_object() {
            Ok(msg.clone())
        } else {
            Err(format!("No message in response: {}", resp_json))
        }
    }

    /// Single-prompt text generation.
    pub async fn generate_content(&self, prompt: &str) -> Result<String, String> {
        let msg = self.chat(&json!([{ "role": "user", "content": prompt }]), None, false).await?;
        msg["content"].as_str().map(str::to_string).ok_or_else(|| "Empty response from LLM".to_string())
    }

    /// The system prompt is identical for every request so llama-server can keep
    /// its processed form cached (only the user turn is new); the router's
    /// guesses ride along in the user turn as a hint instead. Measured on Phi-4-mini:
    /// a per-request trimmed prompt was ~4x slower because it defeats that cache.
    fn task_messages(task: Task, query: &str, candidates: Option<&[String]>, fenced_context: &str, prefs: &[String]) -> Value {
        let mut user = query.to_string() + fenced_context;
        if task == Task::Classify {
            // in the user turn so the system prompt cache stays valid
            user.push_str(&format!("\n\n(Current local time: {})", crate::automations::now_text()));
        }
        if !prefs.is_empty() && task != Task::Command {
            // In the user turn, not the system prompt, so the prompt cache stays valid.
            user.push_str(&format!("\n\n(The user's stated preferences, apply them to anything you write: {})", prefs.join("; ")));
        }
        if let Some(c) = candidates.filter(|c| !c.is_empty() && task == Task::Classify) {
            user.push_str(&format!("\n\n(Hint from the fast classifier, most likely first: {})", c.join(", ")));
        }
        json!([
            { "role": "system", "content": Self::system_prompt(task) },
            { "role": "user", "content": user }
        ])
    }

    /// Fixed system prompt for each task (fixed so llama-server can cache it).
    fn system_prompt(task: Task) -> String {
        let base = match task {
            Task::Classify => INTENT_PROMPT,
            Task::Write => WRITER_PROMPT,
            Task::Command => COMMAND_PROMPT,
        };
        base.to_string() + &system_hint() + &crate::guard::system_rules()
    }

    /// Processes each system prompt once so the first real request is fast.
    /// The model can take a minute or more to load at boot (e.g. from a live
    /// ISO), so keep retrying for up to ~15 minutes instead of giving up.
    pub async fn warm_up(&self) {
        for attempt in 0..90 {
            let mut ok = true;
            for task in [Task::Write, Task::Command, Task::Classify] {
                let messages = json!([{ "role": "system", "content": Self::system_prompt(task) },
                                      { "role": "user", "content": "hi" }]);
                if let Err(e) = self.call(&self.local, &messages, None, false, 1).await {
                    if attempt == 0 {
                        eprintln!("LLM: not ready yet, will retry warm-up: {}", e);
                    }
                    ok = false;
                    break;
                }
            }
            if ok {
                println!("LLM: prompt cache warmed");
                return;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        eprintln!("LLM: warm-up skipped: model never became ready");
    }

    /// Runs one LLM task with the injection guard, returning a shell-ready intent.
    /// `context` is outside content (source, text): it is fenced and sanitised,
    /// and the result reports what was stripped and is marked tainted.
    pub async fn run_task(&self, task: Task, query: &str, candidates: Option<&[String]>, context: &[(String, String)],
                          prefs: &[String], history: &[(String, String)]) -> Result<Value, String> {
        let (fenced, flags) = crate::guard::fence(context);
        let mut messages = Self::task_messages(task, query, candidates, &fenced, prefs);
        // Earlier turns go between the system prompt and the new request, so
        // follow-ups ("and the other one?", "tell me more") make sense.
        if let Some(arr) = messages.as_array_mut() {
            let turns = history.iter().map(|(role, text)| json!({ "role": role, "content": text }));
            let user = arr.pop();
            arr.extend(turns);
            arr.extend(user);
        }
        let msg = self.chat(&messages, None, true).await?;
        let text = msg["content"].as_str().unwrap_or("").trim();

        // JSON mode should guarantee bare JSON, but not every backend honours it
        let clean = text
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        if crate::guard::leaks_canary(text) {
            return Ok(json!({ "intent": "unknown", "params": {}, "injection_detected": true,
                              "reply": "Blocked: that request tried to make me reveal protected data." }));
        }
        let raw = serde_json::from_str::<Value>(clean)
            .map_err(|e| format!("Failed to parse LLM {:?} JSON: {} — raw: {}", task, e, clean))?;
        let mut parsed = match task {
            Task::Classify => raw,
            Task::Write => {
                let content = raw["content"].as_str().unwrap_or("").trim().to_string();
                if content.is_empty() {
                    return Err("The model returned no text".to_string());
                }
                json!({ "intent": "generate_text", "reply": "Here you go:",
                        "params": { "title": raw["title"].as_str().unwrap_or("Answer"), "content": content } })
            }
            Task::Command => {
                let command = raw["command"].as_str().unwrap_or("").trim().to_string();
                if command.is_empty() {
                    return Err("The model proposed no command".to_string());
                }
                let plan = raw["plan"].as_str().unwrap_or("Run a command").to_string();
                json!({ "intent": "execute_command", "reply": plan.clone(), "params": { "command": command, "plan": plan } })
            }
        };
        parsed["backend"] = msg["_backend"].clone();
        if !context.is_empty() {
            // where the outside text came from ("folder", "file", "knowledge", ...), so the
            // shell can tell the user's own folder names from a web page or an email
            let mut from: Vec<&str> = context.iter().map(|(s, _)| s.split(':').next().unwrap_or("untrusted")).collect();
            from.sort();
            from.dedup();
            parsed["context_from"] = json!(from);
            parsed["tainted"] = json!(true);
            parsed["untrusted_flags"] = json!(flags);
        }
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The on-device engine runs with an 8192-token context (start-llm.sh); the
    /// fixed prompt must leave room for file listings, knowledge passages and the
    /// answer. About 4 characters per token (measured on Phi-4-mini: a 17.2k-char
    /// prompt was 4268 tokens, which overflowed the old 4096 context).
    #[test]
    fn system_prompts_fit_the_local_context() {
        for task in [Task::Classify, Task::Write, Task::Command] {
            let chars = LlmClient::system_prompt(task).len();
            println!("{:?}: {} chars", task, chars);
            assert!(chars / 4 < 4500, "{:?} prompt is {} chars, about {} tokens", task, chars, chars / 4);
        }
    }
}
