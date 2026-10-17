//! Commands behind the Operator and Log pages: permissions, Lockdown, the
//! activity log, and the approval flow for internet downloads.

use std::sync::atomic::Ordering;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::{Manager, State};

use crate::operator::{self, Approval, OperatorSettings};
use crate::{activity, home, internet, security, AppState};

type CmdResult<T> = Result<T, String>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPaths {
    home: String,
    memory: String,
    logs: String,
    training: String,
    downloads: String,
}

#[tauri::command]
pub fn app_paths(state: State<AppState>) -> AppPaths {
    let h = state.home();
    AppPaths {
        home: h.display().to_string(),
        memory: home::memory_dir(&h).display().to_string(),
        logs: home::logs_dir(&h).display().to_string(),
        training: home::training_dir(&h).display().to_string(),
        downloads: home::downloads_dir(&h).display().to_string(),
    }
}

#[tauri::command]
pub fn get_operator(state: State<AppState>) -> OperatorSettings {
    state.operator()
}

/// Saves operator settings (from the Operator page only) and logs every change.
pub fn apply_operator(state: &AppState, next: OperatorSettings) -> CmdResult<OperatorSettings> {
    let next = next.normalized()?;
    let before = state.operator();
    operator::save(&state.db(), &next).map_err(|e| e.to_string())?;
    state.lockdown.store(next.lockdown, Ordering::SeqCst);
    let changes = activity::diff(&before, &next);
    if changes.is_empty() {
        return Ok(next);
    }
    // Lockdown, or switching the permission off, stops a running coding task.
    if (next.lockdown && !before.lockdown) || (!next.allow_coding_agent && before.allow_coding_agent) {
        crate::agent::stop_all(state);
    }
    // Lockdown pauses learning, so it pauses a build too (as does switching the permission off).
    if next.lockdown && !before.lockdown {
        crate::build::pause_for(state, "lockdown");
    } else if !next.allow_training && before.allow_training {
        crate::build::pause_for(state, "permission");
    }
    if next.lockdown != before.lockdown {
        if next.lockdown {
            // Drop anything waiting for approval and abort a running download.
            state.approvals.clear();
            state.download_cancel.notify_waiters();
            // A firewall rule switched off for a download that's waiting for its
            // second approval goes back on now. (A running download closes its
            // own when it's cancelled.)
            if let Some((_, gate)) = take_gate(state, None) {
                close_in_background(state, gate, "Lockdown");
            }
            state.log("security", "Lockdown ON: internet blocked, learning and memory writes paused", json!({}));
        } else {
            state.log("security", "Lockdown OFF", json!({}));
        }
    }
    state.log("setting", "operator permissions changed", Value::Object(changes));
    Ok(next)
}

#[tauri::command]
pub fn save_operator(state: State<AppState>, settings: OperatorSettings) -> CmdResult<OperatorSettings> {
    apply_operator(&state, settings)
}

#[tauri::command]
pub fn set_lockdown(state: State<AppState>, on: bool) -> CmdResult<OperatorSettings> {
    let mut s = state.operator();
    s.lockdown = on;
    apply_operator(&state, s)
}

#[tauri::command]
pub fn read_activity(state: State<AppState>, limit: Option<usize>, kind: Option<String>, query: Option<String>) -> Vec<Value> {
    state.activity.read(limit.unwrap_or(300).min(5000), kind.as_deref().filter(|k| !k.is_empty()), query.as_deref())
}

/// Internet use is possible only with downloads allowed and Lockdown off.
fn internet_allowed(state: &AppState) -> CmdResult<OperatorSettings> {
    let op = state.operator();
    if op.lockdown || state.lockdown.load(Ordering::SeqCst) {
        return Err("Lockdown is on: all internet access is blocked.".into());
    }
    if !op.allow_downloads {
        return Err("Model downloads are off. Turn on \"Allow model downloads\" on the Operator page first.".into());
    }
    Ok(op)
}

