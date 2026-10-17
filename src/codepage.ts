// The Coding agent page (design-neumorphic/mockups/NeuCode.dc.html). It works
// like the chat: your request is an inset bubble, and each run of the agent is
// one reply card with the orb beside it. Inside the card: the thinking status,
// the tool steps as they happen, the explanation and every proposed change as
// a diff with Approve / Reject. Nothing is written until you press Approve, and
// once you decide, the buttons are replaced by the result.
import { activityEvent, activityTask, changeCounts, initActivity, showActivity } from './activitypanel';
import { api, type AgentEvent, type FolderState, type PlannedEdit } from './api';
import { diffLines } from './diff';
import { icons } from './icons';
import { renderMarkdown } from './markdown';
import { AGENT_TUNING, FerroOrb } from './orb';
import { getOperator, onOperatorChange, setCodingAgentAllowed } from './operatorpage';
import { currentPage, onLeave, onShow } from './pages';
import { $, errorText, escapeText, toast } from './ui';

interface Deps {
  /** The chat page's RAM/VRAM check; false if the operator cancels. */
  ensureModelFits: (model: string) => Promise<boolean>;
  defaultModel: () => string;
}

const MODEL_KEY = 'democritus.codeModel';
const MAX_DIFF_LINES = 400;
/** The backend's step limit per run (MAX_STEPS in src-tauri/src/agent.rs). */
const MAX_STEPS = 30;

const ORB_LABELS = {
  working: 'The agent is working',
  waiting: 'Waiting for your approval',
  idle: 'The agent is idle',
  off: 'The agent is off',
};

let deps: Deps;
let taskId = '';
let running = false;
let stopping = false;
let homeBackups = '';

/** The backend's folder, as it last reported it. The chip and the strip show only this. */
let folderState: FolderState = { folder: null, error: null, recent: [] };
/** Bumped by every folder request, so an older answer never overwrites a newer one. */
let folderSeq = 0;

/** One run of the agent: everything it does for one request, in one reply card. */
interface Run {
  el: HTMLElement;
  orb: FerroOrb;
  started: number;
  ended: number | null;
  steps: number;
  thoughts: number;
  thinking: string;
  /** The text the current step is writing, and where it goes. */
  text: string;
  textEl: HTMLElement | null;
  /** The tool steps since the last text, shown as one group. */
  toolsEl: HTMLElement | null;
  waiting: number;
  done: boolean;
}
let run: Run | null = null;
let lastRun: Run | null = null;
/** Every run's orb in the transcript, so New task can release them. */
const orbs: FerroOrb[] = [];
const toolRows = new Map<string, HTMLElement>();
const approvals = new Map<string, HTMLElement>();

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
const folderPop = () => $('#code-folder-pop');
const changeFolderBtn = () => $<HTMLButtonElement>('#code-change-folder');
const modelSelect = () => $<HTMLSelectElement>('#code-model');
const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? '' : 's'}`;

function scrollDown() {
  const l = log();
  l.scrollTop = l.scrollHeight;
}

function nearBottom(): boolean {
  const l = log();
  return l.scrollHeight - l.scrollTop - l.clientHeight < 160;
}

function agentAllowed(): boolean {
  const o = getOperator();
  return !!o?.allowCodingAgent && !o.lockdown;
}

function setRunning(on: boolean) {
  running = on;
  if (!on) stopping = false;
  modelSelect().disabled = on;
  // The backend refuses folder changes during a run too.
  if (on) closeFolderPop();
  changeFolderBtn().disabled = on;
  ($('#code-new') as HTMLButtonElement).disabled = on;
  renderControls();
}

/** Send, Stop and the Agent button follow the run and the permission. */
function renderControls() {
  const o = getOperator();
  const allowed = agentAllowed();
  const stop = $<HTMLButtonElement>('#code-stop');
  stop.disabled = !running || stopping;
  ($('#code-send') as HTMLButtonElement).disabled = !allowed || running;
  const btn = $<HTMLButtonElement>('#code-agent');
  const on = !!o?.allowCodingAgent;
  btn.classList.toggle('on', on && !o?.lockdown);
  btn.setAttribute('aria-checked', String(on && !o?.lockdown));
  btn.disabled = !o || !!o.lockdown;
  btn.title = o?.lockdown ? 'Lockdown is on, so the coding agent is paused.' : on ? 'Turn the coding agent off (stops a running task)' : 'Turn the coding agent on';
  $('#code-agent-label').textContent = on && !o?.lockdown ? 'Agent on' : 'Agent off';
  if (!running && lastRun) lastRun.orb.setLabel(allowed ? ORB_LABELS.idle : ORB_LABELS.off);
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
    : 'The coding agent is off. Turn it on with the Agent button above, or under Settings & security.';
  // Switched off anywhere (here or under Settings & security) during a run: it stops, exactly like Stop.
  if (running && off) void stop();
  renderControls();
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

function emptyLog() {
  log().innerHTML =
    `<div class="empty code-empty"><span class="code-empty-icon">${icons.code}</span><h2 class="section-title">Hand Democritus a coding task</h2>` +
    `<p class="muted">Pick a project folder with <strong>Change folder</strong>, pick a brain (a <span class="mono">coder</span> model works best), and describe the change below. ` +
    `The agent reads what it needs, then proposes each edit as a diff. Nothing is written to your folder until you press <strong>Approve</strong>.</p>` +
    `<p class="small muted">It can't run commands: when a build or tests are needed, it tells you which command to run.</p></div>`;
}

