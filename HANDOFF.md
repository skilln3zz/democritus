# Democritus: handoff

Status as of 2026-10-17 (cloud session on branch `cloud-work`, UI redesign the same day, then a Windows test run on the PC: see "Windows test run, 2026-10-17"). Project: `D:\democritus-app` (Tauri 2 + Rust backend + TypeScript UI, Ollama as the model server).

The original app is kept untouched as a backup in `D:\local-qwen-desktop`. **Do not delete it until the operator says so.**

## Build order and status

| Step | What | Status |
|---|---|---|
| Audit | Compare the existing app against the spec | Done; operator approved the plan |
| 1 | Identity and rename | **Done** (commit `3868323`), except the icon (see Known issues) |
| 4 | Model memory management | **Done** (commit `fc98a8b`) |
| 7 | Safety rules | **Done** (commit `8879cc1`): HTTPS via native-tls, real approved download verified |
| 2 | Home folder and memory | **Code complete** (commit `24704c0`). **Windows run fails:** the memory helper (`qwen3.5:4b`) doesn't reliably produce usable proposals, and meaning search misses the test note. See "Windows test run, 2026-10-17" |
| 3 | Continuity across model switches | **Verified on Windows** 2026-10-17: `continuity` 10/10, `restart` 6/6 |
| UI | "Bohemian dusk" redesign, theme switch, Security check (design/UI-SPEC.md) | **Verified on Windows** 2026-10-17 (automated parts): `ui` group and the redesign checks pass; the firewall fix (UAC) and the `OLLAMA_HOST` fix still need the operator |
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

## Step 7: remaining open item

- **WebView2 traffic despite the flags** (see Known issues): the WebView2 browser process briefly connected to `40.99.44.2:443`. The host isn't identified yet. The operator said another session will do the security work, so this is documented here rather than fixed.

## Step 2: memory (code complete)

