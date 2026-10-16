//! Client for any OpenAI-compatible server on localhost (Ollama, llama-server,
//! LM Studio, vLLM). With Ollama, chat uses its native `/api/chat` instead, which
//! lets the app set the context window (`num_ctx`); `/api/tags` enriches the
//! model list with sizes.

use std::net::IpAddr;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::Notify;

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("failed to build HTTP client")
}

/// Validates that the URL is plain HTTP on a loopback address and strips any
/// trailing slash. This is what keeps every request on this machine.
pub fn normalize_base_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim().trim_end_matches('/');
    let parsed = reqwest::Url::parse(trimmed).map_err(|e| format!("Invalid server URL: {e}"))?;
    if parsed.scheme() != "http" {
        return Err("The server URL must start with http:// (the app only talks to a server on this computer).".into());
    }
    let host = parsed.host_str().unwrap_or_default();
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let is_local = bare.eq_ignore_ascii_case("localhost") || bare.parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false);
    if !is_local {
        return Err(format!("The server URL must point at this computer (localhost or 127.0.0.1), not \"{host}\"."));
    }
    Ok(trimmed.to_string())
}

/// `http://127.0.0.1:11434/v1` -> `http://127.0.0.1:11434`
fn server_origin(base: &str) -> &str {
    base.strip_suffix("/v1").unwrap_or(base)
}

async fn get_json(http: &reqwest::Client, url: &str, timeout: Duration) -> Result<Value, String> {
    let resp = http.get(url).timeout(timeout).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

pub async fn probe(http: &reqwest::Client, base: &str) -> bool {
    get_json(http, &format!("{base}/models"), Duration::from_secs(2)).await.is_ok()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub size_bytes: Option<u64>,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
}

pub async fn list_models(http: &reqwest::Client, base: &str) -> Result<Vec<ModelInfo>, String> {
    let listing = get_json(http, &format!("{base}/models"), Duration::from_secs(5))
        .await
        .map_err(|e| format!("Couldn't list models from {base}: {e}"))?;
    let mut models: Vec<ModelInfo> = listing["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["id"].as_str())
                .map(|id| ModelInfo { id: id.to_string(), size_bytes: None, parameter_size: None, quantization: None })
                .collect()
        })
        .unwrap_or_default();

    if let Ok(tags) = get_json(http, &format!("{}/api/tags", server_origin(base)), Duration::from_secs(3)).await {
        if let Some(tags) = tags["models"].as_array() {
            for m in &mut models {
                let found = tags.iter().find(|t| t["name"].as_str() == Some(&m.id) || t["model"].as_str() == Some(&m.id));
                if let Some(t) = found {
                    m.size_bytes = t["size"].as_u64();
                    m.parameter_size = t["details"]["parameter_size"].as_str().map(String::from);
                    m.quantization = t["details"]["quantization_level"].as_str().map(String::from);
                }
            }
        }
    }

    models.sort_by(|a, b| match (a.size_bytes, b.size_bytes) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.id.cmp(&b.id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.id.cmp(&b.id),
    });
    Ok(models)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    Reasoning(String),
    Content(String),
    Usage { prompt_tokens: Option<u64>, completion_tokens: Option<u64> },
    Finish(String),
}

#[derive(Debug, PartialEq)]
pub enum SseLine {
    Ignore,
    Done,
    /// Last line of a native Ollama stream, carrying usage and finish reason.
    DoneWith(Vec<Delta>),
    Deltas(Vec<Delta>),
    Error(String),
}

/// Parses one line of an OpenAI-style server-sent-event stream.
pub fn parse_sse_line(line: &str) -> SseLine {
    let Some(data) = line.trim().strip_prefix("data:") else {
        return SseLine::Ignore;
    };
    let data = data.trim();
    if data == "[DONE]" {
        return SseLine::Done;
    }
    let Ok(v) = serde_json::from_str::<Value>(data) else {
        return SseLine::Ignore;
    };
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        return SseLine::Error(error_text(err));
    }
    let mut out = Vec::new();
    if let Some(choice) = v["choices"].get(0) {
        let delta = &choice["delta"];
        // Ollama uses `reasoning`, llama-server/vLLM use `reasoning_content`.
        let reasoning = ["reasoning_content", "reasoning"]
            .iter()
            .filter_map(|k| delta[*k].as_str())
            .find(|s| !s.is_empty());
        if let Some(r) = reasoning {
            out.push(Delta::Reasoning(r.to_string()));
        }
        if let Some(c) = delta["content"].as_str().filter(|s| !s.is_empty()) {
            out.push(Delta::Content(c.to_string()));
        }
        if let Some(f) = choice["finish_reason"].as_str() {
            out.push(Delta::Finish(f.to_string()));
        }
    }
    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
        out.push(Delta::Usage {
            prompt_tokens: u["prompt_tokens"].as_u64(),
            completion_tokens: u["completion_tokens"].as_u64(),
        });
    }
    SseLine::Deltas(out)
}