// ---------- the reply card ----------

function append(el: HTMLElement) {
  const stick = nearBottom();
  log().querySelector('.code-empty')?.remove();
  log().append(el);
  if (stick) scrollDown();
}

function newRun(): Run {
  const el = document.createElement('article');
  el.className = 'message assistant code-run';
  el.dataset.state = 'working';
  el.innerHTML =
    `<div class="orb-slot"></div><div class="reply-card">` +
    `<div class="reply-meta"><span class="model-tag"></span></div>` +
    `<p class="think-live">Working it out… <span class="think-count">0 thoughts so far</span></p>` +
    `<span class="run-summary" hidden></span>` +
    `<button type="button" class="think-toggle" aria-expanded="false" hidden><span class="chev">${icons.chevronRight}</span><span class="think-toggle-label">Show thoughts</span></button>` +
    `<div class="thinking-body run-thinking" hidden></div>` +
    `<div class="run-flow"></div>` +
    `<p class="run-stopped" hidden></p>` +
    `<div class="run-foot mono" hidden></div></div>`;
  const tag = el.querySelector<HTMLElement>('.model-tag')!;
  tag.textContent = modelSelect().value;
  const orb = new FerroOrb({ size: 'small', tuning: AGENT_TUNING, label: ORB_LABELS.working });
  el.querySelector('.orb-slot')!.append(orb.el);
  orbs.push(orb);
  orb.setMode('active');
  el.querySelector('.think-toggle')!.addEventListener('click', () => toggleThoughts(r));
  const r: Run = { el, orb, started: performance.now(), ended: null, steps: 0, thoughts: 0, thinking: '', text: '', textEl: null, toolsEl: null, waiting: 0, done: false };
  append(el);
  return r;
}

function toggleThoughts(r: Run) {
  const btn = r.el.querySelector<HTMLButtonElement>('.think-toggle')!;
  const open = btn.getAttribute('aria-expanded') !== 'true';
  btn.setAttribute('aria-expanded', String(open));
  const body = r.el.querySelector<HTMLElement>('.run-thinking')!;
  body.hidden = !open;
  if (open) body.textContent = r.thinking.trim();
  btn.querySelector('.chev')!.innerHTML = open ? icons.chevron : icons.chevronRight;
  btn.querySelector('.think-toggle-label')!.textContent = open ? 'Hide thoughts' : 'Show thoughts';
  const summary = r.el.querySelector<HTMLElement>('.run-summary')!;
  if (!summary.hidden) summary.textContent = workedLine(r); // ", hidden" comes and goes
}

