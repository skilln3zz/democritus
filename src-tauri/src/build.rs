//! "Build Democritus" (Step 6E): distils the teacher (a large local model)
//! into a small student of the operator's own, in six stages:
//!
//! 1. Preflight: permission, Python training tools, student weights, teacher,
//!    GPU, disk; exports the operator's examples (dataset.rs, Step 6D).
//! 2. Lessons: the operator's rated and edited answers, the self-description
//!    examples, and teacher answers to the operator's own questions (then
//!    built-in topics). Written as they come, so a pause loses nothing.
//! 3. Training: QLoRA in python/democritus_train.py (a child process at below-
//!    normal priority, killed with the app). Checkpoints, pause/resume, and an
//!    automatic pause above 83 °C.
//! 4. Packaging: merge, convert to an 8-bit GGUF (Q8_0, llama.cpp's
//!    converter), and create democritus:<size>-v<N> in Ollama. Ollama on
//!    Windows can neither import safetensors (that needs MLX) nor quantize a
//!    GGUF, so the converter does the quantizing; Q4_K_M would need
//!    llama.cpp's llama-quantize as well.
//! 5. Testing: test prompts the lessons never contained, answered by student
//!    and teacher, graded by the teacher; speed from Ollama's own timings.
//! 6. Done. The default brain never changes on its own.
//!
//! Everything stays on this computer: the worker runs with HF_HUB_OFFLINE=1,
//! and the only network traffic is to the local model server.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, Notify};

use crate::buildcore::*;
use crate::{dataset, db, home, identity, localtime, ollama, AppState};

type CmdResult<T> = Result<T, String>;

const WORKER: &str = include_str!("../python/democritus_train.py");
const WORKER_FILE: &str = "democritus_train.py";
const EXIT_PAUSED: i32 = 3;
const EXIT_CANCELLED: i32 = 4;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A student the app knows how to build: its folder under training\base, the
/// Ollama model it matches (for the chat template), and what it needs.
pub struct Student {
    pub size: &'static str,
    pub folder: &'static str,
    pub ollama_twin: &'static str,
    pub min_vram_gb: f64,
    pub min_disk_gb: f64,
}

pub const STUDENTS: &[Student] = &[Student { size: "4b", folder: "Qwen3.5-4B", ollama_twin: "qwen3.5:4b", min_vram_gb: 7.0, min_disk_gb: 30.0 }];

fn student(folder: &str) -> &'static Student {
    STUDENTS.iter().find(|s| s.folder == folder).unwrap_or(&STUDENTS[0])
}

// ---------- paths ----------

/// <home>	raining holds the builds; the tools (Python environment, student
/// weights, converter) are there too, unless DEMOCRITUS_TRAINING_TOOLS points
/// elsewhere (automated tests share the real tools but keep their own builds).
pub struct Paths {
    pub root: PathBuf,
    pub tools: PathBuf,
}

impl Paths {
    pub fn new(home: &Path) -> Self {
        let root = home::training_dir(home);
        let tools = std::env::var_os("DEMOCRITUS_TRAINING_TOOLS").map(PathBuf::from).unwrap_or_else(|| root.clone());
        Self { root, tools }
    }
    pub fn python(&self) -> PathBuf {
        self.tools.join("env").join("Scripts").join("python.exe")
    }
    pub fn base(&self, s: &Student) -> PathBuf {
        self.tools.join("base").join(s.folder)
    }
    pub fn toolkit(&self) -> PathBuf {
        self.tools.join("toolkit")
    }
    pub fn converter(&self) -> PathBuf {
        self.toolkit().join("llama.cpp").join("convert_hf_to_gguf.py")
    }
    pub fn builds(&self) -> PathBuf {
        self.root.join("builds")
    }
    pub fn build(&self, id: &str) -> PathBuf {
        self.builds().join(id)
    }
    /// The worker script, written from the copy built into the app.
    fn worker(&self) -> CmdResult<PathBuf> {
        let dir = self.toolkit();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(WORKER_FILE);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(WORKER) {
            std::fs::write(&path, WORKER).map_err(|e| format!("Couldn't write the training worker: {e}"))?;
        }
        Ok(path)
    }
}

// ---------- shared state ----------

#[derive(Debug, Clone, PartialEq)]
enum Request {
    Pause(String),
    Cancel,
}

#[derive(Default)]
pub struct BuildHub {
    current: Mutex<Option<Build>>,
    request: Mutex<Option<Request>>,
    wake: Notify,
    supervisor: AtomicBool,
    child_pid: Mutex<Option<u32>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn current(state: &AppState) -> Option<Build> {
    lock(&state.build.current).clone()
}

fn save(state: &AppState, b: &Build) {
    let dir = Paths::new(&state.home()).build(&b.id);
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join("build.json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(b).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join("build.json"));
    }
}

/// Changes the current build, saves it and tells the window.
fn update(app: &AppHandle, f: impl FnOnce(&mut Build)) -> Option<Build> {
    let state = app.state::<AppState>();
    let b = {
        let mut cur = lock(&state.build.current);
        let b = cur.as_mut()?;
        f(b);
        b.progress = b.percent();
        b.eta = if b.status == Status::Running { b.eta_seconds() } else { None };
        b.clone()
    };
    save(&state, &b);
    let _ = app.emit("build-changed", &b);
    Some(b)
}

fn note(app: &AppHandle, text: impl Into<String>, alert: bool) {
    let text = text.into();
    update(app, |b| {
        // Only the newest progress line is highlighted.
        if alert {
            b.log.iter_mut().for_each(|l| l.alert = false);
        }
        b.note(db::now_ms(), text, alert)
    });
}

fn take_request(state: &AppState) -> Option<Request> {
    lock(&state.build.request).take()
}

fn peek_request(state: &AppState) -> Option<Request> {
    lock(&state.build.request).clone()
}

/// Learning is paused by Lockdown and needs the Training permission.
fn blocked(state: &AppState) -> Option<String> {
    let op = state.operator();
    if op.lockdown {
        Some("lockdown".into())
    } else if !op.allow_training {
        Some("permission".into())
    } else {
        None
    }
}

// ---------- GPU, disk, keeping awake ----------

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStats {
    pub name: String,
    pub temp: u32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub util: u32,
}

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

pub fn gpu_stats() -> Option<GpuStats> {
    let mut cmd = Command::new("nvidia-smi");
    cmd.args(["--query-gpu=name,temperature.gpu,memory.used,memory.total,utilization.gpu", "--format=csv,noheader,nounits"]);
    let out = hidden(&mut cmd).output().ok().filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let parts: Vec<&str> = text.lines().next()?.split(',').map(str::trim).collect();
    let num = |i: usize| parts.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    Some(GpuStats { name: parts.first()?.to_string(), temp: num(1) as u32, mem_used: num(2) * 1024 * 1024, mem_total: num(3) * 1024 * 1024, util: num(4) as u32 })
}

#[cfg(windows)]
fn disk_free(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, free_to_caller: *mut u64, total: *mut u64, total_free: *mut u64) -> i32;
    }
    let mut p = path.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut free = 0u64;
    // SAFETY: a NUL-terminated path and valid out-pointers; the others may be null.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

