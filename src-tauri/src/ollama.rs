//! Ollama-specific endpoints (loaded models, unloading). Used only when the
//! configured server is Ollama; other OpenAI-compatible servers simply don't
//! get these features.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::sysmem::MemoryInfo;

/// `http://127.0.0.1:11434/v1` -> `http://127.0.0.1:11434`
pub fn origin(base: &str) -> &str {
    base.strip_suffix("/v1").unwrap_or(base)
}

pub async fn is_ollama(http: &reqwest::Client, base: &str) -> bool {
    match http.get(format!("{}/api/version", origin(base))).timeout(Duration::from_secs(2)).send().await {
        Ok(r) if r.status().is_success() => r.bytes().await.ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).is_some_and(|v| v["version"].is_string()),
        _ => false,
    }
}

/// The server's version string (`/api/version`), or None if it isn't Ollama.
pub async fn version(http: &reqwest::Client, base: &str) -> Option<String> {
    let r = http.get(format!("{}/api/version", origin(base))).timeout(Duration::from_secs(2)).send().await.ok()?;
    let v: Value = serde_json::from_slice(&r.bytes().await.ok()?).ok()?;
    v["version"].as_str().map(String::from)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoadedModel {
    pub name: String,
    pub size_bytes: u64,
    pub vram_bytes: u64,
    pub context_length: Option<u64>,
    pub expires_at: Option<String>,
}

pub async fn loaded(http: &reqwest::Client, base: &str) -> Result<Vec<LoadedModel>, String> {
    let resp = http
        .get(format!("{}/api/ps", origin(base)))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|e| format!("Couldn't ask the server which models are loaded: {e}"))?;
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(parse_ps(&v))
}

fn parse_ps(v: &Value) -> Vec<LoadedModel> {
    v["models"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    Some(LoadedModel {
                        name: m["name"].as_str().or(m["model"].as_str())?.to_string(),
                        size_bytes: m["size"].as_u64().unwrap_or(0),
                        vram_bytes: m["size_vram"].as_u64().unwrap_or(0),
                        context_length: m["context_length"].as_u64(),
                        expires_at: m["expires_at"].as_str().map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Asks Ollama to release a model's RAM/VRAM now (`keep_alive: 0`).
pub async fn unload(http: &reqwest::Client, base: &str, model: &str) -> Result<(), String> {
    let resp = http
        .post(format!("{}/api/generate", origin(base)))
        .header("content-type", "application/json")
        .body(json!({ "model": model, "keep_alive": 0 }).to_string())
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("Couldn't unload {model}: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Couldn't unload {model}: HTTP {}", resp.status()))
    }
}

/// One non-streaming chat call whose answer must match `schema` (Ollama
/// structured outputs), at temperature 0. Thinking is off: background jobs want
/// the answer only. `num_predict` caps the answer's length so a looping model
/// can't run until the context is full; an answer cut off at the cap is an
/// error, never half-parsed JSON. `num_ctx` None keeps the model's current
/// context so a loaded brain isn't reloaded.
#[allow(clippy::too_many_arguments)]
pub async fn chat_json(
    http: &reqwest::Client,
    base: &str,
    model: &str,
    messages: &[Value],
    schema: &Value,
    num_ctx: Option<u32>,
    num_predict: u32,
    keep_alive: &str,
) -> Result<Value, String> {
    let mut options = json!({ "temperature": 0, "num_predict": num_predict });
    if let Some(n) = num_ctx {
        options["num_ctx"] = json!(n);
    }
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "format": schema,
        "options": options,
        "keep_alive": keep_alive,
        "think": false,
    });
    let mut resp = post(http, base, &body).await?;
    // Models without a thinking mode reject the `think` field: retry without it.
    if let Err(e) = &resp {
        if e.contains("think") {
            body.as_object_mut().map(|o| o.remove("think"));
            resp = post(http, base, &body).await?;
        }
    }
    json_answer(model, &resp?, num_predict)
}

/// The JSON in a `/api/chat` response, refusing an answer that hit the length cap.
fn json_answer(model: &str, v: &Value, num_predict: u32) -> Result<Value, String> {
    if v["done_reason"].as_str() == Some("length") {
        return Err(format!("{model}'s answer was cut off at the {num_predict}-token limit"));
    }
    let text = v["message"]["content"].as_str().unwrap_or_default();
    serde_json::from_str(text).map_err(|e| format!("{model} did not return valid JSON: {e}"))
}

async fn post(http: &reqwest::Client, base: &str, body: &Value) -> Result<Result<Value, String>, String> {
    let resp = http
        .post(format!("{}/api/chat", origin(base)))
        .header("content-type", "application/json")
        .body(body.to_string())
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the model server: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&bytes).unwrap_or_default();
    if !status.is_success() {
        return Ok(Err(v["error"].as_str().map(String::from).unwrap_or_else(|| format!("HTTP {status}"))));
    }
    Ok(Ok(v))
}

