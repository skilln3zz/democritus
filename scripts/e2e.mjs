// End-to-end test that drives the real app window through the WebView2 /
// Chromium DevTools protocol. No dependencies: uses Node's built-in fetch and
// WebSocket (Node 22+).
//
// 1. Start the app with remote debugging enabled (dev or a release build):
//      PowerShell:  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"; npm run tauri dev
// 2. In another terminal:
//      node scripts/e2e.mjs                    # every model + stop + UI checks
//      node scripts/e2e.mjs --models=qwen3.5:4b --only=chat,stop
//      node scripts/e2e.mjs --only=persisted   # after restarting the app
//
// Options: --port=9222  --models=a,b  --only=chat,stop,ui,persisted  --shots=<dir>

import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const args = Object.fromEntries(
  process.argv.slice(2).map((a) => {
    const [k, v] = a.replace(/^--/, '').split('=');
    return [k, v ?? 'true'];
  }),
);
const PORT = Number(args.port ?? 9222);
const ONLY = new Set((args.only ?? 'chat,stop,ui').split(','));
const SHOTS = args.shots ?? null;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
let failures = 0;

function report(name, ok, detail = '') {
  results.push({ name, ok, detail });
  if (!ok) failures++;
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? `  —  ${detail}` : ''}`);
}

class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.nextId = 0;
    this.pending = new Map();
    this.listeners = new Map();
    ws.onmessage = (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (msg.error) reject(new Error(msg.error.message));
        else resolve(msg.result);
      } else if (msg.method) {
        for (const fn of this.listeners.get(msg.method) ?? []) fn(msg.params);
      }
    };
  }
  static async connect(url) {
    const ws = new WebSocket(url);
    await new Promise((resolve, reject) => {
      ws.onopen = resolve;
      ws.onerror = () => reject(new Error(`Could not connect to ${url}`));
    });
    return new Cdp(ws);
  }
  on(method, fn) {
    this.listeners.set(method, [...(this.listeners.get(method) ?? []), fn]);
  }
  send(method, params = {}) {
    const id = ++this.nextId;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
  }
  async eval(expression) {
    const r = await this.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }
  /** A real key press, as if typed on the keyboard. */
  async key(key, code, keyCode, modifiers = 0, text) {
    const base = { key, code, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode, modifiers };
    await this.send('Input.dispatchKeyEvent', { ...base, type: text ? 'keyDown' : 'rawKeyDown', text });
    await this.send('Input.dispatchKeyEvent', { ...base, type: 'keyUp' });
  }
  close() {
    this.ws.close();
  }
}

async function findPage() {
  const deadline = Date.now() + 180_000;
  while (Date.now() < deadline) {
    try {
      const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
      const page = targets.find((t) => t.type === 'page' && /localhost:1420|tauri\.localhost|tauri:\/\//.test(t.url));
      if (page) return page;
    } catch {
      // app not up yet
    }
    await sleep(1000);
  }
  throw new Error(`No app window found on DevTools port ${PORT}. Was the app started with remote debugging?`);
}

async function waitFor(cdp, expression, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const v = await cdp.eval(expression);
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`Timed out after ${timeoutMs / 1000}s waiting for ${label}`);
    await sleep(200);
  }
}

const invoke = (cmd, a = {}) => `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(a)})`;

// Snapshot of the newest assistant message. Opens the thinking section so its
// text renders live and can be measured.
const SNAP = `(() => {
  const all = document.querySelectorAll('#messages .message.assistant');
  const el = all[all.length - 1];
  if (!el) return null;
  const th = el.querySelector('.thinking');
  if (th && !th.open) th.open = true;
  const err = el.querySelector('.error-box');
  return {
    id: Number(el.dataset.id),
    status: el.dataset.status,
    content: el.querySelector('.body')?.innerText ?? '',
    reasoningLen: (el.querySelector('.thinking-body')?.innerText ?? '').length,
    thinkingShown: th ? !th.hidden : false,
    error: err && !err.hidden ? err.innerText : null,
    footer: el.querySelector('.footer')?.innerText ?? '',
    stopVisible: !document.querySelector('#stop').hidden,
  };
})()`;

async function newChat(cdp) {
  await cdp.key('n', 'KeyN', 78, 2); // Ctrl+N
  await waitFor(cdp, `!!document.querySelector('#messages .empty')`, 5000, 'empty new chat');
}

async function selectModel(cdp, model) {
  const ok = await cdp.eval(`(() => {
    const s = document.querySelector('#model-select');
    s.value = ${JSON.stringify(model)};
    s.dispatchEvent(new Event('change'));
    return s.value === ${JSON.stringify(model)};
  })()`);
  if (!ok) throw new Error(`Model ${model} is not in the picker`);
}

async function typeAndSend(cdp, text) {
  await cdp.eval(`document.querySelector('#input').focus()`);
  await cdp.send('Input.insertText', { text });
  await cdp.key('Enter', 'Enter', 13, 0, '\r');
}

/** Waits for the reply to finish; returns samples proving it streamed. */
async function awaitReply(cdp, timeoutMs, onSample) {
  const t0 = Date.now();
  const lengths = new Set();
  let firstTokenAt = null;
  for (;;) {
    const snap = await cdp.eval(SNAP);
    if (snap && snap.id > 0) {
      const len = snap.content.length + snap.reasoningLen;
      if (snap.status === 'streaming' && len > 0) {
        lengths.add(len);
        firstTokenAt ??= Date.now() - t0;
      }
      if (onSample && (await onSample(snap, len))) {
        // caller asked to stop sampling early (e.g. it pressed Esc); keep waiting for the end state
        onSample = null;
      }
      if (snap.status && snap.status !== 'streaming') return { snap, distinctLengths: lengths.size, firstTokenAt, ms: Date.now() - t0 };
    }
    if (Date.now() - t0 > timeoutMs) throw new Error(`No finished reply after ${timeoutMs / 1000}s`);
    await sleep(120);
  }
}

async function activeConversationId(cdp) {
  return cdp.eval(`Number(document.querySelector('.conv-item.active')?.dataset.id ?? 0)`);
}

async function savedMessages(cdp, conversationId) {
  return cdp.eval(invoke('get_messages', { conversationId }));
}

async function screenshot(cdp, name) {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' });
  const file = join(SHOTS, `${name}.png`);
  writeFileSync(file, Buffer.from(data, 'base64'));
  console.log(`      screenshot: ${file}`);
}

// ---------- tests ----------

async function testChat(cdp, model) {
  await newChat(cdp);
  await selectModel(cdp, model);
  await typeAndSend(cdp, 'In one short sentence: why is the sky blue?');
  const r = await awaitReply(cdp, 900_000);
  const convId = await activeConversationId(cdp);
  const saved = (await savedMessages(cdp, convId)).at(-1);
  const s = saved?.stats ?? {};
  const detail =
    `status=${r.snap.status}, load+first token ${(r.firstTokenAt / 1000).toFixed(1)}s, ` +
    `${s.tokensPerSecond ?? '?'} tok/s, ${s.completionTokens ?? '?'} tokens, thinking chars=${r.snap.reasoningLen}, ` +
    `${r.distinctLengths} streamed updates` +
    (r.snap.error ? `, error: ${r.snap.error}` : `, answer: "${r.snap.content.replace(/\s+/g, ' ').slice(0, 90)}"`);
  report(`chat ${model}: streamed reply`, r.snap.status === 'complete' && r.distinctLengths >= 3 && r.snap.content.length > 0, detail);
  report(
    `chat ${model}: saved to SQLite`,
    saved?.status === 'complete' && saved.content.length > 0 && saved.role === 'assistant',
    `conversation ${convId}, ${saved?.content.length ?? 0} chars, reasoning ${saved?.reasoning.length ?? 0} chars`,
  );
  if (saved?.reasoning) {
    report(`chat ${model}: thinking shown separately`, r.snap.thinkingShown && !saved.content.includes('<think>'), 'collapsible section present');
  }
  return { model, convId, stats: s, firstTokenMs: r.firstTokenAt };
}

async function testStop(cdp, model) {
  await newChat(cdp);
  await selectModel(cdp, model);
  await typeAndSend(cdp, 'Count from 1 to 500, one number per line. No commentary.');
  let pressedAt = 0;
  let lenAtPress = 0;
  const r = await awaitReply(cdp, 300_000, async (snap, len) => {
    if (!pressedAt && snap.status === 'streaming' && len > 200) {
      lenAtPress = len;
      pressedAt = Date.now();
      await cdp.key('Escape', 'Escape', 27);
      return true;
    }
    return false;
  });
  const stopMs = Date.now() - pressedAt;
  const convId = await activeConversationId(cdp);
  const saved = (await savedMessages(cdp, convId)).at(-1);
  const ui = await cdp.eval(`({ stop: !document.querySelector('#stop').hidden, send: !document.querySelector('#send').hidden })`);
  report(
    'stop: Esc halts generation',
    r.snap.status === 'stopped' && saved?.status === 'stopped' && stopMs < 5000,
    `stopped ${stopMs} ms after Esc, ${lenAtPress} chars at press, saved status=${saved?.status}`,
  );
  report('stop: UI returns to Send', !ui.stop && ui.send, `stop visible=${ui.stop}, send visible=${ui.send}`);
  report('stop: partial reply kept', (saved?.content.length ?? 0) + (saved?.reasoning.length ?? 0) > 0, `badge: "${r.snap.footer}"`);
}

async function testUi(cdp, model) {
  // Drag-and-drop a text file.
  await newChat(cdp);
  await selectModel(cdp, model);
  const overlay = await cdp.eval(`(() => {
    const dt = new DataTransfer();
    dt.items.add(new File(['The secret word is PINEAPPLE.\\n'], 'notes.txt', { type: 'text/plain' }));
    window.dispatchEvent(new DragEvent('dragenter', { dataTransfer: dt }));
    const shown = !document.querySelector('#drop-overlay').hidden;
    window.dispatchEvent(new DragEvent('drop', { dataTransfer: dt }));
    return shown;
  })()`);
  const chip = await waitFor(cdp, `document.querySelector('.chip-name')?.textContent`, 5000, 'attachment chip');
  await typeAndSend(cdp, 'What is the secret word in the attached file? Answer with just the word.');
  const r = await awaitReply(cdp, 300_000);
  const userText = await cdp.eval(`[...document.querySelectorAll('.message.user')].at(-1)?.innerText ?? ''`);
  report(
    'drag & drop: file contents included',
    overlay && chip === 'notes.txt' && userText.includes('notes.txt') && /pineapple/i.test(r.snap.content),
    `overlay=${overlay}, chip=${chip}, answer="${r.snap.content.trim().slice(0, 40)}"`,
  );

  // Markdown + syntax highlighting + copy button.
  await newChat(cdp);
  await typeAndSend(cdp, 'Write a Python function named add that returns the sum of two numbers. Reply with only a fenced python code block.');
  await awaitReply(cdp, 300_000);
  const code = await cdp.eval(`(async () => {
    const block = [...document.querySelectorAll('.message.assistant .code-block')].at(-1);
    if (!block) return null;
    const btn = block.querySelector('.copy-btn');
    btn.click();
    await new Promise(r => setTimeout(r, 300)); // the copy is async
    return { lang: block.querySelector('.code-lang')?.textContent, spans: block.querySelectorAll('[class^="hljs-"]').length, copied: btn.textContent };
  })()`);
  report('markdown: highlighted code block with copy', !!code && code.spans > 0 && code.copied === 'Copied!', JSON.stringify(code));
  await screenshot(cdp, 'code-light');
  await cdp.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'dark' }] });
  await sleep(300);
  await screenshot(cdp, 'code-dark');
  await cdp.send('Emulation.setEmulatedMedia', { features: [] });

  // Rename via the sidebar.
  const convId = await activeConversationId(cdp);
  const newTitle = `Renamed ${Date.now() % 100000}`;
  await cdp.eval(`document.querySelector('.conv-item.active .conv-rename').click()`);
  await waitFor(cdp, `document.activeElement?.classList.contains('rename-input')`, 3000, 'rename input');
  await cdp.send('Input.insertText', { text: newTitle });
  await cdp.key('Enter', 'Enter', 13, 0, '\r');
  await waitFor(
    cdp,
    `document.querySelector('.conv-item[data-id="${convId}"] .conv-title')?.textContent === ${JSON.stringify(newTitle)}`,
    5000,
    'renamed title',
  );
  report('sidebar: rename', true, newTitle);

  // Search (titles and message text).
  await cdp.eval(`document.querySelector('#search').focus()`);
  await cdp.send('Input.insertText', { text: newTitle });
  const found = await waitFor(
    cdp,
    `(() => { const ids = [...document.querySelectorAll('.conv-item')].map(li => Number(li.dataset.id)); return ids.length === 1 && ids[0] === ${convId} ? ids : null; })()`,
    5000,
    'search by title',
  );
  await cdp.eval(`(() => { const s = document.querySelector('#search'); s.value = 'PINEAPPLE'; s.dispatchEvent(new Event('input')); })()`);
  const byContent = await waitFor(
    cdp,
    `(() => { const n = document.querySelectorAll('.conv-item').length; return n >= 1 && n < ${ONLY.has('chat') ? 3 : 99} ? n : 0; })()`,
    5000,
    'search by message text',
  );
  await cdp.eval(`(() => { const s = document.querySelector('#search'); s.value = ''; s.dispatchEvent(new Event('input')); })()`);
  await sleep(400);
  report('sidebar: search', found.length === 1 && byContent >= 1, `title match ${found.length}, content match ${byContent}`);

  // Delete with confirmation.
  await cdp.eval(`document.querySelector('.conv-item[data-id="${convId}"] .conv-delete').click()`);
  await waitFor(cdp, `document.querySelector('#confirm-dialog').open`, 3000, 'confirm dialog');
  await screenshot(cdp, 'delete-confirm');
  await cdp.eval(`document.querySelector('#confirm-dialog button[value=ok]').click()`);
  await waitFor(cdp, `!document.querySelector('.conv-item[data-id="${convId}"]')`, 5000, 'conversation removed');
  const remaining = await cdp.eval(invoke('list_conversations', { query: null }));
  report('sidebar: delete', !remaining.some((c) => c.id === convId), `${remaining.length} conversations remain`);

  // Settings panel round trip.
  await cdp.eval(`document.querySelector('#open-settings').click()`);
  await waitFor(cdp, `document.querySelector('#settings-dialog').open`, 3000, 'settings dialog');
  await screenshot(cdp, 'settings');
  const settingsOk = await cdp.eval(`(async () => {
    const before = await ${invoke('get_settings')};
    const setTemp = async (v) => {
      if (!document.querySelector('#settings-dialog').open) document.querySelector('#open-settings').click();
      const t = document.querySelector('#set-temp');
      t.value = String(v); t.dispatchEvent(new Event('input'));
      document.querySelector('#settings-form').requestSubmit();
      await new Promise(r => setTimeout(r, 500));
    };
    await setTemp(before.temperature === 0.55 ? 0.6 : 0.55);
    const after = await ${invoke('get_settings')};
    const closed = !document.querySelector('#settings-dialog').open;
    await setTemp(before.temperature); // restore through the UI so the app's copy matches
    const restored = (await ${invoke('get_settings')}).temperature === before.temperature;
    return { closed, changed: after.temperature !== before.temperature, restored };
  })()`);
  report('settings: save', settingsOk.closed && settingsOk.changed && settingsOk.restored, JSON.stringify(settingsOk));
  const badUrl = await cdp.eval(`(async () => {
    try { await ${invoke('save_settings', { settings: { baseUrl: 'https://api.example.com/v1' } })}; return 'accepted'; }
    catch (e) { return String(e); }
  })()`);
  report('settings: rejects non-local server URL', badUrl !== 'accepted', badUrl);
}

async function testIdentity(cdp, models) {
  const title = await cdp.eval('document.title');
  const heading = await cdp.eval(`(document.querySelector('#messages .empty h1') ?? {}).textContent ?? ''`);
  report('identity: app is named Democritus', title === 'Democritus', `title="${title}", heading="${heading}"`);
  for (const m of models) {
    await newChat(cdp);
    await selectModel(cdp, m);
    await typeAndSend(cdp, 'What are you, and what is your brain right now? Answer in two sentences.');
    const r = await awaitReply(cdp, 900_000);
    const answer = r.snap.content.replace(/\s+/g, ' ').trim();
    report(
      `identity ${m}: says it is Democritus with a Qwen brain`,
      r.snap.status === 'complete' && /democritus/i.test(answer) && /qwen/i.test(answer),
      `"${answer.slice(0, 220)}"`,
    );
  }
}

async function testMigration(cdp) {
  const convs = await cdp.eval(invoke('list_conversations', { query: null }));
  const legacy = convs.find((c) => c.title === 'Legacy chat about tomatoes');
  const msgs = legacy ? await savedMessages(cdp, legacy.id) : [];
  const settings = await cdp.eval(invoke('get_settings'));
  report(
    'migration: old conversations and settings carried over',
    !!legacy && msgs.length === 2 && settings.systemPrompt.startsWith('Legacy prompt') && settings.temperature === 0.65,
    `conversation=${!!legacy}, messages=${msgs.length}, prompt="${settings.systemPrompt}", temperature=${settings.temperature}`,
  );
}

async function testPersisted(cdp) {
  const convs = await cdp.eval(invoke('list_conversations', { query: null }));
  report('persistence: conversations survive restart', convs.length > 0, `${convs.length} conversations: ${convs.map((c) => c.title).slice(0, 5).join(' | ')}`);
  if (!convs.length) return;
  await cdp.eval(`document.querySelector('.conv-item .conv-open').click()`);
  const n = await waitFor(cdp, `document.querySelectorAll('#messages .message').length`, 5000, 'messages rendered');
  report('persistence: messages render after restart', n >= 2, `${n} messages in "${convs[0].title}"`);
  await screenshot(cdp, 'after-restart');
}

// ---------- main ----------

const page = await findPage();
console.log(`Connected to ${page.url}`);
const cdp = await Cdp.connect(page.webSocketDebuggerUrl);
const requests = [];
cdp.on('Network.requestWillBeSent', (p) => requests.push(p.request.url));
await cdp.send('Network.enable');
await cdp.send('Runtime.enable');

try {
  const status = await waitFor(
    cdp,
    `document.querySelector('#server-status').dataset.state === 'connected' && document.querySelector('#server-status').title`,
    120_000,
    'model server connection',
  );
  report('server: connected', true, status);
  const options = await waitFor(
    cdp,
    `(() => { const o = [...document.querySelectorAll('#model-select option')].filter(o => o.value).map(o => ({ id: o.value, label: o.textContent })); return o.length ? o : null; })()`,
    30_000,
    'model list',
  );
  report('models: listed with sizes', options.every((o) => /GB|MB/.test(o.label)), options.map((o) => o.label).join(' | '));
  const models = args.models ? args.models.split(',') : options.map((o) => o.id);
  const fast = args.fast ?? options[0].id;

  if (ONLY.has('migration')) await testMigration(cdp).catch((e) => report('migration', false, e.message));
  if (ONLY.has('identity')) await testIdentity(cdp, models).catch((e) => report('identity', false, e.message));
  if (ONLY.has('persisted')) await testPersisted(cdp);
  if (ONLY.has('chat')) {
    for (const m of models) {
      try {
        await testChat(cdp, m);
      } catch (e) {
        report(`chat ${m}`, false, e.message);
      }
    }
    await screenshot(cdp, 'chat');
  }
  if (ONLY.has('stop')) await testStop(cdp, fast).catch((e) => report('stop', false, e.message));
  if (ONLY.has('ui')) await testUi(cdp, fast).catch((e) => report('ui', false, e.message));

  const external = requests.filter((u) => !/^(https?:\/\/(localhost|127\.0\.0\.1|ipc\.localhost|tauri\.localhost)[:/]|ipc:|tauri:|data:|blob:)/.test(u));
  report('privacy: no external network requests from the UI', external.length === 0, `${requests.length} requests, external: ${external.join(', ') || 'none'}`);
} finally {
  cdp.close();
}

console.log(`\n${results.length - failures}/${results.length} checks passed`);
process.exit(failures ? 1 : 0);
