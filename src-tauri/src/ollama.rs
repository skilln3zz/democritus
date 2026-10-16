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
