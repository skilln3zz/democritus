# Local Qwen

A small desktop chat app for Qwen models that run on your own computer. It talks to a local model server (Ollama by default) through the server's OpenAI-compatible API. Nothing is sent to the internet.

Built with [Tauri 2](https://tauri.app): the backend is Rust, and the UI is plain TypeScript running in the system webview. The Windows installer is about 5 MB.

## Features

- Streaming chat. Tokens appear as they're generated, and **Stop** (or <kbd>Esc</kbd>) cancels the reply.
- A model picker that lists every model your server has, with parameter count, quantization and size.
- Multiple conversations stored in SQLite. From the sidebar you can switch, rename (✎ or double-click), search (titles *and* message text) and delete them.
- Qwen "thinking" output is shown in a collapsible section, separate from the answer. Both formats are handled: a separate `reasoning` / `reasoning_content` field (Ollama, llama-server, vLLM) and inline `<think>…</think>` tags.
- Markdown rendering, with syntax-highlighted code blocks and a **Copy** button on each one.
- Drag text files into the window to include their contents in your message (up to 1 MB per file).
- Settings for the system prompt, temperature, max tokens, server URL and server start command.
- The "Bohemian dusk" look (design/UI-SPEC.md): cream paper panels over a dusk city photo. **Light, Dark or Follow system**, chosen under Settings & safety → Appearance and saved with the other settings.
- A **Security check** under Settings & safety: whether Ollama listens only on 127.0.0.1, whether `OLLAMA_HOST` is local, Windows Firewall outbound block rules for Democritus and Ollama, the Ollama version, Lockdown and download permission, and whether model files are only `.gguf` / `.safetensors`. Fix buttons show the exact PowerShell commands and run nothing until you confirm.
- A server status indicator. The app starts the model server if it isn't running, and stops it on exit **only if the app started it**.
- Clear error messages, e.g. when a model needs more memory than you have.

Keyboard shortcuts:

| Key | Action |
|---|---|
| <kbd>Enter</kbd> | Send |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | New line |
| <kbd>Ctrl</kbd>/<kbd>⌘</kbd>+<kbd>N</kbd> | New chat |
| <kbd>Esc</kbd> | Stop generating |

## Requirements

| Tool | Version used | Notes |
|---|---|---|
| [Ollama](https://ollama.com) | 0.40.2 | Or any OpenAI-compatible server on localhost (llama-server, LM Studio, vLLM) |
| [Node.js](https://nodejs.org) | 24.x (needs 22+) | |
| [Rust](https://rustup.rs) | 1.99 stable | Windows: `winget install Rustlang.Rustup`, then `rustup default stable-msvc` |
| C/C++ build tools | | Windows: Visual Studio (Build Tools) with "Desktop development with C++". macOS: `xcode-select --install`. Linux: see [Tauri prerequisites](https://tauri.app/start/prerequisites/) |
| WebView2 | Built into Windows 10/11 | macOS and Linux use WebKit |

## Setup

```sh
cd local-qwen-desktop
npm install          # installs the exact versions in package-lock.json
```

Your models must already be available in Ollama (`ollama list`). The app never downloads models. If you only have raw `.gguf` files, import them without re-downloading:

```sh
# Modelfile
FROM D:/models/Qwen3-8B-Q4_K_M.gguf
```
```sh
ollama create qwen3-8b-local -f Modelfile
```

## Run in dev mode

```sh
npm run tauri dev
```

This starts Vite on `http://localhost:1420` with hot reload, and opens the app window. The first run compiles the Rust dependencies, which takes a few minutes. Later runs take seconds.

## Build an installer

```sh
npm run tauri build
```

Output goes to `src-tauri/target/release/bundle/`:

| OS | Installer |
|---|---|
| Windows | `nsis/Local Qwen_0.1.0_x64-setup.exe` and `msi/Local Qwen_0.1.0_x64_en-US.msi` |
| macOS | `dmg/Local Qwen_0.1.0_aarch64.dmg` (plus the `.app`) |
| Linux | `appimage/*.AppImage` and `deb/*.deb` |

The first `tauri build` on Windows downloads the NSIS and WiX bundler tools. That download happens at build time only. The app itself never makes network calls.

## Configuration

Open **Settings** (⚙ in the top bar):

- **System prompt, temperature, max tokens**: sent with every request. Thinking tokens count toward max tokens (default 8192). If you see *"Hit max-token limit"* under a reply, raise it.
- **Server URL**: defaults to `http://127.0.0.1:11434/v1` (Ollama). The app only accepts `http://` URLs on `localhost`, `127.0.0.1` or `::1`.
- **Start command**: defaults to `ollama serve`. If `ollama` isn't on `PATH`, the app looks in Ollama's standard install folders. For llama.cpp, use something like:
  `"C:\llama\llama-server.exe" -m "D:\models\qwen.gguf" --port 8080 --jinja` with URL `http://127.0.0.1:8080/v1`.
- **Auto-start**: turn this off if you'd rather start the server yourself.

## Where data is stored

| | Windows | macOS | Linux |
|---|---|---|---|
| Database (`local-qwen.db`) | `%APPDATA%\com.localqwen.desktop\` | `~/Library/Application Support/com.localqwen.desktop/` | `~/.local/share/com.localqwen.desktop/` |
| Server log (`model-server.log`, written only when the app starts the server) | `%LOCALAPPDATA%\com.localqwen.desktop\logs\` | `~/Library/Logs/com.localqwen.desktop/` | `~/.local/share/com.localqwen.desktop/logs/` |

Deleting the database removes all conversations and settings.

## Privacy

- The Rust HTTP client is compiled **without any TLS support** and ignores proxy settings, so it can't reach an `https://` endpoint. The server URL is also validated to be a loopback address.
- All frontend libraries are bundled. There are no CDNs. The fonts (Italiana, Manrope, IBM Plex Mono, SIL Open Font License) come from the `@fontsource` npm packages and are bundled with the app. The Content Security Policy only allows Tauri's IPC: no external `connect-src`, and no remote images. Markdown images are disabled, and links in replies don't navigate anywhere.

## Tests

```sh
npm test              # Rust unit tests: think-tag splitter, SSE parsing, URL checks, SQLite
```

End-to-end tests drive the real app window over WebView2's DevTools protocol (Windows). Run them from two terminals:

```powershell
# terminal 1
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"; npm run tauri dev
# terminal 2
npm run test:e2e                                    # every model + stop + UI checks
node scripts/e2e.mjs --models=qwen3.5:4b --only=chat,stop
node scripts/e2e.mjs --only=persisted               # after restarting the app
```

The E2E suite sends a message to each model. It checks that replies stream and are saved, and that Esc stops a reply. It also covers drag-and-drop, code highlighting and the copy button, rename/search/delete, settings, and that the UI makes no external network requests.

## Troubleshooting

- **Red "Disconnected" status.** Click it to start the server. If starting fails, the banner shows the reason, and the server log (see above) has details. A common cause is another program already using port 11434.
- **"Not enough memory to load …"** The model doesn't fit in your free RAM and VRAM. Close other GPU-heavy apps, or pick a smaller model or quantization.
- **A reply ends mid-thought.** Raise max tokens in Settings.
- **The app quit unexpectedly and the server keeps running.** A server the app started is only stopped on a normal exit. End `ollama.exe` in Task Manager, or just leave it running: the app will reuse it next time.

## Project layout

```
public/theme-boot.js applies the saved theme before the first paint
src/                 UI (TypeScript, no framework)
  main.ts            app state, chat page, shortcuts, drag & drop
  api.ts             typed wrappers for the Rust commands
  markdown.ts        markdown-it + highlight.js + DOMPurify
  theme.ts           Light / Dark / Follow system
  securitypage.ts    the Security check section
  ornaments.ts       decorative SVGs (starbursts, dots, checker strip)
  styles.css         theme tokens and layout (design/UI-SPEC.md)
src-tauri/src/
  main.rs            Tauri commands, streaming over an IPC channel, cancellation
  llm.rs             OpenAI-compatible client, SSE parsing, friendly errors
  think.rs           <think> tag splitter for inline reasoning
  server.rs          detect / start / stop the model server
  db.rs              SQLite schema and queries
  security.rs        Security check commands (secrules.rs: the pure, unit-tested rules)
scripts/e2e.mjs      end-to-end tests over the DevTools protocol
design/              UI design handoff; icons come from design/assets/democritus-icon-1024.png
                     (npx tauri icon design/assets/democritus-icon-1024.png)
```
