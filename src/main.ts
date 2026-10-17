// Fonts are bundled from npm (SIL Open Font License); nothing loads from the web.
import '@fontsource/italiana/400.css';
import '@fontsource/manrope/400.css';
import '@fontsource/manrope/500.css';
import '@fontsource/manrope/600.css';
import '@fontsource/manrope/700.css';
import '@fontsource/ibm-plex-mono/400.css';
import '@fontsource/ibm-plex-mono/500.css';
import './styles.css';
import {
  api,
  type Conversation,
  type LoadedModel,
  type Message,
  type ModelInfo,
  type ContextReport,
  type ProjectInfo,
  type ServerStatus,
  type Settings,
  type StreamEvent,
} from './api';
import { renderMarkdown } from './markdown';
import { icons, logo, logoUrl } from './icons';
import { initLogPage } from './logpage';
import { listen } from '@tauri-apps/api/event';
import { initMemoryPage, onPendingChange } from './memorypage';
import { initOperatorPage, loadOperator } from './operatorpage';
import { fillOrnaments, sparkle, starburst } from './ornaments';
import { currentPage, initRail, showPage } from './pages';
import { initCodePage } from './codepage';
import { initFirewallWarning, initSecurityCheck } from './securitypage';
import { applyTheme, initThemeSwitch, type ThemePref } from './theme';
import { $, askConfirm, askText, choose, errorText, escapeText, formatBytes, formatMem, newRequestId, relativeTime, sleep, toast } from './ui';

const isMac = navigator.userAgent.includes('Mac');
const modKey = isMac ? '⌘' : 'Ctrl';
const MAX_FILE_BYTES = 1_000_000;

const els = {
  newChat: $<HTMLButtonElement>('#new-chat'),
  search: $<HTMLInputElement>('#search'),
  convList: $('#conversation-list'),
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
  setCtx: $<HTMLInputElement>('#set-ctx'),
  setAgentCtx: $<HTMLInputElement>('#set-agent-ctx'),
  setUrl: $<HTMLInputElement>('#set-url'),
  setCmd: $<HTMLInputElement>('#set-cmd'),
  setAutostart: $<HTMLInputElement>('#set-autostart'),
  setHome: $<HTMLInputElement>('#set-home'),
  setHelper: $<HTMLSelectElement>('#set-helper'),
  projectSelect: $<HTMLSelectElement>('#project-select'),
  confirmDialog: $<HTMLDialogElement>('#confirm-dialog'),
  confirmText: $('#confirm-text'),
  confirmActions: $('#confirm-actions'),
  loadedBtn: $<HTMLButtonElement>('#loaded-btn'),
  loadedPanel: $('#loaded-panel'),
  toasts: $('#toasts'),
  convNumber: $('#conv-number'),
  convCaps: $('#conv-caps'),
  convTitleCaps: $('#conv-title-caps'),
  brainSpeed: $('#brain-speed'),
  contState: $('#cont-state'),
  contNext: $('#cont-next'),
  contStepsWrap: $('#cont-steps-wrap'),
  contStepsHead: $('#cont-steps-head'),
  contSteps: $('#cont-steps'),
  contRecalled: $('#cont-recalled'),
  attach: $<HTMLButtonElement>('#attach'),
  attachInput: $<HTMLInputElement>('#attach-input'),
};

/** Memory updates waiting for review (shown as a chip on the latest reply). */
let pendingUpdates = 0;

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
  /** Progress shown before the first token (e.g. "Summarizing…"). */
  statusText: string;
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
  loaded: [] as LoadedModel[],
  /** The model the operator last chose, used to unload it on a switch. */
  activeModel: '',
};

/** The message each rendered element currently shows. */
const messageOf = new WeakMap<HTMLElement, Message>();

// ---------- helpers ----------

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
    s.state === 'connected' ? 'Offline · localhost' : s.state === 'starting' ? 'Starting server…' : 'Server disconnected';
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
  if (s.state === 'connected') void refreshLoaded();
  else {
    state.loaded = [];
    renderLoaded();
  }
}

// ---------- loaded models (RAM/VRAM) ----------

async function refreshLoaded() {
  try {
    state.loaded = await api.loadedModels();
  } catch {
    state.loaded = [];
  }
  renderLoaded();
}

function renderLoaded() {
  els.loadedBtn.dataset.count = String(state.loaded.length);
  void renderLoadedPanel();
}