#[cfg(not(windows))]
fn disk_free(_: &Path) -> Option<u64> {
    None
}

/// Keeps Windows from sleeping while a build runs (the display may still turn
/// off). The request belongs to one thread, which holds it until dropped.
struct KeepAwake(Arc<AtomicBool>);

impl KeepAwake {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(windows)]
        {
            let stop = stop.clone();
            std::thread::spawn(move || {
                #[link(name = "kernel32")]
                extern "system" {
                    fn SetThreadExecutionState(flags: u32) -> u32;
                }
                const ES_CONTINUOUS: u32 = 0x8000_0000;
                const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
                // SAFETY: plain flag calls on this thread.
                unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_secs(2));
                }
                unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            });
        }
        Self(stop)
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A Windows job object that ends the worker if Democritus itself ends, even
/// by a crash: the job closes with the app's process.
#[cfg(windows)]
fn tie_to_app(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        per_process_user_time: i64,
        per_job_user_time: i64,
        limit_flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attrs: *mut std::ffi::c_void, name: *const u16) -> isize;
        fn SetInformationJobObject(job: isize, class: u32, info: *mut std::ffi::c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: isize, process: isize) -> i32;
    }
    static JOB: OnceLock<isize> = OnceLock::new();
    let job = *JOB.get_or_init(|| {
        // SAFETY: documented calls with a correctly laid out JOBOBJECT_EXTENDED_LIMIT_INFORMATION.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job != 0 {
                let mut info = ExtendedLimits::default();
                info.basic.limit_flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                SetInformationJobObject(job, 9, &mut info as *mut _ as *mut std::ffi::c_void, std::mem::size_of::<ExtendedLimits>() as u32);
            }
            job
        }
    });
    if job != 0 {
        // SAFETY: the child's process handle is valid while `child` lives.
        unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as isize) };
    }
}

#[cfg(not(windows))]
fn tie_to_app(_: &Child) {}

fn kill_tree(pid: u32) {
    let mut cmd = Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null());
    let _ = hidden(&mut cmd).status();
}

// ---------- the worker process ----------

fn python_command(paths: &Paths, args: &[String]) -> CmdResult<Command> {
    let python = paths.python();
    if !python.exists() {
        return Err(format!("The training tools are not installed ({} is missing).", python.display()));
    }
    let mut cmd = Command::new(python);
    // -I: isolated mode, so nothing in the current folder is imported.
    cmd.arg("-I").args(args).stdin(Stdio::null());
    cmd.env("HF_HUB_OFFLINE", "1").env("TRANSFORMERS_OFFLINE", "1").env("HF_HUB_DISABLE_TELEMETRY", "1").env("PYTHONUNBUFFERED", "1");
    cmd.env("HF_HOME", paths.tools.join("cache").join("hf"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }
    Ok(cmd)
}

/// Runs the worker, forwarding its JSON lines; stderr goes to `log_file`.
/// Returns the exit code.
async fn run_worker(app: &AppHandle, mut cmd: Command, log_file: &Path, mut on_line: impl FnMut(&AppHandle, WorkerEvent), mut on_tick: impl FnMut(&AppHandle)) -> CmdResult<i32> {
    let log = std::fs::OpenOptions::new().create(true).append(true).open(log_file).map_err(|e| e.to_string())?;
    cmd.stdout(Stdio::piped()).stderr(Stdio::from(log));
    let mut child = cmd.spawn().map_err(|e| format!("Couldn't start the training worker: {e}"))?;
    tie_to_app(&child);
    let state = app.state::<AppState>();
    *lock(&state.build.child_pid) = Some(child.id());
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let stdout = child.stdout.take().ok_or("no worker output")?;
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            line = rx.recv() => match line {
                Some(l) => if let Some(ev) = parse_worker_line(&l) { on_line(app, ev) },
                None => break,
            },
            _ = tick.tick() => on_tick(app),
            _ = state.build.wake.notified() => on_tick(app),
        }
    }
    let code = tauri::async_runtime::spawn_blocking(move || child.wait().map(|s| s.code().unwrap_or(-1))).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    *lock(&state.build.child_pid) = None;
    Ok(code)
}

