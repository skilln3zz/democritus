// The Code page: the coding agent works in one project folder. Reading is
// shown as it happens; every edit arrives as a diff card with Apply / Reject,
// and nothing is written until the operator presses Apply.
import { api, type AgentEvent, type PlannedEdit } from './api';
import { diffLines } from './diff';
import { icons } from './icons';
import { renderMarkdown } from './markdown';
import { getOperator, onOperatorChange } from './operatorpage';
import { sparkle, starburst } from './ornaments';
import { currentPage, onShow } from './pages';
import { $, errorText, escapeText, toast } from './ui';

interface Deps {
  /** The chat page's RAM/VRAM check; false if the operator cancels. */
  ensureModelFits: (model: string) => Promise<boolean>;
  defaultModel: () => string;
}

const FOLDER_KEY = 'democritus.codeFolder';
const MODEL_KEY = 'democritus.codeModel';
const MAX_DIFF_LINES = 400;

let deps: Deps;
let taskId = '';
let running = false;
let homeBackups = '';
const changed = new Set<string>();

/** The card of the current model turn. */
interface StepCard {
  el: HTMLElement;
  thinking: string;
  content: string;
}
let card: StepCard | null = null;
const toolRows = new Map<string, HTMLElement>();
const approvalCards = new Map<string, HTMLElement>();

const store = {
  get(k: string) {
    try {
      return localStorage.getItem(k) ?? '';
    } catch {
      return '';
    }
  },
  set(k: string, v: string) {
    try {
      localStorage.setItem(k, v);
    } catch {
      // only a convenience
    }
  },
};

const log = () => $('#code-log');
const input = () => $<HTMLTextAreaElement>('#code-input');
const folderInput = () => $<HTMLInputElement>('#code-folder');
const modelSelect = () => $<HTMLSelectElement>('#code-model');

function scrollDown() {
  const l = log();
  l.scrollTop = l.scrollHeight;
}

function nearBottom(): boolean {
  const l = log();
  return l.scrollHeight - l.scrollTop - l.clientHeight < 160;
}

function setStatus(text: string) {
  $('#code-status').textContent = text;
}

function setRunning(on: boolean) {
  running = on;
  $('#code-send').hidden = on;
  $('#code-stop').hidden = !on;
  ($('#code-stop') as HTMLButtonElement).disabled = false;
  modelSelect().disabled = on;
  folderInput().disabled = on;
  ($('#code-new') as HTMLButtonElement).disabled = on;
  if (!on) setStatus('Idle');
}

// ---------- availability ----------

function renderAvailability() {
  const o = getOperator();
  const banner = $('#code-off');
  const off = !o?.allowCodingAgent;
  const locked = !!o?.lockdown;
  banner.hidden = !(off || locked);
  banner.querySelector('.banner-text')!.textContent = locked
    ? 'Lockdown is on, so the coding agent is paused.'
    : 'The coding agent is off. Turn on "Coding agent" under Settings & safety to use it.';
  ($('#code-send') as HTMLButtonElement).disabled = off || locked;
}

async function loadModels() {
  const sel = modelSelect();
  const models = (await api.listModels().catch(() => [])).filter((m) => !/embed/i.test(m.id));
  const want = store.get(MODEL_KEY);
  sel.replaceChildren(...models.map((m) => new Option(m.id, m.id)));
  if (!models.length) {
    sel.add(new Option('No models', ''));
    return;
  }
  const coder = models.find((m) => /coder/i.test(m.id))?.id;
  const pick = [want, coder, deps.defaultModel()].find((id) => id && models.some((m) => m.id === id));
  sel.value = pick ?? models[0].id;
}

function renderFolderCaps() {
  const f = folderInput().value.trim();
  $('#code-folder-caps').textContent = f || 'No folder chosen';
}

function renderChanged() {
  const list = $('#code-files');
  list.innerHTML = changed.size
    ? [...changed].map((p) => `<li class="mono">${escapeText(p)}</li>`).join('')
    : '<li class="small muted">Nothing yet.</li>';
}

function emptyLog() {
  log().innerHTML =
    `<div class="empty paper-card code-empty">${starburst(28, 'accent')}<h2 class="display-title">Hand Democritus a coding task</h2>` +
    `<p class="muted">Pick a project folder on the left, choose a brain (a <span class="mono">coder</span> model works best), and describe the change. ` +
    `Democritus reads what it needs, then proposes each edit as a diff. Nothing is written until you press <strong>Apply</strong>.</p>` +
    `<p class="small muted">It can't run commands: when a build or tests are needed, it tells you which command to run.</p></div>`;
}