fn error_text(err: &Value) -> String {
    err.get("message")
        .and_then(Value::as_str)
        .or_else(|| err.as_str())
        .map(String::from)
        .unwrap_or_else(|| err.to_string())
}

fn http_error_message(status: u16, body: &[u8]) -> String {
    let parsed = serde_json::from_slice::<Value>(body).ok().and_then(|v| v.get("error").map(error_text));
    let msg = parsed.unwrap_or_else(|| String::from_utf8_lossy(body).trim().to_string());
    if msg.is_empty() {
        format!("The server returned HTTP {status}.")
    } else {
        format!("{msg} (HTTP {status})")
    }
}

/// Turns raw server errors into something a person can act on, keeping the
/// original text for diagnosis.
pub fn friendly_error(model: &str, raw: &str) -> String {
    let lower = raw.to_lowercase();
    let memory = ["more system memory", "out of memory", "insufficient memory", "not enough memory", "cudamalloc", "failed to allocate"];
    if memory.iter().any(|m| lower.contains(m)) {
        format!(
            "Not enough memory to load \"{model}\". Another model may still be loaded (servers keep models in memory for a few minutes after use): wait a moment and try again, close other GPU-heavy apps, or pick a smaller model.\n\nServer said: {raw}"
        )
    } else if lower.contains("not found") && lower.contains("model") {
        format!("\"{model}\" isn't available on the server. Refresh the model list, or install it with your model server.\n\nServer said: {raw}")
    } else if ["unable to load model", "failed to load", "process has terminated", "error loading model", "cuda error"].iter().any(|m| lower.contains(m)) {
        format!("The server couldn't load \"{model}\". This is sometimes transient right after another large model was unloaded, so try sending again. If it keeps failing, the model may not fit in memory.\n\nServer said: {raw}")
    } else {
        raw.to_string()
    }
}

fn request_error(base: &str, e: &reqwest::Error) -> String {
    if e.is_connect() || e.is_timeout() {
        format!("Can't reach the model server at {base}. Click the status indicator in the top bar to start it.")
    } else {
        format!("The request to the model server failed: {e}")
    }
}

pub struct ChatRequest<'a> {
    pub base: &'a str,
    pub model: &'a str,
    pub messages: Vec<Value>,
    pub temperature: f64,
    pub max_tokens: u32,
    /// Some(n): Ollama's native /api/chat with this context window. None: the
    /// OpenAI-compatible endpoint with the server's own context setting.
    pub num_ctx: Option<u32>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Finished,
    Cancelled,
}

/// Parses one line of Ollama's native NDJSON chat stream.
pub fn parse_ndjson_line(line: &str) -> SseLine {
    let line = line.trim();
    if line.is_empty() {
        return SseLine::Ignore;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return SseLine::Ignore;
    };
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        return SseLine::Error(error_text(err));
    }
    let mut out = Vec::new();
    let msg = &v["message"];
    if let Some(t) = msg["thinking"].as_str().filter(|s| !s.is_empty()) {
        out.push(Delta::Reasoning(t.to_string()));
    }
    if let Some(c) = msg["content"].as_str().filter(|s| !s.is_empty()) {
        out.push(Delta::Content(c.to_string()));
    }
    if v["done"].as_bool() == Some(true) {
        out.push(Delta::Usage { prompt_tokens: v["prompt_eval_count"].as_u64(), completion_tokens: v["eval_count"].as_u64() });
        // Same names as the OpenAI API, so "length" still shows the max-token badge.
        out.push(Delta::Finish(v["done_reason"].as_str().unwrap_or("stop").to_string()));
        return SseLine::DoneWith(out);
    }
    SseLine::Deltas(out)
}