fn last_lines(path: &Path, n: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn write_control(dir: &Path, action: &str) {
    let _ = std::fs::write(dir.join("control.json"), json!({ "action": action }).to_string());
}

// ---------- Ollama helpers ----------

struct Answer {
    text: String,
    tokens_per_second: f64,
}

/// One non-streamed chat turn on the local server, thinking off.
async fn chat(state: &AppState, model: &str, messages: &[Value], temperature: f64, num_predict: u32, purpose: &str) -> CmdResult<Answer> {
    let base = state.settings().base_url;
    let url = format!("{}/api/chat", ollama::origin(&base));
    state.log("network", "local model request", json!({ "url": url, "model": model, "purpose": purpose }));
    let body = json!({ "model": model, "messages": messages, "stream": false, "think": false, "keep_alive": "10m",
                       "options": { "temperature": temperature, "num_predict": num_predict, "num_ctx": 4096 } });
    let resp = state.http.post(&url).header("content-type", "application/json").body(body.to_string()).timeout(Duration::from_secs(1800)).send().await.map_err(|e| format!("Couldn't reach the model server: {e}"))?;
    let status = resp.status();
    let v: Value = serde_json::from_slice(&resp.bytes().await.map_err(|e| e.to_string())?).unwrap_or_default();
    if !status.is_success() {
        return Err(v["error"].as_str().map(String::from).unwrap_or_else(|| format!("HTTP {status}")));
    }
    if v["done_reason"].as_str() == Some("length") {
        return Err(format!("{model}'s answer ran past {num_predict} tokens"));
    }
    let n = v["eval_count"].as_f64().unwrap_or(0.0);
    let ns = v["eval_duration"].as_f64().unwrap_or(0.0);
    Ok(Answer { text: v["message"]["content"].as_str().unwrap_or_default().trim().to_string(), tokens_per_second: if ns > 0.0 { n / (ns / 1e9) } else { 0.0 } })
}

async fn installed_models(state: &AppState) -> Vec<String> {
    let base = state.settings().base_url;
    let Ok(resp) = state.http.get(format!("{}/api/tags", ollama::origin(&base))).timeout(Duration::from_secs(5)).send().await else { return Vec::new() };
    let v: Value = resp.bytes().await.ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    v["models"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect()).unwrap_or_default()
}

async fn unload_all(state: &AppState) {
    let base = state.settings().base_url;
    if let Ok(loaded) = ollama::loaded(&state.http, &base).await {
        for m in loaded {
            let ok = ollama::unload(&state.http, &base, &m.name).await.is_ok();
            state.log("model", "unload", json!({ "model": m.name, "purpose": "free the GPU for training", "ok": ok }));
        }
    }
}

/// The Ollama command-line program (from the server start command, or PATH).
fn ollama_program(state: &AppState) -> String {
    let cmd = state.settings().server_command;
    let first = cmd.split_whitespace().next().unwrap_or("ollama").trim_matches('"').to_string();
    if first.to_lowercase().trim_end_matches(".exe").ends_with("ollama") {
        first
    } else {
        "ollama".into()
    }
}

// ---------- preflight ----------

fn check(id: &str, label: &str, ok: bool, detail: impl Into<String>) -> Check {
    Check { id: id.into(), label: label.into(), ok, detail: detail.into() }
}

/// What a build needs. `converter` is reported but not required here:
/// lessons and training work without it, packaging does not.
pub async fn preflight(state: &AppState, teacher: &str, s: &Student, test_run: bool) -> Vec<Check> {
    let paths = Paths::new(&state.home());
    let mut out = Vec::new();
    let op = state.operator();
    out.push(check("permission", "Training permission", op.allow_training && !op.lockdown, if op.lockdown {
        "Lockdown is on."
    } else if op.allow_training {
        "On (Settings & safety)."
    } else {
        "Turn on \"Allow fine-tuning\" under Settings & safety."
    }));

    let env = match paths.worker().and_then(|w| python_command(&paths, &[w.display().to_string(), "check".into()])) {
        Ok(mut cmd) => tauri::async_runtime::spawn_blocking(move || cmd.output()).await.ok().and_then(|r| r.ok()),
        Err(_) => None,
    };
    let report: Value = env.as_ref().and_then(|o| String::from_utf8_lossy(&o.stdout).lines().last().and_then(|l| serde_json::from_str(l).ok())).unwrap_or_default();
    out.push(if report["ok"] == true {
        check("tools", "Training tools", true, format!("PyTorch {} (CUDA {}), transformers {}, PEFT {}, bitsandbytes {}", report["torch"].as_str().unwrap_or("?"), report["cuda"].as_str().unwrap_or("?"), report["transformers"].as_str().unwrap_or("?"), report["peft"].as_str().unwrap_or("?"), report["bitsandbytes"].as_str().unwrap_or("?")))
    } else if paths.python().exists() {
        check("tools", "Training tools", false, report["error"].as_str().map(String::from).unwrap_or_else(|| "The training environment doesn't work (CUDA or Qwen3.5 support missing).".into()))
    } else {
        check("tools", "Training tools", false, format!("Not installed: {} is missing.", paths.python().display()))
    });

    let base = paths.base(s);
    let source: Value = std::fs::read_to_string(base.join("democritus-source.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    out.push(if source["verified"] == true {
        check("student", "Student weights", true, format!("{} at {} (checksums verified)", source["repo"].as_str().unwrap_or(s.folder), &source["revision"].as_str().unwrap_or("?")[..12.min(source["revision"].as_str().unwrap_or("?").len())]))
    } else {
        check("student", "Student weights", false, format!("Not found or not verified in {}", base.display()))
    });

    let models = installed_models(state).await;
    out.push(check("teacher", "Teacher model", models.iter().any(|m| m == teacher), if models.iter().any(|m| m == teacher) { format!("{teacher} is installed.") } else { format!("{teacher} is not installed in Ollama.") }));
    out.push(check("twin", "Chat template source", models.iter().any(|m| m == s.ollama_twin), format!("{} provides the chat template for the built model.", s.ollama_twin)));

    let gpu = tauri::async_runtime::spawn_blocking(gpu_stats).await.ok().flatten();
    out.push(match gpu {
        Some(g) => {
            let total = g.mem_total as f64 / 1024f64.powi(3);
            check("gpu", "GPU", total >= s.min_vram_gb + 1.0, format!("{}, {:.1} GB VRAM, {} °C (pauses at {PAUSE_AT} °C)", g.name, total, g.temp))
        }
        None => check("gpu", "GPU", false, "No NVIDIA GPU found (nvidia-smi didn't answer)."),
    });

    let need = if test_run { s.min_disk_gb * 0.7 } else { s.min_disk_gb };
    out.push(match disk_free(&paths.root) {
        Some(free) => {
            let gb = free as f64 / 1024f64.powi(3);
            check("disk", "Free disk space", gb >= need, format!("{gb:.0} GB free on the training drive; a build needs about {need:.0} GB."))
        }
        None => check("disk", "Free disk space", true, "Couldn't measure; make sure there are 30 GB free."),
    });

    let conv = paths.converter();
    out.push(check(
        "converter",
        "GGUF converter (for packaging)",
        conv.exists(),
        if conv.exists() { format!("{}", conv.display()) } else { "llama.cpp's convert_hf_to_gguf.py is not installed yet. Lessons and training work without it; packaging needs it.".to_string() },
    ));
    out
}

fn required_ok(checks: &[Check]) -> bool {
    checks.iter().filter(|c| c.id != "converter").all(|c| c.ok)
}

// ---------- the stages ----------

enum Flow {
    Next,
    Stop,
}

async fn stage_preflight(app: &AppHandle) -> CmdResult<Flow> {
    let state = app.state::<AppState>();
    let b = current(&state).ok_or("no build")?;
    let s = student(&b.student);
    update(app, |b| b.status_text = "Checking tools, weights, GPU and disk".into());
    let checks = preflight(&state, &b.teacher, s, b.test_run).await;
    let ok = required_ok(&checks);
    let failed: Vec<String> = checks.iter().filter(|c| !c.ok).map(|c| format!("{}: {}", c.label, c.detail)).collect();
    update(app, |b| b.preflight = checks);
    if !ok {
        return Err(format!("Preflight failed. {}", failed.join(" ")));
    }
    // The operator's examples, fresh (6D's export, not a second one).
    let t = localtime::now();
    let m = dataset::export(&state.db(), &state.home(), &t.stamp(), &t.datetime())?;
    state.log("training", "dataset exported", json!({ "file": m.file, "rated": m.rated, "edited": m.edited, "identity": m.identity, "for": b.name }));
    note(app, format!("preflight passed · {} examples from you, {} about Democritus", m.rated + m.edited, m.identity), false);
    Ok(Flow::Next)
}

/// The lesson plan, made once per build and saved, so a resumed build
/// continues exactly where it stopped.
fn make_plan(state: &AppState, b: &Build, use_history: bool) -> CmdResult<Vec<Value>> {
    let home = state.home();
    let settings = state.settings();
    let system = identity::system_prompt(&b.name, &settings.system_prompt);
    let sys = json!({ "role": "system", "content": system });
    let mut plan = Vec::new();
    for line in dataset::read_latest(&home).unwrap_or_default() {
        let mut msgs = vec![sys.clone()];
        let template = line["template"] == true;
        for m in line["messages"].as_array().cloned().unwrap_or_default() {
            let content = m["content"].as_str().unwrap_or_default();
            msgs.push(json!({ "role": m["role"], "content": if template { fill_model(content, &b.name) } else { content.to_string() } }));
        }
        let source = line["source"].as_str().unwrap_or("you");
        plan.push(json!({ "kind": "fixed", "source": if source == "identity" { "identity" } else { "you" }, "category": if source == "identity" { "identity" } else { "everyday" }, "messages": msgs }));
    }
    let eval: Vec<&str> = EVAL_PROMPTS.iter().map(|e| e.prompt).collect();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if use_history {
        let conn = state.db();
        let mut stmt = conn
            .prepare("SELECT content FROM messages WHERE role = 'user' AND length(content) BETWEEN 15 AND 2000 ORDER BY id DESC LIMIT 3000")
            .map_err(|e| e.to_string())?;
        let rows: Vec<String> = stmt.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
        for q in rows {
            let key = q.trim().to_lowercase();
            if !eval.contains(&q.trim()) && seen.insert(key) {
                plan.push(json!({ "kind": "teacher", "source": "teacher", "category": "yours", "prompt": q.trim() }));
            }
        }
    }
    // Built-in topics, repeated with a different angle if more are needed.
    let angles = ["", " Keep it brief.", " Explain your reasoning step by step.", " Answer for a beginner."];
    for angle in angles {
        for (cat, p) in SEED_PROMPTS {
            plan.push(json!({ "kind": "teacher", "source": "teacher", "category": cat, "prompt": format!("{p}{angle}") }));
        }
    }
    // Own examples first, then a mix; leave room for answers that get skipped.
    let keep = (b.lesson_target as f64 * 1.3).ceil() as usize + 10;
    plan.truncate(keep.max(b.lesson_target));
    Ok(plan)
}

async fn stage_lessons(app: &AppHandle) -> CmdResult<Flow> {
    let state = app.state::<AppState>();
    let b = current(&state).ok_or("no build")?;
    let dir = Paths::new(&state.home()).build(&b.id);
    let plan_file = dir.join("plan.json");
    let plan: Vec<Value> = match std::fs::read_to_string(&plan_file).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(p) => p,
        None => {
            let use_history = !std::fs::read_to_string(dir.join("options.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).is_some_and(|o| o["useHistory"] == false);
            let p = make_plan(&state, &b, use_history)?;
            std::fs::write(&plan_file, serde_json::to_string(&p).unwrap_or_default()).map_err(|e| e.to_string())?;
            p
        }
    };
    let lessons_file = dir.join("lessons.jsonl");
    let done_lines = std::fs::read_to_string(&lessons_file).map(|t| t.lines().filter(|l| !l.trim().is_empty()).count()).unwrap_or(0);
    // Which plan entries are already used (written or skipped) is saved too.
    let mut cursor: usize = std::fs::read_to_string(dir.join("plan-cursor.txt")).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(0);
    update(app, |b| {
        b.lessons_done = done_lines;
        b.status_text = format!("Writing lessons with {}", b.teacher);
    });
    let system = identity::system_prompt(&b.name, &state.settings().system_prompt);
    let mut written = done_lines;
    let mut from_you = b.lessons_from_you;
    while written < b.lesson_target {
        if let Some(r) = peek_request(&state) {
            return Ok(stop_for(app, r));
        }
        if let Some(reason) = blocked(&state) {
            return Ok(pause_now(app, &reason));
        }
        let Some(entry) = plan.get(cursor) else {
            note(app, format!("ran out of lesson topics at {written}; carrying on with what there is"), true);
            break;
        };
        cursor += 1;
        let started = Instant::now();
        let lesson = if entry["kind"] == "fixed" {
            if entry["source"] == "you" {
                from_you += 1;
            }
            Some(json!({ "source": entry["source"], "category": entry["category"], "messages": entry["messages"] }))
        } else {
            let prompt = entry["prompt"].as_str().unwrap_or_default();
            let messages = vec![json!({ "role": "system", "content": system }), json!({ "role": "user", "content": prompt })];
            match chat(&state, &b.teacher, &messages, 0.7, 1500, "build lessons").await {
                Ok(a) if !a.text.is_empty() => {
                    let mut msgs = messages.clone();
                    msgs.push(json!({ "role": "assistant", "content": a.text }));
                    Some(json!({ "source": "teacher", "category": entry["category"], "messages": msgs }))
                }
                Ok(_) => None,
                Err(e) if e.contains("ran past") => None,
                Err(e) => return Err(format!("The teacher couldn't answer: {e}")),
            }
        };
        let _ = std::fs::write(dir.join("plan-cursor.txt"), cursor.to_string());
        let Some(lesson) = lesson else { continue };
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&lessons_file).map_err(|e| e.to_string())?;
        writeln!(f, "{lesson}").map_err(|e| e.to_string())?;
        written += 1;
        let secs = started.elapsed().as_secs_f64();
        let teacher_lesson = entry["kind"] != "fixed";
        update(app, |b| {
            b.lessons_done = written;
            b.lessons_from_you = from_you;
            if teacher_lesson {
                b.seconds_per_lesson = Some(b.seconds_per_lesson.map_or(secs, |s| s * 0.8 + secs * 0.2));
            }
            b.status_text = format!("Lesson {written} of {}", b.lesson_target);
        });
        if written % 10 == 0 || written == b.lesson_target {
            note(app, format!("{written} lessons written"), true);
        }
    }
    update(app, |b| b.lesson_target = b.lesson_target.min(written).max(1));
    let b = current(&state).ok_or("no build")?;
    let (steps, accum) = if b.test_run { plan_steps(written, true) } else { plan_steps(written, false) };
    update(app, |b| {
        b.steps = steps;
        b.grad_accum = accum;
    });
    state.log("training", "lessons finished", json!({ "build": b.name, "lessons": written, "fromYou": from_you }));
    note(app, format!("lessons finished · {written} examples ({from_you} from you)"), false);
    Ok(Flow::Next)
}

/// Stops at a pause or cancel request; returns how the stage ended.
fn stop_for(app: &AppHandle, r: Request) -> Flow {
    match r {
        Request::Pause(reason) => pause_now(app, &reason),
        Request::Cancel => {
            let state = app.state::<AppState>();
            take_request(&state);
            update(app, |b| {
                b.status = Status::Cancelled;
                b.status_text = "Cancelled".into();
                b.finished_at = Some(db::now_ms());
            });
            note(app, "cancelled by you", true);
            state.log("training", "build cancelled", json!({}));
            Flow::Stop
        }
    }
}

fn pause_now(app: &AppHandle, reason: &str) -> Flow {
    let state = app.state::<AppState>();
    take_request(&state);
    // What is safe on disk depends on the stage: lessons are written one by
    // one, training keeps a checkpoint.
    let saved = match current(&state).map(|b| b.stage) {
        Some(Stage::Training) => "checkpoint saved",
        Some(Stage::Lessons) => "lessons so far are saved",
        _ => "progress saved",
    };
    let text = match reason {
        "heat" => format!("Paused: the GPU is too warm. It carries on below {RESUME_BELOW} °C."),
        "lockdown" => format!("Paused by Lockdown · {saved}"),
        "permission" => format!("Paused: the training permission is off · {saved}"),
        "app closed" => format!("Paused when Democritus closed · {saved}"),
        _ => format!("Paused by you · {saved} · GPU free for chat"),
    };
    update(app, |b| {
        b.status = Status::Paused;
        b.pause_reason = Some(reason.to_string());
        b.status_text = text.clone();
    });
    note(app, text.to_lowercase(), true);
    state.log("training", "build paused", json!({ "reason": reason }));
    Flow::Stop
}

async fn stage_training(app: &AppHandle) -> CmdResult<Flow> {
    let state = app.state::<AppState>();
    loop {
        let b = current(&state).ok_or("no build")?;
        let paths = Paths::new(&state.home());
        let dir = paths.build(&b.id);
        let s = student(&b.student);
        update(app, |b| b.status_text = "Freeing the GPU".into());
        unload_all(&state).await;
        write_control(&dir, "run");
        let resume = dir.join("checkpoint").join("trainer_state.pt").exists();
        let worker = paths.worker()?;
        let args: Vec<String> = vec![
            worker.display().to_string(),
            "train".into(),
            "--base".into(),
            paths.base(s).display().to_string(),
            "--lessons".into(),
            dir.join("lessons.jsonl").display().to_string(),
            "--out".into(),
            dir.display().to_string(),
            "--steps".into(),
            b.steps.to_string(),
            "--grad-accum".into(),
            b.grad_accum.to_string(),
            "--checkpoint-every".into(),
            if b.test_run { "2".into() } else { "25".into() },
        ];
        let mut args = args;
        if resume {
            args.push("--resume".into());
        }
        let cmd = python_command(&paths, &args)?;
        update(app, |b| b.status_text = if resume { "Resuming from the last checkpoint".into() } else { "Loading the student".into() });
        state.log("training", "training started", json!({ "build": b.name, "steps": b.steps, "resume": resume }));
        let heat_pause = Arc::new(AtomicBool::new(false));
        let heat = heat_pause.clone();
        let control_dir = dir.clone();
        let code = run_worker(
            app,
            cmd,
            &dir.join("worker.log"),
            |app, ev| match ev {
                WorkerEvent::Stage(t) => {
                    update(app, |b| b.status_text = t);
                }
                WorkerEvent::Started { lessons, steps } => note(app, format!("training started · {} · QLoRA rank 16 · {lessons} lessons, {steps} steps", student(&b.student).ollama_twin), false),
                WorkerEvent::Resumed(step) => note(app, format!("resumed at step {step}"), false),
                WorkerEvent::Step { step, total, loss, seconds, vram_peak } => {
                    let temp = gpu_stats().map(|g| g.temp).unwrap_or(0);
                    let b2 = update(app, |b| {
                        b.step = step;
                        b.steps = total;
                        if loss.is_finite() {
                            b.loss_start.get_or_insert(loss);
                            b.loss = Some(b.loss.map_or(loss, |l| l * 0.7 + loss * 0.3));
                        }
                        // The first step includes loading; leave it out of the average.
                        if step > 1 || b.seconds_per_step.is_none() {
                            b.seconds_per_step = Some(b.seconds_per_step.map_or(seconds, |s| s * 0.8 + seconds * 0.2));
                        }
                        b.vram_peak = Some(b.vram_peak.unwrap_or(0).max(vram_peak));
                        b.status_text = format!("Stage 3 of 6 · training the student · step {step} of {total}");
                    });
                    if let Some(b2) = b2 {
                        if step == 1 || step % 10 == 0 || step == total {
                            note(app, format!("step {step} · loss {:.2} · {temp}°C", b2.loss.unwrap_or(loss)), true);
                        }
                    }
                }
                WorkerEvent::Checkpoint(step) => {
                    update(app, |b| b.checkpoint_at = Some(db::now_ms()));
                    note(app, format!("checkpoint {step} saved"), false);
                }
                WorkerEvent::Error(e) => note(app, format!("worker error: {e}"), true),
                _ => {}
            },
            |app| {
                let state = app.state::<AppState>();
                if let Some(g) = gpu_stats() {
                    if thermal(g.temp, false) == Thermal::Pause && !heat.swap(true, Ordering::SeqCst) {
                        note(app, format!("GPU at {}°C: pausing at the next step", g.temp), true);
                        write_control(&control_dir, "pause");
                    }
                }
                match peek_request(&state) {
                    Some(Request::Cancel) => write_control(&control_dir, "cancel"),
                    Some(Request::Pause(_)) => write_control(&control_dir, "pause"),
                    None => {}
                }
                if let Some(reason) = blocked(&state) {
                    lock(&state.build.request).get_or_insert(Request::Pause(reason));
                    write_control(&control_dir, "pause");
                }
            },
        )
        .await?;
        match code {
            0 => break,
            EXIT_CANCELLED => return Ok(stop_for(app, Request::Cancel)),
            EXIT_PAUSED if heat_pause.load(Ordering::SeqCst) && peek_request(&state).is_none() => {
                // Too warm: wait until it has cooled, then carry on by itself.
                pause_now(app, "heat");
                let cooled = wait_cool(app).await;
                if !cooled {
                    return Ok(Flow::Stop);
                }
                update(app, |b| {
                    b.status = Status::Running;
                    b.pause_reason = None;
                });
                note(app, "GPU cooled down: carrying on", true);
                continue;
            }
            EXIT_PAUSED => {
                let r = take_request(&state).unwrap_or(Request::Pause("you".into()));
                let reason = match r {
                    Request::Pause(r) => r,
                    Request::Cancel => return Ok(stop_for(app, Request::Cancel)),
                };
                return Ok(pause_now(app, &reason));
            }
            other => {
                let detail = last_lines(&dir.join("worker.log"), 6);
                return Err(format!("The training worker stopped with code {other}. {}", detail.lines().last().unwrap_or("")));
            }
        }
    }
    let b = current(&state).ok_or("no build")?;
    state.log("training", "training finished", json!({ "build": b.name, "steps": b.step, "loss": b.loss }));
    note(app, format!("training finished · loss {:.2} (from {:.2})", b.loss.unwrap_or(0.0), b.loss_start.unwrap_or(0.0)), false);
    Ok(Flow::Next)
}

/// Waits until the GPU is cool enough; false if cancelled or paused meanwhile.
async fn wait_cool(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    loop {
        if peek_request(&state).is_some() || blocked(&state).is_some() {
            return false;
        }
        let temp = tauri::async_runtime::spawn_blocking(gpu_stats).await.ok().flatten().map(|g| g.temp).unwrap_or(0);
        if thermal(temp, true) == Thermal::Resume {
            return true;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(15)) => {}
            _ = state.build.wake.notified() => {}
        }
    }
}

