// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod activity;
mod context;
mod db;
mod home;
mod identity;
mod internet;
mod llm;
mod localtime;
mod memcmds;
mod memindex;
mod memory;
mod ollama;
mod operator;
mod safety_cmds;
mod secrules;
mod security;
mod server;
mod shutdown;
mod sysmem;
mod think;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::{Manager, State};
use tokio::sync::Notify;

pub struct AppState {
    db: Mutex<rusqlite::Connection>,
    /// Plain-HTTP client for the local model server (loopback only).
    http: reqwest::Client,
    /// The only client that may reach the internet, used solely for approved downloads.
    internet: reqwest::Client,
    server: server::ServerManager,
    /// Cancellation handles for in-flight generations, keyed by request id.
    active: Mutex<HashMap<String, Arc<Notify>>>,
    home: Mutex<PathBuf>,
    activity: activity::ActivityLog,
    approvals: operator::Approvals,
    /// Notified by Lockdown to abort any download in progress.
    download_cancel: Notify,
    /// Mirror of the Lockdown setting, checked on hot paths.
    lockdown: std::sync::atomic::AtomicBool,
    /// Wakes the background memory worker.
    memory_notify: Notify,
    /// Democritus's firewall rule switched off for an approved download, keyed
    /// by the download approval it belongs to.
    firewall_gate: Mutex<Option<(String, security::FirewallGate)>>,
    /// For background tasks started from synchronous commands.
    app_handle: std::sync::OnceLock<tauri::AppHandle>,
}

impl AppState {
    fn db(&self) -> MutexGuard<'_, rusqlite::Connection> {
        self.db.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn settings(&self) -> db::Settings {
        db::get_settings(&self.db())
    }

    fn operator(&self) -> operator::OperatorSettings {
        operator::load(&self.db())
    }