/// Streams a chat completion, calling `on_delta` for every parsed delta.
/// Resolves to `Cancelled` as soon as `cancel` is notified; dropping the
/// response closes the connection, which makes the server stop generating.
pub async fn stream_chat(
    http: &reqwest::Client,
    req: ChatRequest<'_>,
    cancel: &Notify,
    mut on_delta: impl FnMut(Delta),
) -> Result<Outcome, String> {
    let native = req.num_ctx.is_some();
    let (url, body) = match req.num_ctx {
        Some(num_ctx) => (
            format!("{}/api/chat", server_origin(req.base)),
            json!({
                "model": req.model,
                "messages": req.messages,
                "stream": true,
                "options": { "temperature": req.temperature, "num_predict": req.max_tokens, "num_ctx": num_ctx },
            }),
        ),
        None => (
            format!("{}/chat/completions", req.base),
            json!({
                "model": req.model,
                "messages": req.messages,
                "stream": true,
                "temperature": req.temperature,
                "max_tokens": req.max_tokens,
                "stream_options": { "include_usage": true },
            }),
        ),
    };
    let send = http
        .post(url)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send();
    let resp = tokio::select! {
        _ = cancel.notified() => return Ok(Outcome::Cancelled),
        r = send => r.map_err(|e| request_error(req.base, &e))?,
    };
    let status = resp.status();
    if !status.is_success() {
        let bytes = resp.bytes().await.unwrap_or_default();
        return Err(friendly_error(req.model, &http_error_message(status.as_u16(), &bytes)));
    }

    let mut resp = resp;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = cancel.notified() => return Ok(Outcome::Cancelled),
            c = resp.chunk() => c.map_err(|e| format!("The connection to the model server was interrupted: {e}"))?,
        };
        let Some(chunk) = chunk else { break };
        buf.extend_from_slice(&chunk);
        // Lines end in '\n', which never occurs inside a UTF-8 multi-byte sequence.
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&line);
            let parsed = if native { parse_ndjson_line(&text) } else { parse_sse_line(&text) };
            match parsed {
                SseLine::Done => return Ok(Outcome::Finished),
                SseLine::DoneWith(deltas) => {
                    deltas.into_iter().for_each(&mut on_delta);
                    return Ok(Outcome::Finished);
                }
                SseLine::Deltas(deltas) => deltas.into_iter().for_each(&mut on_delta),
                SseLine::Error(msg) => return Err(friendly_error(req.model, &msg)),
                SseLine::Ignore => {}
            }
        }
    }
    Ok(Outcome::Finished)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_loopback_http() {
        assert_eq!(normalize_base_url("http://127.0.0.1:11434/v1/").unwrap(), "http://127.0.0.1:11434/v1");
        assert!(normalize_base_url("http://localhost:8080/v1").is_ok());
        assert!(normalize_base_url("http://[::1]:11434/v1").is_ok());
        assert!(normalize_base_url("https://127.0.0.1:11434/v1").is_err());
        assert!(normalize_base_url("http://192.168.1.5:11434/v1").is_err());
        assert!(normalize_base_url("http://api.example.com/v1").is_err());
    }

    #[test]
    fn parses_ollama_reasoning_and_content() {
        let line = r#"data: {"choices":[{"index":0,"delta":{"content":"","reasoning":"Hmm"},"finish_reason":null}]}"#;
        assert_eq!(parse_sse_line(line), SseLine::Deltas(vec![Delta::Reasoning("Hmm".into())]));
        let line = r#"data: {"choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":"stop"}]}"#;
        assert_eq!(parse_sse_line(line), SseLine::Deltas(vec![Delta::Content("Hi".into()), Delta::Finish("stop".into())]));
    }

    #[test]
    fn parses_reasoning_content_usage_done_and_errors() {
        let line = r#"data: {"choices":[{"delta":{"reasoning_content":"x"}}]}"#;
        assert_eq!(parse_sse_line(line), SseLine::Deltas(vec![Delta::Reasoning("x".into())]));
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":7}}"#;
        assert_eq!(
            parse_sse_line(line),
            SseLine::Deltas(vec![Delta::Usage { prompt_tokens: Some(5), completion_tokens: Some(7) }])
        );
        assert_eq!(parse_sse_line("data: [DONE]"), SseLine::Done);
        assert_eq!(parse_sse_line(": keep-alive"), SseLine::Ignore);
        let line = r#"data: {"error":{"message":"model requires more system memory (40 GiB) than is available (20 GiB)"}}"#;
        assert!(matches!(parse_sse_line(line), SseLine::Error(m) if m.contains("more system memory")));
    }

    #[test]
    fn parses_ollama_native_stream() {
        let line = r#"{"model":"qwen3.5:4b","message":{"role":"assistant","content":"","thinking":"Hmm"},"done":false}"#;
        assert_eq!(parse_ndjson_line(line), SseLine::Deltas(vec![Delta::Reasoning("Hmm".into())]));
        let line = r#"{"model":"qwen3.5:4b","message":{"role":"assistant","content":"Hi"},"done":false}"#;
        assert_eq!(parse_ndjson_line(line), SseLine::Deltas(vec![Delta::Content("Hi".into())]));
        let line = r#"{"model":"m","message":{"role":"assistant","content":""},"done":true,"done_reason":"length","prompt_eval_count":12,"eval_count":34}"#;
        assert_eq!(
            parse_ndjson_line(line),
            SseLine::DoneWith(vec![Delta::Usage { prompt_tokens: Some(12), completion_tokens: Some(34) }, Delta::Finish("length".into())])
        );
        assert!(matches!(parse_ndjson_line(r#"{"error":"model requires more system memory"}"#), SseLine::Error(m) if m.contains("memory")));
        assert_eq!(parse_ndjson_line(""), SseLine::Ignore);
    }

    #[test]
    fn explains_memory_errors() {
        let msg = friendly_error("big", &http_error_message(500, br#"{"error":{"message":"model requires more system memory (40.0 GiB) than is available (20.1 GiB)"}}"#));
        assert!(msg.starts_with("Not enough memory to load \"big\""));
        assert!(msg.contains("40.0 GiB"));
    }
}
