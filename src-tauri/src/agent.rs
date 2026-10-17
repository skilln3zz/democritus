//! The coding agent (Code page): the current brain works on one project folder
//! with five tools. Listing, reading and searching run straight away; every
//! edit or new file is shown to the operator as a diff and waits for Apply or
//! Reject. It can't run commands or reach the internet. Uses Ollama's native
//! tool calling (/api/chat with `tools`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::{Manager, State};
use tokio::sync::{oneshot, Notify};

use crate::agentcore::*;
use crate::workspace::{PlannedEdit, Workspace};
use crate::{home, identity, llm, ollama, AppState};

type CmdResult<T> = Result<T, String>;

/// Model turns per request before the agent stops and reports back.
const MAX_STEPS: usize = 30;
/// Characters per token, as in context.rs (conservative).
const CHARS_PER_TOKEN: usize = 3;

// ---------- shared state ----------

#[derive(Default)]
pub struct AgentHub {
    /// Edits waiting for the operator, by approval id.
    approvals: Mutex<HashMap<String, oneshot::Sender<(bool, String)>>>,
    /// Conversation of each task (kept for follow-up requests this session).
    tasks: Mutex<HashMap<String, Task>>,
    /// The task running now, if any (one at a time).
    running: Mutex<Option<String>>,
    next_id: std::sync::atomic::AtomicU64,
}

struct Task {
    folder: PathBuf,
    messages: Vec<Value>,
}

impl AgentHub {
    fn new_id(&self, prefix: &str) -> String {
        format!("{prefix}-{}-{}", crate::db::now_ms(), self.next_id.fetch_add(1, Ordering::SeqCst))
    }
}

/// Stops the running task (Lockdown, or the permission switched off).
pub fn stop_all(state: &AppState) {
    let running = state.agent.running.lock().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(id) = running {
        state.cancel_handle(&id).notify_one();
    }
    // Pending approvals resolve as rejected when their senders drop.
    state.agent.approvals.lock().unwrap_or_else(|p| p.into_inner()).clear();
}

// ---------- events to the window ----------

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AgentEvent {
    Started { task_id: String, folder: String, num_ctx: u32 },
    Step { step: usize },
    Thinking { text: String },
    Content { text: String },
    ToolCall { call_id: String, name: String, summary: String },
    ToolResult { call_id: String, ok: bool, summary: String },
    Approval { approval_id: String, call_id: String, edit: PlannedEdit },
    Resolved { approval_id: String, applied: bool, message: String },
    Done { status: String, message: String, edits_applied: usize, tokens: u64 },
}

struct Step {
    content: String,
    thinking: String,
    tool_calls: Vec<Value>,
    tokens: u64,
}

#[allow(clippy::too_many_arguments)]
async fn stream_step(
    http: &reqwest::Client,
    base: &str,
    model: &str,
    messages: &[Value],
    num_ctx: u32,
    temperature: f64,
    max_tokens: u32,
    cancel: &Notify,
    on_event: &Channel<AgentEvent>,
) -> Result<Option<Step>, String> {
    let body = json!({
        "model": model,
        "messages": messages,
        "tools": tool_specs(),
        "stream": true,
        "options": { "temperature": temperature, "num_predict": max_tokens, "num_ctx": num_ctx },
    });
    let send = http.post(format!("{}/api/chat", ollama::origin(base))).header("content-type", "application/json").body(body.to_string()).send();
    let resp = tokio::select! {
        _ = cancel.notified() => return Ok(None),
        r = send => r.map_err(|e| format!("Can't reach the model server: {e}"))?,
    };
    let status = resp.status();
    if !status.is_success() {
        let bytes = resp.bytes().await.unwrap_or_default();
        let raw = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v["error"].as_str().map(String::from))
            .unwrap_or_else(|| format!("HTTP {status}"));
        if raw.contains("does not support tools") {
            return Err(format!("{model} can't use tools. Pick qwen3-coder or another Qwen3 model for the coding agent."));
        }
        return Err(llm::friendly_error(model, &raw));
    }
    let mut resp = resp;
    let mut step = Step { content: String::new(), thinking: String::new(), tool_calls: Vec::new(), tokens: 0 };
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = cancel.notified() => return Ok(None),
            c = resp.chunk() => c.map_err(|e| format!("The connection to the model server was interrupted: {e}"))?,
        };
        let Some(chunk) = chunk else { break };
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let Some(l) = parse_step_line(&String::from_utf8_lossy(&line)) else { continue };
            if let Some(e) = l.error {
                return Err(llm::friendly_error(model, &e));
            }
            if !l.thinking.is_empty() {
                step.thinking.push_str(&l.thinking);
                let _ = on_event.send(AgentEvent::Thinking { text: l.thinking });
            }
            if !l.content.is_empty() {
                step.content.push_str(&l.content);
                let _ = on_event.send(AgentEvent::Content { text: l.content });
            }
            step.tool_calls.extend(l.tool_calls);
            if l.done {
                step.tokens = l.eval_count;
                return Ok(Some(step));
            }
        }
    }
    Ok(Some(step))
}