/** "Worked for X seconds · N steps · N thoughts, hidden" */
function workedLine(r: Run): string {
  const secs = Math.max(1, Math.round(((r.ended ?? performance.now()) - r.started) / 1000));
  const open = r.el.querySelector('.think-toggle')?.getAttribute('aria-expanded') === 'true';
  const thoughts = r.thoughts ? ` · ${plural(r.thoughts, 'thought')}${open ? '' : ', hidden'}` : '';
  return `Worked for ${plural(secs, 'second')} · ${plural(r.steps, 'step')}${thoughts}`;
}

/** The status line and the orb follow what the agent is doing. */
function renderState(r: Run) {
  const working = !r.done && r.waiting === 0;
  r.el.dataset.state = r.done ? 'done' : working ? 'working' : 'waiting';
  const live = r.el.querySelector<HTMLElement>('.think-live')!;
  live.hidden = !working;
  if (working) live.querySelector('.think-count')!.textContent = `${plural(r.thoughts, 'thought')} so far`;
  const summary = r.el.querySelector<HTMLElement>('.run-summary')!;
  summary.hidden = working;
  if (!working) summary.textContent = workedLine(r);
  r.el.querySelector<HTMLElement>('.think-toggle')!.hidden = !r.thinking.trim();
  const body = r.el.querySelector<HTMLElement>('.run-thinking')!;
  if (!body.hidden) body.textContent = r.thinking.trim();
  if (r.done) {
    // Done, stopped or refused: it breathes quietly (never completely still while visible).
    r.orb.setMode('calm');
    r.orb.setLabel(agentAllowed() ? ORB_LABELS.idle : ORB_LABELS.off);
  } else if (working) {
    r.orb.setMode('active');
    r.orb.setLabel(ORB_LABELS.working);
  } else {
    // It has stopped and is waiting for you, so it must not look busy.
    r.orb.setMode('calm');
    r.orb.setLabel(ORB_LABELS.waiting);
  }
}

// Streaming text is drawn once per frame.
let frame = 0;
let dirty: Run | null = null;

function flush() {
  if (frame) cancelAnimationFrame(frame);
  frame = 0;
  const r = dirty;
  dirty = null;
  if (!r) return;
  const stick = nearBottom();
  if (r.textEl) {
    r.textEl.hidden = !r.text.trim();
    r.textEl.innerHTML = renderMarkdown(r.text.trim());
  }
  renderState(r);
  if (stick) scrollDown();
}

function scheduleRender(r: Run) {
  dirty = r;
  if (!frame) frame = requestAnimationFrame(flush);
}

function flow(r: Run): HTMLElement {
  return r.el.querySelector<HTMLElement>('.run-flow')!;
}

/** A new piece of explanation; the tool steps before it stay one group. */
function textSegment(r: Run): HTMLElement {
  if (!r.textEl) {
    r.textEl = document.createElement('div');
    r.textEl.className = 'body md';
    r.textEl.hidden = true;
    flow(r).append(r.textEl);
    r.toolsEl = null;
  }
  return r.textEl;
}

const ING: Record<string, string> = { List: 'Listing', Read: 'Reading', Search: 'Searching', Edit: 'Editing', Write: 'Writing' };