/// Runs a blocking program to the end, logging to `log`.
async fn run_to_end(mut cmd: Command, log: &Path) -> CmdResult<()> {
    let file = std::fs::OpenOptions::new().create(true).append(true).open(log).map_err(|e| e.to_string())?;
    let err = file.try_clone().map_err(|e| e.to_string())?;
    cmd.stdout(Stdio::from(file)).stderr(Stdio::from(err));
    let log = log.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        tie_to_app(&child);
        let status = child.wait().map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("exit code {:?}: {}", status.code(), last_lines(&log, 3).lines().last().unwrap_or("")))
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn stage_packaging(app: &AppHandle) -> CmdResult<Flow> {
    let state = app.state::<AppState>();
    let b = current(&state).ok_or("no build")?;
    let paths = Paths::new(&state.home());
    let dir = paths.build(&b.id);
    let s = student(&b.student);
    let log = dir.join("packaging.log");
    let merged = dir.join("merged");
    let gguf = dir.join("model-q8_0.gguf");
    if !gguf.exists() {
        if !merged.join("config.json").exists() {
            update(app, |b| b.status_text = "Stage 4 of 6 · merging the adapter into the student".into());
            let worker = paths.worker()?;
            let cmd = python_command(&paths, &[worker.display().to_string(), "merge".into(), "--base".into(), paths.base(s).display().to_string(), "--adapter".into(), dir.join("adapter").display().to_string(), "--out".into(), merged.display().to_string()])?;
            run_to_end(cmd, &log).await.map_err(|e| format!("Merging failed: {e}"))?;
            note(app, "adapter merged into the student", false);
        }
        let converter = paths.converter();
        if !converter.exists() {
            return Err("Packaging needs llama.cpp's GGUF converter, which is not installed (it is a download you haven't approved yet). Training finished and is kept: after installing the converter, press Resume to package this build.".into());
        }
        update(app, |b| b.status_text = "Stage 4 of 6 · converting to GGUF (8-bit)".into());
        let worker = paths.worker()?;
        let llama = paths.toolkit().join("llama.cpp");
        let cmd = python_command(&paths, &[worker.display().to_string(), "convert".into(), "--llama".into(), llama.display().to_string(), "--model".into(), merged.display().to_string(), "--outfile".into(), gguf.display().to_string(), "--outtype".into(), "q8_0".into()])?;
        run_to_end(cmd, &log).await.map_err(|e| format!("Converting to GGUF failed: {e}"))?;
        let gb = std::fs::metadata(&gguf).map(|m| m.len() as f64 / 1e9).unwrap_or(0.0);
        note(app, format!("converted to GGUF (Q8_0, {gb:.1} GB)"), false);
    }
    if peek_request(&state).is_some() {
        return Ok(stop_for(app, take_request(&state).unwrap_or(Request::Pause("you".into()))));
    }
    // The chat template and settings of the Ollama model the student matches.
    let base_url = state.settings().base_url;
    let resp = state.http.post(format!("{}/api/show", ollama::origin(&base_url))).header("content-type", "application/json").body(json!({ "model": s.ollama_twin }).to_string()).send().await.map_err(|e| e.to_string())?;
    let show: Value = serde_json::from_slice(&resp.bytes().await.map_err(|e| e.to_string())?).unwrap_or_default();
    let mf = modelfile(&gguf.display().to_string(), show["modelfile"].as_str().unwrap_or_default());
    std::fs::write(dir.join("Modelfile"), &mf).map_err(|e| e.to_string())?;
    update(app, |b| b.status_text = format!("Stage 4 of 6 · creating {} in Ollama", b.name));
    let mut cmd = Command::new(ollama_program(&state));
    cmd.args(["create", &b.name, "-f"]).arg(dir.join("Modelfile")).stdin(Stdio::null());
    hidden(&mut cmd);
    state.log("model", "create built model", json!({ "model": b.name, "from": gguf.display().to_string(), "quantization": "Q8_0" }));
    run_to_end(cmd, &log).await.map_err(|e| format!("Ollama couldn't create {}: {e}", b.name))?;
    // Make sure it loads and answers before the intermediate files go.
    update(app, |b| b.status_text = format!("Stage 4 of 6 · checking that {} answers", b.name));
    let probe = vec![json!({ "role": "user", "content": "Say hello in three words." })];
    chat(&state, &b.name, &probe, 0.3, 64, "build check (student loads)").await.map_err(|e| format!("{} was created but doesn't answer: {e}", b.name))?;
    // The big intermediate files are no longer needed (the adapter is kept).
    let _ = std::fs::remove_dir_all(&merged);
    let _ = std::fs::remove_file(&gguf);
    note(app, format!("packaged as {}", b.name), false);
    state.log("training", "build packaged", json!({ "model": b.name }));
    Ok(Flow::Next)
}