// ---------- transcript ----------

function append(el: HTMLElement) {
  const stick = nearBottom();
  log().querySelector('.code-empty')?.remove();
  log().append(el);
  if (stick) scrollDown();
}

function newCard(step: number): StepCard {
  const el = document.createElement('article');
  el.className = 'message assistant code-step';
  el.innerHTML =
    `<div class="meta caps muted">${starburst(12, 'accent')}<span class="ink">Democritus</span><span>· ${escapeText(modelSelect().value)}</span><span class="num">Step ${step}</span></div>` +
    `<details class="thinking" hidden><summary><span class="thinking-label">Thinking…</span></summary><div class="thinking-body"></div></details>` +
    `<div class="body md" hidden></div><ul class="tool-list"></ul>`;
  append(el);
  return { el, thinking: '', content: '' };
}

// Streaming text is drawn once per frame. Every card that changed is drawn,
// including one the next step has already replaced as "current".
const dirty = new Set<StepCard>();
let frame = 0;

function drawCard(c: StepCard, finished: boolean) {
  const think = c.el.querySelector<HTMLDetailsElement>('.thinking')!;
  think.hidden = !c.thinking.trim();
  const active = !finished && !c.content;
  think.querySelector('.thinking-label')!.textContent = active ? 'Thinking…' : 'Thought process';
  think.classList.toggle('active', active);
  think.querySelector('.thinking-body')!.textContent = c.thinking.trim();
  const body = c.el.querySelector<HTMLElement>('.body')!;
  body.hidden = !c.content.trim();
  body.innerHTML = renderMarkdown(c.content.trim());
}

function flushCards() {
  if (frame) cancelAnimationFrame(frame);
  frame = 0;
  const stick = nearBottom();
  for (const c of dirty) drawCard(c, c !== card);
  dirty.clear();
  if (stick) scrollDown();
}

function scheduleRender() {
  if (!card) return;
  dirty.add(card);
  if (!frame) frame = requestAnimationFrame(flushCards);
}

function toolRow(callId: string, summary: string) {
  if (!card) card = newCard(0);
  const li = document.createElement('li');
  li.className = 'tool-row';
  li.dataset.state = 'running';
  li.innerHTML = `<span class="tool-mark" aria-hidden="true"></span><span class="tool-summary mono"></span><span class="tool-result muted"></span>`;
  li.querySelector('.tool-summary')!.textContent = summary;
  card.el.querySelector('.tool-list')!.append(li);
  toolRows.set(callId, li);
  if (nearBottom()) scrollDown();
}

function diffHtml(e: PlannedEdit): string {
  const lines = diffLines(e.before, e.after, 3);
  const shown = lines.slice(0, MAX_DIFF_LINES);
  const more = lines.length > shown.length ? `<div class="diff-line diff-gap">… ${lines.length - shown.length} more lines</div>` : '';
  return (
    shown
      .map((l) => `<div class="diff-line diff-${l.kind}">${l.kind === 'add' ? '+ ' : l.kind === 'del' ? '- ' : '  '}${escapeText(l.text)}</div>`)
      .join('') + more
  );
}

