import './styles.css';
import { api, type Conversation, type Message, type ModelInfo, type ServerStatus, type Settings, type StreamEvent } from './api';
import { renderMarkdown } from './markdown';
import { icons, logo } from './icons';

const isMac = navigator.userAgent.includes('Mac');
const modKey = isMac ? '⌘' : 'Ctrl';
const MAX_FILE_BYTES = 1_000_000;

function $<T extends HTMLElement = HTMLElement>(selector: string): T {
  const el = document.querySelector<T>(selector);
  if (!el) throw new Error(`Missing element ${selector}`);
  return el;
}

const els = {
  newChat: $<HTMLButtonElement>('#new-chat'),
  search: $<HTMLInputElement>('#search'),
  convList: $<HTMLUListElement>('#conversation-list'),
  sidebarEmpty: $('#sidebar-empty'),
  modelSelect: $<HTMLSelectElement>('#model-select'),
  refreshModels: $<HTMLButtonElement>('#refresh-models'),
  status: $<HTMLButtonElement>('#server-status'),
  statusLabel: $('#server-status .label'),
  openSettings: $<HTMLButtonElement>('#open-settings'),
  banner: $('#banner'),
  bannerText: $('#banner .banner-text'),
  bannerStart: $<HTMLButtonElement>('#banner-start'),
  messages: $('#messages'),
  dropOverlay: $('#drop-overlay'),
  attachments: $('#attachments'),
  input: $<HTMLTextAreaElement>('#input'),
  send: $<HTMLButtonElement>('#send'),
  stop: $<HTMLButtonElement>('#stop'),
  composerHint: $('.composer-hint'),
  settingsDialog: $<HTMLDialogElement>('#settings-dialog'),
  settingsForm: $<HTMLFormElement>('#settings-form'),
  settingsCancel: $<HTMLButtonElement>('#settings-cancel'),
  settingsError: $('#settings-error'),
  setSystem: $<HTMLTextAreaElement>('#set-system'),
  setTemp: $<HTMLInputElement>('#set-temp'),
  setTempOut: $<HTMLOutputElement>('#set-temp-out'),
  setMax: $<HTMLInputElement>('#set-max'),
  setUrl: $<HTMLInputElement>('#set-url'),
  setCmd: $<HTMLInputElement>('#set-cmd'),
  setAutostart: $<HTMLInputElement>('#set-autostart'),
  confirmDialog: $<HTMLDialogElement>('#confirm-dialog'),
  confirmText: $('#confirm-text'),
  toasts: $('#toasts'),
};

interface Attachment {
  name: string;
  content: string;
}

interface Generation {
  requestId: string;
  conversationId: number;
  /** The live assistant message; mutated as tokens arrive. */
  message: Message;
  el: HTMLElement | null;
  stopRequested: boolean;
  finished: Promise<void>;
  markFinished: () => void;
}

const state = {
  settings: null as Settings | null,
  models: [] as ModelInfo[],
  conversations: [] as Conversation[],
  currentId: null as number | null,
  messages: [] as Message[],
  attachments: [] as Attachment[],
  generation: null as Generation | null,
  server: null as ServerStatus | null,
  query: '',
};

/** The message each rendered element currently shows. */
const messageOf = new WeakMap<HTMLElement, Message>();

// ---------- helpers ----------

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

function errorText(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}

