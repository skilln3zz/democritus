//! Detects the local model server and starts it when it isn't running. A server
//! the app started is shut down when the app exits; one that was already
//! running (e.g. the Ollama tray app) is left alone.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::llm;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    /// connected | starting | disconnected
    pub state: &'static str,
    pub started_by_app: bool,
    pub base_url: String,
    pub error: Option<String>,
}

pub struct ServerManager {
    child: Mutex<Option<Child>>,
    starting: AtomicBool,
    last_error: Mutex<Option<String>>,
    log_path: PathBuf,
}

impl ServerManager {
    pub fn new(log_path: PathBuf) -> Self {
        Self { child: Mutex::new(None), starting: AtomicBool::new(false), last_error: Mutex::new(None), log_path }
    }

    fn child(&self) -> MutexGuard<'_, Option<Child>> {
        self.child.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether a server process this app spawned is still alive.
    pub fn started_by_app(&self) -> bool {
        let mut guard = self.child();
        let alive = matches!(guard.as_mut().map(|c| c.try_wait()), Some(Ok(None)));
        if !alive {
            *guard = None;
        }
        alive
    }

    pub async fn status(&self, http: &reqwest::Client, base: &str) -> ServerStatus {
        let connected = llm::probe(http, base).await;
        let state = if connected {
            "connected"
        } else if self.starting.load(Ordering::SeqCst) {
            "starting"
        } else {
            "disconnected"
        };
        let error = if connected { None } else { self.last_error.lock().unwrap_or_else(|p| p.into_inner()).clone() };
        ServerStatus { state, started_by_app: self.started_by_app(), base_url: base.to_string(), error }
    }

    /// Starts the server if nothing answers at `base`, then waits until it does.
    pub async fn ensure_running(&self, http: &reqwest::Client, base: &str, command: &str) -> Result<(), String> {
        if llm::probe(http, base).await {
            return Ok(());
        }
        if self.starting.swap(true, Ordering::SeqCst) {
            return Ok(()); // a start is already in progress
        }
        let result = self.start_and_wait(http, base, command).await;
        *self.last_error.lock().unwrap_or_else(|p| p.into_inner()) = result.as_ref().err().cloned();
        self.starting.store(false, Ordering::SeqCst);
        result
    }

    async fn start_and_wait(&self, http: &reqwest::Client, base: &str, command: &str) -> Result<(), String> {
        if !self.started_by_app() {
            self.spawn(base, command)?;
        }
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(400)).await;
            if llm::probe(http, base).await {
                return Ok(());
            }
            if let Some(code) = self.exit_code() {
                return Err(format!(
                    "The model server exited (code {code}) before it was ready. Another program may already be using the port. Log: {}",
                    self.log_path.display()
                ));
            }
        }
        Err(format!(
            "The model server didn't answer at {base} within {} seconds. Log: {}",
            STARTUP_TIMEOUT.as_secs(),
            self.log_path.display()
        ))
    }

    fn exit_code(&self) -> Option<String> {
        let mut guard = self.child();
        let status = guard.as_mut()?.try_wait().ok()??;
        *guard = None;
        Some(status.code().map_or_else(|| "unknown".to_string(), |c| c.to_string()))
    }

    fn spawn(&self, base: &str, command: &str) -> Result<(), String> {
        let parts = split_command(command);
        let (program, args) = parts.split_first().ok_or("The server start command in Settings is empty.")?;
        let program = resolve_program(program);

        let mut cmd = Command::new(&program);
        cmd.args(args).stdin(Stdio::null());
        match File::create(&self.log_path) {
            Ok(log) => {
                let out = log.try_clone().map(Stdio::from).unwrap_or_else(|_| Stdio::null());
                cmd.stdout(out).stderr(Stdio::from(log));
            }
            Err(_) => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        // Ollama reads its listen address from OLLAMA_HOST; keep it in sync with the URL.
        if is_ollama(&program) {
            if let Some(host_port) = host_port(base) {
                cmd.env("OLLAMA_HOST", host_port);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("Couldn't find \"{}\". Install Ollama, or change the server start command in Settings.", program.display())
            } else {
                format!("Couldn't start the model server ({}): {e}", program.display())
            }
        })?;
        *self.child() = Some(child);
        Ok(())
    }

    /// Stops the server only if this app started it.
    pub fn shutdown(&self) {
        if let Some(mut child) = self.child().take() {
            kill_tree(&mut child);
        }
    }
}

fn split_command(s: &str) -> Vec<String> {
    let (mut parts, mut cur, mut quoted, mut has) = (Vec::new(), String::new(), false, false);
    for ch in s.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                has = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has {
                    parts.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        parts.push(cur);
    }
    parts
}

fn is_ollama(program: &Path) -> bool {
    program.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("ollama"))
}

fn host_port(base: &str) -> Option<String> {
    let url = reqwest::Url::parse(base).ok()?;
    Some(format!("{}:{}", url.host_str()?, url.port_or_known_default()?))
}

/// Finds a bare program name on PATH, falling back to Ollama's default install
/// locations (GUI apps don't always inherit the shell's PATH).
pub(crate) fn resolve_program(name: &str) -> PathBuf {
    let given = PathBuf::from(name);
    if given.components().count() > 1 {
        return given;
    }
    let exe_names: Vec<String> =
        if cfg!(windows) && given.extension().is_none() { vec![format!("{name}.exe"), name.to_string()] } else { vec![name.to_string()] };
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for exe in &exe_names {
                let candidate = dir.join(exe);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    if is_ollama(&given) {
        let mut fallbacks: Vec<PathBuf> = Vec::new();
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            fallbacks.push(PathBuf::from(local).join("Programs").join("Ollama").join("ollama.exe"));
        }
        fallbacks.extend(
            ["/Applications/Ollama.app/Contents/Resources/ollama", "/opt/homebrew/bin/ollama", "/usr/local/bin/ollama", "/usr/bin/ollama"]
                .map(PathBuf::from),
        );
        if let Some(found) = fallbacks.into_iter().find(|p| p.is_file()) {
            return found;
        }
    }
    given
}

#[cfg(windows)]
fn kill_tree(child: &mut Child) {
    use std::os::windows::process::CommandExt;
    // /T also ends the model runner processes the server spawned.
    let _ = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(windows))]
fn kill_tree(child: &mut Child) {
    // SIGTERM lets the server stop its runners cleanly; force-kill if it lingers.
    let _ = Command::new("kill").args(["-TERM", &child.id().to_string()]).status();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_commands_with_quotes() {
        assert_eq!(split_command("ollama serve"), vec!["ollama", "serve"]);
        assert_eq!(
            split_command(r#""C:\Program Files\llama\llama-server.exe" -m "D:\m\qwen.gguf" --port 8080"#),
            vec![r"C:\Program Files\llama\llama-server.exe", "-m", r"D:\m\qwen.gguf", "--port", "8080"]
        );
        assert!(split_command("   ").is_empty());
    }

    #[test]
    fn derives_ollama_host_from_url() {
        assert_eq!(host_port("http://127.0.0.1:11434/v1").as_deref(), Some("127.0.0.1:11434"));
        assert_eq!(host_port("http://localhost/v1").as_deref(), Some("localhost:80"));
    }
}
