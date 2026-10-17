//! The Security check on the Settings & safety page: read-only checks of how
//! Ollama and Windows are set up, plus two fixes that run only after the
//! operator has seen the exact commands and confirmed them.
//!
//! The window only ever sends a fix *id*; the commands are built here again
//! from the same inputs, so nothing typed or injected in the UI can reach an
//! elevated PowerShell.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::State;

use crate::secrules::*;
use crate::{home, llm, ollama, server, AppState};

type CmdResult<T> = Result<T, String>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fix {
    pub id: &'static str,
    pub label: String,
    /// The exact commands that will run (empty for in-app fixes like Lockdown).
    pub commands: Vec<String>,
    /// Runs in an elevated PowerShell (Windows shows a UAC prompt).
    pub admin: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckItem {
    pub id: &'static str,
    pub title: String,
    /// ok (green) | bad (red) | unknown (grey) | info (grey, nothing to judge)
    pub status: &'static str,
    pub detail: String,
    pub fix: Option<Fix>,
}

#[cfg(windows)]
fn run(program: &str, args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new(program).args(args).creation_flags(0x0800_0000).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(windows)]
fn powershell(script: &str) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encode_command(script)])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| format!("Couldn't start PowerShell: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { format!("PowerShell exited with {}", out.status) } else { err })
    }
}

#[cfg(windows)]
const USER_ENV: &str = r"HKCU\Environment";
#[cfg(windows)]
const MACHINE_ENV: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

/// (where, value) for every place an environment variable is set.
fn env_sources(name: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    #[cfg(windows)]
    for (label, key) in [("your user account", USER_ENV), ("the whole computer", MACHINE_ENV)] {
        if let Some(v) = run("reg", &["query", key, "/v", name]).and_then(|t| parse_reg_query(&t, name)) {
            out.push((label, v));
        }
    }
    if let Ok(v) = std::env::var(name) {
        if !out.iter().any(|(_, x)| *x == v) {
            out.push(("this app's environment", v));
        }
    }
    out
}

pub(crate) fn ollama_models_dir() -> PathBuf {
    if let Some((_, v)) = env_sources("OLLAMA_MODELS").into_iter().next() {
        if !v.trim().is_empty() {
            return PathBuf::from(expand_env(v.trim(), |n| std::env::var(n).ok()));
        }
    }
    let user = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
    user.join(".ollama").join("models")
}

/// The programs the firewall rules are for: Democritus itself, Ollama's server
/// and (when present) Ollama's tray app, which is what checks for updates.
#[cfg_attr(not(windows), allow(dead_code))]
fn firewall_targets() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        out.push(("Democritus".to_string(), exe));
    }
    let ollama = server::resolve_program("ollama");
    if ollama.is_file() {
        if let Some(tray) = ollama.parent().map(|d| d.join("ollama app.exe")).filter(|p| p.is_file()) {
            out.push(("the Ollama tray app".to_string(), tray));
        }
        out.push(("Ollama".to_string(), ollama));
    }
    out
}

/// Programs that already have an enabled outbound Block rule.
#[cfg(windows)]
fn blocked_programs() -> Result<Vec<String>, String> {
    let text = powershell(
        "$p = @(Get-NetFirewallRule -Direction Outbound -Action Block -Enabled True -PolicyStore ActiveStore -ErrorAction SilentlyContinue | \
         Get-NetFirewallApplicationFilter | ForEach-Object { [Environment]::ExpandEnvironmentVariables($_.Program) }); ConvertTo-Json -InputObject $p -Compress",
    )?;
    let v: Value = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
    Ok(match v {
        Value::Array(a) => a.into_iter().filter_map(|x| x.as_str().map(String::from)).collect(),
        Value::String(s) => vec![s],
        _ => Vec::new(),
    })
}

/// Commands for the programs that don't have a rule yet.
fn firewall_plan() -> Result<(Vec<(String, PathBuf, bool)>, Vec<String>), String> {
    #[cfg(windows)]
    {
        let blocked = blocked_programs()?;
        let targets: Vec<(String, PathBuf, bool)> =
            firewall_targets().into_iter().map(|(l, p)| { let has = blocked.iter().any(|b| same_path(b, &p)); (l, p, has) }).collect();
        let commands = targets.iter().filter(|t| !t.2).map(|(l, p, _)| firewall_command(l, p)).collect();
        Ok((targets, commands))
    }
    #[cfg(not(windows))]
    Err("Windows Firewall rules can only be checked on Windows.".into())
}

fn ollama_host_fix(port: u16) -> Option<Fix> {
    let sources = env_sources("OLLAMA_HOST");
    let bad: Vec<&(&str, String)> = sources.iter().filter(|(_, v)| !host_is_loopback(v)).collect();
    if bad.is_empty() || !bad.iter().any(|(w, _)| *w != "this app's environment") {
        return None;
    }
    let value = format!("127.0.0.1:{port}");
    let mut commands = Vec::new();
    let mut admin = false;
    for (w, _) in &bad {
        let scope = match *w {
            "your user account" => "User",
            "the whole computer" => {
                admin = true;
                "Machine"
            }
            _ => continue,
        };
        commands.push(format!("[Environment]::SetEnvironmentVariable('OLLAMA_HOST', {}, '{scope}')", ps_quote(&value)));
    }
    Some(Fix {
        id: "ollama_host",
        label: format!("Set OLLAMA_HOST to {value}"),
        commands,
        admin,
        note: "Afterwards quit Ollama from the tray icon and start it again, so it picks up the new value.".into(),
    })
}