#[derive(Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TestItem {
    pub category: String,
    pub prompt: String,
    pub student: String,
    pub teacher: String,
    pub student_score: f64,
    pub teacher_score: f64,
}

async fn stage_testing(app: &AppHandle) -> CmdResult<Flow> {
    let state = app.state::<AppState>();
    let b = current(&state).ok_or("no build")?;
    let dir = Paths::new(&state.home()).build(&b.id);
    let custom = state.settings().system_prompt;
    let prompts = eval_prompts(b.test_run);
    let ask = |model: &str, p: &str| vec![json!({ "role": "system", "content": identity::system_prompt(model, &custom) }), json!({ "role": "user", "content": p })];
    // Student answers first, then the teacher's: one model switch instead of many.
    let mut items = Vec::new();
    let (mut s_tps, mut t_tps) = (Vec::new(), Vec::new());
    for (i, e) in prompts.iter().enumerate() {
        update(app, |b| b.status_text = format!("Stage 5 of 6 · {} answers test {} of {}", b.name, i + 1, prompts.len()));
        let a = match chat(&state, &b.name, &ask(&b.name, e.prompt), 0.3, 1200, "build test (student)").await {
            Ok(a) => a,
            Err(err) if err.contains("ran past") => Answer { text: String::new(), tokens_per_second: 0.0 },
            Err(err) => return Err(format!("{} couldn't answer the tests: {err}", b.name)),
        };
        s_tps.push(a.tokens_per_second);
        items.push(TestItem { category: e.category.into(), prompt: e.prompt.into(), student: a.text, teacher: String::new(), student_score: 0.0, teacher_score: 0.0 });
    }
    for (i, e) in prompts.iter().enumerate() {
        if peek_request(&state) == Some(Request::Cancel) {
            return Ok(stop_for(app, Request::Cancel));
        }
        update(app, |b| b.status_text = format!("Stage 5 of 6 · {} answers and grades test {} of {}", b.teacher, i + 1, prompts.len()));
        let a = chat(&state, &b.teacher, &ask(&b.teacher, e.prompt), 0.3, 1200, "build test (teacher)").await.unwrap_or(Answer { text: String::new(), tokens_per_second: 0.0 });
        t_tps.push(a.tokens_per_second);
        items[i].teacher = a.text;
        let schema: Value = serde_json::from_str(JUDGE_SCHEMA).unwrap_or_default();
        for (who, answer) in [("student", items[i].student.clone()), ("teacher", items[i].teacher.clone())] {
            let score = if answer.is_empty() {
                0.0
            } else {
                let msgs = vec![json!({ "role": "user", "content": judge_prompt(e.category, e.rubric, e.prompt, &answer) })];
                state.log("network", "local model request", json!({ "model": b.teacher, "purpose": "build test (grading)" }));
                let judged = ollama::chat_json(&state.http, &state.settings().base_url, &b.teacher, &msgs, &schema, Some(4096), 400, "10m").await;
                let mut s = judged.ok().and_then(|v| v["score"].as_f64()).unwrap_or(0.0).clamp(0.0, 10.0);
                if e.category == "identity" {
                    let model = if who == "student" { &b.name } else { &b.teacher };
                    s = s * 0.5 + identity_check(&answer, model) * 5.0;
                }
                s
            };
            if who == "student" {
                items[i].student_score = score;
            } else {
                items[i].teacher_score = score;
            }
        }
    }
    let avg = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let cat = |c: &str| {
        let s: Vec<f64> = items.iter().filter(|i| i.category == c).map(|i| i.student_score).collect();
        let t: Vec<f64> = items.iter().filter(|i| i.category == c).map(|i| i.teacher_score).collect();
        relative_score(avg(&s), avg(&t))
    };
    let mut scores = Scores {
        identity: cat("identity"),
        rules: cat("rules"),
        everyday: cat("everyday"),
        coding: cat("coding"),
        student_tps: (avg(&s_tps) * 10.0).round() / 10.0,
        teacher_tps: (avg(&t_tps) * 10.0).round() / 10.0,
        speedup: 0.0,
        tested_at: db::now_ms(),
    };
    scores.speedup = (scores.speedup() * 10.0).round() / 10.0;
    std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&json!({ "scores": scores, "items": items })).unwrap_or_default()).map_err(|e| e.to_string())?;
    state.log("training", "build tested", json!({ "model": b.name, "scores": scores }));
    note(app, format!("tested · knows who it is {}% · rules {}% · everyday {}% · coding {}% · {:.0} tok/s", scores.identity, scores.rules, scores.everyday, scores.coding, scores.student_tps), false);
    update(app, |b| b.scores = Some(scores));
    Ok(Flow::Next)
}

