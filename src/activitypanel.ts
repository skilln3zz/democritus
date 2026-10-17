// The Coding agent's live activity panel: a read-only, timestamped log of every
// event of a run, as it arrives (design: a dark terminal surface in a raised
// frame). It is a view, not a shell: there is no input and nothing here runs.
// Model and tool text only ever goes in through text nodes (never innerHTML).
import { api, type AgentEvent, type PlannedEdit } from './api';
import { diffLines } from './diff';
import { $, errorText, toast } from './ui';

const SHOWN_KEY = 'democritus.codeActivity';
/** Oldest lines are dropped beyond this. */
const MAX_LINES = 4000;

const TOOL_TAGS: Record<string, string> = {
  list_files: 'LIST',
  read_file: 'READ',
  search_files: 'SEARCH',
  edit_file: 'EDIT',
  write_file: 'WRITE',
};

let maxSteps = 0;
let paused = false;
/** The THINK or TEXT line that streamed chunks are appended to, while they come in a row. */
let stream: { kind: 'thinking' | 'content'; node: Text } | null = null;
/** Tool lines by call id, so the result lands on the call's line. */
const toolLines = new Map<string, HTMLElement>();
/** Proposed paths by approval id, for the APPROVED / REJECTED lines. */
const proposed = new Map<string, string>();
let scrollFrame = 0;

const panel = () => $('#code-activity');
const body = () => $('#activity-log');
const toggleBtn = () => $<HTMLButtonElement>('#code-activity-btn');

const counts = new WeakMap<PlannedEdit, { adds: number; dels: number }>();

/** Lines added and removed by a proposed edit (computed once per edit, shared with the approval card). */
export function changeCounts(edit: PlannedEdit): { adds: number; dels: number } {
  let c = counts.get(edit);
  if (!c) {
    const all = diffLines(edit.before, edit.after, 0);
    c = { adds: all.filter((l) => l.kind === 'add').length, dels: all.filter((l) => l.kind === 'del').length };
    counts.set(edit, c);
  }
  return c;
}