// ---------- the checks ----------

fn check_bind(port: u16) -> CheckItem {
    let title = "Ollama listens only on this computer".to_string();
    #[cfg(windows)]
    {
        let Some(text) = run("netstat", &["-an"]) else {
            return CheckItem { id: "bind", title, status: "unknown", detail: "Couldn't run netstat to see where Ollama listens.".into(), fix: None };
        };
        let hosts = parse_netstat_listeners(&text, port);
        let open: Vec<&String> = hosts.iter().filter(|h| !is_loopback_host(h)).collect();
        let (status, detail) = if hosts.is_empty() {
            ("unknown", format!("Nothing is listening on port {port}. Is Ollama running?"))
        } else if open.is_empty() {
            ("ok", format!("Port {port} is bound to {} only.", hosts.join(", ")))
        } else {
            (
                "bad",
                format!(
                    "Port {port} is bound to {}, so other devices on your network can use your models. Usually OLLAMA_HOST (below) is the cause.",
                    open.iter().map(|h| h.as_str()).collect::<Vec<_>>().join(", ")
                ),
            )
        };
        CheckItem { id: "bind", title, status, detail, fix: None }
    }
    #[cfg(not(windows))]
    {
        let _ = port;
        CheckItem { id: "bind", title, status: "unknown", detail: "This check runs on Windows.".into(), fix: None }
    }
}

fn check_ollama_host(port: u16) -> CheckItem {
    let sources = env_sources("OLLAMA_HOST");
    let title = "OLLAMA_HOST is unset or local".to_string();
    if sources.is_empty() {
        return CheckItem { id: "ollama_host", title, status: "ok", detail: format!("Not set, so Ollama uses its default, 127.0.0.1:{port}."), fix: None };
    }
    let desc = sources.iter().map(|(w, v)| format!("\"{v}\" for {w}")).collect::<Vec<_>>().join("; ");
    if sources.iter().all(|(_, v)| host_is_loopback(v)) {
        CheckItem { id: "ollama_host", title, status: "ok", detail: format!("Set to {desc}: loopback only."), fix: None }
    } else {
        CheckItem {
            id: "ollama_host",
            title,
            status: "bad",
            detail: format!("Set to {desc}. That lets Ollama listen beyond this computer."),
            fix: ollama_host_fix(port),
        }
    }
}

fn check_firewall() -> CheckItem {
    let title = "Firewall blocks Democritus and Ollama from the internet".to_string();
    match firewall_plan() {
        Err(e) => CheckItem { id: "firewall", title, status: "unknown", detail: e, fix: None },
        Ok((targets, commands)) => {
            let missing: Vec<&str> = targets.iter().filter(|t| !t.2).map(|t| t.0.as_str()).collect();
            let found = targets.len() - missing.len();
            let detail = if missing.is_empty() {
                format!("Outbound block rules exist for {}.", targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(", "))
            } else {
                format!(
                    "No outbound block rule for {}{}. Without one, only the app's own rules keep it offline.",
                    missing.join(", "),
                    if found > 0 { format!(" ({found} already blocked)") } else { String::new() }
                )
            };
            let fix = (!commands.is_empty()).then(|| Fix {
                id: "firewall",
                label: "Create rules".into(),
                commands,
                admin: true,
                note: "The rules block every address except 127.0.0.1 and ::1, so chat with the local model keeps working. \
                       Approved model downloads from the Operator page will fail while the Democritus rule is on; \
                       you can switch it off in Windows Defender Firewall (group \"Democritus\")."
                    .into(),
            });
            CheckItem { id: "firewall", title, status: if missing.is_empty() { "ok" } else { "bad" }, detail, fix }
        }
    }
}

async fn check_version(state: &AppState, base: &str) -> CheckItem {
    let title = "Ollama version".to_string();
    match ollama::version(&state.http, base).await {
        Some(v) => CheckItem {
            id: "version",
            title,
            status: "info",
            detail: format!("Ollama {v}. Keep it updated: get new versions yourself from ollama.com. Democritus never checks for updates."),
            fix: None,
        },
        None => CheckItem { id: "version", title, status: "unknown", detail: "Ollama didn't answer, so its version is unknown.".into(), fix: None },
    }
}