/// Runs the build from its current stage until it finishes, pauses or fails.
async fn supervise(app: AppHandle) {
    let state = app.state::<AppState>();
    if state.build.supervisor.swap(true, Ordering::SeqCst) {
        return;
    }
    let _awake = KeepAwake::start();
    loop {
        let Some(b) = current(&state) else { break };
        if b.status != Status::Running {
            break;
        }
        let result = match b.stage {
            Stage::Preflight => stage_preflight(&app).await,
            Stage::Lessons => stage_lessons(&app).await,
            Stage::Training => stage_training(&app).await,
            Stage::Packaging => stage_packaging(&app).await,
            Stage::Testing => stage_testing(&app).await,
            Stage::Done => break,
        };
        match result {
            Ok(Flow::Next) => {
                let next = b.stage.next();
                update(&app, |b| {
                    b.stage = next;
                    if next == Stage::Done {
                        b.status = Status::Done;
                        b.finished_at = Some(db::now_ms());
                        b.status_text = format!("{} is ready", b.name);
                    }
                });
                if next == Stage::Done {
                    let b = current(&state);
                    note(&app, format!("done · {} is in the brain picker", b.as_ref().map(|b| b.name.as_str()).unwrap_or("")), false);
                    state.log("training", "build finished", json!({ "model": b.map(|b| b.name) }));
                }
            }
            Ok(Flow::Stop) => break,
            Err(e) => {
                update(&app, |b| {
                    b.status = Status::Failed;
                    b.error = Some(e.clone());
                    b.status_text = "Stopped: something went wrong".into();
                });
                note(&app, format!("failed: {e}"), true);
                state.log("training", "build failed", json!({ "error": e }));
                break;
            }
        }
    }
    state.build.supervisor.store(false, Ordering::SeqCst);
}