function newRequestId(): string {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function formatBytes(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)} MB`;
  if (n >= 1e3) return `${Math.round(n / 1e3)} KB`;
  return `${n} B`;
}

function relativeTime(ms: number): string {
  const d = new Date(ms);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  if (now.getTime() - ms < 6 * 86_400_000) return d.toLocaleDateString([], { weekday: 'short' });
  return d.toLocaleDateString([], { month: 'short', day: 'numeric' });
}

function toast(text: string, kind: 'info' | 'error' = 'info', ms = 4500) {
  const t = document.createElement('div');
  t.className = `toast ${kind}`;
  t.textContent = text;
  els.toasts.append(t);
  setTimeout(() => t.classList.add('leaving'), ms);
  setTimeout(() => t.remove(), ms + 300);
}

function askConfirm(text: string): Promise<boolean> {
  els.confirmText.textContent = text;
  els.confirmDialog.returnValue = '';
  els.confirmDialog.showModal();
  return new Promise((resolve) =>
    els.confirmDialog.addEventListener('close', () => resolve(els.confirmDialog.returnValue === 'ok'), { once: true }),
  );
}

function nearBottom(): boolean {
  const m = els.messages;
  return m.scrollHeight - m.scrollTop - m.clientHeight < 120;
}

function scrollToBottom() {
  els.messages.scrollTop = els.messages.scrollHeight;
}

async function copyText(text: string, button: HTMLElement) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.append(ta);
    ta.select();
    document.execCommand('copy');
    ta.remove();
  }
  const original = button.innerHTML;
  button.classList.add('copied');
  button.textContent = 'Copied!';
  setTimeout(() => {
    button.innerHTML = original;
    button.classList.remove('copied');
  }, 1500);
}

// ---------- server status ----------

let statusTimer: number | undefined;

async function pollStatus() {
  window.clearTimeout(statusTimer);
  try {
    applyStatus(await api.serverStatus());
  } catch (e) {
    console.error('server_status failed', e);
  }
  statusTimer = window.setTimeout(pollStatus, state.server?.state === 'connected' ? 8000 : 1500);
}

function applyStatus(s: ServerStatus) {
  const previous = state.server?.state;
  state.server = s;
  els.status.dataset.state = s.state;
  els.statusLabel.textContent =
    s.state === 'connected' ? 'Connected' : s.state === 'starting' ? 'Starting server…' : 'Disconnected';
  els.status.title =
    s.state === 'connected'
      ? `Model server at ${s.baseUrl}${s.startedByApp ? ' (started by this app)' : ''}`
      : s.state === 'starting'
        ? `Starting the model server at ${s.baseUrl}`
        : 'Click to start the model server';
  els.banner.hidden = s.state !== 'disconnected';
  if (s.state === 'disconnected') {
    els.bannerText.textContent = s.error
      ? `Couldn't start the model server. ${s.error}`
      : `No model server is answering at ${s.baseUrl}.`;
  }
  if (s.state === 'connected' && previous !== 'connected') void loadModels();
}

async function startServer() {
  if (state.server?.state === 'starting') return;
  const base = state.server ?? { startedByApp: false, baseUrl: state.settings?.baseUrl ?? '', error: null };
  applyStatus({ ...base, state: 'starting', error: null });
  try {
    applyStatus(await api.startServer());
  } catch (e) {
    applyStatus({ ...base, state: 'disconnected', error: errorText(e) });
  }
}

// ---------- models ----------

function modelLabel(m: ModelInfo): string {
  const details = [m.parameterSize, m.quantization, m.sizeBytes != null ? formatBytes(m.sizeBytes) : null].filter(Boolean);
  return details.length ? `${m.id}  —  ${details.join(' · ')}` : m.id;
}

async function loadModels() {
  try {
    state.models = await api.listModels();
  } catch (e) {
    state.models = [];
    if (state.server?.state === 'connected') toast(errorText(e), 'error');
  }
  renderModels();
}

function renderModels() {
  const sel = els.modelSelect;
  const previous = sel.value;
  sel.replaceChildren();
  if (state.models.length === 0) {
    sel.add(new Option(state.server?.state === 'connected' ? 'No models installed' : 'Server not connected', ''));
    sel.disabled = true;
    return;
  }
  sel.disabled = false;
  for (const m of state.models) sel.add(new Option(modelLabel(m), m.id));
  const conv = state.conversations.find((c) => c.id === state.currentId);
  selectModel(conv?.model || previous || state.settings?.defaultModel);
}

function selectModel(id?: string | null) {
  const has = (x?: string | null) => !!x && state.models.some((m) => m.id === x);
  const fallback = state.settings?.defaultModel;
  if (has(id)) els.modelSelect.value = id!;
  else if (has(fallback)) els.modelSelect.value = fallback!;
  else if (state.models.length) els.modelSelect.value = state.models[0].id;
}

// ---------- sidebar ----------

async function refreshConversations() {
  try {
    state.conversations = await api.listConversations(state.query);
  } catch (e) {
    toast(`Couldn't load conversations: ${errorText(e)}`, 'error');
  }
  renderConversations();
}