function toolRow(r: Run, callId: string, summary: string) {
  flush();
  // The text before a tool call is finished; the next text starts below the tools.
  r.textEl = null;
  r.text = '';
  if (!r.toolsEl) {
    r.toolsEl = document.createElement('ul');
    r.toolsEl.className = 'tool-list';
    flow(r).append(r.toolsEl);
  }
  const space = summary.indexOf(' ');
  const verb = space > 0 ? summary.slice(0, space) : summary;
  const rest = space > 0 ? summary.slice(space) : '';
  const li = document.createElement('li');
  li.className = 'tool-row';
  li.dataset.state = 'running';
  li.dataset.verb = verb;
  li.innerHTML =
    `<span class="tool-mark" aria-hidden="true">${icons.check}</span>` +
    `<span class="tool-summary"><span class="tool-verb"></span><span class="mono tool-target"></span></span><span class="tool-result"></span>`;
  li.querySelector('.tool-verb')!.textContent = ING[verb] ?? verb;
  li.querySelector('.tool-target')!.textContent = rest;
  r.toolsEl.append(li);
  toolRows.set(callId, li);
  if (nearBottom()) scrollDown();
}

function finishToolRow(li: HTMLElement, ok: boolean, summary: string) {
  li.dataset.state = ok ? 'ok' : 'failed';
  li.querySelector('.tool-verb')!.textContent = li.dataset.verb ?? '';
  li.querySelector('.tool-mark')!.innerHTML = ok ? icons.check : icons.close;
  li.querySelector('.tool-result')!.textContent = summary;
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

/** The first line the edit changes (1-based). */
function firstChangedLine(e: PlannedEdit): number {
  const a = e.before.split('\n');
  const b = e.after.split('\n');
  let i = 0;
  while (i < a.length && i < b.length && a[i] === b[i]) i++;
  return i + 1;
}

function setApprovalTitle(el: HTMLElement, state: string, title: string) {
  el.dataset.state = state;
  el.querySelector('.approval-label')!.textContent = title;
}

/** Replaces the buttons with the result, so a decision can't be flipped by a second click. */
function showResult(el: HTMLElement, label: string, kind: 'ok' | 'muted' | 'fail' | 'busy', note: string) {
  el.querySelectorAll<HTMLElement>('.approval-actions, .approval-note-row').forEach((x) => x.remove());
  const box = el.querySelector<HTMLElement>('.approval-result')!;
  box.hidden = false;
  box.innerHTML = `<span class="result-pill" data-kind="${kind}"></span><span class="result-note small"></span>`;
  box.querySelector('.result-pill')!.textContent = label;
  box.querySelector('.result-note')!.textContent = note;
}

function approvalBlock(r: Run, approvalId: string, callId: string, edit: PlannedEdit) {
  flush();
  r.textEl = null;
  r.text = '';
  r.toolsEl = null;
  const el = document.createElement('div');
  el.className = 'code-approval';
  el.dataset.state = 'waiting';
  el.dataset.call = callId;
  const { adds, dels } = changeCounts(edit);
  el.innerHTML =
    `<div class="approval-head"><span class="approval-title"><span class="approval-dot" aria-hidden="true"></span><span class="approval-label">Proposed change · waiting for you</span></span>` +
    `<span class="mono approval-where"><span class="approval-path"></span><span class="approval-line"></span></span>` +
    `<span class="mono approval-count">+${adds} −${dels}</span></div>` +
    `<div class="diff-file code-diff">${diffHtml(edit)}</div>` +
    `<div class="approval-actions"><button type="button" class="btn accent" data-act="apply">Approve</button><button type="button" class="btn" data-act="reject">Reject</button>` +
    `<span class="small muted">Nothing is written to your folder until you approve.</span></div>` +
    `<div class="approval-note-row"><label class="sr-only" for="note-${approvalId}">Tell the agent what to change</label>` +
    `<input id="note-${approvalId}" class="approval-note" type="text" placeholder="Tell the agent what to change (optional, sent with Approve or Reject)" spellcheck="true" /></div>` +
    `<div class="approval-result" hidden></div>`;
  el.querySelector('.approval-path')!.textContent = edit.path;
  el.querySelector('.approval-line')!.textContent = edit.newFile ? ' · new file' : ` · line ${firstChangedLine(edit)}`;
  el.addEventListener('click', async (e) => {
    const act = (e.target as HTMLElement).closest<HTMLButtonElement>('[data-act]')?.dataset.act;
    if (!act || el.dataset.state !== 'waiting' || el.dataset.answering) return;
    el.dataset.answering = '1';
    const buttons = el.querySelectorAll<HTMLButtonElement>('[data-act]');
    buttons.forEach((b) => (b.disabled = true));
    const note = el.querySelector<HTMLInputElement>('.approval-note')!.value;
    try {
      await api.agentRespond(approvalId, act === 'apply', note);
      // The result comes from the backend (toolResult), which may already have arrived;
      // until then, say what is happening.
      if (el.dataset.state !== 'waiting') return;
      if (act === 'apply') {
        setApprovalTitle(el, 'applying', 'Writing the change…');
        showResult(el, 'Writing…', 'busy', '');
      } else {
        setApprovalTitle(el, 'rejected', 'Change rejected');
        showResult(el, 'Rejected', 'muted', 'Nothing was written. Tell the agent what to try instead.');
      }
    } catch (err) {
      toast(errorText(err), 'error');
      delete el.dataset.answering;
      buttons.forEach((b) => (b.disabled = false));
    }
  });
  approvals.set(callId, el);
  // The edit's step row waits with you rather than looking busy.
  const row = toolRows.get(callId);
  if (row) {
    row.dataset.state = 'waiting';
    row.querySelector('.tool-verb')!.textContent = row.dataset.verb ?? '';
    row.querySelector('.tool-result')!.textContent = 'waiting for you';
  }
  approvals.set(approvalId, el);
  flow(r).append(el);
  r.waiting += 1;
  renderState(r);
  if (nearBottom()) scrollDown();
  el.querySelector<HTMLButtonElement>('[data-act="apply"]')?.focus({ preventScroll: true });
  if (currentPage() !== 'code') toast(`The coding agent wants to change ${edit.path}. Open the Coding agent to review it.`);
}

function onToolResult(ev: Extract<AgentEvent, { type: 'toolResult' }>) {
  const row = toolRows.get(ev.callId);
  const a = approvals.get(ev.callId);
  // An edit's full result is in its change block; its step row only says what happened.
  if (row) finishToolRow(row, ev.ok, !a ? ev.summary : ev.ok ? 'written' : ev.summary === 'Rejected.' ? 'rejected' : 'not written');
  if (!a) return;
  const path = a.querySelector('.approval-path')!.textContent ?? '';
  if (ev.ok) {
    const backup = ev.summary.replace(/^Applied\.\s*/, '').replace(/^Backup of the old version:\s*/, 'old version kept in ');
    setApprovalTitle(a, 'applied', 'Change applied');
    showResult(a, 'Applied ✓', 'ok', `Written to ${path} · logged${backup ? ` · ${backup}` : ''}`);
  } else if (ev.summary === 'Rejected.') {
    setApprovalTitle(a, 'rejected', 'Change rejected');
    showResult(a, 'Rejected', 'muted', 'Nothing was written. Tell the agent what to try instead.');
  } else {
    setApprovalTitle(a, 'failed', 'Change not written');
    showResult(a, 'Not written', 'fail', ev.summary);
  }
}

function onResolved(r: Run, ev: Extract<AgentEvent, { type: 'resolved' }>) {
  const a = approvals.get(ev.approvalId);
  r.waiting = Math.max(0, r.waiting - 1);
  if (a && ev.message) {
    setApprovalTitle(a, 'stopped', 'Change not made');
    showResult(a, 'Stopped', 'muted', 'Nothing was written.');
    const row = toolRows.get(a.dataset.call ?? '');
    if (row) {
      finishToolRow(row, false, 'stopped');
      row.dataset.state = 'stopped';
    }
  }
  renderState(r);
}

function onDone(r: Run, ev: Extract<AgentEvent, { type: 'done' }>) {
  dirty = r;
  flush();
  r.done = true;
  r.ended = performance.now();
  r.waiting = 0;
  const summary = r.el.querySelector<HTMLElement>('.run-summary')!;
  summary.classList.add('code-done');
  summary.dataset.status = ev.status;
  r.el.dataset.status = ev.status;
  const line = r.el.querySelector<HTMLElement>('.run-stopped')!;
  if (ev.status === 'stopped') {
    const locked = !!getOperator()?.lockdown;
    const who = locked ? 'Stopped by Lockdown.' : 'Stopped by you.';
    line.textContent =
      ev.editsApplied === 0
        ? `${who} Nothing was written to your folder.`
        : `${who} Nothing more was written; the ${plural(ev.editsApplied, 'change')} you approved earlier ${ev.editsApplied === 1 ? 'stays' : 'stay'} applied.`;
    line.hidden = false;
  } else if (ev.status === 'limit' && ev.message) {
    const note = document.createElement('p');
    note.className = 'run-note small muted';
    note.textContent = ev.message;
    flow(r).append(note);
  } else if (ev.status === 'error') {
    const box = document.createElement('div');
    box.className = 'error-box';
    box.textContent = ev.message || 'Something went wrong.';
    flow(r).append(box);
  }
  const foot = r.el.querySelector<HTMLElement>('.run-foot')!;
  foot.hidden = !ev.tokens;
  foot.textContent = `${ev.tokens.toLocaleString()} tokens`;
  renderState(r);
}

function onEvent(ev: AgentEvent) {
  // The activity panel sees every event first, as it arrives.
  activityEvent(ev);
  if (ev.type === 'started') {
    taskId = ev.taskId;
    // The folder this run really works in, as the backend reports it.
    folderSeq++;
    renderFolder({ ...folderState, folder: ev.folder, error: null });
  }
  const r = run;
  if (!r) return;
  switch (ev.type) {
    case 'started':
      r.el.querySelector<HTMLElement>('.model-tag')!.title = `Context window for this task: ${ev.numCtx.toLocaleString()} tokens (Chat & model settings → Coding agent window)`;
      break;
    case 'step':
      flush();
      r.steps = ev.step;
      $('#code-step').textContent = String(ev.step);
      // Each step writes its own piece of text.
      r.textEl = null;
      r.text = '';
      renderState(r);
      break;
    case 'thinking':
      r.thinking += ev.text;
      r.thoughts += 1;
      r.orb.event();
      scheduleRender(r);
      break;
    case 'content':
      textSegment(r);
      r.text += ev.text;
      r.orb.event();
      scheduleRender(r);
      break;
    case 'toolCall':
      toolRow(r, ev.callId, ev.summary);
      r.orb.event(0.5);
      break;
    case 'toolResult':
      onToolResult(ev);
      r.orb.event(0.3);
      break;
    case 'approval':
      approvalBlock(r, ev.approvalId, ev.callId, ev.edit);
      break;
    case 'resolved':
      onResolved(r, ev);
      break;
    case 'done':
      onDone(r, ev);
      run = null;
      setRunning(false);
      break;
  }
}

// ---------- actions ----------

async function start() {
  if (running) return;
  const message = input().value.trim();
  const model = modelSelect().value;
  if (!message) return;
  if (!agentAllowed()) {
    renderAvailability();
    return;
  }
  // The backend holds the folder and re-checks it here; without one nothing runs.
  // (agent_run refuses too: the backend stays the authority.)
  await refreshFolder();
  if (!folderState.folder) {
    toast('Pick a folder first.', 'error');
    openFolderPop();
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
  run = newRun();
  lastRun = run;
  $('#code-step').textContent = '0';
  setRunning(true);
  showActivity(true);
  activityTask(message);
  try {
    await api.agentRun({ taskId, model, message }, onEvent);
  } catch (e) {
    // Refused before it started (permission, folder, another task running).
    // It never ran, so there is no reply card; the reason goes under your request.
    run?.orb.dispose();
    run?.el.remove();
    run = null;
    lastRun = null;
    const box = document.createElement('div');
    box.className = 'error-box';
    box.textContent = errorText(e);
    append(box);
    input().value = message;
    autoGrow();
    setRunning(false);
  }
}

/** Stop agent, Esc, and switching the agent off during a run all end it here. */
async function stop() {
  if (!running || stopping) return;
  stopping = true;
  renderControls();
  await api.agentStop().catch((e) => {
    stopping = false;
    renderControls();
    toast(errorText(e), 'error');
  });
}

/** The Agent on/off button: the "Coding agent" permission. Off during a run stops it exactly like Stop. */
async function toggleAgent() {
  const o = getOperator();
  if (!o || o.lockdown) return;
  if (o.allowCodingAgent) {
    if (running) await stop();
    await setCodingAgentAllowed(false);
  } else {
    await setCodingAgentAllowed(true);
  }
}

function newTask() {
  if (running) return;
  if (taskId) void api.agentForget(taskId);
  taskId = '';
  toolRows.clear();
  approvals.clear();
  for (const orb of orbs.splice(0)) orb.dispose();
  run = null;
  lastRun = null;
  $('#code-step').textContent = '0';
  emptyLog();
  input().focus();
}

// ---------- folder ----------
// The backend holds the folder. The chip and the strip show only what it
// reports (or what a run's `started` event says), never what was typed.

/** Shows a FolderState from the backend: the chip, the strip and the Recent list. */
function renderFolder(s: FolderState) {
  const previous = folderState.folder;
  folderState = s;
  const chip = $('#code-folder');
  chip.textContent = s.folder ?? 'No folder chosen';
  chip.classList.toggle('is-empty', !s.folder);
  const strip = $('.code-safety-sandbox');
  strip.classList.toggle('is-refused', !s.folder);
  const why = s.error ?? 'Pick a folder first.';
  $('#code-sandbox-path').textContent = s.folder ?? `none — ${why}`;
  strip.title = s.folder ? `The agent sees and changes only files inside ${s.folder}.` : why;
  const list = $('#code-folder-recent');
  list.replaceChildren(
    ...s.recent.map((path) => {
      const li = document.createElement('li');
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'code-folder-recent-btn mono';
      b.dataset.path = path;
      b.textContent = path;
      if (path === s.folder) b.setAttribute('aria-current', 'true');
      li.append(b);
      return li;
    }),
  );
  if (!s.recent.length) {
    const li = document.createElement('li');
    li.className = 'small muted';
    li.textContent = 'No recent folders yet.';
    list.append(li);
  }
  // A different folder is a different task.
  if (previous !== s.folder && taskId && !running) newTask();
}

/** Asks the backend for its folder (it re-checks it) and shows the answer. */
async function refreshFolder() {
  const seq = ++folderSeq;
  try {
    const s = await api.agentFolder();
    if (seq === folderSeq && !running) renderFolder(s);
  } catch (e) {
    if (seq === folderSeq && !running) renderFolder({ folder: null, error: errorText(e), recent: folderState.recent });
  }
}

function folderPopOpen(): boolean {
  return !folderPop().hidden;
}

function showFolderError(message: string) {
  const err = $('#code-folder-error');
  err.textContent = message;
  err.hidden = !message;
}

function openFolderPop() {
  if (changeFolderBtn().disabled) return;
  showFolderError('');
  folderPop().hidden = false;
  changeFolderBtn().setAttribute('aria-expanded', 'true');
  $<HTMLInputElement>('#code-folder-paste').value = '';
  $('#code-folder-browse').focus();
}

function closeFolderPop(focusButton = false) {
  if (!folderPopOpen()) return;
  folderPop().hidden = true;
  changeFolderBtn().setAttribute('aria-expanded', 'false');
  if (focusButton) changeFolderBtn().focus();
}

/**
 * Runs one folder request (paste, Recent or Browse). A refusal leaves the folder
 * as it was and shows the backend's reason here and as a toast.
 */
async function folderRequest(req: () => Promise<FolderState | null>) {
  const pop = folderPop();
  if (pop.dataset.busy) return;
  pop.dataset.busy = '1';
  // Disabling drops the focus; it goes back afterwards unless the popover closes.
  const had = document.activeElement as HTMLElement | null;
  const controls = pop.querySelectorAll<HTMLButtonElement | HTMLInputElement>('button, input');
  controls.forEach((c) => (c.disabled = true));
  showFolderError('');
  const seq = ++folderSeq;
  let close = false;
  try {
    const s = await req();
    if (!s) return; // the dialog was cancelled
    if (seq === folderSeq) renderFolder(s);
    if (s.folder) close = true;
    else showFolderError(s.error ?? 'Pick a folder first.');
  } catch (e) {
    const message = errorText(e);
    showFolderError(message);
    toast(message, 'error');
  } finally {
    delete pop.dataset.busy;
    controls.forEach((c) => (c.disabled = false));
  }
  if (close) {
    closeFolderPop();
    input().focus();
  } else if (folderPopOpen() && had && pop.contains(had)) {
    had.focus();
  }
}

function initFolderPop() {
  changeFolderBtn().addEventListener('click', () => (folderPopOpen() ? closeFolderPop() : openFolderPop()));
  $('#code-folder-browse').addEventListener('click', () => void folderRequest(() => api.agentPickFolder()));
  $('#code-folder-form').addEventListener('submit', (e) => {
    e.preventDefault();
    const path = $<HTMLInputElement>('#code-folder-paste').value.trim();
    void folderRequest(() => api.agentSetFolder(path));
  });
  $('#code-folder-recent').addEventListener('click', (e) => {
    const path = (e.target as HTMLElement).closest<HTMLButtonElement>('.code-folder-recent-btn')?.dataset.path;
    if (path) void folderRequest(() => api.agentSetFolder(path));
  });
  folderPop().addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeFolderPop(true);
    }
  });
  // A click outside closes it (not while a request or the native dialog is open).
  document.addEventListener('pointerdown', (e) => {
    if (folderPopOpen() && !folderPop().dataset.busy && !(e.target as HTMLElement).closest('.code-folder-wrap')) closeFolderPop();
  });
  // So does tabbing out of it.
  $('.code-folder-wrap').addEventListener('focusout', (e) => {
    const to = e.relatedTarget as HTMLElement | null;
    if (to && !to.closest('.code-folder-wrap') && !folderPop().dataset.busy) closeFolderPop();
  });
}