// ---------- startup, exit, safety hooks ----------

fn load_builds(home: &Path) -> Vec<Build> {
    let mut out: Vec<Build> = std::fs::read_dir(Paths::new(home).builds())
        .map(|rd| rd.filter_map(Result::ok).filter_map(|e| std::fs::read_to_string(e.path().join("build.json")).ok()).filter_map(|t| serde_json::from_str(&t).ok()).collect())
        .unwrap_or_default();
    out.sort_by_key(|b: &Build| b.created_at);
    out
}

/// At startup: a build that was running when the app closed is shown as
/// paused; a paused or stopped one comes back as it was, ready to resume.
pub fn restore(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Some(mut b) = load_builds(&state.home()).into_iter().next_back().filter(|b| matches!(b.status, Status::Running | Status::Paused | Status::Failed)) else { return };
    if b.status == Status::Running {
        b.status = Status::Paused;
        b.pause_reason = Some("app closed".into());
        b.status_text = format!("Paused when Democritus closed · {}", if b.stage == Stage::Training { "resumes from the last checkpoint" } else { "progress saved" });
        save(&state, &b);
    }
    *lock(&state.build.current) = Some(b);
}

/// On exit: ask the worker to checkpoint, then make sure it is gone.
pub fn on_exit(state: &AppState) {
    let Some(pid) = *lock(&state.build.child_pid) else { return };
    if let Some(b) = current(state) {
        write_control(&Paths::new(&state.home()).build(&b.id), "pause");
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && lock(&state.build.child_pid).is_some() {
        std::thread::sleep(Duration::from_millis(250));
    }
    if lock(&state.build.child_pid).is_some() {
        kill_tree(pid);
    }
    let mut cur = lock(&state.build.current);
    if let Some(b) = cur.as_mut().filter(|b| b.status == Status::Running) {
        b.status = Status::Paused;
        b.pause_reason = Some("app closed".into());
        b.status_text = format!("Paused when Democritus closed · {}", if b.stage == Stage::Training { "resumes from the last checkpoint" } else { "progress saved" });
        let b = b.clone();
        drop(cur);
        save(state, &b);
    }
}

/// Lockdown or the permission switched off pauses a running build.
pub fn pause_for(state: &AppState, reason: &str) {
    if current(state).is_some_and(|b| b.status == Status::Running) {
        lock(&state.build.request).get_or_insert(Request::Pause(reason.into()));
        state.build.wake.notify_one();
    }
}

// ---------- commands ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildSummary {
    pub id: String,
    pub name: String,
    pub test_run: bool,
    pub teacher: String,
    pub finished_at: Option<i64>,
    pub scores: Option<Scores>,
    pub lessons: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub current: Option<Build>,
    pub finished: Vec<BuildSummary>,
    pub training_dir: String,
    pub students: Vec<String>,
    pub default_teacher: String,
}

#[tauri::command]
pub async fn build_overview(state: State<'_, AppState>) -> CmdResult<Overview> {
    let home = state.home();
    let finished = load_builds(&home)
        .into_iter()
        .filter(|b| b.status == Status::Done)
        .rev()
        .map(|b| BuildSummary { id: b.id, name: b.name, test_run: b.test_run, teacher: b.teacher, finished_at: b.finished_at, scores: b.scores, lessons: b.lesson_target })
        .collect();
    let models = installed_models(&state).await;
    let default_teacher = ["qwen3.8:27b", "qwen3-coder:latest", "qwen3.5:9b"].into_iter().find(|m| models.iter().any(|x| x == m)).unwrap_or("qwen3.8:27b").to_string();
    Ok(Overview { current: current(&state), finished, training_dir: Paths::new(&home).root.display().to_string(), students: STUDENTS.iter().map(|s| s.size.to_string()).collect(), default_teacher })
}