function renderConversations() {
  const items = state.conversations.map((c) => {
    const li = document.createElement('li');
    li.className = 'conv-item';
    li.dataset.id = String(c.id);
    li.classList.toggle('active', c.id === state.currentId);
    li.classList.toggle('busy', state.generation?.conversationId === c.id);
    li.innerHTML =
      `<button type="button" class="conv-open"><span class="conv-title"></span><span class="conv-date"></span></button>` +
      `<span class="conv-actions"><button type="button" class="icon-btn conv-rename" title="Rename">${icons.edit}</button>` +
      `<button type="button" class="icon-btn conv-delete" title="Delete">${icons.trash}</button></span>`;
    li.querySelector('.conv-title')!.textContent = c.title;
    li.querySelector('.conv-date')!.textContent = relativeTime(c.updatedAt);
    return li;
  });
  els.convList.replaceChildren(...items);
  els.sidebarEmpty.hidden = items.length > 0;
  els.sidebarEmpty.textContent = state.query ? 'No chats match your search' : 'No conversations yet';
}

function startRename(li: HTMLElement, id: number) {
  const conv = state.conversations.find((c) => c.id === id);
  if (!conv || li.querySelector('input')) return;
  const input = document.createElement('input');
  input.className = 'rename-input';
  input.value = conv.title;
  input.maxLength = 80;
  li.querySelector('.conv-open')!.replaceWith(input);
  input.focus();
  input.select();
  let done = false;
  const finish = async (save: boolean) => {
    if (done) return;
    done = true;
    const title = input.value.trim();
    if (save && title && title !== conv.title) {
      try {
        await api.renameConversation(id, title);
      } catch (e) {
        toast(`Couldn't rename: ${errorText(e)}`, 'error');
      }
    }
    await refreshConversations();
  };
  input.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter') {
      ev.preventDefault();
      void finish(true);
    } else if (ev.key === 'Escape') {
      ev.preventDefault();
      ev.stopPropagation();
      void finish(false);
    }
  });
  input.addEventListener('blur', () => void finish(true));
}

async function deleteConversation(id: number) {
  const conv = state.conversations.find((c) => c.id === id);
  if (!conv || !(await askConfirm(`Delete “${conv.title}”? This can't be undone.`))) return;
  const g = state.generation;
  if (g && g.conversationId === id) {
    await api.stopGeneration(g.requestId);
    await Promise.race([g.finished, sleep(3000)]);
  }
  try {
    await api.deleteConversation(id);
  } catch (e) {
    toast(`Couldn't delete: ${errorText(e)}`, 'error');
    return;
  }
  if (state.currentId === id) newChat();
  await refreshConversations();
}

// ---------- conversations ----------

function newChat() {
  state.currentId = null;
  state.messages = [];
  renderMessages();
  renderConversations();
  selectModel(state.settings?.defaultModel);
  els.input.focus();
}

async function openConversation(id: number) {
  try {
    state.messages = await api.getMessages(id);
    state.currentId = id;
  } catch (e) {
    toast(`Couldn't open the conversation: ${errorText(e)}`, 'error');
    return;
  }
  const conv = state.conversations.find((c) => c.id === id);
  if (conv?.model) selectModel(conv.model);
  renderConversations();
  renderMessages();
  els.input.focus();
}

// ---------- messages ----------

function emptyState(): HTMLElement {
  const div = document.createElement('div');
  div.className = 'empty';
  div.innerHTML =
    `${logo}<h1>Local Qwen</h1>` +
    `<p class="muted">Chat with Qwen models running on this computer. Nothing leaves your machine.</p>` +
    `<ul class="hints"><li><kbd>Enter</kbd> send</li><li><kbd>Shift</kbd> + <kbd>Enter</kbd> new line</li>` +
    `<li><kbd>${modKey}</kbd> + <kbd>N</kbd> new chat</li><li><kbd>Esc</kbd> stop generating</li></ul>` +
    `<p class="muted small">Drop text files anywhere in the window to include them in your message.</p>`;
  return div;
}