fn check_lockdown(state: &AppState) -> CheckItem {
    let op = state.operator();
    let title = "Lockdown and network permission".to_string();
    let (status, detail, fix) = if op.lockdown {
        ("ok", "Lockdown is on: all internet access is blocked and learning is paused.".to_string(), None)
    } else if !op.allow_downloads {
        ("ok", "Lockdown is off, and downloads are switched off: Democritus talks only to the local model server.".to_string(), None)
    } else {
        (
            "info",
            "Model downloads are allowed. Each one still needs your approval twice; switch them off when you're done.".to_string(),
            Some(Fix { id: "lockdown", label: "Turn on Lockdown".into(), commands: Vec::new(), admin: false, note: String::new() }),
        )
    };
    CheckItem { id: "lockdown", title, status, detail, fix }
}

fn check_model_files(downloads: PathBuf) -> CheckItem {
    let title = "Model files are only .gguf / .safetensors".to_string();
    let store = ollama_models_dir();
    let d = scan_downloads(&downloads);
    let o = scan_ollama_blobs(&store);
    let mut problems = d.problems;
    problems.extend(o.problems);
    if d.scanned + o.scanned == 0 {
        return CheckItem {
            id: "model_files",
            title,
            status: "unknown",
            detail: format!("No model files found in {} or {}.", downloads.display(), store.join("blobs").display()),
            fix: None,
        };
    }
    let where_ = format!("{} file{} in {} and Ollama's store ({})", d.scanned + o.scanned, if d.scanned + o.scanned == 1 { "" } else { "s" }, downloads.display(), store.display());
    if problems.is_empty() {
        CheckItem { id: "model_files", title, status: "ok", detail: format!("Checked {where_}: no executables, archives or pickles."), fix: None }
    } else {
        let shown: Vec<&str> = problems.iter().take(4).map(String::as_str).collect();
        let more = if problems.len() > 4 { format!(" (+{} more)", problems.len() - 4) } else { String::new() };
        CheckItem {
            id: "model_files",
            title,
            status: "bad",
            detail: format!("Checked {where_}. Suspicious: {}{more}. Democritus never deletes model files; remove them yourself if you don't trust them.", shown.join("; ")),
            fix: None,
        }
    }
}

fn port_of(base: &str) -> u16 {
    reqwest::Url::parse(base).ok().and_then(|u| u.port_or_known_default()).unwrap_or(11434)
}

#[tauri::command]
pub async fn security_check(state: State<'_, AppState>) -> CmdResult<Vec<CheckItem>> {
    let base = llm::normalize_base_url(&state.settings().base_url).unwrap_or_else(|_| state.settings().base_url);
    let port = port_of(&base);
    let version = check_version(&state, &base).await;
    let downloads = home::downloads_dir(&state.home());
    // netstat, reg, PowerShell and the file scan all block: keep them off the async runtime.
    let [bind, host, firewall, model_files] =
        tauri::async_runtime::spawn_blocking(move || [check_bind(port), check_ollama_host(port), check_firewall(), check_model_files(downloads)])
            .await
            .map_err(|e| e.to_string())?;
    let items = vec![bind, host, firewall, version, check_lockdown(&state), model_files];
    let summary: serde_json::Map<String, Value> = items.iter().map(|i| (i.id.to_string(), json!(i.status))).collect();
    state.log("security", "security check", Value::Object(summary));
    Ok(items)
}

/// Runs a fix the operator confirmed. The commands are rebuilt here, never
/// taken from the window.
#[tauri::command]
pub async fn security_fix(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let port = port_of(&state.settings().base_url);
    let fix = match id.as_str() {
        "firewall" => tauri::async_runtime::spawn_blocking(check_firewall).await.map_err(|e| e.to_string())?.fix,
        "ollama_host" => tauri::async_runtime::spawn_blocking(move || ollama_host_fix(port)).await.map_err(|e| e.to_string())?,
        _ => return Err(format!("Unknown fix \"{id}\".")),
    };
    let Some(fix) = fix else { return Err("Nothing to fix any more. Run the check again.".into()) };
    state.log("security", "security fix confirmed", json!({ "fix": fix.id, "admin": fix.admin, "commands": fix.commands }));
    let commands = fix.commands.clone();
    let admin = fix.admin;
    let result = tauri::async_runtime::spawn_blocking(move || run_fix(&commands, admin)).await.map_err(|e| e.to_string())?;
    state.log("security", if result.is_ok() { "security fix done" } else { "security fix failed" }, json!({ "fix": fix.id, "error": result.as_ref().err() }));
    result
}

fn run_fix(commands: &[String], admin: bool) -> CmdResult<()> {
    #[cfg(windows)]
    {
        let script = format!("$ErrorActionPreference = 'Stop'\n{}", commands.join("\n"));
        if !admin {
            return powershell(&script).map(|_| ());
        }
        // Start-Process -Verb RunAs shows the UAC prompt; declining it is an error.
        let outer = format!(
            "$p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-EncodedCommand','{}'; exit $p.ExitCode",
            encode_command(&script)
        );
        powershell(&outer).map(|_| ()).map_err(|e| {
            if e.to_lowercase().contains("cancel") {
                "Cancelled at the Windows administrator prompt. Nothing was changed.".into()
            } else {
                format!("The commands didn't finish: {e}")
            }
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (commands, admin);
        Err("Fixes can only run on Windows.".into())
    }
}