function approvalCard(approvalId: string, callId: string, edit: PlannedEdit) {
  const el = document.createElement('article');
  el.className = 'code-approval';
  el.dataset.state = 'waiting';
  const adds = diffLines(edit.before, edit.after, 0).filter((l) => l.kind === 'add').length;
  const dels = diffLines(edit.before, edit.after, 0).filter((l) => l.kind === 'del').length;
  el.innerHTML =
    `<div class="approval-head caps"><span class="ink">${edit.newFile ? 'New file' : 'Edit'}</span><span class="mono approval-path"></span>` +
    `<span class="muted">+${adds} −${dels}</span><span class="approval-state">Waiting for you</span></div>` +
    `<div class="diff-file code-diff">${diffHtml(edit)}</div>` +
    `<label class="sr-only" for="note-${approvalId}">Note for Democritus</label>` +
    `<input id="note-${approvalId}" class="approval-note" type="text" placeholder="Optional note for Democritus (why, or what to do instead)" spellcheck="true" />` +
    `<div class="proposal-actions"><button type="button" class="btn hex accent" data-act="apply">Apply</button><button type="button" class="btn" data-act="reject">Reject</button></div>`;
  el.querySelector('.approval-path')!.textContent = edit.path;
  el.addEventListener('click', async (e) => {
    const act = (e.target as HTMLElement).closest<HTMLButtonElement>('[data-act]')?.dataset.act;
    if (!act || el.dataset.state !== 'waiting') return;
    el.querySelectorAll<HTMLButtonElement>('[data-act]').forEach((b) => (b.disabled = true));
    const note = el.querySelector<HTMLInputElement>('.approval-note')!.value;
    try {
      await api.agentRespond(approvalId, act === 'apply', note);
      el.dataset.state = act === 'apply' ? 'applying' : 'rejected';
    } catch (err) {
      toast(errorText(err), 'error');
    }
  });
  approvalCards.set(callId, el);
  approvalCards.set(approvalId, el);
  append(el);
  el.querySelector<HTMLButtonElement>('[data-act="apply"]')?.focus({ preventScroll: true });
  setStatus('Waiting for your OK');
  if (currentPage() !== 'code') toast(`The coding agent wants to change ${edit.path}. Open Code to review it.`);
}

function doneLine(ev: Extract<AgentEvent, { type: 'done' }>) {
  const el = document.createElement('div');
  el.className = 'brain-switch code-done';
  el.setAttribute('role', 'separator');
  const label =
    ev.status === 'complete' ? 'Task finished' : ev.status === 'stopped' ? 'Stopped' : ev.status === 'limit' ? 'Paused' : 'Something went wrong';
  el.innerHTML =
    `<div class="brain-switch-line"><span class="rule"></span>${sparkle('accent')}<span>${label} · ${ev.editsApplied} edit${ev.editsApplied === 1 ? '' : 's'} applied</span>${sparkle('accent')}<span class="rule"></span></div>` +
    (ev.message ? `<div class="handoff-note small"></div>` : '');
  if (ev.message) el.querySelector('.handoff-note')!.textContent = ev.message;
  append(el);
  if (ev.status === 'error') {
    const box = document.createElement('div');
    box.className = 'error-box';
    box.textContent = ev.message;
    el.after(box);
  }
}

function onEvent(ev: AgentEvent) {
  switch (ev.type) {
    case 'started':
      taskId = ev.taskId;
      $('#code-ctx').textContent = `Context window for this task: ${ev.numCtx.toLocaleString()} tokens (Chat & model settings → Coding agent window).`;
      break;
    case 'step':
      flushCards();
      card = newCard(ev.step);
      setStatus(`Step ${ev.step} · thinking`);
      break;
    case 'thinking':
      if (!card) card = newCard(0);
      card.thinking += ev.text;
      scheduleRender();
      break;
    case 'content':
      if (!card) card = newCard(0);
      card.content += ev.text;
      scheduleRender();
      break;
    case 'toolCall':
      toolRow(ev.callId, ev.summary);
      setStatus(ev.summary);
      break;
    case 'toolResult': {
      const row = toolRows.get(ev.callId);
      if (row) {
        row.dataset.state = ev.ok ? 'ok' : 'failed';
        row.querySelector('.tool-result')!.textContent = ev.summary;
      }
      const a = approvalCards.get(ev.callId);
      if (a) {
        a.dataset.state = ev.ok ? 'applied' : a.dataset.state === 'rejected' || ev.summary === 'Rejected.' ? 'rejected' : 'failed';
        a.querySelector('.approval-state')!.textContent = ev.ok ? 'Applied' : ev.summary === 'Rejected.' ? 'Rejected' : 'Not written';
        if (!ev.ok && ev.summary !== 'Rejected.') a.append(Object.assign(document.createElement('p'), { className: 'small form-error', textContent: ev.summary }));
        if (ev.ok) {
          const path = a.querySelector('.approval-path')!.textContent ?? '';
          changed.add(path);
          renderChanged();
          const note = Object.assign(document.createElement('p'), { className: 'small muted', textContent: ev.summary });
          a.append(note);
        }
        a.querySelectorAll<HTMLElement>('.approval-note, .proposal-actions').forEach((x) => x.remove());
      }
      setStatus('Working');
      break;
    }
    case 'approval':
      approvalCard(ev.approvalId, ev.callId, ev.edit);
      break;
    case 'resolved': {
      const a = approvalCards.get(ev.approvalId);
      if (a && ev.message) {
        a.dataset.state = 'stopped';
        a.querySelector('.approval-state')!.textContent = 'Stopped';
        a.querySelectorAll<HTMLElement>('.approval-note, .proposal-actions').forEach((x) => x.remove());
      }
      break;
    }
    case 'done':
      if (card) dirty.add(card);
      card = null;
      flushCards();
      doneLine(ev);
      setRunning(false);
      break;
  }
}