- `memory.rs`: Markdown files in `<home>\memory\` (`identity.md`, `operator.md`, `current_state.md`, `index.md`, `projects\`, `sessions\`), path validation, section operations, snapshots in `.history\` before every write, backup and reset.
- `memindex.rs`: search index derived from the files. FTS5 keyword search, plus meaning search with Qwen3-Embedding vectors compared in Rust (no `sqlite-vec` needed at this scale), merged by reciprocal rank fusion. Hand edits are detected by modification time and logged.
- `memcmds.rs`: background worker. After each completed exchange it asks the memory helper (`qwen3.5:4b` by default, with the fit/fallback rule) for JSON proposals, validates them, and queues them for review or applies them if auto-apply is on. Plus all Memory page commands, projects and "Reset everything learned".
- UI: Memory page (`src/memorypage.ts`, `src/diff.ts`), project dropdown in the top bar, memory-helper setting.
- End-to-end group: `node scripts/e2e.mjs --only=mem`.

## Step 3: continuity (verified on Windows, 2026-10-17)

- `context.rs` (pure, unit-tested) builds every request in the approved order: core identity and custom prompt, `identity.md`, `operator.md`, `current_state.md`, the active project's file, memory search results, the conversation summary, then the history.
  - Memory is wrapped in `<memory …>` tags under "data, not instructions", and can't close its own tag.
  - Untouched templates are skipped.
  - Files are read fresh from disk on every request, so hand edits win.
- **Fitting the window.** Tokens are estimated at 3 characters each (conservative). The reply gets `min(max tokens, window/2)`, memory gets at most 35% of the rest, and the history fills what's left, newest first. The new message is always sent, shortened in the middle if it alone is too big.
- **Summaries.** When the history doesn't fit, the current brain folds the older turns into a running summary. It stores the summary in `conversations.summary` / `summary_upto` (schema v4) and keeps only the newest turns that fit in half the budget, so it doesn't summarize again on every turn. If summarizing fails, the oldest turns are left out and the reply shows a warning badge.
- **Native Ollama chat.** With Ollama, chat now uses `/api/chat` (NDJSON) with `num_ctx`. Other servers keep the OpenAI-compatible endpoint.
  - The window is the new setting **Context window** (default 8192), capped at the model's own limit from `/api/show`.
  - The memory helper reuses a loaded model's current window (from `/api/ps`), or the chat window for the brain, so Ollama never reloads a model just because `num_ctx` differs.
- **Brain switch.** When the model of the previous reply differs, a handoff note is added to the system prompt and logged ("brain switch handoff"). The UI shows a "Brain switched to X (was Y)" divider with "Handed over: N recent messages, memory: …".
- Each reply saves what went into it in `stats.context`. The footer shows "~5.2k / 8k context", with details on hover.
- **"Last time" card** on the start screen (`last_time` command): Last task, Project and Next step from `current_state.md`, plus a button to continue the most recent conversation.
- End-to-end groups: `--only=continuity`, then restart the app, then `--only=restart`.

## Windows test run, 2026-10-17

Run on the PC (RTX 5070 12 GB, 32 GB RAM, Ollama 0.40.2) against the four real models, branch `cloud-work` at `e608e40`. Test data only (`.e2e\`); `D:\Democritus` was not touched.

| Check | Result |
|---|---|
| `cargo test --offline` | 77/77 pass |
| `npm run build` (tsc + vite) | passes |
| `--only=continuity` | **10/10**: code word survives the 4B → 9B switch, divider and handoff note, memory reaches a new chat, 4096-token window summarizes and keeps "blue pot" |
| `--only=restart` (after a restart without `-Fresh`) | **6/6**: "Last time" card shows Kestrel, the 9B knows what we were doing |
| `--only=identity,memory,safety,mem,chat,stop,ui` (fresh) | 51/56. All of identity (4 models), safety, chat (4 models, thinking shown separately), stop, ui and privacy pass. Failures are in `memory` and `mem`, below |
| Redesign checks, automated over DevTools | Light / Dark / Follow system apply, are saved and cached, and Follow system tracks the OS live; arrow keys move the theme choice; at 1024×700 no page overflows and the pickers and conversation list stay usable; Tab gives a visible focus ring everywhere (the pickers draw it on their wrapper); Security check takes 2.5 s, rows 1, 2 and 6 green with Ollama on 127.0.0.1, a fake `test.bin` in downloads turns row 6 red and is not deleted, runs are logged |

**Failures and what they are**

1. **Step 2 memory helper is unreliable (real problem).** In three fresh `mem` runs the `qwen3.5:4b` helper never produced the expected proposal for the Garden Planner exchange:
   - once it proposed the project file, `operator.md` and a session note but no `current_state.md`;
   - once (run alone, as intended) it looped until the JSON was cut off: "did not return valid JSON: EOF while parsing a string at line 300".
   - `ollama::chat_json` sets no `num_predict` and the schema has no `maxItems` / `maxLength`, so a looping small model runs until the context is full. Suggested fix: cap the output, bound the schema, retry once on invalid JSON, and test against `qwen3.5:4b` specifically.
   - The rest of the `mem` group (history and undo, path refusals, reject, pause, auto-apply, Lockdown, the 27B fallback, reset) did not run on Windows because it depends on that first proposal. Its `Cannot read properties of undefined (reading 'id')` is a knock-on failure: no `current_state.md` history.
2. **Meaning search misses the test note (twice).** "Where does the operator live, and what do they grow?" returns `operator.md › Operator`, `identity.md › Operator`… but not `operator.md › Facts` (Vienna). The query shares the word "operator", so the keyword list ranks those headings first and reciprocal rank fusion keeps them ahead. Either the test query isn't a pure meaning test, or fusion should weight meaning results more. Not yet decided.
3. **`memory: panel shows model …` (test is out of date).** The "In memory" list now also shows `qwen3-embedding:0.6b` (loaded by memory search), and it comes first. The app is right; the test expects the brain in the first row.
4. **Fit warning wording (depends on free RAM).** With the 27B loaded by another program, the coder check said "needs about 22.2 GB … only 2.0 GB is free, even after unloading the other models", without an Unload button, so the test couldn't continue. About 18.5 GB of RAM was free before the test (other programs open). Re-run Step 4's group with more free RAM before treating this as a bug.
5. **One-off: Ctrl+N timed out after 5 s** in the first `memory` run. It could not be reproduced (Ctrl+N shows the empty chat in about 10 ms, also right after an external 27B load).
- Running `memory` and `mem` in the same run makes `mem` pick up stale helper jobs from the `memory` group's chats. Run `mem` on its own, as the list below says.

**Shutdown hang reproduced once** (see Known issues): after the continuity run, the first close request did nothing for 30+ s; the window then went away but `democritus.exe` stayed. Stacks taken with `dbghelp` (no debugger installed): every thread idle, the main thread waiting in tao's `GetMessageW` loop, no windows left, and the `ollama serve` the app started still alive, so the `RunEvent::Exit` handler never ran. Four later closes exited in 0.3–0.8 s and stopped Ollama as they should.

**Still needs the operator:** the Security check fixes that change system settings (the `OLLAMA_HOST=0.0.0.0` test and its fix; "Create rules" with the UAC prompt), the firewall download exception (`e608e40`) with a real approved download, the attach button's native file picker, the Windows dark/light switch itself (only emulated here), and the VRAM comparison in Task Manager.

## Exact next steps

1. **Step 2 fixes:** make the memory helper reliable with `qwen3.5:4b` (failure 1), decide on meaning search (failure 2), update the out-of-date `memory` panel check (failure 3), then re-run `--only=mem` on fresh data until the whole group passes.
2. **Operator:** the "Still needs the operator" items above.
3. **Step 6A:** benchmark and per-model profiles, a Performance page, and memory monitoring during chats.
4. **Step 6B:** thumbs up/down and "edit response", model-per-task stats, optional auto-select (off by default), and prompt suggestions that need approval.
5. **Step 6C:** weekly or on-demand memory cleanup proposals, applied only with approval.
6. **Step 6D:** dataset export of highly rated and edited answers plus self-description examples, saved to `<home>\training\`, and a Train page. 6E reuses both. Do not build a second export.
7. Final: rebuild the installers (`npm run tauri build`), update the README, and report to the operator in the same format as last time.
8. Then **Step 6E**, as specified in the operator's 6E message: preflight, lesson set, QLoRA, packaging as `democritus:<size>-v<N>`, evaluation, overnight mode, a small end-to-end test run first. Do not start a full build.

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
- **Redesign and security panel:** these were out of scope for the Step 1–7 session and have now been done by the redesign session (see "UI redesign" below). New screens for 6A–6E should use the new components and tokens in `src/styles.css`, and keep layout and logic separate.
- **Defaults:** permissions all off. Memory updates default to "ask me first".

## Known issues

- **WebView2 connection (security session, please look):** during a dev run with the privacy flags active (confirmed on the browser process's command line), the browser process (`msedgewebview2.exe`, no `--type`) held a TCP connection to `40.99.44.2:443` (Microsoft address range, no reverse DNS, not in the Windows DNS cache).
  - It was gone a minute later.
  - Next diagnostic: `scripts/dev-e2e.ps1 -ExtraArgs "--log-net-log=<path>\netlog.json --net-log-capture-mode=Default"`, then look for the hostname in the net log.
  - The page itself makes no external requests; every end-to-end run checks this.
- **Shutdown hang, seen twice, intermittent.** After a full safety test run, and again on 2026-10-17 after the continuity run, the window closed but `democritus.exe` stayed alive: idle, 22–25 threads, WebView2 children and the DevTools port gone. Force-stopped both times.
  - The second time, the first close request did nothing for over 30 s before the window went away.
  - Stacks (2026-10-17): the main thread sits idle in tao's event loop (`GetMessageW`), no windows remain, and the app's own `ollama serve` is still running, so `RunEvent::Exit` was never reached. This is not a deadlock in Democritus's code. `tauri-runtime-wry` 2.12.2 only requests exit when it handles the window's `Destroyed` event and finds no windows left, so look there: a slow WebView2 teardown, or a close that bypasses `Destroyed`.
  - Other closes exited in 0.2–0.8 s (six times). It seems to need a long session with the memory helper busy.
  - A possible guard: on `WindowEvent::Destroyed` of the main window, call `app.exit(0)` explicitly.
- **How Lockdown was interpreted:** Lockdown blocks all *internet* (non-loopback) access and pauses learning and memory writes. Chat with the local Ollama server on 127.0.0.1 still works. Confirm with the operator that this is what they want.
- **Ollama's own model loading** can still fail transiently with a CUDA error right after a large model unloads, as seen in the first build. The app shows a "try sending again" message.
- **Context window vs. VRAM.** The fit check (Step 4) estimates from file size and doesn't yet account for the context window. Measured footprints update on the next load. Step 6A should pick the window per model from the benchmark.
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

## Cloud session (branch `cloud-work`)

The branch is pushed to GitHub: `https://github.com/skilln3zz/democritus`, branch `cloud-work`, based on `main` at `24704c0`. On the PC: `git fetch origin`, then `git checkout cloud-work`.