// ---------- commands ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    /// Continue this task (same folder), or start a new one when empty.
    task_id: String,
    folder: String,
    model: String,
    message: String,
}

fn protected_dirs(app: &tauri::AppHandle, state: &AppState) -> Vec<PathBuf> {
    let mut out = vec![state.home(), crate::security::ollama_models_dir()];
    if let Ok(d) = app.path().app_data_dir() {
        out.push(d);
    }
    if let Some(d) = std::env::var_os("DEMOCRITUS_DATA_DIR") {
        out.push(PathBuf::from(d));
    }
    out
}

#[tauri::command]
pub async fn agent_run(app: tauri::AppHandle, state: State<'_, AppState>, req: RunRequest, on_event: Channel<AgentEvent>) -> CmdResult<()> {
    let op = state.operator();
    if !op.allow_coding_agent {
        return Err("The coding agent is off. Turn on \"Coding agent\" under Settings & safety first.".into());
    }
    if op.lockdown {
        return Err("Lockdown is on, so the coding agent is paused.".into());
    }
    if req.message.trim().is_empty() {
        return Err("Describe what the agent should do.".into());
    }
    let settings = state.settings();
    if !ollama::is_ollama(&state.http, &settings.base_url).await {
        return Err("The coding agent needs Ollama (it uses Ollama's tool calling).".into());
    }
    let ws = Workspace::open(&req.folder, &protected_dirs(&app, &state))?;
    {
        let mut running = state.agent.running.lock().unwrap_or_else(|p| p.into_inner());
        if running.is_some() {
            return Err("The coding agent is already working on a task. Stop it first.".into());
        }
        let id = if req.task_id.is_empty() { state.agent.new_id("task") } else { req.task_id.clone() };
        *running = Some(id);
    }
    let task_id = state.agent.running.lock().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_default();
    let result = run(&state, &ws, &task_id, &req, &on_event).await;
    *state.agent.running.lock().unwrap_or_else(|p| p.into_inner()) = None;
    state.active.lock().unwrap_or_else(|p| p.into_inner()).remove(&task_id);
    if let Err(e) = &result {
        state.log("agent", "coding task failed", json!({ "task": task_id, "error": e }));
        let _ = on_event.send(AgentEvent::Done { status: "error".into(), message: e.clone(), edits_applied: 0, tokens: 0 });
    }
    Ok(())
}