/// Step 1 of a download: validate the URL and ask the operator to approve a
/// one-request lookup of its size and checksum. Nothing touches the network yet.
#[tauri::command]
pub fn request_download(state: State<AppState>, url: String) -> CmdResult<Approval> {
    let refuse = |reason: String| {
        state.log("security", "download request refused", json!({ "url": url, "reason": reason }));
        reason
    };
    let op = internet_allowed(&state).map_err(refuse)?;
    let target = operator::validate_download_url(&url, &op.download_allowlist).map_err(refuse)?;
    let approval = state.approvals.create(Approval {
        id: String::new(),
        kind: "lookup".into(),
        url: target.url,
        file_name: target.file_name,
        size_bytes: None,
        sha256: None,
        served_from: None,
        created_ms: 0,
    });
    state.log("approval", "approval requested", serde_json::to_value(&approval).unwrap_or_default());
    Ok(approval)
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ApprovalOutcome {
    Denied,
    /// The lookup ran; the download itself needs its own approval.
    NeedsApproval { approval: Approval },
    Downloaded { result: internet::DownloadResult },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    done: u64,
    total: Option<u64>,
    /// What's happening besides bytes, e.g. waiting for the admin prompt.
    stage: Option<String>,
}

fn stage(text: &str) -> Progress {
    Progress { done: 0, total: None, stage: Some(text.into()) }
}

/// How long the rule may stay off while the second (download) dialog waits.
const GATE_DIALOG_MINUTES: u64 = 10;

/// Takes the open firewall gate: any gate (key None), or only the one that
/// belongs to this download approval.
fn take_gate(state: &AppState, key: Option<&str>) -> Option<(String, security::FirewallGate)> {
    let mut g = state.firewall_gate.lock().unwrap_or_else(|p| p.into_inner());
    if key.is_none() || g.as_ref().is_some_and(|(k, _)| Some(k.as_str()) == key) {
        g.take()
    } else {
        None
    }
}

/// Switches the rule back on from a synchronous context (Lockdown, expiry).
fn close_in_background(state: &AppState, gate: security::FirewallGate, why: &'static str) {
    security::signal_gate_done(&gate);
    if let Some(handle) = state.app_handle.get().cloned() {
        tauri::async_runtime::spawn(async move {
            let st = handle.state::<AppState>();
            security::close_gate(&st, gate, why).await;
        });
    }
}

/// Switches Democritus's own firewall rule off (one admin prompt) unless a
/// gate is already open or there's no rule.
async fn ensure_gate(state: &AppState, approval: &Approval, on_progress: &Channel<Progress>) -> CmdResult<Option<security::FirewallGate>> {
    if let Some((_, gate)) = take_gate(state, Some(&approval.id)) {
        return Ok(Some(gate));
    }
    let _ = on_progress.send(stage("Waiting for the Windows administrator prompt to switch off Democritus's firewall rule for this download…"));
    let gate = security::open_gate(state, &approval.id, &approval.url).await?;
    let _ = on_progress.send(stage(if gate.is_some() { "Firewall rule switched off for this download." } else { "" }));
    Ok(gate)
}

/// The operator's answer to an approval dialog. Each approval works exactly once.
#[tauri::command]
pub async fn respond_approval(state: State<'_, AppState>, id: String, approve: bool, on_progress: Channel<Progress>) -> CmdResult<ApprovalOutcome> {
    let approval = state.approvals.take(&id).ok_or("This approval was already used or has expired. Each approval works once.")?;
    let summary = serde_json::to_value(&approval).unwrap_or_default();
    if !approve {
        state.log("approval", "denied", summary);
        if let Some((_, gate)) = take_gate(&state, Some(&approval.id)) {
            security::close_gate(&state, gate, "download denied").await;
        }
        return Ok(ApprovalOutcome::Denied);
    }
    state.log("approval", "approved", summary);
    // Re-check everything at the moment of use: settings may have changed.
    let checked = internet_allowed(&state).and_then(|op| operator::validate_download_url(&approval.url, &op.download_allowlist).map(|_| ()));
    if let Err(e) = checked {
        if let Some((_, gate)) = take_gate(&state, Some(&approval.id)) {
            security::close_gate(&state, gate, "refused").await;
        }
        return Err(e);
    }
    let log_request = |method: &str, url: &str, status: u16| {
        state.log("network", "internet request", json!({ "method": method, "url": url, "status": status }));
    };

    match approval.kind.as_str() {
        "lookup" => {
            // The lookup is already internet access: the rule goes off now and
            // stays off only until the download ends (or is denied / expires).
            let gate = ensure_gate(&state, &approval, &on_progress).await?;
            let found = match internet::lookup(&state.internet, &approval.url, log_request).await {
                Ok(f) => f,
                Err(e) => {
                    state.log("network", "lookup failed", json!({ "url": approval.url, "error": e }));
                    if let Some(g) = gate {
                        security::close_gate(&state, g, "lookup failed").await;
                    }
                    return Err(e);
                }
            };
            let next = state.approvals.create(Approval {
                kind: "download".into(),
                size_bytes: found.size_bytes,
                sha256: found.sha256,
                served_from: found.served_from,
                ..approval
            });
            state.log("approval", "approval requested", serde_json::to_value(&next).unwrap_or_default());
            if let Some(g) = gate {
                *state.firewall_gate.lock().unwrap_or_else(|p| p.into_inner()) = Some((next.id.clone(), g));
                // If the second dialog isn't answered in time, the rule goes back on.
                if let Some(handle) = state.app_handle.get().cloned() {
                    let next_id = next.id.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(GATE_DIALOG_MINUTES * 60)).await;
                        let st = handle.state::<AppState>();
                        if st.approvals.take(&next_id).is_some() {
                            st.log("approval", "expired", json!({ "id": next_id }));
                        }
                        if let Some((_, gate)) = take_gate(&st, Some(&next_id)) {
                            security::close_gate(&st, gate, "download approval expired").await;
                        }
                    });
                }
            }
            Ok(ApprovalOutcome::NeedsApproval { approval: next })
        }
        "download" => {
            let gate = ensure_gate(&state, &approval, &on_progress).await?;
            let dest = home::downloads_dir(&state.home());
            state.log("download", "started", json!({ "url": approval.url, "file": approval.file_name, "to": dest.display().to_string() }));
            let mut last_sent = 0u64;
            let result = internet::download(
                &state.internet,
                &approval,
                &dest,
                &state.download_cancel,
                &state.lockdown,
                |done, total| {
                    if done - last_sent >= 4 * 1024 * 1024 || Some(done) == total {
                        last_sent = done;
                        let _ = on_progress.send(Progress { done, total, stage: None });
                    }
                },
                log_request,
            )
            .await;
            // Finished, failed or cancelled: the rule goes back on right away.
            if let Some(g) = gate {
                let _ = on_progress.send(stage("Switching Democritus's firewall rule back on…"));
                let why = match &result {
                    Ok(_) => "download finished",
                    Err(e) if e.contains("cancel") => "download cancelled",
                    Err(_) => "download failed",
                };
                security::close_gate(&state, g, why).await;
            }
            match result {
                Ok(r) => {
                    state.log(
                        "download",
                        "finished",
                        json!({ "file": r.path, "bytes": r.size_bytes, "sha256": r.sha256, "checksumMatches": r.checksum_matches }),
                    );
                    Ok(ApprovalOutcome::Downloaded { result: r })
                }
                Err(e) => {
                    state.log("download", "failed", json!({ "url": approval.url, "error": e }));
                    Err(e)
                }
            }
        }
        other => Err(format!("Unknown approval kind: {other}")),
    }
}