function stamp(d = new Date()): string {
  const p = (n: number, w = 2) => String(n).padStart(w, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

function scrollSoon() {
  if (paused || scrollFrame) return;
  scrollFrame = requestAnimationFrame(() => {
    scrollFrame = 0;
    const b = body();
    b.scrollTop = b.scrollHeight;
  });
}

/** Appends one line: time, tag, text. Returns the line and its text node (for streaming). */
function line(tag: string, text: string, kind = ''): { el: HTMLElement; node: Text } {
  stream = null;
  const el = document.createElement('div');
  el.className = 'act-line';
  el.dataset.tag = tag;
  if (kind) el.classList.add(`is-${kind}`);
  const time = document.createElement('span');
  time.className = 'act-time';
  time.textContent = stamp();
  const t = document.createElement('span');
  t.className = 'act-tag';
  t.textContent = tag;
  const msg = document.createElement('span');
  msg.className = 'act-text';
  const node = document.createTextNode(text);
  msg.append(node);
  el.append(time, t, msg);
  const b = body();
  b.querySelector('.act-empty')?.remove();
  b.append(el);
  while (b.childElementCount > MAX_LINES) {
    const old = b.firstElementChild as HTMLElement;
    if (old.dataset.call) toolLines.delete(old.dataset.call);
    old.remove();
  }
  scrollSoon();
  return { el, node };
}

/** Streamed reasoning / answer text: consecutive chunks extend the same line. */
function streamed(kind: 'thinking' | 'content', chunk: string) {
  if (stream?.kind === kind && stream.node.isConnected) {
    stream.node.appendData(chunk);
    scrollSoon();
    return;
  }
  const { node } = line(kind === 'thinking' ? 'THINK' : 'TEXT', chunk.replace(/^\s+/, ''));
  stream = { kind, node };
}

/** "Read src/a.ts" → "src/a.ts"; "Search “vat” in ." → "'vat' in ." */
function toolTarget(summary: string): string {
  const space = summary.indexOf(' ');
  const rest = space > 0 ? summary.slice(space + 1) : summary;
  return rest.replace(/[“”]/g, "'");
}

function toolResult(ev: Extract<AgentEvent, { type: 'toolResult' }>) {
  const el = toolLines.get(ev.callId);
  stream = null;
  if (!el) {
    line(ev.ok ? 'RESULT' : 'ERROR', ev.summary, ev.ok ? '' : 'error');
    return;
  }
  const res = document.createElement('span');
  res.className = 'act-result';
  const tag = el.dataset.tag;
  if (ev.ok) {
    res.textContent = tag === 'READ' ? ` (${ev.summary})` : ` → ${ev.summary}`;
  } else if ((tag === 'EDIT' || tag === 'WRITE') && ev.summary === 'Rejected.') {
    res.textContent = ' → rejected';
  } else {
    res.classList.add('is-error');
    res.textContent = ` → ERROR ${ev.summary}`;
  }
  el.querySelector('.act-text')!.append(res);
  scrollSoon();
}

/** One line (or more) per agent event, appended the moment it arrives. */
export function activityEvent(ev: AgentEvent) {
  switch (ev.type) {
    case 'started':
      line('START', `model ${ev.model} · folder ${ev.folder}`);
      break;
    case 'step':
      line('STEP', `${ev.step} of ${maxSteps}`);
      break;
    case 'thinking':
      streamed('thinking', ev.text);
      break;
    case 'content':
      streamed('content', ev.text);
      break;
    case 'toolCall': {
      const { el } = line(TOOL_TAGS[ev.name] ?? ev.name.toUpperCase(), toolTarget(ev.summary));
      el.dataset.call = ev.callId;
      toolLines.set(ev.callId, el);
      break;
    }
    case 'toolResult':
      toolResult(ev);
      break;
    case 'approval': {
      const { adds, dels } = changeCounts(ev.edit);
      proposed.set(ev.approvalId, ev.edit.path);
      line('PROPOSED', `${ev.edit.path} +${adds} −${dels}${ev.edit.newFile ? ' (new file)' : ''}`);
      break;
    }
    case 'resolved': {
      const path = proposed.get(ev.approvalId) ?? '';
      proposed.delete(ev.approvalId);
      if (ev.approved) {
        line('APPROVED', path);
        if (ev.applied) line('WRITTEN', path);
        else line('FAILED', ev.message || path, 'error');
      } else if (ev.message === 'Stopped.') {
        // Stop, Lockdown or Agent off while the edit waited: nobody rejected it.
        line('STOPPED', `${path} — not written`);
      } else {
        line('REJECTED', path);
      }
      break;
    }
    case 'refused':
      line('REFUSED', `${ev.tool} ${ev.path} — ${ev.reason}`, 'error');
      break;
    case 'stats':
      line('STATS', `${ev.tokensPerSec.toFixed(1)} tok/s · ${ev.tokens} tokens`);
      break;
    case 'done':
      line('DONE', ev.message ? `${ev.status} — ${ev.message}` : ev.status, ev.status === 'error' ? 'error' : '');
      break;
  }
}

/** The request as sent, before the run starts. */
export function activityTask(message: string) {
  line('TASK', message);
}

/** The whole log as plain text: one "time TAG text" line per entry. */
export function activityText(): string {
  return [...body().querySelectorAll<HTMLElement>('.act-line')]
    .map((l) => `${l.querySelector('.act-time')!.textContent} ${(l.dataset.tag ?? '').padEnd(8)} ${l.querySelector('.act-text')!.textContent}`)
    .join('\n');
}

function clear() {
  stream = null;
  toolLines.clear();
  proposed.clear();
  const empty = document.createElement('p');
  empty.className = 'act-empty';
  empty.textContent = 'Nothing yet. Every step of the agent appears here as it happens.';
  body().replaceChildren(empty);
}

export function activityShown(): boolean {
  return !panel().hidden;
}

/** Shows or hides the panel; remembered for next time. */
export function showActivity(on: boolean) {
  panel().hidden = !on;
  $('#page-code').classList.toggle('with-activity', on);
  toggleBtn().setAttribute('aria-pressed', String(on));
  try {
    localStorage.setItem(SHOWN_KEY, on ? '1' : '0');
  } catch {
    // only a convenience
  }
  if (on) scrollSoon();
}

export function initActivity(steps: number) {
  maxSteps = steps;
  clear();
  let shown = false;
  try {
    shown = localStorage.getItem(SHOWN_KEY) === '1';
  } catch {
    // only a convenience
  }
  showActivity(shown);
  toggleBtn().addEventListener('click', () => showActivity(!activityShown()));
  const pause = $<HTMLButtonElement>('#activity-pause');
  pause.addEventListener('click', () => {
    paused = !paused;
    pause.setAttribute('aria-pressed', String(paused));
    if (!paused) scrollSoon();
  });
  $('#activity-copy').addEventListener('click', () => {
    void navigator.clipboard.writeText(activityText()).then(
      () => toast('Copied'),
      () => toast("Couldn't copy to the clipboard.", 'error'),
    );
  });
  $('#activity-clear').addEventListener('click', clear);
  const save = $<HTMLButtonElement>('#activity-save');
  save.addEventListener('click', async () => {
    save.disabled = true;
    try {
      const path = await api.agentSaveLog(activityText());
      toast(`Log saved to ${path}`);
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      save.disabled = false;
    }
  });
}