async fn run(state: &AppState, ws: &Workspace, task_id: &str, req: &RunRequest, on_event: &Channel<AgentEvent>) -> CmdResult<()> {
    let settings = state.settings();
    let base = settings.base_url.clone();
    let limit = ollama::context_limit(&state.http, &base, &req.model).await.unwrap_or(u32::MAX);
    let num_ctx = settings.agent_context_tokens.min(limit).max(4096);
    let cancel = state.cancel_handle(task_id);
    let folder = ws.root().display().to_string();

    // A follow-up continues the same conversation if the folder is unchanged.
    let mut messages = {
        let tasks = state.agent.tasks.lock().unwrap_or_else(|p| p.into_inner());
        tasks.get(task_id).filter(|t| t.folder == ws.root()).map(|t| t.messages.clone())
    }
    .unwrap_or_else(|| {
        let core = identity::system_prompt(&req.model, &settings.system_prompt);
        vec![json!({ "role": "system", "content": system_prompt(&core, &folder) })]
    });
    messages.push(json!({ "role": "user", "content": req.message }));
    state.log("agent", "coding task started", json!({ "task": task_id, "folder": folder, "model": req.model, "request": req.message.chars().take(300).collect::<String>() }));
    let _ = on_event.send(AgentEvent::Started { task_id: task_id.to_string(), folder: folder.clone(), num_ctx });

    let backups = home::backups_dir(&state.home()).join("agent").join(task_id);
    let reply_room = (settings.max_tokens as usize).min(num_ctx as usize / 2);
    let budget = (num_ctx as usize).saturating_sub(reply_room) * CHARS_PER_TOKEN;
    let mut edits_applied = 0usize;
    let mut tokens = 0u64;
    let save = |messages: &Vec<Value>| {
        state.agent.tasks.lock().unwrap_or_else(|p| p.into_inner()).insert(task_id.to_string(), Task { folder: ws.root().to_path_buf(), messages: messages.clone() });
    };

    for step_no in 1..=MAX_STEPS {
        if state.lockdown.load(Ordering::SeqCst) {
            break;
        }
        fit_messages(&mut messages, budget);
        let _ = on_event.send(AgentEvent::Step { step: step_no });
        state.log("network", "local model request", json!({ "url": format!("{}/api/chat", ollama::origin(&base)), "model": req.model, "purpose": "coding agent", "task": task_id, "step": step_no }));
        let Some(step) = stream_step(&state.http, &base, &req.model, &messages, num_ctx, settings.temperature, settings.max_tokens, &cancel, on_event).await? else {
            return finish(state, task_id, on_event, &messages, save, "stopped", "Stopped.", edits_applied, tokens);
        };
        tokens += step.tokens;
        let calls: Vec<ToolCall> = step.tool_calls.iter().filter_map(parse_tool_call).collect();
        let mut assistant = json!({ "role": "assistant", "content": step.content });
        if !step.tool_calls.is_empty() {
            assistant["tool_calls"] = json!(step.tool_calls);
        }
        messages.push(assistant);
        if calls.is_empty() {
            return finish(state, task_id, on_event, &messages, save, "complete", "", edits_applied, tokens);
        }
        for call in calls {
            let call_id = state.agent.new_id("call");
            let summary = summarize(&call);
            let _ = on_event.send(AgentEvent::ToolCall { call_id: call_id.clone(), name: call.name.clone(), summary: summary.clone() });
            let result = match run_tool(ws, &call) {
                Err(e) => {
                    let _ = on_event.send(AgentEvent::ToolResult { call_id: call_id.clone(), ok: false, summary: e.clone() });
                    format!("Error: {e}")
                }
                Ok(ToolOutcome::Text(t)) => {
                    state.log("agent", &summary, json!({ "task": task_id }));
                    let lines = t.lines().count().saturating_sub(1);
                    let _ = on_event.send(AgentEvent::ToolResult { call_id: call_id.clone(), ok: true, summary: format!("{lines} line{}", if lines == 1 { "" } else { "s" }) });
                    t
                }
                Ok(ToolOutcome::Edit(edit)) => {
                    let Some((approved, note)) = ask(state, &call_id, &edit, &cancel, on_event).await else {
                        return finish(state, task_id, on_event, &messages, save, "stopped", "Stopped while an edit was waiting.", edits_applied, tokens);
                    };
                    let (text, applied, message) = if approved {
                        match ws.apply(&edit, &backups) {
                            Ok(backup) => {
                                edits_applied += 1;
                                let where_ = backup.map(|b| format!(" Backup of the old version: {}", b.display())).unwrap_or_default();
                                state.log("agent", "edit applied", json!({ "task": task_id, "path": edit.path, "newFile": edit.new_file, "backup": where_.trim() }));
                                (format!("The operator approved it; {} was written.", edit.path), true, format!("Applied.{where_}"))
                            }
                            Err(e) => {
                                state.log("agent", "edit failed", json!({ "task": task_id, "path": edit.path, "error": e }));
                                (format!("Error: {e}"), false, e)
                            }
                        }
                    } else {
                        state.log("agent", "edit rejected", json!({ "task": task_id, "path": edit.path, "note": note }));
                        let why = if note.trim().is_empty() { String::new() } else { format!(" Their note: {}", note.trim()) };
                        (format!("The operator rejected this change to {}; nothing was written.{why}", edit.path), false, "Rejected.".into())
                    };
                    let _ = on_event.send(AgentEvent::ToolResult { call_id: call_id.clone(), ok: applied, summary: message });
                    text
                }
            };
            messages.push(json!({ "role": "tool", "content": result, "tool_name": call.name }));
        }
        save(&messages);
    }
    let msg = if state.lockdown.load(Ordering::SeqCst) { "Lockdown turned on: the agent stopped." } else { "Stopped after 30 steps. Send a follow-up to let it continue." };
    finish(state, task_id, on_event, &messages, save, "limit", msg, edits_applied, tokens)
}