function autoGrow() {
  const i = input();
  i.style.height = 'auto';
  i.style.height = `${Math.min(i.scrollHeight, 200)}px`;
}

export function initCodePage(d: Deps) {
  deps = d;
  $('#code-send').innerHTML = icons.arrowUp;
  $('#code-step-max').textContent = String(MAX_STEPS);
  emptyLog();
  initActivity(MAX_STEPS);
  initFolderPop();
  void api.appPaths().then((p) => {
    homeBackups = `${p.home}\\backups\\agent\\`;
    $('.code-folder-chip').title += ` Before a file is changed, its old version is copied to ${homeBackups}<task>.`;
  });
  onOperatorChange(renderAvailability);
  onShow('code', () => {
    renderAvailability();
    if (!running) {
      void loadModels();
      // The re-check: the backend says whether its folder can still be used.
      void refreshFolder();
    }
    input().focus();
  });
  onLeave('code', () => closeFolderPop());
  modelSelect().addEventListener('change', () => store.set(MODEL_KEY, modelSelect().value));
  $('#code-send').addEventListener('click', () => void start());
  $('#code-stop').addEventListener('click', () => void stop());
  $('#code-agent').addEventListener('click', () => void toggleAgent());
  $('#code-new').addEventListener('click', newTask);
  input().addEventListener('input', autoGrow);
  input().addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      void start();
    }
  });
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && currentPage() === 'code' && folderPopOpen()) {
      e.preventDefault();
      closeFolderPop(true);
      return;
    }
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
  renderControls();
}