    fn home(&self) -> PathBuf {
        self.home.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn log(&self, kind: &str, action: &str, detail: Value) {
        self.activity.log(kind, action, detail);
    }

    fn cancel_handle(&self, request_id: &str) -> Arc<Notify> {
        let mut active = self.active.lock().unwrap_or_else(|p| p.into_inner());
        active.entry(request_id.to_string()).or_insert_with(|| Arc::new(Notify::new())).clone()
    }
}

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn clean_title(raw: &str) -> String {
    let title: String = raw.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
    if title.is_empty() {
        "New chat".into()
    } else {
        title
    }
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> db::Settings {
    state.settings()
}

#[tauri::command]
fn save_settings(state: State<AppState>, settings: db::Settings) -> CmdResult<db::Settings> {
    let mut s = settings;
    s.base_url = llm::normalize_base_url(&s.base_url)?;
    if !(0.0..=2.0).contains(&s.temperature) {
        return Err("Temperature must be between 0 and 2.".into());
    }
    if s.max_tokens == 0 || s.max_tokens > 262_144 {
        return Err("Max tokens must be between 1 and 262144.".into());
    }
    if !(2048..=262_144).contains(&s.context_tokens) {
        return Err("The context window must be between 2048 and 262144 tokens.".into());
    }
    if !["system", "light", "dark"].contains(&s.theme.as_str()) {
        return Err("The theme must be light, dark or system.".into());
    }
    s.server_command = s.server_command.trim().to_string();
    s.home_dir = s.home_dir.trim().to_string();
    let before = state.settings();
    db::save_settings(&state.db(), &s).map_err(err)?;
    let changes = activity::diff(&before, &s);
    if !changes.is_empty() {
        state.log("setting", "settings changed", Value::Object(changes));
    }
    if before.home_dir != s.home_dir {
        let new_home = home::resolve(&s.home_dir);
        *state.home.lock().unwrap_or_else(|p| p.into_inner()) = new_home.clone();
        state.activity.set_dir(home::logs_dir(&new_home));
        state.log("setting", "home folder changed", json!({ "home": new_home.display().to_string() }));
    }
    Ok(s)
}

#[tauri::command]
async fn list_models(state: State<'_, AppState>) -> CmdResult<Vec<llm::ModelInfo>> {
    let base = state.settings().base_url;
    llm::list_models(&state.http, &base).await
}

#[tauri::command]
async fn server_status(state: State<'_, AppState>) -> CmdResult<server::ServerStatus> {
    let base = state.settings().base_url;
    Ok(state.server.status(&state.http, &base).await)
}

#[tauri::command]
async fn start_server(state: State<'_, AppState>) -> CmdResult<server::ServerStatus> {
    let s = state.settings();
    state.log("model", "start model server requested", json!({ "command": s.server_command, "url": s.base_url }));
    state.server.ensure_running(&state.http, &s.base_url, &s.server_command).await?;
    Ok(state.server.status(&state.http, &s.base_url).await)
}

/// Models currently in RAM/VRAM. Also remembers each one's real footprint for
/// later fit checks. Empty when the server isn't Ollama.
#[tauri::command]
async fn loaded_models(state: State<'_, AppState>) -> CmdResult<Vec<ollama::LoadedModel>> {
    let base = state.settings().base_url;
    if !ollama::is_ollama(&state.http, &base).await {
        return Ok(Vec::new());
    }
    let loaded = ollama::loaded(&state.http, &base).await?;
    let conn = state.db();
    for m in &loaded {
        let _ = db::record_model_memory(&conn, &m.name, m.size_bytes, m.vram_bytes, m.context_length);
    }
    Ok(loaded)
}

#[tauri::command]
async fn unload_model(state: State<'_, AppState>, model: String) -> CmdResult<()> {
    let base = state.settings().base_url;
    let result = ollama::unload(&state.http, &base, &model).await;
    state.log("model", "unload", json!({ "model": model, "url": format!("{}/api/generate", ollama::origin(&base)), "ok": result.is_ok() }));
    result
}

#[tauri::command]
fn memory_info() -> sysmem::MemoryInfo {
    sysmem::snapshot()
}

/// Whether `model` fits in free RAM+VRAM, and whether unloading the other
/// loaded models would make room. None when the server isn't Ollama.
#[tauri::command]
async fn model_fit(state: State<'_, AppState>, model: String) -> CmdResult<Option<ollama::FitReport>> {
    let base = state.settings().base_url;
    if !ollama::is_ollama(&state.http, &base).await {
        return Ok(None);
    }
    let loaded = ollama::loaded(&state.http, &base).await?;
    let measured = db::model_memory(&state.db(), &model).map_err(err)?;
    let (needed, is_estimate) = match measured {
        Some(bytes) => (bytes, false),
        None => {
            let models = llm::list_models(&state.http, &base).await?;
            let file = models.iter().find(|m| m.id == model).and_then(|m| m.size_bytes).unwrap_or(0);
            (ollama::estimate_needed(file), true)
        }
    };
    let memory = tauri::async_runtime::spawn_blocking(sysmem::snapshot).await.map_err(err)?;
    Ok(Some(ollama::evaluate(&model, needed, is_estimate, memory, &loaded)))
}

#[tauri::command]
fn list_conversations(state: State<AppState>, query: Option<String>) -> CmdResult<Vec<db::Conversation>> {
    db::list_conversations(&state.db(), query.as_deref()).map_err(err)
}

#[tauri::command]
fn create_conversation(state: State<AppState>, title: String, model: String) -> CmdResult<db::Conversation> {
    let conn = state.db();
    let conv = db::create_conversation(&conn, &clean_title(&title), &model).map_err(err)?;
    // New chats belong to the active project, if one is set.
    let active = db::get_settings(&conn).active_project;
    if active.is_empty() {
        return Ok(conv);
    }
    db::set_conversation_project(&conn, conv.id, &active).map_err(err)?;
    Ok(db::Conversation { project: active, ..conv })
}

#[tauri::command]
fn rename_conversation(state: State<AppState>, id: i64, title: String) -> CmdResult<()> {
    db::rename_conversation(&state.db(), id, &clean_title(&title)).map_err(err)
}

#[tauri::command]
fn delete_conversation(state: State<AppState>, id: i64) -> CmdResult<()> {
    db::delete_conversation(&state.db(), id).map_err(err)
}

#[tauri::command]
fn set_conversation_model(state: State<AppState>, id: i64, model: String) -> CmdResult<()> {
    db::set_conversation_model(&state.db(), id, &model).map_err(err)
}

#[tauri::command]
fn get_messages(state: State<AppState>, conversation_id: i64) -> CmdResult<Vec<db::Message>> {
    db::get_messages(&state.db(), conversation_id).map_err(err)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendRequest {
    request_id: String,
    conversation_id: i64,
    model: String,
    content: String,
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum StreamEvent {
    Started { user_message: db::Message, assistant_message_id: i64 },
    /// Progress before the first token (e.g. summarizing older messages).
    Status { text: String },
    Reasoning { text: String },
    Content { text: String },
    Reclassify,
    Done { message: db::Message },
}

/// Collects a streamed reply and forwards each piece to the window.
struct Accumulator<'a> {
    channel: &'a Channel<StreamEvent>,
    splitter: think::ThinkSplitter,
    content: String,
    reasoning: String,
    first_token: Option<Instant>,
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    finish_reason: Option<String>,
}

impl<'a> Accumulator<'a> {
    fn new(channel: &'a Channel<StreamEvent>) -> Self {
        Self {
            channel,
            splitter: think::ThinkSplitter::default(),
            content: String::new(),
            reasoning: String::new(),
            first_token: None,
            prompt_tokens: None,
            completion_tokens: None,
            finish_reason: None,
        }
    }

    fn on_delta(&mut self, delta: llm::Delta) {
        match delta {
            llm::Delta::Reasoning(t) => self.piece(think::Piece::Reasoning(t)),
            llm::Delta::Content(t) => {
                for p in self.splitter.push(&t) {
                    self.piece(p);
                }
            }
            llm::Delta::Usage { prompt_tokens, completion_tokens } => {
                self.prompt_tokens = prompt_tokens;
                self.completion_tokens = completion_tokens;
            }
            llm::Delta::Finish(reason) => self.finish_reason = Some(reason),
        }
    }

    fn flush(&mut self) {
        for p in self.splitter.finish() {
            self.piece(p);
        }
    }

    fn piece(&mut self, piece: think::Piece) {
        self.first_token.get_or_insert_with(Instant::now);
        let event = match piece {
            think::Piece::Reasoning(text) => {
                self.reasoning.push_str(&text);
                StreamEvent::Reasoning { text }
            }
            think::Piece::Content(text) => {
                self.content.push_str(&text);
                StreamEvent::Content { text }
            }
            think::Piece::Reclassify => {
                let moved = std::mem::take(&mut self.content);
                self.reasoning.push_str(&moved);
                StreamEvent::Reclassify
            }
        };
        let _ = self.channel.send(event);
    }

    fn stats(&self, started: Instant) -> Value {
        let ended = Instant::now();
        let mut stats = serde_json::Map::new();
        stats.insert("durationMs".into(), json!(ended.duration_since(started).as_millis() as u64));
        if let Some(first) = self.first_token {
            stats.insert("timeToFirstTokenMs".into(), json!(first.duration_since(started).as_millis() as u64));
        }
        if let Some(n) = self.prompt_tokens {
            stats.insert("promptTokens".into(), json!(n));
        }
        if let Some(n) = self.completion_tokens {
            stats.insert("completionTokens".into(), json!(n));
            if let Some(first) = self.first_token {
                let secs = ended.duration_since(first).as_secs_f64();
                if secs > 0.05 {
                    stats.insert("tokensPerSecond".into(), json!((n as f64 / secs * 10.0).round() / 10.0));
                }
            }
        }
        if let Some(reason) = &self.finish_reason {
            stats.insert("finishReason".into(), json!(reason));
        }
        Value::Object(stats)
    }
}

#[tauri::command]
async fn send_message(state: State<'_, AppState>, req: SendRequest, on_event: Channel<StreamEvent>) -> CmdResult<()> {
    if req.content.trim().is_empty() {
        return Err("The message is empty.".into());
    }
    if req.model.trim().is_empty() {
        return Err("Pick a model first.".into());
    }
    let cancel = state.cancel_handle(&req.request_id);
    let result = run_chat(&state, &req, &cancel, &on_event).await;
    state.active.lock().unwrap_or_else(|p| p.into_inner()).remove(&req.request_id);
    result
}

/// The context window to request for `model`: the operator's setting, capped at
/// the model's own limit. None when the server isn't Ollama (the OpenAI-
/// compatible endpoint can't set it). The memory helper uses the same value
/// when it borrows the brain, so the loaded model is never reloaded.
pub(crate) async fn chat_num_ctx(state: &AppState, model: &str) -> Option<u32> {
    let base = state.settings().base_url;
    if !ollama::is_ollama(&state.http, &base).await {
        return None;
    }
    let want = state.settings().context_tokens;
    let limit = ollama::context_limit(&state.http, &base, model).await.unwrap_or(u32::MAX);
    Some(want.min(limit).max(2048))
}

// The prompt asks for about 300 words; the length limits keep a looping model
// from filling the whole window.
const SUMMARY_SCHEMA: &str = r#"{"type":"object","properties":{"summary":{"type":"string","maxLength":4000}},"required":["summary"]}"#;
const SUMMARY_NUM_PREDICT: u32 = 1536;

/// Folds `turns` into the conversation's running summary with the current brain.
async fn summarize(state: &AppState, model: &str, num_ctx: u32, previous: &str, turns: &[context::Turn]) -> CmdResult<String> {
    let base = state.settings().base_url;
    let max_chars = (num_ctx as usize * 3) * 6 / 10;
    let messages = vec![json!({ "role": "user", "content": context::summary_prompt(previous, turns, max_chars) })];
    state.log(
        "network",
        "local model request",
        json!({ "url": format!("{}/api/chat", ollama::origin(&base)), "model": model, "purpose": "summarize older messages", "messages": turns.len() }),
    );
    let schema: Value = serde_json::from_str(SUMMARY_SCHEMA).expect("valid schema");
    let v = ollama::chat_json(&state.http, &base, model, &messages, &schema, Some(num_ctx), SUMMARY_NUM_PREDICT, "5m").await?;
    let summary = v["summary"].as_str().unwrap_or_default().trim().to_string();
    if summary.is_empty() {
        return Err(format!("{model} returned an empty summary"));
    }
    Ok(summary)
}

async fn run_chat(state: &AppState, req: &SendRequest, cancel: &Notify, on_event: &Channel<StreamEvent>) -> CmdResult<()> {
    let settings = state.settings();
    let (user_message, assistant_id, conv) = {
        let conn = state.db();
        let user_id = db::insert_message(&conn, req.conversation_id, "user", &req.content, &req.model, "complete").map_err(err)?;
        db::set_conversation_model(&conn, req.conversation_id, &req.model).map_err(err)?;
        db::touch_conversation(&conn, req.conversation_id).map_err(err)?;
        let assistant_id = db::insert_message(&conn, req.conversation_id, "assistant", "", &req.model, "streaming").map_err(err)?;
        let user_message = db::get_message(&conn, user_id).map_err(err)?.ok_or("The message could not be saved.")?;
        let conv = db::get_conversation(&conn, req.conversation_id).map_err(err)?.ok_or("The conversation was deleted.")?;
        (user_message, assistant_id, conv)
    };
    let user_message_id = user_message.id;
    let _ = on_event.send(StreamEvent::Started { user_message, assistant_message_id: assistant_id });

    // ---- context: identity, memory, search results, summary, history ----
    let num_ctx = chat_num_ctx(state, &req.model).await;
    let window = num_ctx.unwrap_or(settings.context_tokens);
    let core = identity::system_prompt(&req.model, &settings.system_prompt);
    let project = if conv.project.is_empty() { settings.active_project.clone() } else { conv.project.clone() };
    let memory = memcmds::context_memory(state, &project);
    let query: String = req.content.chars().take(2000).collect();
    let hits: Vec<context::SearchHit> = memcmds::search(state, &query, 6)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|h| context::SearchHit { path: h.path, heading: h.heading, text: h.text })
        .collect();
    let mut summary_failed = false;
    let assembled = loop {
        let (summary, upto) = db::get_summary(&state.db(), req.conversation_id).map_err(err)?;
        let turns = db::history(&state.db(), req.conversation_id, upto).map_err(err)?;
        let a = context::assemble(&context::Input {
            core: &core,
            model: &req.model,
            memory: &memory,
            hits: &hits,
            summary: &summary,
            turns: &turns,
            num_ctx: window,
            max_tokens: settings.max_tokens,
        });
        // Older messages don't fit: fold them into the summary with the current
        // brain (Ollama only), then assemble again.
        let (Some(n), Some(last), false) = (num_ctx, a.to_summarize.last(), summary_failed) else { break a };
        let last_id = last.id;
        let _ = on_event.send(StreamEvent::Status { text: format!("Summarizing {} older messages to fit the context window…", a.to_summarize.len()) });
        let result = tokio::select! {
            _ = cancel.notified() => None,
            r = summarize(state, &req.model, n, &summary, &a.to_summarize) => Some(r),
        };
        match result {
            None => {
                let conn = state.db();
                db::finish_message(&conn, assistant_id, "", "", "stopped", None, None).map_err(err)?;
                if let Some(message) = db::get_message(&conn, assistant_id).map_err(err)? {
                    let _ = on_event.send(StreamEvent::Done { message });
                }
                return Ok(());
            }
            Some(Ok(new_summary)) => {
                db::set_summary(&state.db(), req.conversation_id, &new_summary, last_id).map_err(err)?;
                state.log("memory", "older messages summarized", json!({ "conversation": req.conversation_id, "messages": a.to_summarize.len(), "upTo": last_id, "model": req.model }));
            }
            Some(Err(e)) => {
                state.log("memory", "summarizing failed; oldest messages left out", json!({ "conversation": req.conversation_id, "error": e }));
                summary_failed = true;
            }
        }
    };
    let report = assembled.report;
    let mut messages = Vec::with_capacity(assembled.messages.len() + 1);
    messages.push(json!({ "role": "system", "content": assembled.system }));
    messages.extend(assembled.messages.into_iter().map(|(role, content)| json!({ "role": role, "content": content })));

    let url = match num_ctx {
        Some(_) => format!("{}/api/chat", ollama::origin(&settings.base_url)),
        None => format!("{}/chat/completions", settings.base_url),
    };
    state.log(
        "network",
        "local model request",
        json!({
            "url": url,
            "model": req.model,
            "conversation": req.conversation_id,
            "messages": messages.len(),
            "numCtx": num_ctx,
            "memoryFiles": report.memory_files,
            "searchHits": report.search_hits,
            "estimatedPromptTokens": report.estimated_prompt_tokens,
        }),
    );
    if let Some(prev) = &report.switched_from {
        state.log("model", "brain switch handoff", json!({ "conversation": req.conversation_id, "from": prev, "to": req.model }));
    }
    let started = Instant::now();
    let mut acc = Accumulator::new(on_event);
    let request = llm::ChatRequest {
        base: &settings.base_url,
        model: &req.model,
        messages,
        temperature: settings.temperature,
        max_tokens: settings.max_tokens,
        num_ctx,
    };
    let outcome = llm::stream_chat(&state.http, request, cancel, |d| acc.on_delta(d)).await;
    acc.flush();

    let (status, error) = match outcome {
        Ok(llm::Outcome::Finished) => ("complete", None),
        Ok(llm::Outcome::Cancelled) => ("stopped", None),
        Err(e) => ("error", Some(e)),
    };
    let mut stats = acc.stats(started);
    stats["context"] = serde_json::to_value(&report).map_err(err)?;
    let stats = stats.to_string();
    let message = {
        let conn = state.db();
        db::finish_message(&conn, assistant_id, acc.content.trim(), acc.reasoning.trim(), status, error.as_deref(), Some(&stats))
            .map_err(err)?;
        db::touch_conversation(&conn, req.conversation_id).map_err(err)?;
        db::get_message(&conn, assistant_id).map_err(err)?
    };
    // None means the conversation was deleted mid-stream; nothing left to show.
    if let Some(message) = message {
        let completed = message.status == "complete";
        let _ = on_event.send(StreamEvent::Done { message });
        // Learn from the exchange in the background (skipped while memory is paused or locked down).
        if completed {
            memcmds::enqueue(state, req.conversation_id, user_message_id, assistant_id);
        }
    }
    Ok(())
}

#[tauri::command]
fn stop_generation(state: State<AppState>, request_id: String) {
    // notify_one stores a permit, so a stop that races ahead of the request still lands.
    state.cancel_handle(&request_id).notify_one();
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // DEMOCRITUS_DATA_DIR redirects app data for automated tests, so test
            // runs never touch the operator's real conversations.
            let (data_dir, log_dir) = match std::env::var_os("DEMOCRITUS_DATA_DIR") {
                Some(dir) => {
                    let dir = std::path::PathBuf::from(dir);
                    (dir.clone(), dir.join("logs"))
                }
                None => (app.path().app_data_dir()?, app.path().app_log_dir()?),
            };
            std::fs::create_dir_all(&data_dir)?;
            std::fs::create_dir_all(&log_dir)?;
            let db_path = data_dir.join("democritus.db");
            // One-time copy from the earlier "Local Qwen" build (%APPDATA%\com.localqwen.desktop).
            // Tests point DEMOCRITUS_LEGACY_DB at a fixture instead.
            let legacy = match std::env::var_os("DEMOCRITUS_LEGACY_DB") {
                Some(p) => Some(std::path::PathBuf::from(p)),
                None if std::env::var_os("DEMOCRITUS_DATA_DIR").is_none() => {
                    data_dir.parent().map(|p| p.join("com.localqwen.desktop").join("local-qwen.db"))
                }
                None => None,
            };
            if let Some(legacy) = legacy {
                match db::migrate_legacy(&db_path, &legacy) {
                    Ok(true) => eprintln!("Migrated conversations and settings from {}", legacy.display()),
                    Ok(false) => {}
                    Err(e) => eprintln!("Could not migrate {}: {e}", legacy.display()),
                }
            }
            let conn = db::open(&db_path)?;
            let home_dir = home::resolve(&db::get_settings(&conn).home_dir);
            let lockdown = operator::load(&conn).lockdown;

            app.manage(AppState {
                db: Mutex::new(conn),
                http: llm::client(),
                internet: internet::client(),
                server: server::ServerManager::new(log_dir.join("model-server.log")),
                active: Mutex::new(HashMap::new()),
                activity: activity::ActivityLog::new(home::logs_dir(&home_dir)),
                home: Mutex::new(home_dir.clone()),
                approvals: operator::Approvals::default(),
                download_cancel: Notify::new(),
                lockdown: std::sync::atomic::AtomicBool::new(lockdown),
                memory_notify: Notify::new(),
                firewall_gate: Mutex::new(None),
                app_handle: std::sync::OnceLock::new(),
            });
            let _ = app.state::<AppState>().app_handle.set(app.handle().clone());
            tauri::async_runtime::spawn(memcmds::worker(app.handle().clone()));
            shutdown::start_watchdog(app.handle());

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<AppState>();
                let s = state.settings();
                let op = state.operator();
                state.log("security", "app started", json!({ "home": state.home().display().to_string(), "lockdown": op.lockdown }));
                // Crash safety: a download interrupted by a crash or power loss can
                // leave Democritus's firewall rule switched off. Warn at once.
                let fw = security::startup_firewall_check(&state).await;
                if fw.state == "off" {
                    use tauri::Emitter;
                    let _ = handle.emit("firewall-warning", &fw);
                }
                if s.auto_start_server {
                    let _ = state.server.ensure_running(&state.http, &s.base_url, &s.server_command).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            list_models,
            server_status,
            start_server,
            loaded_models,
            unload_model,
            memory_info,
            model_fit,
            list_conversations,
            create_conversation,
            rename_conversation,
            delete_conversation,
            set_conversation_model,
            get_messages,
            send_message,
            stop_generation,
            safety_cmds::app_paths,
            safety_cmds::get_operator,
            safety_cmds::save_operator,
            safety_cmds::set_lockdown,
            safety_cmds::read_activity,
            safety_cmds::request_download,
            safety_cmds::respond_approval,
            security::security_check,
            security::security_fix,
            security::firewall_status,
            security::firewall_reenable,
            memcmds::memory_overview,
            memcmds::memory_read,
            memcmds::memory_write,
            memcmds::memory_delete,
            memcmds::memory_history,
            memcmds::memory_snapshot,
            memcmds::memory_restore,
            memcmds::memory_search,
            memcmds::memory_proposals,
            memcmds::memory_preview,
            memcmds::memory_apply,
            memcmds::memory_reject,
            memcmds::memory_kick,
            memcmds::memory_install_semantic,
            memcmds::list_projects,
            memcmds::create_project,
            memcmds::set_active_project,
            memcmds::reset_learning,
            memcmds::last_time,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Democritus")
        .run(|app, event| match event {
            tauri::RunEvent::Exit => shutdown::cleanup(app, "normal exit"),
            // Test hook (debug builds only): DEMOCRITUS_TEST_PREVENT_EXIT=once|always
            // blocks Tauri's own exit, the way the shutdown hang did, so the
            // watchdog in shutdown.rs can be tested on demand.
            #[cfg(debug_assertions)]
            tauri::RunEvent::ExitRequested { api, .. } => {
                static PREVENTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                match std::env::var("DEMOCRITUS_TEST_PREVENT_EXIT").as_deref() {
                    Ok("always") => api.prevent_exit(),
                    Ok("once") if !PREVENTED.swap(true, std::sync::atomic::Ordering::SeqCst) => api.prevent_exit(),
                    _ => {}
                }
            }
            _ => {}
        });
}