// Free RAM/VRAM runs nvidia-smi, so the figure is reused for half a minute
// unless the operator asks for a fresh look.
let memCache: { at: number; text: string } | null = null;

async function memoryLine(fresh: boolean): Promise<string> {
  if (!fresh && memCache && Date.now() - memCache.at < 30_000) return memCache.text;
  try {
    const info = await api.memoryInfo();
    const text =
      `RAM ${formatMem(info.freeRam)} free of ${formatMem(info.totalRam)}` +
      (info.totalVram != null ? ` · VRAM ${formatMem(info.freeVram ?? 0)} free of ${formatMem(info.totalVram)}` : '');
    memCache = { at: Date.now(), text };
    return text;
  } catch {
    return ''; // memory figures are optional
  }
}

async function renderLoadedPanel(fresh = false) {
  const connected = state.server?.state === 'connected';
  const helper = state.settings?.memoryHelperModel;
  const rows = state.loaded.map((m) => {
    const gpu = m.sizeBytes ? Math.min(100, Math.round((m.vramBytes / m.sizeBytes) * 100)) : 0;
    const where = gpu >= 100 ? '100% GPU' : gpu === 0 ? 'CPU only' : `${gpu}% GPU / ${100 - gpu}% CPU`;
    const role = m.name === helper ? ' · helper' : '';
    return (
      `<div class="loaded-row"><div class="loaded-top"><span class="loaded-name mono">${escapeText(m.name)}</span><span class="mono">${formatMem(m.sizeBytes)}</span></div>` +
      `<div class="mem-bar" role="img" aria-label="${where}"><span class="gpu" style="width:${gpu}%"></span><span class="ram" style="width:${100 - gpu}%"></span></div>` +
      `<div class="caps muted loaded-split"><span>GPU ${formatMem(m.vramBytes)}</span><span>RAM ${formatMem(Math.max(0, m.sizeBytes - m.vramBytes))}</span></div>` +
      `<div class="loaded-bottom"><span class="muted small">${escapeText(where)}${m.contextLength ? ` · ${m.contextLength.toLocaleString()} ctx` : ''}${role}</span>` +
      `<button type="button" class="btn small unload-btn" data-model="${escapeText(m.name)}">Unload</button></div></div>`
    );
  });
  const mem = connected ? await memoryLine(fresh) : '';
  const empty = connected ? 'None. A model loads when you send a message.' : 'The model server is not connected.';
  els.loadedPanel.innerHTML =
    (rows.length ? rows.join('') : `<div class="muted small">${empty}</div>`) + (mem ? `<div class="popover-foot muted small">${mem}</div>` : '');
}

async function unloadModel(model: string, quiet = false) {
  try {
    await api.unloadModel(model);
    if (!quiet) toast(`Unloaded ${model}`);
  } catch (e) {
    toast(errorText(e), 'error');
  }
  await refreshLoaded();
}

/**
 * Checks free RAM/VRAM before a model loads. If it won't fit, offers to unload
 * the other loaded models first. Returns false if the operator cancels.
 */
