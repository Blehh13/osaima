//! Fast path of the NLU cascade (PDR §3.3): asks the intent router service
//! (ai-core/intent-router/router_service.py) before falling back to the LLM.
//!
//! The router answers in ~15 ms; when it is not confident, or the request needs
//! generation, it returns `fast: null` and the caller uses the LLM as before.

use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;

/// $OSAIMA_ROUTER_SOCKET, else $XDG_RUNTIME_DIR/osaima/router.sock, else /tmp.
fn router_socket() -> String {
    if let Ok(p) = std::env::var("OSAIMA_ROUTER_SOCKET") {
        return p;
    }
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) if !dir.is_empty() => format!("{}/osaima/router.sock", dir),
        _ => "/tmp/osaima_router.sock".to_string(),
    }
}

/// The router's full verdict, or None if the service is not running or slow.
pub async fn route(text: &str) -> Option<Value> {
    ask(json!({ "text": text })).await
}

/// Knowledge-base passages for RAG: (source, text) pairs, best first.
pub async fn search(query: &str, k: usize) -> Vec<(String, String)> {
    ask(json!({ "search": query, "k": k })).await
        .and_then(|v| v["hits"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|h| Some((h["source"].as_str()?.to_string(), h["text"].as_str()?.to_string())))
        .collect()
}

async fn ask(request: Value) -> Option<Value> {
    let ask = async {
        let mut stream = UnixStream::connect(router_socket()).await.ok()?;
        let mut req = request.to_string();
        req.push('\n');
        stream.write_all(req.as_bytes()).await.ok()?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).await.ok()?;
        serde_json::from_str::<Value>(&line).ok()
    };
    // A missing or stuck router must never block the LLM path for long.
    timeout(Duration::from_secs(2), ask).await.ok().flatten()
}

/// Shell-ready intent from the fast path, tagged with how it was decided.
pub fn fast_intent(verdict: &Value, started: Instant) -> Option<Value> {
    let mut fast = verdict.get("fast").filter(|f| f.is_object())?.clone();
    fast["source"] = json!("fast");
    fast["confidence"] = verdict["confidence"].clone();
    fast["ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
    fast["router"] = summary(verdict);
    Some(fast)
}

/// What the router decided, for the shell's pipeline trace (changes no behaviour).
pub fn summary(verdict: &Value) -> Value {
    json!({ "intent": verdict["intent"], "track": verdict["track"], "confidence": verdict["confidence"],
            "scores": verdict["scores"], "ms": verdict["ms"] })
}

/// The router's top guesses, used to shorten the LLM prompt.
pub fn candidates(verdict: &Value) -> Option<Vec<String>> {
    let list: Vec<String> = verdict["candidates"].as_array()?
        .iter().filter_map(|c| c.as_str().map(str::to_string)).collect();
    (!list.is_empty()).then_some(list)
}