// ---------- actions ----------

async function start() {
  if (running) return;
  const message = input().value.trim();
  const folder = folderInput().value.trim();
  const model = modelSelect().value;
  if (!message) return;
  if (!folder) {
    toast('Pick a project folder first.', 'error');
    folderInput().focus();
    return;
  }
  if (!model) {
    toast('No model available. Is the model server running?', 'error');
    return;
  }
  if (!(await deps.ensureModelFits(model))) return;
  const slip = document.createElement('div');
  slip.className = 'message user';
  slip.innerHTML = '<div class="bubble"><div class="body md"></div></div>';
  slip.querySelector('.body')!.innerHTML = renderMarkdown(message);
  append(slip);
  scrollDown();
  input().value = '';
  autoGrow();
  card = null;
  setRunning(true);
  setStatus('Starting');
  try {
    await api.agentRun({ taskId, folder, model, message }, onEvent);
  } catch (e) {
    // Refused before it started (permission, folder, another task running).
    const box = document.createElement('div');
    box.className = 'error-box';
    box.textContent = errorText(e);
    append(box);
    input().value = message;
    autoGrow();
    setRunning(false);
  }
}

async function stop() {
  if (!running) return;
  ($('#code-stop') as HTMLButtonElement).disabled = true;
  setStatus('Stopping');
  await api.agentStop().catch((e) => toast(errorText(e), 'error'));
}

function newTask() {
  if (running) return;
  if (taskId) void api.agentForget(taskId);
  taskId = '';
  changed.clear();
  toolRows.clear();
  approvalCards.clear();
  card = null;
  renderChanged();
  emptyLog();
  $('#code-ctx').textContent = '';
  input().focus();
}

function autoGrow() {
  const i = input();
  i.style.height = 'auto';
  i.style.height = `${Math.min(i.scrollHeight, 200)}px`;
}

export function initCodePage(d: Deps) {
  deps = d;
  $('#code-send').innerHTML = icons.arrowUp;
  $('#code-stop').innerHTML = icons.square;
  folderInput().value = store.get(FOLDER_KEY);
  renderFolderCaps();
  renderChanged();
  emptyLog();
  void api.appPaths().then((p) => {
    homeBackups = `${p.home}\\backups\\agent\\`;
    $('#code-backups').textContent = `Before a file is changed, its old version is copied to ${homeBackups}<task>.`;
  });
  onOperatorChange(renderAvailability);
  onShow('code', () => {
    renderAvailability();
    if (!running) void loadModels();
    if (!folderInput().value) folderInput().focus();
    else input().focus();
  });
  folderInput().addEventListener('change', () => {
    store.set(FOLDER_KEY, folderInput().value.trim());
    renderFolderCaps();
    // A different folder is a different task.
    if (taskId) newTask();
  });
  modelSelect().addEventListener('change', () => store.set(MODEL_KEY, modelSelect().value));
  $('#code-send').addEventListener('click', () => void start());
  $('#code-stop').addEventListener('click', () => void stop());
  $('#code-new').addEventListener('click', newTask);
  input().addEventListener('input', autoGrow);
  input().addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      void start();
    }
  });
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && running && currentPage() === 'code' && !document.querySelector('dialog[open]')) {
      e.preventDefault();
      void stop();
    }
  });
  log().addEventListener('click', (e) => {
    const t = e.target as HTMLElement;
    if (t.closest('a')) e.preventDefault(); // never navigate away
    const copy = t.closest<HTMLButtonElement>('.copy-btn');
    const code = copy?.closest('.code-block')?.querySelector('code')?.textContent;
    if (copy && code != null) {
      void navigator.clipboard.writeText(code).then(
        () => {
          copy.textContent = 'Copied!';
          setTimeout(() => (copy.textContent = 'Copy'), 1500);
        },
        () => toast("Couldn't copy to the clipboard.", 'error'),
      );
    }
  });
}