async function ensureModelFits(model: string): Promise<boolean> {
  let fit;
  try {
    fit = await api.modelFit(model);
  } catch {
    return true; // can't tell; let the server decide
  }
  if (!fit || fit.fitsNow) return true;
  const need = `${formatMem(fit.neededBytes)}${fit.neededIsEstimate ? ' (estimated)' : ''}`;
  const free = formatMem(fit.freeBytes);
  if (fit.fitsAfterUnloadingOthers && fit.others.length) {
    const names = fit.others.map((o) => `${o.name} (${formatMem(o.sizeBytes)})`).join(', ');
    const c = await choose(
      `${model} needs about ${need} of memory, but only ${free} is free right now. Unload ${names} first?`,
      [
        { value: 'cancel', label: 'Cancel' },
        { value: 'anyway', label: 'Load anyway' },
        { value: 'unload', label: 'Unload and continue', kind: 'primary' },
      ],
    );
    if (c === 'unload') {
      for (const o of fit.others) await unloadModel(o.name, true);
      toast(`Unloaded ${fit.others.map((o) => o.name).join(', ')}`);
      return true;
    }
    return c === 'anyway';
  }
  const c = await choose(
    `${model} needs about ${need} of memory, but only ${free} is free${fit.others.length ? ', even after unloading the other models' : ''}. It will probably fail to load.`,
    [
      { value: 'cancel', label: 'Cancel' },
      { value: 'anyway', label: 'Try anyway', kind: 'danger' },
    ],
  );
  return c === 'anyway';
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
    // Embedding models (used for memory search) can't chat, so they're not brains.
    state.models = (await api.listModels()).filter((m) => !/embed/i.test(m.id));
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

// ---------- projects ----------

let projects: ProjectInfo[] = [];

async function loadProjects() {
  projects = await api.listProjects().catch(() => []);
  renderProjects();
}

function renderProjects() {
  const sel = els.projectSelect;
  sel.replaceChildren(new Option('No project', ''), ...projects.map((p) => new Option(p.title, p.slug)), new Option('+ New project…', '__new__'));
  const conv = state.conversations.find((c) => c.id === state.currentId);
  const want = state.currentId !== null ? (conv?.project ?? '') : (state.settings?.activeProject ?? '');
  sel.value = projects.some((p) => p.slug === want) ? want : '';
  if (state.conversations.some((c) => c.project)) renderConversations();
}

async function chooseProject() {
  let slug = els.projectSelect.value;
  if (slug === '__new__') {
    const name = await askText('Project name', 'e.g. Garden planner');
    if (!name) {
      renderProjects();
      return;
    }
    try {
      slug = await api.createProject(name);
    } catch (e) {
      toast(errorText(e), 'error');
      renderProjects();
      return;
    }
  }
  try {
    await api.setActiveProject(slug, state.currentId);
    if (state.settings) state.settings.activeProject = slug;
    await refreshConversations();
    await loadProjects();
    toast(slug ? `Working on ${projects.find((p) => p.slug === slug)?.title ?? slug}` : 'No active project');
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

function selectModel(id?: string | null) {
  const has = (x?: string | null) => !!x && state.models.some((m) => m.id === x);
  const fallback = state.settings?.defaultModel;
  if (has(id)) els.modelSelect.value = id!;
  else if (has(fallback)) els.modelSelect.value = fallback!;
  else if (state.models.length) els.modelSelect.value = state.models[0].id;
  state.activeModel = els.modelSelect.value;
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

/** Conversation numbers are their ids, shown as "014". */
const convNumber = (id: number) => String(id).padStart(3, '0');

function convItem(c: Conversation): HTMLElement {
  const li = document.createElement('li');
  li.className = 'conv-item';
  li.dataset.id = String(c.id);
  li.classList.toggle('active', c.id === state.currentId);
  li.classList.toggle('busy', state.generation?.conversationId === c.id);
  li.innerHTML =
    `<button type="button" class="conv-open"><span class="conv-num mono"></span><span class="conv-text"><span class="conv-title"></span><span class="conv-date"></span></span></button>` +
    `<span class="conv-actions"><button type="button" class="icon-btn conv-rename" title="Rename" aria-label="Rename">${icons.edit}</button>` +
    `<button type="button" class="icon-btn conv-delete" title="Delete" aria-label="Delete">${icons.trash}</button></span>`;
  li.querySelector('.conv-num')!.textContent = convNumber(c.id);
  li.querySelector('.conv-title')!.textContent = c.title;
  li.querySelector('.conv-date')!.textContent = relativeTime(c.updatedAt);
  if (c.id === state.currentId) li.querySelector('.conv-open')!.setAttribute('aria-current', 'true');
  return li;
}

/** Groups conversations by project (newest group first); the rest are "loose pages". */
function renderConversations() {
  const groups = new Map<string, Conversation[]>();
  for (const c of state.conversations) {
    const key = c.project || '';
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push(c);
  }
  const loose = groups.get('');
  groups.delete('');
  if (loose) groups.set('', loose);
  const sections = [...groups].map(([slug, list]) => {
    const box = document.createElement('div');
    box.className = 'conv-group';
    const name = slug ? `Project · ${projects.find((p) => p.slug === slug)?.title ?? slug}` : groups.size > 1 ? 'Loose pages' : 'Conversations';
    box.innerHTML = `<div class="caps muted group-head"><span class="group-name"></span><span>${String(list.length).padStart(2, '0')}</span></div><ul class="conv-items"></ul>`;
    box.querySelector('.group-name')!.textContent = name;
    box.querySelector('ul')!.append(...list.map(convItem));
    return box;
  });
  els.convList.replaceChildren(...sections);
  els.sidebarEmpty.hidden = state.conversations.length > 0;
  els.sidebarEmpty.textContent = state.query ? 'No chats match your search' : 'No conversations yet';
  renderHeader();
}

/** Conversation number and title in the top band; speed of the last reply. */
function renderHeader() {
  const conv = state.conversations.find((c) => c.id === state.currentId);
  els.convNumber.textContent = conv ? `N° ${convNumber(conv.id)}` : 'N° —';
  els.convCaps.textContent = conv ? (projects.find((p) => p.slug === conv.project)?.title ?? 'Loose page') : 'New conversation';
  els.convTitleCaps.textContent = conv ? `“${conv.title}”` : '“A private assistant on this computer”';
  const last = [...state.messages].reverse().find((m) => m.role === 'assistant' && m.stats?.tokensPerSecond);
  els.brainSpeed.textContent = last?.stats?.tokensPerSecond ? `${last.stats.tokensPerSecond} tok/s` : '';
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
  renderProjects();
  renderMessages();
  renderConversations();
  selectModel(state.settings?.defaultModel);
  void refreshContinuity();
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
  renderProjects();
  renderMessages();
  void refreshContinuity();
  els.input.focus();
}

// ---------- continuity panel ----------

/** Current state (from current_state.md), project steps and what the last reply recalled. */
async function refreshContinuity() {
  const t = await api.lastTime().catch(() => null);
  els.contState.textContent = t?.lastTask || (t?.conversationTitle ? `“${t.conversationTitle}”` : 'A fresh start');
  els.contNext.hidden = !t?.nextStep;
  els.contNext.textContent = t?.nextStep ? `Next: ${t.nextStep}` : '';
  await renderSteps();
  renderRecalled();
}

/** Task-list items ("- [x] …") from the active project's memory file. */
async function renderSteps() {
  const conv = state.conversations.find((c) => c.id === state.currentId);
  const slug = state.currentId !== null ? (conv?.project ?? '') : (state.settings?.activeProject ?? '');
  const text = slug ? await api.memoryRead(`projects/${slug}.md`).catch(() => '') : '';
  const steps = [...text.matchAll(/^\s*[-*+]\s+\[( |x|X)\]\s+(.+)$/gm)].map((m) => ({ done: m[1] !== ' ', text: m[2].trim() }));
  els.contStepsWrap.hidden = steps.length === 0;
  if (!steps.length) return;
  const current = steps.findIndex((s) => !s.done);
  els.contStepsHead.textContent = current === -1 ? `Project · all ${steps.length} done` : `Project · ${current + 1} of ${steps.length}`;
  els.contSteps.innerHTML = steps
    .map(
      (s, i) =>
        `<li class="${s.done ? 'done' : i === current ? 'current' : ''}"><span class="mono">${String(i + 1).padStart(2, '0')}</span>` +
        `<span>${escapeText(s.text)}${i === current ? ` ${starburst(11, 'accent')}` : ''}</span></li>`,
    )
    .join('');
}

function renderRecalled() {
  const last = [...state.messages].reverse().find((m) => m.role === 'assistant' && m.stats?.context);
  const c = last?.stats?.context;
  if (!c) {
    els.contRecalled.textContent = 'Recalled · —';
    els.contRecalled.removeAttribute('title');
    return;
  }
  const n = c.memoryFiles.length;
  els.contRecalled.textContent = `Recalled · ${n} file${n === 1 ? '' : 's'}${c.searchHits ? ` · ${c.searchHits} note${c.searchHits === 1 ? '' : 's'}` : ''}`;
  els.contRecalled.title = handoffText(c);
}

// ---------- messages ----------

function emptyState(): HTMLElement {
  const div = document.createElement('div');
  div.className = 'empty paper-card';
  div.innerHTML =
    `${logo}<h1>Democritus</h1>` +
    `<p class="muted">Your private assistant. Its brain is the local Qwen model you pick; its identity and memory stay on this computer.</p>` +
    `<ul class="hints"><li><kbd>Enter</kbd> send</li><li><kbd>Shift</kbd> + <kbd>Enter</kbd> new line</li>` +
    `<li><kbd>${modKey}</kbd> + <kbd>N</kbd> new chat</li><li><kbd>Esc</kbd> stop generating</li></ul>` +
    `<p class="muted small">Drop text files anywhere in the window to include them in your message.</p>`;
  const last = document.createElement('div');
  last.className = 'last-time';
  last.hidden = true;
  div.querySelector('h1')!.nextElementSibling!.after(last);
  void fillLastTime(last);
  return div;
}

/** "Last time: …" from current_state.md and the most recent conversation. */
async function fillLastTime(box: HTMLElement) {
  const t = await api.lastTime().catch(() => null);
  if (!t || !box.isConnected) return;
  const what = t.lastTask || (t.conversationTitle ? `“${t.conversationTitle}”` : '');
  const lines: string[] = [];
  if (what) lines.push(`<div><strong>Last time:</strong> ${escapeText(what)}${t.updatedAt ? ` <span class="muted small">(${relativeTime(t.updatedAt)})</span>` : ''}</div>`);
  if (t.project) lines.push(`<div><strong>Project:</strong> ${escapeText(t.project)}</div>`);
  if (t.nextStep) lines.push(`<div><strong>Next:</strong> ${escapeText(t.nextStep)}</div>`);
  if (!lines.length) return;
  const open = t.conversationId !== null ? `<button type="button" class="btn small" data-open="${t.conversationId}">Continue “${escapeText(t.conversationTitle)}”</button>` : '';
  box.innerHTML = `<div class="last-time-text">${lines.join('')}</div>${open}`;
  box.querySelector<HTMLButtonElement>('[data-open]')?.addEventListener('click', () => void openConversation(t.conversationId!));
  box.hidden = false;
}

// ---------- brain switches ----------

function handoffText(c: ContextReport | undefined): string {
  if (!c) return 'Same conversation, same memory: Democritus carries on with the new brain.';
  const parts = [`${c.historyTurns} recent message${c.historyTurns === 1 ? '' : 's'}${c.hasSummary ? ' plus a summary of earlier ones' : ''}`];
  if (c.memoryFiles.length) parts.push(`memory: ${c.memoryFiles.join(', ')}`);
  if (c.searchHits) parts.push(`${c.searchHits} related note${c.searchHits === 1 ? '' : 's'}`);
  return `Handed over: ${parts.join('; ')}.`;
}

/** Divider shown above the first reply after a brain switch. */
function switchEl(from: string, m: Message): HTMLElement {
  const el = document.createElement('div');
  el.className = 'brain-switch';
  el.dataset.for = String(m.id);
  el.setAttribute('role', 'separator');
  el.innerHTML =
    `<div class="brain-switch-line"><span class="rule"></span>${sparkle('accent')}` +
    `<span>Brain switched to <strong>${escapeText(m.model)}</strong> <span class="was">(was ${escapeText(from)})</span> · handoff passed</span>` +
    `${sparkle('accent')}<span class="rule"></span></div><div class="handoff-note small"></div>`;
  el.querySelector('.handoff-note')!.textContent = handoffText(m.stats?.context);
  return el;
}

/** The brain of the last reply before index `i` (or before the end). */
function previousBrain(messages: Message[], i = messages.length): string {
  for (let k = i - 1; k >= 0; k--) if (messages[k].role === 'assistant' && messages[k].model) return messages[k].model;
  return '';
}

function renderMessages() {
  const g = state.generation;
  if (state.messages.length === 0) {
    els.messages.replaceChildren(emptyState());
    return;
  }
  const items: HTMLElement[] = [];
  let brain = '';
  state.messages.forEach((m, i) => {
    // While a reply streams, the saved copy is a stale placeholder: show the live one.
    const live = g && g.message.id === m.id ? g.message : m;
    state.messages[i] = live;
    if (live.role === 'assistant' && live.model) {
      if (brain && live.model !== brain) items.push(switchEl(brain, live));
      brain = live.model;
    }
    const el = messageEl(live);
    if (g && live === g.message) g.el = el;
    items.push(el);
  });
  els.messages.replaceChildren(...items);
  renderReviewChip();
  renderHeader();
  scrollToBottom();
}

function messageEl(m: Message): HTMLElement {
  const el = document.createElement('article');
  el.className = `message ${m.role}`;
  el.innerHTML =
    m.role === 'assistant'
      ? `<div class="meta caps muted">${starburst(12, 'accent')}<span class="ink">Democritus</span><span class="who"></span><span class="num"></span></div>` +
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

  el.querySelector('.who')!.textContent = `· ${m.model || 'assistant'}`;
  el.querySelector('.num')!.textContent = m.id > 0 ? `N° ${String(m.id).padStart(3, '0')}` : '';
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
  const status = state.generation?.message === m ? state.generation.statusText : '';
  body.innerHTML = content
    ? renderMarkdown(content)
    : waiting
      ? `<span class="typing"><i></i><i></i><i></i></span>${status ? ` <span class="muted small">${escapeText(status)}</span>` : ''}`
      : '';

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
  const c = m.stats?.context;
  if (c) {
    if (c.omittedTurns) bits.push(`<span class="badge warn" title="Summarizing failed, so the oldest messages weren't sent">${c.omittedTurns} older messages left out</span>`);
    const tip = [handoffText(c), c.clipped ? 'Some text was shortened to fit.' : ''].filter(Boolean).join(' ');
    bits.push(`<span title="${escapeText(tip)}">~${kTokens(c.estimatedPromptTokens)} / ${kTokens(c.numCtx)} context</span>`);
  }
  if (m.content.trim()) bits.push(`<button type="button" class="link-btn copy-message">${icons.copy}Copy</button>`);
  return bits.join('<span class="sep">·</span>');
}

/** "N memory updates to review" on the latest finished reply. */
function renderReviewChip() {
  document.querySelectorAll('#messages .review-chip').forEach((c) => c.remove());
  if (!pendingUpdates) return;
  const replies = els.messages.querySelectorAll<HTMLElement>('.message.assistant:not([data-status="streaming"]) .footer');
  const footer = replies[replies.length - 1];
  if (!footer) return;
  const chip = document.createElement('button');
  chip.type = 'button';
  chip.className = 'review-chip';
  chip.textContent = `${pendingUpdates} memory update${pendingUpdates === 1 ? '' : 's'} to review`;
  chip.addEventListener('click', () => showPage('memory'));
  footer.append(chip);
}

function kTokens(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}k` : String(n);
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
  // Switching brains mid-reply would unload the model that is answering.
  els.modelSelect.disabled = on || state.models.length === 0;
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
  if (!(await ensureModelFits(model))) return;

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
    statusText: '',
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
    void refreshLoaded();
    void refreshContinuity();
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
      const before = previousBrain(state.messages);
      state.messages.push(ev.userMessage, m);
      const userEl = messageEl(ev.userMessage);
      g.el = messageEl(m);
      els.messages.append(userEl);
      if (before && before !== m.model) els.messages.append(switchEl(before, m));
      els.messages.append(g.el);
      scrollToBottom();
      break;
    }
    case 'status':
      g.statusText = ev.text;
      scheduleLive();
      break;
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
      g.statusText = '';
      const note = document.querySelector(`.brain-switch[data-for="${m.id}"] .handoff-note`);
      if (note) note.textContent = handoffText(m.stats?.context);
      if (g.el?.isConnected) {
        const stick = nearBottom();
        updateMessageEl(g.el, m);
        if (stick) scrollToBottom();
      }
      renderReviewChip();
      renderHeader();
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
  els.setCtx.value = String(s.contextTokens);
  els.setAgentCtx.value = String(s.agentContextTokens);
  els.setUrl.value = s.baseUrl;
  els.setCmd.value = s.serverCommand;
  els.setAutostart.checked = s.autoStartServer;
  els.setHome.value = s.homeDir;
  els.setHelper.replaceChildren(
    new Option('Use the current brain', ''),
    ...state.models.filter((m) => !m.id.includes('embedding')).map((m) => new Option(m.id, m.id)),
  );
  els.setHelper.value = s.memoryHelperModel;
  void api.appPaths().then((p) => (els.setHome.placeholder = p.home));
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
  const contextTokens = Number(els.setCtx.value);
  if (!Number.isInteger(contextTokens) || contextTokens < 2048 || contextTokens > 262144) {
    els.settingsError.textContent = 'The context window must be a whole number between 2048 and 262144.';
    els.settingsError.hidden = false;
    return;
  }
  const agentContextTokens = Number(els.setAgentCtx.value);
  if (!Number.isInteger(agentContextTokens) || agentContextTokens < 4096 || agentContextTokens > 262144) {
    els.settingsError.textContent = "The coding agent's window must be a whole number between 4096 and 262144.";
    els.settingsError.hidden = false;
    return;
  }
  const next: Settings = {
    ...current,
    systemPrompt: els.setSystem.value,
    temperature: Number(els.setTemp.value),
    maxTokens,
    contextTokens,
    agentContextTokens,
    baseUrl: els.setUrl.value,
    serverCommand: els.setCmd.value,
    autoStartServer: els.setAutostart.checked,
    homeDir: els.setHome.value.trim(),
    memoryHelperModel: els.setHelper.value,
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

/** Saves the theme choice in the app's settings (the cache in localStorage follows). */
async function chooseTheme(p: ThemePref) {
  applyTheme(p);
  if (!state.settings) return;
  try {
    state.settings = await api.saveSettings({ ...state.settings, theme: p });
  } catch (e) {
    toast(`Couldn't save the theme: ${errorText(e)}`, 'error');
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

els.loadedBtn.addEventListener('click', async () => {
  try {
    state.loaded = state.server?.state === 'connected' ? await api.loadedModels() : [];
  } catch {
    state.loaded = [];
  }
  await renderLoadedPanel(true);
});
els.loadedPanel.addEventListener('click', (e) => {
  const btn = (e.target as HTMLElement).closest<HTMLButtonElement>('.unload-btn');
  if (btn?.dataset.model) {
    btn.disabled = true;
    void unloadModel(btn.dataset.model);
  }
});
els.attach.addEventListener('click', () => els.attachInput.click());
els.attachInput.addEventListener('change', async () => {
  for (const file of Array.from(els.attachInput.files ?? [])) await addAttachment(file);
  els.attachInput.value = '';
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
  const previous = state.activeModel;
  state.activeModel = model;
  // Free the previous brain's RAM/VRAM right away instead of waiting for the server's idle timeout.
  if (previous && previous !== model) {
    void refreshLoaded().then(async () => {
      if (!state.loaded.some((l) => l.name === previous)) return;
      await unloadModel(previous, true);
      toast(`Switched to ${model}; unloaded ${previous} to free memory`);
    });
  }
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
    if (!dialogOpen) {
      showPage('chat');
      newChat();
    }
  } else if (e.key === 'Escape' && state.generation && !dialogOpen) {
    e.preventDefault();
    void stop();
  }
});
window.addEventListener('focus', () => void pollStatus());

async function init() {
  fillOrnaments();
  initRail();
  $<HTMLImageElement>('#band-icon').src = logoUrl;
  initThemeSwitch($('#theme-switch'), (p) => void chooseTheme(p));
  initSecurityCheck();
  initCodePage({ ensureModelFits, defaultModel: () => state.settings?.defaultModel ?? '' });
  initFirewallWarning();
  onPendingChange((n) => {
    pendingUpdates = n;
    renderReviewChip();
  });
  $('#op-open-settings').addEventListener('click', openSettings);
  initLogPage();
  initOperatorPage();
  initMemoryPage();
  // A proposal may have created a project; keep the dropdown current.
  void listen('memory-changed', () => {
    void loadProjects();
    if (currentPage() === 'chat') void refreshContinuity();
  });
  els.projectSelect.addEventListener('change', () => void chooseProject());
  void loadOperator().catch((e) => toast(`Couldn't load operator settings: ${errorText(e)}`, 'error'));
  els.newChat.innerHTML = `${starburst(14)}<span>New conversation</span>`;
  els.newChat.title = `New chat (${modKey}+N)`;
  els.refreshModels.innerHTML = icons.refresh;
  els.openSettings.innerHTML = icons.settings;
  els.loadedBtn.innerHTML = icons.refresh;
  els.attach.innerHTML = icons.clip;
  els.send.innerHTML = icons.arrowUp;
  els.stop.innerHTML = icons.square;
  els.composerHint.innerHTML =
    '<span class="dots" aria-hidden="true"><span class="on"></span><span></span></span><span>Drop or attach text files</span>' +
    '<span class="spacer"></span><span>Enter · send</span><span>Shift+Enter · new line</span><span>Esc · stop</span>';
  void api.appPaths().then((p) => ($('#home-path').textContent = p.home)).catch(() => {});
  try {
    state.settings = await api.getSettings();
    applyTheme(state.settings.theme);
  } catch (e) {
    toast(`Couldn't load settings: ${errorText(e)}`, 'error');
  }
  renderModels();
  renderMessages();
  renderAttachments();
  await refreshConversations();
  void loadProjects();
  void pollStatus();
  void renderLoadedPanel();
  void refreshContinuity();
  els.input.focus();
}

void init();