/// Sends the edit to the window and waits for Apply/Reject. None if stopped.
async fn ask(state: &AppState, call_id: &str, edit: &PlannedEdit, cancel: &Notify, on_event: &Channel<AgentEvent>) -> Option<(bool, String)> {
    let approval_id = state.agent.new_id("edit");
    let (tx, rx) = oneshot::channel();
    state.agent.approvals.lock().unwrap_or_else(|p| p.into_inner()).insert(approval_id.clone(), tx);
    state.log("agent", "edit proposed", json!({ "path": edit.path, "newFile": edit.new_file }));
    let _ = on_event.send(AgentEvent::Approval { approval_id: approval_id.clone(), call_id: call_id.to_string(), edit: edit.clone() });
    let answer = tokio::select! {
        _ = cancel.notified() => None,
        r = rx => r.ok(),
    };
    state.agent.approvals.lock().unwrap_or_else(|p| p.into_inner()).remove(&approval_id);
    let _ = on_event.send(AgentEvent::Resolved {
        approval_id,
        applied: answer.as_ref().is_some_and(|a| a.0),
        message: if answer.is_none() { "Stopped.".into() } else { String::new() },
    });
    answer
}

#[allow(clippy::too_many_arguments)]
fn finish(
    state: &AppState,
    task_id: &str,
    on_event: &Channel<AgentEvent>,
    messages: &Vec<Value>,
    save: impl Fn(&Vec<Value>),
    status: &str,
    message: &str,
    edits_applied: usize,
    tokens: u64,
) -> CmdResult<()> {
    save(messages);
    state.log("agent", "coding task finished", json!({ "task": task_id, "status": status, "editsApplied": edits_applied }));
    let _ = on_event.send(AgentEvent::Done { status: status.into(), message: message.into(), edits_applied, tokens });
    Ok(())
}

#[tauri::command]
pub fn agent_respond(state: State<AppState>, approval_id: String, approve: bool, note: String) -> CmdResult<()> {
    let tx = state.agent.approvals.lock().unwrap_or_else(|p| p.into_inner()).remove(&approval_id);
    match tx {
        Some(tx) => {
            let _ = tx.send((approve, note));
            Ok(())
        }
        None => Err("This edit is no longer waiting (the task was stopped).".into()),
    }
}

#[tauri::command]
pub fn agent_stop(state: State<AppState>) {
    stop_all(&state);
}

/// Forgets a task's conversation (the "New task" button).
#[tauri::command]
pub fn agent_forget(state: State<AppState>, task_id: String) {
    state.agent.tasks.lock().unwrap_or_else(|p| p.into_inner()).remove(&task_id);
}