/// Embeddings for each input (Ollama /api/embed).
pub async fn embed(http: &reqwest::Client, base: &str, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
    let resp = http
        .post(format!("{}/api/embed", origin(base)))
        .header("content-type", "application/json")
        .body(json!({ "model": model, "input": inputs, "keep_alive": "10m" }).to_string())
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the model server: {e}"))?;
    let status = resp.status();
    let v: Value = serde_json::from_slice(&resp.bytes().await.map_err(|e| e.to_string())?).unwrap_or_default();
    if !status.is_success() {
        return Err(v["error"].as_str().map(String::from).unwrap_or_else(|| format!("HTTP {status}")));
    }
    let rows = v["embeddings"].as_array().ok_or("no embeddings in the response")?;
    Ok(rows
        .iter()
        .map(|r| r.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect()).unwrap_or_default())
        .collect())
}

/// Whether a model with this exact name is installed.
pub async fn has_model(http: &reqwest::Client, base: &str, name: &str) -> bool {
    let Ok(resp) = http.get(format!("{}/api/tags", origin(base))).timeout(Duration::from_secs(3)).send().await else {
        return false;
    };
    let v: Value = resp.bytes().await.ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    v["models"].as_array().is_some_and(|a| a.iter().any(|m| m["name"].as_str() == Some(name) || m["model"].as_str() == Some(name)))
}

/// Registers a local GGUF file with Ollama as a new model `name`: uploads the
/// blob to the local server (127.0.0.1) and creates the model from it. The
/// original file is only read.
pub async fn import_gguf(http: &reqwest::Client, base: &str, path: &std::path::Path, name: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let digest: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    let blob_url = format!("{}/api/blobs/sha256:{digest}", origin(base));
    let exists = http.head(&blob_url).send().await.map(|r| r.status().is_success()).unwrap_or(false);
    if !exists {
        let r = http.post(&blob_url).body(bytes).timeout(Duration::from_secs(900)).send().await.map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("Uploading the model file to Ollama failed: HTTP {}", r.status()));
        }
    }
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("model.gguf");
    let r = http
        .post(format!("{}/api/create", origin(base)))
        .header("content-type", "application/json")
        .body(json!({ "model": name, "files": { file_name: format!("sha256:{digest}") }, "stream": false }).to_string())
        .timeout(Duration::from_secs(900))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let v: Value = serde_json::from_slice(&r.bytes().await.map_err(|e| e.to_string())?).unwrap_or_default();
    if !status.is_success() {
        return Err(format!("Ollama couldn't create {name}: {}", v["error"].as_str().unwrap_or("unknown error")));
    }
    Ok(())
}

/// The longest context a model was trained for (`/api/show` model_info
/// `<architecture>.context_length`), or None if the server doesn't say.
pub async fn context_limit(http: &reqwest::Client, base: &str, model: &str) -> Option<u32> {
    let resp = http
        .post(format!("{}/api/show", origin(base)))
        .header("content-type", "application/json")
        .body(json!({ "model": model }).to_string())
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    let v: Value = serde_json::from_slice(&resp.bytes().await.ok()?).ok()?;
    parse_context_limit(&v)
}

fn parse_context_limit(v: &Value) -> Option<u32> {
    let info = v["model_info"].as_object()?;
    let arch = info.get("general.architecture").and_then(Value::as_str);
    let exact = arch.and_then(|a| info.get(&format!("{a}.context_length"))).and_then(Value::as_u64);
    let any = || info.iter().find(|(k, _)| k.ends_with(".context_length")).and_then(|(_, v)| v.as_u64());
    exact.or_else(any).map(|n| n.min(u32::MAX as u64) as u32)
}

/// Memory reserved for the OS and other apps when deciding whether a model fits.
const RAM_MARGIN: u64 = 2 * 1024 * 1024 * 1024;
const VRAM_MARGIN: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FitReport {
    pub model: String,
    pub already_loaded: bool,
    /// Bytes the model needs once loaded (measured earlier, or estimated).
    pub needed_bytes: u64,
    pub needed_is_estimate: bool,
    /// Free RAM + VRAM minus a safety margin.
    pub free_bytes: u64,
    /// Other loaded models that could be unloaded to make room.
    pub others: Vec<LoadedModel>,
    pub fits_now: bool,
    pub fits_after_unloading_others: bool,
    pub memory: MemoryInfo,
}

/// Rough footprint before a model has been seen loaded: weights plus KV cache
/// and runtime buffers.
pub fn estimate_needed(file_size: u64) -> u64 {
    file_size + file_size / 5 + 1536 * 1024 * 1024
}