function renderMessages() {
  const g = state.generation;
  if (state.messages.length === 0) {
    els.messages.replaceChildren(emptyState());
    return;
  }
  const items = state.messages.map((m, i) => {
    // While a reply streams, the saved copy is a stale placeholder: show the live one.
    const live = g && g.message.id === m.id ? g.message : m;
    state.messages[i] = live;
    const el = messageEl(live);
    if (g && live === g.message) g.el = el;
    return el;
  });
  els.messages.replaceChildren(...items);
  scrollToBottom();
}

function messageEl(m: Message): HTMLElement {
  const el = document.createElement('article');
  el.className = `message ${m.role}`;
  el.innerHTML =
    m.role === 'assistant'
      ? `<div class="meta"><span class="who"></span></div>` +
        `<details class="thinking" hidden><summary><span class="thinking-label"></span></summary><div class="thinking-body md"></div></details>` +
        `<div class="body md"></div><div class="error-box" hidden></div><div class="footer"></div>`
      : `<div class="bubble"><div class="body md"></div></div>`;
  updateMessageEl(el, m);
  return el;
}

function updateMessageEl(el: HTMLElement, m: Message) {
  messageOf.set(el, m);
  el.dataset.id = String(m.id);
  el.dataset.status = m.status;
  const body = el.querySelector<HTMLElement>('.body')!;
  if (m.role === 'user') {
    body.innerHTML = renderMarkdown(m.content);
    return;
  }

  el.querySelector('.who')!.textContent = m.model || 'Assistant';
  const streaming = m.status === 'streaming';
  const content = m.content.replace(/^\s+/, '');
  const reasoning = m.reasoning.trim();

  const thinking = el.querySelector<HTMLDetailsElement>('.thinking')!;
  thinking.hidden = reasoning === '';
  if (reasoning) {
    const active = streaming && content === '';
    thinking.classList.toggle('active', active);
    el.querySelector('.thinking-label')!.textContent = active ? 'Thinking…' : 'Thought process';
    // Closed sections are rendered lazily when opened.
    if (thinking.open) renderThinking(thinking, m);
  }

  const waiting = streaming && !content && !reasoning;
  body.hidden = !content && !waiting;
  body.innerHTML = content ? renderMarkdown(content) : waiting ? '<span class="typing"><i></i><i></i><i></i></span>' : '';

  const errorBox = el.querySelector<HTMLElement>('.error-box')!;
  errorBox.hidden = !m.error;
  errorBox.textContent = m.error ?? '';

  el.querySelector('.footer')!.innerHTML = streaming ? '' : footerHtml(m);
}

function renderThinking(details: HTMLDetailsElement, m: Message) {
  details.querySelector('.thinking-body')!.innerHTML = renderMarkdown(m.reasoning.trim());
}

function footerHtml(m: Message): string {
  const bits: string[] = [];
  if (m.status === 'stopped') bits.push('<span class="badge">Stopped</span>');
  if (m.stats?.finishReason === 'length') bits.push('<span class="badge warn">Hit max-token limit</span>');
  if (m.stats?.tokensPerSecond) bits.push(`<span>${m.stats.tokensPerSecond} tok/s</span>`);
  if (m.stats?.completionTokens) bits.push(`<span>${m.stats.completionTokens} tokens</span>`);
  if (m.content.trim()) bits.push(`<button type="button" class="link-btn copy-message">${icons.copy}Copy</button>`);
  return bits.join('<span class="sep">·</span>');
}

// Streaming re-renders are throttled to keep long replies smooth.
let liveTimer: number | null = null;
let lastLive = 0;

function scheduleLive() {
  if (liveTimer !== null) return;
  const wait = Math.max(0, 60 - (performance.now() - lastLive));
  liveTimer = window.setTimeout(
    () =>
      requestAnimationFrame(() => {
        liveTimer = null;
        lastLive = performance.now();
        renderLive();
      }),
    wait,
  );
}

function renderLive() {
  const g = state.generation;
  if (!g?.el?.isConnected) return;
  const stick = nearBottom();
  updateMessageEl(g.el, g.message);
  if (stick) scrollToBottom();
}

// ---------- sending ----------

