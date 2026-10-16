# Democritus: handoff

Status as of 2026-10-16. Project: `D:\democritus-app` (Tauri 2 + Rust backend + TypeScript UI, Ollama as the model server).

The original app is kept untouched as a backup in `D:\local-qwen-desktop`. **Do not delete it until the operator says so.**

## Build order and status

| Step | What | Status |
|---|---|---|
| Audit | Compare the existing app against the spec | Done; operator approved the plan |
| 1 | Identity and rename | **Done** (commit `3868323`), except the icon (see Known issues) |
| 4 | Model memory management | **Done** (commit `fc98a8b`) |
| 7 | Safety rules | **Mostly done** (commit "Handoff"); open items listed below |
| 2 | Home folder and memory | Not started |
| 3 | Continuity across model switches | Not started |
| 6A | Hardware auto-tuning | Not started |
| 6B | Feedback loop | Not started |
| 6C | Memory cleanup proposals | Not started |
| 6D | Training dataset export and Train page (dataset side only) | Not started |
| Final | Installers, README, report to the operator | Not started |
| 6E | "Build Democritus" distillation | **Not started on purpose.** Begins only after everything above is verified and reported |

## What is done

### Step 1: Identity
- **Renamed:** product name, window title and installers are now "Democritus". Bundle id is `com.democritus.app`. Package and crate are both named `democritus`.
- **Core identity:** `src-tauri/src/identity.rs` builds it into every request in the backend. The operator's custom prompt comes after it.
  - The identity is restated *after* the custom prompt. Without that, qwen3-coder took on a "gardener" persona from a custom prompt.
- **Migration:** on first run, the old database `%APPDATA%\com.localqwen.desktop\local-qwen.db` is copied with `VACUUM INTO` (read-only on the old file) to `%APPDATA%\com.democritus.app\democritus.db`. The old file is never changed.
- **Verified:**
  - End-to-end migration test using a fixture database (`scripts/make-legacy-db.py`).
  - All 4 models answer "I am Democritus…" and name their current brain.

### Step 4: Model memory
- **Backend:**
  - `sysmem.rs` reads free RAM (Windows memory API) and VRAM (`nvidia-smi`).
  - `ollama.rs` lists loaded models (`/api/ps`), unloads with `keep_alive: 0`, and runs the fit check.
  - The `model_memory` table (schema v2) stores each model's measured footprint.
- **UI:**
  - The previous model is unloaded on switch.
  - A "loaded models" pill and panel in the top bar, with Unload buttons and free RAM/VRAM.
  - A pre-send fit check that offers "Unload and continue".
- **Verified:** 10/10 end-to-end checks, including loading qwen3-coder while the 27B is resident.

### Step 7: Safety
**Backend modules:**
- `operator.rs`:
  - Permissions, all off by default, stored in the `settings` table (key `operator`).
  - The download allowlist (default `https://huggingface.co/Qwen/`), normalized to https with a trailing slash.
  - URL rules: https only; must be on the allowlist; no credentials in the URL; `.gguf` and `.safetensors` only; pickle formats refused.
  - First-bytes sniffing that refuses executables, scripts, zip files and pickles.
  - Single-use approvals.