Work in this branch was written on a Linux cloud machine. It can't run Ollama, the Qwen models, WebView2 or the Windows build, so everything here is **type-checked and unit-tested only**. Nothing was run against real models.

How it was checked on Linux (no system packages installed):
- `npx tsc --noEmit` for the UI.
- `cargo check` for the whole Rust crate, with stub `.pc` files for the GTK/WebKit libraries (`cargo check` never links, so stubs are enough).
- `cargo test` for every module that doesn't use Tauri, through a small harness crate that includes those files by `#[path]`. Tests in `main.rs`, `memcmds.rs` and `safety_cmds.rs` can't run there; run them on Windows.

### Test on Windows

Run these on the Windows PC after pulling `cloud-work`:

1. `cd src-tauri; cargo test --offline` — all unit tests pass (including `memcmds` tests that the cloud couldn't run).
2. `npm run build` — the UI type-checks and bundles.
3. `scripts\dev-e2e.ps1 -Fresh`, then `node scripts/e2e.mjs --only=mem` — the Step 2 memory group (needs `qwen3.5:4b`, and the embedding model for the meaning-search check).
4. Same app run: `node scripts/e2e.mjs --only=continuity`. This covers:
   - the code word survives a switch from `qwen3.5:4b` to `qwen3.5:9b`
   - the divider and handoff note
   - memory reaching a new chat
   - summarizing with a 4096-token window, which keeps "the shed key is under the blue pot"
5. Close the app, start it again **without** `-Fresh`, then `node scripts/e2e.mjs --only=restart`. This checks:
   - the "Last time" card shows the codename Kestrel
   - the 9B model says what we were working on
6. Regression check, because chat moved to Ollama's native API: `node scripts/e2e.mjs --only=chat,stop,ui,identity,memory,safety`. In particular, check:
   - thinking still shows in its own section
   - Stop still stops the reply
   - the "Hit max-token limit" badge still appears
7. By hand:
   - Open Settings and check the Context window field.
   - In Task Manager, compare the VRAM of `qwen3.5:9b` and `qwen3.8:27b` with the earlier numbers. The window used to be Ollama's default; it is now 8192.
   - (The old "loaded models" pill is gone: loaded models now live in the Continuity panel, see "UI redesign".)

## UI redesign (cloud session, branch `cloud-work`)

Implements `design/UI-SPEC.md` on top of the existing app. App logic, data, safety rules and features are unchanged; the only new behavior is the theme setting and the Security check. Written and type-checked on Linux. It was also rendered in headless Chromium against a fake backend (screenshots looked like the mockups in both themes and at 1024×700), but **not** run in WebView2 or against real models.

### What changed

- **Look:** theme tokens on `:root`, switched by `data-theme` on `<html>`; the dusk photo behind everything; chamfered paper panels; Italiana / Manrope / IBM Plex Mono bundled from `@fontsource/*` 5.3.0 (the three approved packages, nothing loads from the web).
- **Chat:** top band (icon and labels, the conversation's N° and title, DEMOCRITUS wordmark, the Brain picker, Project picker, server status and tok/s); a left panel with conversations grouped by project with 3-digit numbers, then Memory, Settings & safety, Activity log and Lockdown; messages over the photo; the composer band with a new attach button (opens a file picker; same checks as drag and drop); a right **Continuity** panel (current state and next step, project steps, models in memory with a GPU/RAM bar and Unload, files recalled by the last reply).
- **Memory:** band with Auto-apply and Pause memory; file index; the file as a page; proposed updates with Apply / Reject; recent memory events from the activity log; Reset.
- **Settings & safety** (the old Operator page): the Lockdown band (inverts when locked), Appearance, numbered permissions with round switches, allowlist chips and editor, downloads, the **Security check**, a short activity log and Reset. The settings dialog opens from the gear in the top band or "Chat & model settings…".
- **Theme:** Light / Dark / Follow system, stored as `theme` in the app settings (default `system`). `public/theme-boot.js` applies it before the first paint from a localStorage copy; the saved setting wins on start. Follow system tracks Windows live.
- **Security check** (`src-tauri/src/security.rs`, pure rules in `secrules.rs`, UI in `src/securitypage.ts`). Six rows, green / red / grey:
  1. Ollama's port (from the server URL) is bound only to loopback: `netstat -an`, recognizing listening sockets by their foreign address so it works in any Windows language.
  2. `OLLAMA_HOST` for the user (`HKCU\Environment`), the computer (`HKLM\…\Session Manager\Environment`) and the app's own environment. Fix: set it to `127.0.0.1:<port>` with `[Environment]::SetEnvironmentVariable` (admin only for the computer-wide value), then restart Ollama.
  3. Windows Firewall: an enabled outbound Block rule for `democritus.exe`, `ollama.exe` and `ollama app.exe` (the tray app, which does Ollama's update checks). Fix: **Create rules** shows the exact `New-NetFirewallRule` commands, then runs them in an elevated PowerShell (UAC prompt) only after "Run commands". Rules go in the group "Democritus" and block every address except 127.0.0.1 and ::1, so local chat keeps working.
  4. Ollama version from `/api/version`, with a note to update it yourself. No update checks.
  5. Lockdown and download permission (fix: Turn on Lockdown).
  6. Model files: `<home>\downloads` must hold only `.gguf` / `.safetensors` whose first bytes match; Ollama's blobs (`OLLAMA_MODELS` or `%USERPROFILE%\.ollama\models\blobs`) must not be executables, zips or pickles. Nothing is ever deleted.
  The window only sends a fix id; the backend rebuilds the commands itself. Every check run and every fix (with its commands and result) is in the activity log under "security".
- **Icons:** regenerated with `npx tauri icon design/assets/democritus-icon-1024.png` (same picture as before); the old purple placeholder `app-icon.png.svg` is removed.
- **Window minimum** is now 1024×700. Below 820 px of height the frame and bands get tighter (14 px instead of 22 px padding) so the conversation list keeps room.
- `scripts/e2e.mjs`: the brain-switch check is now case-insensitive (the divider is in caps). All other selectors the tests use were kept (`#model-select`, `#loaded-btn` / `#loaded-panel`, `.rail-btn[data-page=…]`, `#lockdown-btn`, `#op-toggles input.switch`, …). `#loaded-btn` is now the refresh button of the always-visible "In memory" list.

### Decisions for the operator

- **The Democritus firewall rule would also block approved downloads** (they come from `democritus.exe`), so there is now an exception (commit `e608e40`):
  - For each approved download, an elevated helper (one UAC prompt) switches off only the `democritus.exe` rule in group "Democritus".
  - It switches the rule back on when the download finishes, fails or is cancelled, when the app exits, or at a deadline. It never touches Ollama's rules and never creates an Allow rule.
  - At start, the app warns with a "Re-enable rule" banner if the rule was left off. The Security check row stays red until the rule is back on.
  - Memory writes outside `memory\` are now logged too.
  - Unit-tested (`secrules`); not yet tried with a real download on Windows.
- Firewall rules are tied to a program path. A rule created from a dev run covers `src-tauri\target\debug\democritus.exe`, not the installed app; run "Create rules" again from the installed app.
- `msedgewebview2.exe` (the WebView2 connection in Known issues) is **not** covered: a rule for it would cut every WebView2 app on the PC.
- Left out on purpose because the features don't exist yet: the Performance link (Step 6A), thumbs up/down and edit on replies (6B), "Edit first" on memory proposals. The mockups show them; they'll slot into the same places.
- The thinking text uses the body font, not Italiana, because long reasoning in a display face is hard to read.
- The window's title bar still follows Windows' own theme (changing it needs an extra Tauri window permission).

### Test on Windows (redesign)

1. `git pull`, then `npm ci` (adds the three `@fontsource` packages from the lockfile), then `npm run build`.
2. `cd src-tauri; cargo test --offline` — includes the new `secrules` tests (they also passed on Linux) and compiles the Windows-only code in `security.rs`, which the cloud could not compile.
3. `scripts\dev-e2e.ps1 -Fresh`, then `node scripts/e2e.mjs --only=ui,safety` and `--only=memory,continuity` — the selectors changed with the layout.
4. Look at every page in Light, Dark and Follow system. While on Follow system, switch Windows between light and dark: the app should follow at once. Restart: no flash of the other theme.
5. Resize to 1024×700: nothing overlaps, the Brain/Project pickers and the conversation list stay usable; dropdowns open over the band, not clipped.
6. Keyboard only: Tab through each page; focus rings are visible (accent), the theme switch moves with the arrow keys, Esc still stops a reply.
7. Settings & safety → Security check. It runs once per session when you open the page:
   - Row 1 green with Ollama on 127.0.0.1. Then set a user variable `OLLAMA_HOST=0.0.0.0`, restart Ollama, "Check again": rows 1 and 2 red. Press the row-2 fix, check the dialog shows the exact command, run it, restart Ollama: both green.
   - Row 3: "Create rules" → the dialog lists one command per program → "Run commands" → UAC prompt. Decline once (expect "Cancelled at the Windows administrator prompt", or a generic error on a non-English Windows), then accept. The rules should appear in `wf.msc` under group "Democritus". "Check again" → green. Then confirm chat still works and `ollama pull` fails. To remove the rules: `Remove-NetFirewallRule -Group 'Democritus'` in an admin PowerShell.
   - How long the check takes (the firewall query is PowerShell; tell me if it's over ~5 s).
   - Row 6 with your real model store, and with a fake `test.bin` in `D:\Democritus\downloads` (should turn red, and nothing gets deleted).
   - The Log page shows "security check", "security fix confirmed" and "security fix done/failed".
8. Attach button in the composer: pick a text file and a binary file (the binary should be refused, as with drag and drop).