function buildContent(text: string, files: Attachment[]): string {
  const parts = text ? [text] : [];
  for (const f of files) {
    let longest = 0;
    for (const run of f.content.matchAll(/`+/g)) longest = Math.max(longest, run[0].length);
    const fence = '`'.repeat(Math.max(3, longest + 1));
    const ext = f.name.includes('.') ? f.name.split('.').pop()!.toLowerCase() : '';
    parts.push(`**File: ${f.name}**\n\n${fence}${ext}\n${f.content.replace(/\s+$/, '')}\n${fence}`);
  }
  return parts.join('\n\n');
}

function setGenerating(on: boolean) {
  els.send.hidden = on;
  els.stop.hidden = !on;
  els.stop.disabled = false;
  document.body.classList.toggle('generating', on);
}

async function send() {
  if (state.generation) {
    toast('Wait for the current reply to finish, or press Esc to stop it.');
    return;
  }
  const text = els.input.value.trim();
  const files = state.attachments;
  if (!text && files.length === 0) return;
  const model = els.modelSelect.value;
  if (!model) {
    toast('No model selected. Is the model server running?', 'error');
    return;
  }

  let conversationId = state.currentId;
  if (conversationId === null) {
    try {
      const title = text.split('\n')[0] || `Files: ${files.map((f) => f.name).join(', ')}`;
      const conv = await api.createConversation(title, model);
      conversationId = conv.id;
      state.currentId = conv.id;
      state.messages = [];
    } catch (e) {
      toast(`Couldn't create the conversation: ${errorText(e)}`, 'error');
      return;
    }
  }

  const content = buildContent(text, files);
  els.input.value = '';
  autoGrow();
  state.attachments = [];
  renderAttachments();

  let markFinished!: () => void;
  const finished = new Promise<void>((r) => (markFinished = r));
  const g: Generation = {
    requestId: newRequestId(),
    conversationId,
    message: {
      id: -1,
      conversationId,
      role: 'assistant',
      content: '',
      reasoning: '',
      model,
      status: 'streaming',
      error: null,
      stats: null,
      createdAt: Date.now(),
    },
    el: null,
    stopRequested: false,
    finished,
    markFinished,
  };
  state.generation = g;
  let started = false;
  setGenerating(true);
  await refreshConversations();

  try {
    await api.sendMessage({ requestId: g.requestId, conversationId, model, content }, (ev) => {
      if (ev.type === 'started') started = true;
      onStreamEvent(g, ev);
    });
    // Channel messages can trail the command's return value by a moment.
    await Promise.race([finished, sleep(3000)]);
  } catch (e) {
    if (!started) {
      els.input.value = text;
      state.attachments = files;
      renderAttachments();
      autoGrow();
    }
    toast(errorText(e), 'error');
  } finally {
    const stale = g.message.status === 'streaming';
    state.generation = null;
    setGenerating(false);
    if (stale && state.currentId === conversationId) await openConversation(conversationId);
    await refreshConversations();
  }
}

function onStreamEvent(g: Generation, ev: StreamEvent) {
  const m = g.message;
  // After Stop, tokens still queued in the channel are not drawn; the final
  // `done` message carries exactly what was saved.
  if (g.stopRequested && ev.type !== 'started' && ev.type !== 'done') return;
  switch (ev.type) {
    case 'started': {
      m.id = ev.assistantMessageId;
      if (state.currentId !== g.conversationId) break;
      if (state.messages.length === 0) els.messages.replaceChildren();
      state.messages.push(ev.userMessage, m);
      const userEl = messageEl(ev.userMessage);
      g.el = messageEl(m);
      els.messages.append(userEl, g.el);
      scrollToBottom();
      break;
    }
    case 'reasoning':
      m.reasoning += ev.text;
      scheduleLive();
      break;
    case 'content':
      m.content += ev.text;
      scheduleLive();
      break;
    case 'reclassify':
      m.reasoning += m.content;
      m.content = '';
      scheduleLive();
      break;
    case 'done': {
      Object.assign(m, ev.message);
      if (g.el?.isConnected) {
        const stick = nearBottom();
        updateMessageEl(g.el, m);
        if (stick) scrollToBottom();
      }
      g.markFinished();
      break;
    }
  }
}

async function stop() {
  const g = state.generation;
  if (!g) return;
  g.stopRequested = true;
  els.stop.disabled = true;
  try {
    await api.stopGeneration(g.requestId);
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

// ---------- attachments (drag & drop) ----------

function renderAttachments() {
  const chips = state.attachments.map((a, i) => {
    const chip = document.createElement('span');
    chip.className = 'chip';
    chip.innerHTML =
      `${icons.file}<span class="chip-name"></span><span class="chip-size muted"></span>` +
      `<button type="button" class="chip-remove" title="Remove">${icons.close}</button>`;
    chip.querySelector('.chip-name')!.textContent = a.name;
    chip.querySelector('.chip-size')!.textContent = formatBytes(new Blob([a.content]).size);
    chip.querySelector('button')!.addEventListener('click', () => {
      state.attachments.splice(i, 1);
      renderAttachments();
    });
    return chip;
  });
  els.attachments.replaceChildren(...chips);
  els.attachments.hidden = chips.length === 0;
}

async function addAttachment(file: File) {
  if (file.size > MAX_FILE_BYTES) {
    toast(`${file.name} is too large (limit ${formatBytes(MAX_FILE_BYTES)}).`, 'error');
    return;
  }
  const text = await file.text();
  const replacement = (text.match(/�/g) ?? []).length;
  if (text.includes('\u0000') || replacement > Math.max(8, text.length * 0.01)) {
    toast(`${file.name} doesn't look like a text file.`, 'error');
    return;
  }
  state.attachments.push({ name: file.name, content: text });
  renderAttachments();
  els.input.focus();
}

const hasFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes('Files');
let dragDepth = 0;

window.addEventListener('dragenter', (e) => {
  if (!hasFiles(e)) return;
  dragDepth++;
  els.dropOverlay.hidden = false;
});
window.addEventListener('dragleave', (e) => {
  if (!hasFiles(e)) return;
  dragDepth = Math.max(0, dragDepth - 1);
  if (dragDepth === 0) els.dropOverlay.hidden = true;
});
// Always cancel the default so a dropped file never navigates the webview.
window.addEventListener('dragover', (e) => {
  e.preventDefault();
  if (e.dataTransfer) e.dataTransfer.dropEffect = hasFiles(e) ? 'copy' : 'none';
});
window.addEventListener('drop', async (e) => {
  e.preventDefault();
  dragDepth = 0;
  els.dropOverlay.hidden = true;
  for (const file of Array.from(e.dataTransfer?.files ?? [])) await addAttachment(file);
});

// ---------- settings ----------

function openSettings() {
  const s = state.settings;
  if (!s) return;
  els.setSystem.value = s.systemPrompt;
  els.setTemp.value = String(s.temperature);
  els.setTempOut.textContent = s.temperature.toFixed(2);
  els.setMax.value = String(s.maxTokens);
  els.setUrl.value = s.baseUrl;
  els.setCmd.value = s.serverCommand;
  els.setAutostart.checked = s.autoStartServer;
  els.settingsError.hidden = true;
  els.settingsDialog.showModal();
  els.setSystem.focus();
}

async function saveSettings(e: SubmitEvent) {
  e.preventDefault();
  const current = state.settings;
  if (!current) return;
  const maxTokens = Number(els.setMax.value);
  if (!Number.isInteger(maxTokens) || maxTokens < 1) {
    els.settingsError.textContent = 'Max tokens must be a whole number of at least 1.';
    els.settingsError.hidden = false;
    return;
  }
  const next: Settings = {
    ...current,
    systemPrompt: els.setSystem.value,
    temperature: Number(els.setTemp.value),
    maxTokens,
    baseUrl: els.setUrl.value,
    serverCommand: els.setCmd.value,
    autoStartServer: els.setAutostart.checked,
  };
  try {
    state.settings = await api.saveSettings(next);
  } catch (err) {
    els.settingsError.textContent = errorText(err);
    els.settingsError.hidden = false;
    return;
  }
  els.settingsDialog.close();
  toast('Settings saved');
  if (state.settings.baseUrl !== current.baseUrl) {
    state.server = null;
    state.models = [];
    renderModels();
    void pollStatus();
  }
}

// ---------- wiring ----------

function autoGrow() {
  els.input.style.height = 'auto';
  els.input.style.height = `${Math.min(els.input.scrollHeight, 240)}px`;
}

els.newChat.addEventListener('click', newChat);
els.send.addEventListener('click', () => void send());
els.stop.addEventListener('click', () => void stop());
els.refreshModels.addEventListener('click', () => void loadModels());
els.openSettings.addEventListener('click', openSettings);
els.settingsCancel.addEventListener('click', () => els.settingsDialog.close());
els.settingsForm.addEventListener('submit', (e) => void saveSettings(e));
els.setTemp.addEventListener('input', () => (els.setTempOut.textContent = Number(els.setTemp.value).toFixed(2)));
els.bannerStart.addEventListener('click', () => void startServer());
els.status.addEventListener('click', () => {
  if (state.server?.state === 'disconnected') void startServer();
  else void pollStatus();
});

els.input.addEventListener('input', autoGrow);
els.input.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    void send();
  }
});

els.modelSelect.addEventListener('change', async () => {
  const model = els.modelSelect.value;
  try {
    if (state.currentId !== null) await api.setConversationModel(state.currentId, model);
    if (state.settings) state.settings = await api.saveSettings({ ...state.settings, defaultModel: model });
  } catch (e) {
    toast(errorText(e), 'error');
  }
});

let searchTimer: number | undefined;
els.search.addEventListener('input', () => {
  window.clearTimeout(searchTimer);
  searchTimer = window.setTimeout(() => {
    state.query = els.search.value.trim();
    void refreshConversations();
  }, 200);
});

els.convList.addEventListener('click', (e) => {
  const target = e.target as HTMLElement;
  const li = target.closest<HTMLElement>('.conv-item');
  if (!li) return;
  const id = Number(li.dataset.id);
  if (target.closest('.conv-rename')) startRename(li, id);
  else if (target.closest('.conv-delete')) void deleteConversation(id);
  else if (target.closest('.conv-open') && id !== state.currentId) void openConversation(id);
});
els.convList.addEventListener('dblclick', (e) => {
  const li = (e.target as HTMLElement).closest<HTMLElement>('.conv-item');
  if (li && (e.target as HTMLElement).closest('.conv-open')) startRename(li, Number(li.dataset.id));
});

els.messages.addEventListener('click', (e) => {
  const target = e.target as HTMLElement;
  if (target.closest('a')) {
    e.preventDefault(); // never navigate away from the app
    return;
  }
  const codeCopy = target.closest<HTMLElement>('.copy-btn');
  if (codeCopy) {
    void copyText(codeCopy.closest('.code-block')?.querySelector('code')?.textContent ?? '', codeCopy);
    return;
  }
  const msgCopy = target.closest<HTMLElement>('.copy-message');
  const msgEl = msgCopy?.closest<HTMLElement>('.message');
  const m = msgEl && messageOf.get(msgEl);
  if (msgCopy && m) void copyText(m.content.trim(), msgCopy);
});
// <details> toggle doesn't bubble; capture it to render thinking on open.
els.messages.addEventListener(
  'toggle',
  (e) => {
    const details = e.target;
    if (!(details instanceof HTMLDetailsElement) || !details.open) return;
    const el = details.closest<HTMLElement>('.message');
    const m = el && messageOf.get(el);
    if (m) renderThinking(details, m);
  },
  true,
);

window.addEventListener('keydown', (e) => {
  const dialogOpen = document.querySelector('dialog[open]') !== null;
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'n') {
    e.preventDefault();
    if (!dialogOpen) newChat();
  } else if (e.key === 'Escape' && state.generation && !dialogOpen) {
    e.preventDefault();
    void stop();
  }
});
window.addEventListener('focus', () => void pollStatus());

async function init() {
  els.newChat.innerHTML = `${icons.plus}<span>New chat</span>`;
  els.newChat.title = `New chat (${modKey}+N)`;
  els.refreshModels.innerHTML = icons.refresh;
  els.openSettings.innerHTML = icons.settings;
  els.composerHint.innerHTML =
    '<kbd>Enter</kbd> send · <kbd>Shift</kbd>+<kbd>Enter</kbd> new line · <kbd>Esc</kbd> stop · drop text files to attach';
  try {
    state.settings = await api.getSettings();
  } catch (e) {
    toast(`Couldn't load settings: ${errorText(e)}`, 'error');
  }
  renderModels();
  renderMessages();
  renderAttachments();
  await refreshConversations();
  void pollStatus();
  els.input.focus();
}

void init();
