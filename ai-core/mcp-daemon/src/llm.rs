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
/// Short prompt for the distilled intent model (ai-core/intent-model): it was
/// fine-tuned on exactly this text, so it must not drift from fmt.py.
const FAST_INTENT_PROMPT: &str = include_str!("prompts/intent_fast.txt");
/// Every intent the distilled model may answer with; the JSON schema sent to
/// llama-server restricts decoding to these names.
pub const FAST_INTENTS: [&str; 39] = [
    "open_app", "focus_app", "close_window", "close_all_windows", "minimize_all", "layout", "workspace",
    "set_gaps", "os_action", "timer", "stop_timer", "reminder", "calendar", "schedule_event", "calculator",
    "weather", "system_status", "optimize_cpu", "wake_apps", "clean_storage", "fix_network", "scratchpad",
    "search_web", "files", "create_note", "edit_note", "delete_note", "view_note", "list_notes",
    "search_notes", "execute_command", "task", "orchestrate", "automation", "generate_text", "greeting",
    "help", "chat", "unknown",
];
/// Help text for the "help" intent, which the shell shows as a greeting.
const HELP_REPLY: &str = "I can open apps, manage windows and workspaces, set timers and reminders, do quick maths, \
search the web, manage notes and files, change settings, automate things, and run or plan tasks for you.";

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
    format!("\n\nThis system is {}. Its system package manager is {}, which needs an administrator, so never propose it. To install, remove or find APPS use os_action action apps (Flathub, per user, no root).", name, pm)
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

    /// The distilled intent model: a second llama-server (start-intent-llm.sh).
    ///   OSAIMA_INTENT_URL  default http://127.0.0.1:8081/v1
    fn intent_from_env() -> Self {
        Self {
            name: "local-intent".to_string(),
            base_url: env::var("OSAIMA_INTENT_URL").unwrap_or_else(|_| "http://127.0.0.1:8081/v1".to_string()),
            model: "osaima-intent".to_string(),
            api_key: None,
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

/// Which backend is asked first. `local` (the default) keeps everything on the
/// device and free; the cloud endpoints, if the user added a key, are only a
/// fallback. `cloud` puts them first and keeps the local model as the offline
/// fallback. It is chosen explicitly (Settings, or OSAIMA_LLM_MODE=cloud); a key
/// alone never switches to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Cloud,
    Local,
}

/// The mode at start-up: local, unless the cloud was asked for by name.
/// Chat client over the local model and any cloud endpoints the user has
/// explicitly opted into by supplying a key. No keys are built in.
pub struct LlmClient {
    client: Client,
    local: Endpoint,
    cloud: RwLock<Vec<Endpoint>>,
    mode: RwLock<Mode>,
    /// The distilled on-device intent model, asked before any full-prompt LLM.
    intent: Endpoint,
    fast_intent: bool,
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
        let fast_intent = fast_intent_setting(env::var("OSAIMA_FAST_INTENT").ok().as_deref());
        let mode = default_mode(env::var("OSAIMA_LLM_MODE").ok().as_deref(), !cloud.is_empty());

        Ok(Self { client, local: Endpoint::local_from_env(), cloud: RwLock::new(cloud), mode: RwLock::new(mode),
                  intent: Endpoint::intent_from_env(), fast_intent })
    }

    pub async fn mode(&self) -> Mode {
        *self.mode.read().await
    }

    pub async fn set_mode(&self, mode: Mode) {
        *self.mode.write().await = mode;
        // remembered, so Fast mode stays the user's explicit choice across restarts
        let text = match mode { Mode::Cloud => "cloud", Mode::Local => "local" };
        if let Err(e) = save_env_key("OSAIMA_LLM_MODE", text) {
            eprintln!("LLM: could not save the mode: {}", e);
        }
    }

    /// Whether requests the router can't settle go to the distilled model first.
    pub fn fast_intent_enabled(&self) -> bool {
        self.fast_intent
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
        // Adding a key makes Fast mode available; it never makes the cloud the first
        // choice (the user turns Fast mode on in Settings). Removing the last key
        // leaves nothing to prefer.
        if self.cloud.read().await.is_empty() {
            self.set_mode(Mode::Local).await;
        }
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
        let reachable = matches!(self.client.get(self.intent.url("models")).timeout(Duration::from_secs(2)).send().await,
                                 Ok(r) if r.status().is_success());
        let intent = json!({ "enabled": self.fast_intent, "url": self.intent.base_url, "reachable": reachable });
        json!({ "url": self.local.base_url, "model": self.local.model, "local": local, "cloud_fallbacks": cloud, "mode": mode,
                "intent_model": intent })
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
    /// Loads the distilled model and caches its prompt, retrying while it boots.
    pub async fn warm_up_intent(&self) {
        if !self.fast_intent {
            return;
        }
        for _ in 0..90 {
            if self.fast_intent("hello").await.is_ok() {
                println!("LLM: intent model ready");
                return;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        eprintln!("LLM: intent model never became ready; the full-prompt LLM answers instead");
    }

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

    /// The distilled model's messages: the fixed short prompt (cached by
    /// llama-server) and the request with the local time, exactly as in training
    /// (ai-core/intent-model/fmt.py).
    pub fn fast_intent_messages(query: &str, now: &str) -> Value {
        json!([
            { "role": "system", "content": FAST_INTENT_PROMPT.trim() },
            { "role": "user", "content": format!("{}\n(Now: {})", query.trim(), now) }
        ])
    }

    /// Output schema: llama-server turns it into a grammar, so every answer is
    /// valid JSON with a known intent name. Keys are emitted in map order
    /// (serde_json sorts them), which is the order the model was trained on.
    pub fn fast_intent_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": { "type": "string", "enum": FAST_INTENTS.to_vec() },
                "params": { "type": "object" },
                "reply": { "type": "string" }
            },
            "required": ["intent", "params", "reply"],
            "additionalProperties": false
        })
    }

    pub fn fast_intent_body(model: &str, messages: &Value) -> Value {
        json!({
            "model": model,
            "messages": messages,
            "temperature": 0.0,
            "max_tokens": 384,
            "response_format": { "type": "json_schema", "json_schema": { "name": "intent", "schema": Self::fast_intent_schema() } }
        })
    }

    /// Checks and normalises the distilled model's answer into a shell-ready intent.
    pub fn parse_fast_intent(text: &str) -> Result<Value, String> {
        let mut v: Value = serde_json::from_str(text.trim()).map_err(|e| format!("intent model JSON: {}: {}", e, text))?;
        let intent = v["intent"].as_str().unwrap_or("").to_string();
        if !FAST_INTENTS.contains(&intent.as_str()) {
            return Err(format!("intent model gave an unknown intent: {}", intent));
        }
        if !v["params"].is_object() {
            v["params"] = json!({});
        }
        if !v["reply"].is_string() {
            v["reply"] = json!("");
        }
        let params = &v["params"];
        let incomplete = match intent.as_str() {
            "os_action" => params["action"].as_str().is_none(),
            "automation" => !params["rule"].is_object(),
            "orchestrate" => !params["steps"].is_array(),
            "execute_command" => params["command"].as_str().map_or(true, |c| c.trim().is_empty()),
            "open_app" | "focus_app" => params["app"].as_str().is_none(),
            "files" => params["op"].as_str().is_none(),
            _ => false,
        };
        if incomplete {
            return Err(format!("intent model left out a required parameter: {}", v));
        }
        if intent == "help" {
            // the shell has no help screen; the reply carries it (as on the router's path)
            v["intent"] = json!("greeting");
            v["reply"] = json!(HELP_REPLY);
        }
        if intent == "execute_command" && v["params"]["plan"].as_str().is_none() {
            v["params"]["plan"] = v["reply"].clone();
        }
        Ok(v)
    }

    /// True when the answer still needs long-form text from the writer model:
    /// generate_text (the distilled model only names it) or a note to compose.
    pub fn needs_writing(intent: &Value) -> bool {
        match intent["intent"].as_str() {
            Some("generate_text") => intent["params"]["content"].as_str().map_or(true, |c| c.trim().is_empty()),
            Some("create_note") | Some("edit_note") => intent["params"]["write"].as_bool().unwrap_or(false),
            _ => false,
        }
    }

    /// One request to the distilled intent model, with constrained decoding.
    pub async fn fast_intent(&self, query: &str) -> Result<Value, String> {
        let messages = Self::fast_intent_messages(query, &crate::automations::now_text());
        let body = Self::fast_intent_body(&self.intent.model, &messages);
        let response = self.client.post(self.intent.url("chat/completions")).json(&body)
            .timeout(Duration::from_secs(30)).send().await.map_err(|e| format!("intent model: {}", e))?;
        if !response.status().is_success() {
            return Err(format!("intent model: HTTP {}", response.status()));
        }
        let resp: Value = response.json().await.map_err(|e| format!("intent model: {}", e))?;
        let text = resp["choices"][0]["message"]["content"].as_str().unwrap_or("");
        let mut v = Self::parse_fast_intent(text)?;
        v["backend"] = json!(self.intent.name);
        Ok(v)
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

/// Whether the distilled intent model is used when OSAIMA_FAST_INTENT is unset.
/// Off until a trained model has passed the bar in ai-core/intent-model/RESULTS.md
/// (intent accuracy >= 90% on the held-out split, p50 < 2 s on CPU); then flip
/// this to true to make the default routing local-first.
const DEFAULT_FAST_INTENT: bool = false;

/// OSAIMA_FAST_INTENT (env or llm.env): on/1/true/yes or off/0/false/no.
fn fast_intent_setting(v: Option<&str>) -> bool {
    match v.map(|s| s.trim().to_lowercase()).as_deref() {
        Some("on") | Some("1") | Some("true") | Some("yes") => true,
        Some("off") | Some("0") | Some("false") | Some("no") => false,
        _ => DEFAULT_FAST_INTENT,
    }
}

/// The starting mode: on the device (free, private) unless the user chose Fast
/// mode (OSAIMA_LLM_MODE=cloud, written by set_mode) and a key is there for it.
/// A key alone never sends requests to the cloud.
fn default_mode(setting: Option<&str>, has_cloud: bool) -> Mode {
    match setting {
        Some("cloud") if has_cloud => Mode::Cloud,
        _ => Mode::Local,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_intent_request_is_short_and_constrained() {
        let m = LlmClient::fast_intent_messages(" turn on night light ", "Thu 2026-10-22 14:05");
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[1]["content"], "turn on night light\n(Now: Thu 2026-10-22 14:05)");
        // the short prompt is what makes CPU inference fast: keep it a few hundred tokens
        assert!(m[0]["content"].as_str().unwrap().len() / 4 < 450);
        let body = LlmClient::fast_intent_body("osaima-intent", &m);
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["response_format"]["type"], "json_schema");
        let schema = &body["response_format"]["json_schema"]["schema"];
        assert_eq!(schema["properties"]["intent"]["enum"].as_array().unwrap().len(), FAST_INTENTS.len());
        assert_eq!(schema["required"], json!(["intent", "params", "reply"]));
        // the grammar emits keys in map order; training targets use intent, params, reply
        let keys: Vec<&String> = schema["properties"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["intent", "params", "reply"]);
    }

    #[test]
    fn parses_and_normalises_fast_intents() {
        let v = LlmClient::parse_fast_intent(r#"{"intent":"os_action","params":{"action":"volume","op":"set","value":30},"reply":"Volume 30%."}"#).unwrap();
        assert_eq!(v["params"]["value"], 30);
        let v = LlmClient::parse_fast_intent(r#"{"intent":"help","params":{},"reply":"x"}"#).unwrap();
        assert_eq!(v["intent"], "greeting");
        assert!(v["reply"].as_str().unwrap().starts_with("I can"));
        let v = LlmClient::parse_fast_intent(r#"{"intent":"execute_command","params":{"command":"df -h"},"reply":"Showing disk space."}"#).unwrap();
        assert_eq!(v["params"]["plan"], "Showing disk space.");
        let v = LlmClient::parse_fast_intent(r#" {"intent":"list_notes","params":null} "#).unwrap();
        assert_eq!(v["params"], json!({}));
        assert!(LlmClient::parse_fast_intent(r#"{"intent":"rm_rf","params":{},"reply":""}"#).is_err());
        assert!(LlmClient::parse_fast_intent(r#"{"intent":"os_action","params":{},"reply":""}"#).is_err());
        assert!(LlmClient::parse_fast_intent(r#"{"intent":"execute_command","params":{"command":" "},"reply":""}"#).is_err());
        assert!(LlmClient::parse_fast_intent("not json").is_err());
    }

    #[test]
    fn knows_when_the_writer_is_needed() {
        assert!(LlmClient::needs_writing(&json!({"intent":"generate_text","params":{"title":"DNS"}})));
        assert!(!LlmClient::needs_writing(&json!({"intent":"generate_text","params":{"content":"done"}})));
        assert!(LlmClient::needs_writing(&json!({"intent":"create_note","params":{"title":"Pancakes","write":true}})));
        assert!(!LlmClient::needs_writing(&json!({"intent":"create_note","params":{"title":"Milk","content":"buy milk"}})));
        assert!(!LlmClient::needs_writing(&json!({"intent":"timer","params":{"seconds":60}})));
    }

    #[test]
    fn fast_intent_settings_and_mode() {
        assert_eq!(fast_intent_setting(None), DEFAULT_FAST_INTENT);
        assert_eq!(fast_intent_setting(Some("maybe")), DEFAULT_FAST_INTENT);
        assert!(fast_intent_setting(Some("on")));
        assert!(fast_intent_setting(Some("1")));
        assert!(!fast_intent_setting(Some("off")));
        assert!(!fast_intent_setting(Some(" FALSE ")));
        // a key alone never sends requests to the cloud; only choosing Fast mode does
        assert_eq!(default_mode(None, true), Mode::Local);
        assert_eq!(default_mode(None, false), Mode::Local);
        assert_eq!(default_mode(Some("cloud"), true), Mode::Cloud);
        assert_eq!(default_mode(Some("cloud"), false), Mode::Local);
        assert_eq!(default_mode(Some("local"), true), Mode::Local);
        assert_eq!(default_mode(Some("anything else"), true), Mode::Local);
    }

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