pub fn evaluate(model: &str, needed: u64, is_estimate: bool, memory: MemoryInfo, loaded: &[LoadedModel]) -> FitReport {
    let already_loaded = loaded.iter().any(|m| m.name == model);
    let others: Vec<LoadedModel> = loaded.iter().filter(|m| m.name != model).cloned().collect();
    let free_ram = memory.free_ram.saturating_sub(RAM_MARGIN);
    let free_vram = memory.free_vram.unwrap_or(0).saturating_sub(VRAM_MARGIN);
    let free_bytes = free_ram + free_vram;
    let freeable: u64 = others.iter().map(|m| m.size_bytes).sum();
    FitReport {
        model: model.to_string(),
        already_loaded,
        needed_bytes: needed,
        needed_is_estimate: is_estimate,
        free_bytes,
        fits_now: already_loaded || needed <= free_bytes,
        fits_after_unloading_others: already_loaded || needed <= free_bytes + freeable,
        others,
        memory,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn mem(free_ram: u64, free_vram: u64) -> MemoryInfo {
        MemoryInfo { total_ram: 32 * GB, free_ram, gpu_name: None, total_vram: Some(12 * GB), free_vram: Some(free_vram) }
    }

    fn lm(name: &str, size: u64) -> LoadedModel {
        LoadedModel { name: name.into(), size_bytes: size, vram_bytes: 0, context_length: None, expires_at: None }
    }

    #[test]
    fn reads_context_limit_from_api_show() {
        let v = json!({ "model_info": { "general.architecture": "qwen3", "qwen3.context_length": 262144, "qwen3.embedding_length": 4096 } });
        assert_eq!(parse_context_limit(&v), Some(262144));
        let v = json!({ "model_info": { "qwen35moe.context_length": 32768 } });
        assert_eq!(parse_context_limit(&v), Some(32768));
        assert_eq!(parse_context_limit(&json!({})), None);
    }

    #[test]
    fn json_answers_cut_off_or_broken_are_refused() {
        let ok = json!({ "message": { "content": "{\"a\": 1}" }, "done_reason": "stop" });
        assert_eq!(json_answer("m", &ok, 100).unwrap(), json!({ "a": 1 }));
        // Valid-looking JSON that stopped at the cap is still refused.
        let cut = json!({ "message": { "content": "{\"a\": 1}" }, "done_reason": "length" });
        assert!(json_answer("m", &cut, 100).unwrap_err().contains("cut off at the 100-token limit"));
        let broken = json!({ "message": { "content": "{\"a\": \"unterminated" }, "done_reason": "stop" });
        assert!(json_answer("m", &broken, 100).unwrap_err().contains("did not return valid JSON"));
    }

    #[test]
    fn parses_api_ps() {
        let v: Value = serde_json::from_str(
            r#"{"models":[{"name":"qwen3.5:4b","model":"qwen3.5:4b","size":5300000000,"size_vram":5300000000,"expires_at":"2026-10-16T19:20:00Z","context_length":64000}]}"#,
        )
        .unwrap();
        assert_eq!(
            parse_ps(&v),
            vec![LoadedModel { name: "qwen3.5:4b".into(), size_bytes: 5_300_000_000, vram_bytes: 5_300_000_000, context_length: Some(64000), expires_at: Some("2026-10-16T19:20:00Z".into()) }]
        );
        assert!(parse_ps(&json!({"models": []})).is_empty());
    }

    #[test]
    fn big_model_does_not_fit_beside_another_but_fits_after_unloading_it() {
        // 27B resident (~19 GB), 10 GB RAM + 0.5 GB VRAM free, coder needs ~24 GB.
        let r = evaluate("qwen3-coder", 24 * GB, false, mem(10 * GB, GB / 2), &[lm("qwen3.8:27b", 19 * GB)]);
        assert!(!r.fits_now);
        assert!(r.fits_after_unloading_others);
        assert_eq!(r.others.len(), 1);
    }

    #[test]
    fn already_loaded_model_always_fits() {
        let r = evaluate("a", 100 * GB, false, mem(0, 0), &[lm("a", 5 * GB)]);
        assert!(r.fits_now && r.already_loaded && r.others.is_empty());
    }

    #[test]
    fn small_model_fits_with_room_to_spare() {
        let r = evaluate("small", estimate_needed(3 * GB), true, mem(20 * GB, 11 * GB), &[]);
        assert!(r.fits_now);
    }

    #[test]
    fn too_big_even_alone() {
        let r = evaluate("huge", 80 * GB, true, mem(20 * GB, 11 * GB), &[lm("x", 5 * GB)]);
        assert!(!r.fits_now && !r.fits_after_unloading_others);
    }
}