#[tauri::command]
pub async fn build_preflight(state: State<'_, AppState>, teacher: String, test_run: bool) -> CmdResult<Vec<Check>> {
    Ok(preflight(&state, &teacher, &STUDENTS[0], test_run).await)
}

#[tauri::command]
pub async fn build_start(app: AppHandle, state: State<'_, AppState>, teacher: String, test_run: bool, lessons: usize, use_history: bool) -> CmdResult<Build> {
    if let Some(reason) = blocked(&state) {
        return Err(if reason == "lockdown" { "Lockdown is on, so learning is paused.".into() } else { "Turn on \"Allow fine-tuning\" under Settings & safety first.".into() });
    }
    if current(&state).is_some_and(|b| matches!(b.status, Status::Running | Status::Paused)) {
        return Err("A build is already in progress. Resume or cancel it first.".into());
    }
    let s = &STUDENTS[0];
    let lessons = if test_run { 20 } else { lessons.clamp(50, 5000) };
    let name = if test_run { test_name(s.size) } else { version_name(&installed_models(&state).await, s.size) };
    let t = localtime::now();
    let b = Build::new(format!("b{}", t.stamp()), name, test_run, s.folder.into(), teacher, lessons, db::now_ms());
    let dir = Paths::new(&state.home()).build(&b.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("options.json"), json!({ "useHistory": use_history }).to_string()).map_err(|e| e.to_string())?;
    take_request(&state);
    *lock(&state.build.current) = Some(b.clone());
    save(&state, &b);
    state.log("training", "build started", json!({ "model": b.name, "testRun": test_run, "teacher": b.teacher, "lessons": lessons, "useHistory": use_history }));
    note(&app, format!("build started · {} from {} · {} lessons{}", b.name, b.teacher, lessons, if test_run { " (test run)" } else { "" }), false);
    tauri::async_runtime::spawn(supervise(app.clone()));
    Ok(current(&state).unwrap_or(b))
}

#[tauri::command]
pub fn build_pause(state: State<AppState>) -> CmdResult<()> {
    if !current(&state).is_some_and(|b| b.status == Status::Running) {
        return Err("No build is running.".into());
    }
    lock(&state.build.request).replace(Request::Pause("you".into()));
    state.build.wake.notify_one();
    state.log("training", "pause requested", json!({}));
    Ok(())
}

#[tauri::command]
pub fn build_resume(app: AppHandle, state: State<AppState>) -> CmdResult<()> {
    if let Some(reason) = blocked(&state) {
        return Err(if reason == "lockdown" { "Lockdown is on, so learning is paused.".into() } else { "Turn on \"Allow fine-tuning\" under Settings & safety first.".into() });
    }
    let b = current(&state).ok_or("There is no build to resume.")?;
    if !matches!(b.status, Status::Paused | Status::Failed) {
        return Err("This build isn't paused.".into());
    }
    if state.build.supervisor.load(Ordering::SeqCst) {
        // Still winding down (e.g. waiting to cool): just clear the pause.
        take_request(&state);
    }
    take_request(&state);
    update(&app, |b| {
        b.status = Status::Running;
        b.pause_reason = None;
        b.error = None;
        b.status_text = "Resuming".into();
    });
    note(&app, "resumed by you", false);
    state.log("training", "build resumed", json!({ "model": b.name, "stage": b.stage.label() }));
    tauri::async_runtime::spawn(supervise(app.clone()));
    Ok(())
}

#[tauri::command]
pub fn build_cancel(app: AppHandle, state: State<AppState>) -> CmdResult<()> {
    let b = current(&state).ok_or("There is no build.")?;
    match b.status {
        Status::Running => {
            lock(&state.build.request).replace(Request::Cancel);
            state.build.wake.notify_one();
        }
        Status::Paused | Status::Failed => {
            update(&app, |b| {
                b.status = Status::Cancelled;
                b.status_text = "Cancelled".into();
                b.finished_at = Some(db::now_ms());
            });
            note(&app, "cancelled by you", true);
            state.log("training", "build cancelled", json!({ "model": b.name }));
        }
        _ => return Err("This build has already ended.".into()),
    }
    Ok(())
}

/// Hides a finished, failed or cancelled build from the status panel (its
/// files stay in the builds folder).
#[tauri::command]
pub fn build_dismiss(state: State<AppState>) -> CmdResult<()> {
    let mut cur = lock(&state.build.current);
    if cur.as_ref().is_some_and(|b| matches!(b.status, Status::Running | Status::Paused)) {
        return Err("Cancel the build first.".into());
    }
    *cur = None;
    Ok(())
}

#[tauri::command]
pub async fn build_gpu() -> Option<GpuStats> {
    tauri::async_runtime::spawn_blocking(gpu_stats).await.ok().flatten()
}

/// The test answers of a build, student beside teacher ("Compare answers").
#[tauri::command]
pub fn build_report(state: State<AppState>, id: String) -> CmdResult<Value> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("Unknown build.".into());
    }
    let path = Paths::new(&state.home()).build(&id).join("report.json");
    let text = std::fs::read_to_string(&path).map_err(|_| "This build has no test report yet.".to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Makes a built model the default brain: only ever on the operator's click.
#[tauri::command]
pub fn build_make_default(state: State<AppState>, model: String) -> CmdResult<()> {
    if !model.starts_with("democritus:") {
        return Err("Only a built Democritus model can be made the default here.".into());
    }
    let mut s = state.settings();
    let before = s.default_model.clone();
    s.default_model = model.clone();
    db::save_settings(&state.db(), &s).map_err(|e| e.to_string())?;
    state.log("setting", "default brain changed", json!({ "from": before, "to": model }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_script_is_built_in_and_offline() {
        assert!(WORKER.contains("HF_HUB_OFFLINE"));
        assert!(WORKER.contains("def train(") && WORKER.contains("def merge(") && WORKER.contains("def check(") && WORKER.contains("def convert("));
    }

    #[test]
    fn preflight_requires_everything_but_the_converter() {
        let mk = |id: &str, ok: bool| check(id, id, ok, "");
        assert!(required_ok(&[mk("tools", true), mk("converter", false)]));
        assert!(!required_ok(&[mk("tools", false), mk("converter", true)]));
    }

    #[test]
    fn builds_are_saved_and_found_again() {
        let home = db::test_dir("builds");
        let paths = Paths::new(&home);
        let mut b = Build::new("b20261017-120000".into(), "democritus:4b-test".into(), true, "Qwen3.5-4B".into(), "qwen3.8:27b".into(), 20, 1);
        b.status = Status::Done;
        std::fs::create_dir_all(paths.build(&b.id)).unwrap();
        std::fs::write(paths.build(&b.id).join("build.json"), serde_json::to_string(&b).unwrap()).unwrap();
        let found = load_builds(&home);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "democritus:4b-test");
        assert!(paths.worker().unwrap().ends_with("toolkit\\democritus_train.py") || paths.worker().unwrap().ends_with("toolkit/democritus_train.py"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