- `internet.rs`: the only code that can reach the internet.
  - HEAD lookup to get size and SHA-256 (Hugging Face's `x-linked-etag` header), following at most one redirect, to a host the operator sees and approves.
  - Download to a `.part` file, with SHA-256 and size verification.
  - Lockdown cancels a running download.
- `safety_cmds.rs`: commands for the Operator and Log pages.
- `activity.rs`: the activity log, one JSON line per event, in `<home>\logs\activity-YYYY-MM-DD.jsonl`.
- `home.rs`: home folder. Default `D:\Democritus`; setting `homeDir`; `DEMOCRITUS_HOME` overrides it for tests.
- `localtime.rs`: local time from the Windows API (no date crate).

**What gets logged:**
- Setting changes, with old and new values.
- Every chat request to the local model server, and model unloads and server starts.
- Approvals, refusals, internet requests, and Lockdown on/off.
- The status and loaded-model polling (every few seconds) is deliberately not logged.

**WebView2:** `tauri.conf.json` sets `additionalBrowserArgs` to switch off background networking, the component updater, SmartScreen, sync, crash uploads and metrics uploads. These replace Tauri's defaults, so the three default `--disable-features` entries are included.

**UI:** a left rail (Chat, Log, Operator), a Lockdown button with a banner, the Operator page (toggles, allowlist, download box), the Log page (filter and search, refreshes every 4 s), the approval dialog, and a home-folder field in Settings.

**Frontend refactor:** shared helpers moved to `src/ui.ts`. Pages live in `src/pages.ts`, `src/logpage.ts` and `src/operatorpage.ts`.

**Verified:**
- 15/15 end-to-end safety checks:
  - all permissions off by default
  - Lockdown blocks internet and shows a banner
  - bad URLs refused
  - the approval dialog shows the exact URL, and Deny sends nothing
  - approvals work once
  - setting changes logged with old and new values
  - an injected "turn on downloads" in chat text or a dropped file changes nothing
- 36/36 Rust unit tests.

## In progress: Step 7 open items

1. **HTTPS isn't built in yet.** reqwest is compiled without TLS, so approved downloads currently fail with "HTTPS isn't available in this build".
   - **Needs operator approval** to download from crates.io: reqwest's `native-tls` feature, which uses Windows SChannel. That pulls in about four small crates: `native-tls`, `schannel`, `tokio-native-tls` and `hyper-tls` (roughly 1 MB of source). `sha2` is already in the local cache.
   - Cargo features apply to the whole crate, so enabling TLS also gives `llm::client()` TLS. The local model client still refuses anything except loopback `http://` (`llm::normalize_base_url`). Keep that check.
2. **End-to-end test of a real download.** Planned file: `https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF/resolve/main/Qwen3-Embedding-0.6B-Q8_0.gguf` (about 640 MB). It doubles as Step 2's embedding model. **Needs operator approval** of this download, after item 1.
3. **WebView2 traffic despite the flags** (see Known issues): the WebView2 browser process briefly connected to `40.99.44.2:443`. The host isn't identified yet. The operator said another session will do the security work, so this is documented here rather than fixed.
4. **"Reset everything learned"** moves to Step 2, because it needs the memory templates. Required behavior: back up `memory\` to `<home>\backups\memory-<stamp>\` first, then reset memory, the search index, pending proposals, and (once built) feedback stats and prompt suggestions. Keep conversations and settings. Show a confirmation listing exactly what will be reset.

## Exact next steps

1. Ask the operator to approve the TLS crates (item 1 above). Then set `reqwest = { version = "=0.13.5", default-features = false, features = ["native-tls"] }` and run `cargo build`.
2. Ask the operator to approve the embedding-model download. Run it through the app (Operator page → Allow model downloads → paste the URL → approve lookup → approve download). Confirm that the checksum matches, and that an activity-log entry exists for each request.
3. Commit Step 7.
4. **Step 2, memory**, as in the approved audit plan:
   - Create `<home>\memory\` with `identity.md`, `operator.md`, `current_state.md`, `projects\`, `sessions\` and `index.md`.
   - Hand edits win: re-read files whenever their modification time changes.
   - After each exchange, a background call to the memory helper proposes updates as JSON. Use Ollama's native `/api/chat` with `format` set to a JSON schema and `think: false`.
   - In Rust, validate every path: inside `memory\` only, no `..`, no absolute paths or drive letters. Log rejections.
   - Apply updates only after review (diff panel; "ask me first" is the default) unless `autoApplyMemory` is on. Never write while `memoryPaused` or Lockdown is on (`OperatorSettings::memory_writable`).
   - Take a dated snapshot in `memory\.history\` before every change, with Undo.
   - Search: SQLite FTS5 now (built in, no download). After approvals, add `sqlite-vec` (crates.io) plus the Qwen3-Embedding model for hybrid search. The index can always be rebuilt from the Markdown files.
   - A Memory page (browse, search, edit, delete, pause), projects (detection, active-project dropdown, per-conversation link), and "Reset everything learned".
5. **Step 3:**
   - Assemble context in this order: core identity, `identity.md`, `operator.md`, `current_state.md`, the active project file, memory search results, then history. Wrap memory content as *data*, not instructions.
   - Fit the context to the model's window. Move chat to Ollama's native `/api/chat` to control `num_ctx`.
   - Summarize older messages with the current model and store the summary in the conversation.
   - Show a "Brain switched to X" divider and a handoff note, and a "Last time: …" banner on open.
   - Run the switch-model continuity test and the restart test from the spec.
6. **Step 6A:** benchmark and per-model profiles, a Performance page, and memory monitoring during chats.
7. **Step 6B:** thumbs up/down and "edit response", model-per-task stats, optional auto-select (off by default), and prompt suggestions that need approval.
8. **Step 6C:** weekly or on-demand memory cleanup proposals, applied only with approval.
9. **Step 6D:** dataset export of highly rated and edited answers plus self-description examples, saved to `<home>\training\`, and a Train page. 6E reuses both. Do not build a second export.
10. Final: rebuild the installers (`npm run tauri build`), update the README, and report to the operator in the same format as last time.
11. Then **Step 6E**, as specified in the operator's 6E message: preflight, lesson set, QLoRA, packaging as `democritus:<size>-v<N>`, evaluation, overnight mode, a small end-to-end test run first. Do not start a full build.

## Operator decisions (binding)

- **Rules from Step 0:**
  - Ask before **every** network access: package installs, downloads, web pages. Say what it is, where it comes from, and roughly how big it is.
  - Official sources only, with pinned versions.
  - No `curl | sh`.
  - No paid services.
  - No admin rights without asking.
  - Never delete or overwrite outside `D:\democritus-app` and the Democritus home folder.
  - Model files are read-only.
- **Folders:** the project was copied to `D:\democritus-app` and verified. `D:\local-qwen-desktop` stays untouched as a backup until the operator says it can be deleted.
- **Memory helper:**
  - `qwen3.5:4b`, with a fallback: check free RAM/VRAM before loading it.
  - If it won't fit beside the current brain, either use the current brain or queue the update until a smaller model is active, whichever is faster.
  - The helper model must be a setting.
- **Training:** the earlier "skip training, no downloads" answer is **replaced** by the 6E message. Downloads are allowed, but only with approval for each one.
  - 6D builds the dataset side and the Train page.
  - 6E adds the training and reuses them.
- **Icon:** use `democritus-split-1024.png` from the project folder with `npx tauri icon`. Do not design an atom icon.
- **Out of scope for this session:** do **not** restyle the app, and do **not** build a firewall rule or a security-check panel. Another session will do the visual redesign and the security work. Build new screens from the existing components and styling, and keep layout and logic separate.
- **Defaults:** permissions all off. Memory updates default to "ask me first".

## Known issues

- **Icon not applied yet.** `democritus-split-1024.png` isn't in `D:\democritus-app`. There is a `C:\Users\elias\Desktop\democritus-split.svg` (modified 19:46), but the operator asked for the PNG, so it wasn't used. The old "Local Qwen" chat-bubble icon is still in `src-tauri/icons/`.
- **WebView2 connection (security session, please look):** during a dev run with the privacy flags active (confirmed on the browser process's command line), the browser process (`msedgewebview2.exe`, no `--type`) held a TCP connection to `40.99.44.2:443` (Microsoft address range, no reverse DNS, not in the Windows DNS cache).
  - It was gone a minute later.
  - Next diagnostic: `scripts/dev-e2e.ps1 -ExtraArgs "--log-net-log=<path>\netlog.json --net-log-capture-mode=Default"`, then look for the hostname in the net log.
  - The page itself makes no external requests; every end-to-end run checks this.
- **Shutdown hang, seen once, not reproduced.** After a full safety test run, the window closed but `democritus.exe` stayed alive: idle, 23 threads, WebView2 children and the DevTools port gone. I force-stopped it.
  - Two later identical runs exited in 0.2–0.3 s.
  - Cause unknown. Suspects to check: the `RunEvent::Exit` handler, or a pending IPC call during webview teardown.
- **How Lockdown was interpreted:** Lockdown blocks all *internet* (non-loopback) access and pauses learning and memory writes. Chat with the local Ollama server on 127.0.0.1 still works. Confirm with the operator that this is what they want.
- **Ollama's own model loading** can still fail transiently with a CUDA error right after a large model unloads, as seen in the first build. The app shows a "try sending again" message.
- `cargo` reports 4 dead-code warnings for helpers reserved for Step 2 (`home::backups_dir`, `OperatorSettings::memory_writable`, and so on).
- The end-to-end safety group assumes **fresh** test data. Launch with `-Fresh`.

## How to run and test

```powershell
# Rust unit tests (offline)
cd D:\democritus-app\src-tauri; cargo test --offline

# Test-mode app: data in .e2e\ (gitignored), never the operator's real data or D:\Democritus
powershell -ExecutionPolicy Bypass -File scripts\dev-e2e.ps1 -Fresh [-Legacy] [-ExtraArgs "..."]

# End-to-end tests drive the real window over WebView2's DevTools port 9222
node scripts/e2e.mjs --only=safety                     # groups: chat, stop, ui, persisted, migration, identity, memory, safety
node scripts/e2e.mjs --only=identity --models=qwen3.5:4b,qwen3.5:9b
```

- Rust needs `%USERPROFILE%\.cargo\bin` on PATH (installed via winget, toolchain `stable-msvc`, rustc 1.99.1).
- The Ollama tray app is running and serves `127.0.0.1:11434`. The app reuses it and never stops a server it didn't start.
- Models: `qwen3.5:4b` (3.3 GB), `qwen3.5:9b` (6.6 GB), `qwen3.8:27b` (17.7 GB, about 6.7 tokens/sec, partly on CPU), `qwen3-coder:latest` (18.6 GB MoE, about 39 tokens/sec). Hardware: RTX 5070 with 12 GB VRAM, 32 GB RAM.
