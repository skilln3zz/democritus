// The Build Democritus page (Steps 6D and 6E): the status of a build with its
// six stages, GPU temperature and memory, the capabilities of the last built
// model, and "Your examples" (the rated and edited replies a build learns from).
// The backend (build.rs) runs the build; this page only shows it and sends
// Begin / Pause / Resume / Cancel.
import { listen } from '@tauri-apps/api/event';
import { api, type Build, type BuildCheck, type BuildOverview, type BuildStage, type BuildSummary, type GpuStats, type TrainingExample } from './api';
import { icons } from './icons';
import { getOperator, onOperatorChange } from './operatorpage';
import { currentPage, onShow } from './pages';
import { $, choose, errorText, escapeText, formatMem, toast } from './ui';

interface Deps {
  /** Called after "Make default brain" so the chat page picks it up. */
  onDefaultChanged: () => Promise<void>;
}

const STAGES: { id: BuildStage; label: string }[] = [
  { id: 'preflight', label: 'Preflight' },
  { id: 'lessons', label: 'Lessons' },
  { id: 'training', label: 'Training' },
  { id: 'packaging', label: 'Packaging' },
  { id: 'testing', label: 'Testing' },
  { id: 'done', label: 'Done' },
];

let deps: Deps;
let overview: BuildOverview | null = null;
let build: Build | null = null;
let gpu: GpuStats | null = null;
let checks: BuildCheck[] | null = null;
let examples: TrainingExample[] = [];
let tab: 'build' | 'examples' = 'build';
let busy = false;

// ---------- formatting ----------

function duration(secs: number): string {
  if (secs < 60) return 'under a minute';
  const h = Math.floor(secs / 3600);
  const m = Math.round((secs % 3600) / 60);
  if (h === 0) return `${m} min`;
  return m ? `${h} h ${m} min` : `${h} h`;
}

const clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
const ago = (ms: number) => {
  const mins = Math.round((Date.now() - ms) / 60000);
  return mins < 1 ? 'just now' : mins === 1 ? '1 min ago' : mins < 60 ? `${mins} min ago` : `${Math.round(mins / 60)} h ago`;
};
const pct = (a: number, b: number) => `${Math.max(0, Math.min(100, b ? (a / b) * 100 : 0)).toFixed(1)}%`;
const active = () => build && (build.status === 'running' || build.status === 'paused' || build.status === 'failed');
const blockedReason = (): string | null => {
  const o = getOperator();
  if (!o) return null;
  if (o.lockdown) return 'Lockdown is on, so learning (and building) is paused.';
  if (!o.allowTraining) return 'Building needs the "Allow fine-tuning" permission under Settings & safety.';
  return null;
};

// ---------- rendering ----------

function renderStages() {
  const at = STAGES.findIndex((s) => s.id === (build?.stage ?? 'preflight'));
  const finished = build?.status === 'done';
  $('#bd-stages').innerHTML = STAGES.map((s, i) => {
    const state = !build ? 'todo' : finished || i < at ? 'done' : i === at ? (build.status === 'running' ? 'now' : build.status === 'failed' ? 'failed' : 'held') : 'todo';
    const mark = state === 'done' ? `<span class="bd-check">${icons.check}</span>` : `<span class="bd-dot" aria-hidden="true"></span>`;
    return `<li data-state="${state}"${state === 'now' ? ' aria-current="step"' : ''}>${mark}${s.label}</li>`;
  }).join('');
}

function renderStatus() {
  const b = build;
  const p = b ? (b.status === 'done' ? 100 : b.progress) : 0;
  const dial = $('#bd-dial');
  dial.style.setProperty('--p', `${p}%`);
  dial.setAttribute('aria-label', `Overall progress ${p} percent`);
  $('#bd-pct').textContent = `${p}%`;
  const stageLabel = b ? (b.status === 'paused' ? 'Paused' : b.status === 'failed' ? 'Stopped' : b.status === 'cancelled' ? 'Cancelled' : STAGES.find((s) => s.id === b.stage)?.label ?? '') : 'Idle';
  $('#bd-stage-label').textContent = stageLabel;

  const block = blockedReason();
  const off = $('#bd-off');
  off.hidden = !block || (!!b && b.status === 'done');
  off.querySelector('.banner-text')!.textContent = block ?? '';

  $('#bd-setup').hidden = !!active();
  let line = '';
  let headline = '';
  let times = '';
  if (!b || (!active() && b.status !== 'done' && b.status !== 'cancelled')) {
    line = 'Ready when you are';
    headline = 'Build a brain of your own';
    times = 'A test run (20 lessons, a few training steps) takes 20–40 minutes; a full build is meant to run overnight.';
  } else {
    line = `${b.name}${b.testRun ? ' · test run' : ''} · ${b.statusText}`;
    if (b.status === 'running') headline = b.eta != null ? `About ${duration(b.eta)} left` : 'Measuring…';
    else if (b.status === 'paused') headline = 'Paused';
    else if (b.status === 'failed') headline = 'Stopped';
    else if (b.status === 'cancelled') headline = 'Cancelled';
    else headline = `${b.name} is ready`;
    const bits = [`Started ${clock(b.createdAt)}`];
    if (b.status === 'running' && b.eta != null) bits.push(`estimated finish ${clock(Date.now() + b.eta * 1000)}`);
    if (b.finishedAt) bits.push(`finished ${clock(b.finishedAt)}`);
    if (b.checkpointAt) bits.push(`checkpoint saved ${ago(b.checkpointAt)}`);
    times = bits.join(' · ');
  }
  $('#bd-status-line').textContent = line;
  $('#bd-headline').textContent = headline;
  const t = $('#bd-times');
  t.textContent = times;
  if (b?.status === 'failed' && b.error) {
    const err = document.createElement('span');
    err.className = 'form-error bd-error';
    err.textContent = b.error;
    t.append(document.createElement('br'), err);
  }
  renderActions(block);
  $('#bd-awake').hidden = b?.status !== 'running';
  $('#build-badge').hidden = b?.status !== 'running';
}

function button(act: string, label: string, kind = '', icon = ''): string {
  return `<button type="button" class="pill-btn ${kind}" data-act="${act}">${icon ? `<span class="bd-btn-icon" aria-hidden="true">${icon}</span>` : ''}${escapeText(label)}</button>`;
}

function renderActions(block: string | null) {
  const b = build;
  let html = '';
  if (!b || !active()) {
    if (b && (b.status === 'done' || b.status === 'cancelled')) html += button('dismiss', 'Close this build');
    html += button('check', 'Check requirements') + button('test', 'Test run · 20 lessons') + button('begin', 'Begin training', 'accent');
  } else if (b.status === 'running') {
    html = button('pause', 'Pause', 'bd-pause', icons.pause) + button('cancel', 'Cancel build');
  } else {
    html = button('resume', b.status === 'failed' ? 'Try again' : 'Resume', 'bd-pause', icons.play) + button('cancel', 'Cancel build');
  }
  const box = $('#bd-actions');
  box.innerHTML = html;
  box.querySelectorAll<HTMLButtonElement>('[data-act]').forEach((btn) => {
    const act = btn.dataset.act!;
    btn.disabled = busy || (!!block && ['test', 'begin', 'resume'].includes(act));
  });
}

function renderStats() {
  const b = build;
  if (gpu) {
    $('#bd-temp').textContent = `${gpu.temp}°C`;
    const bar = $('#bd-temp-bar');
    bar.style.width = pct(gpu.temp, 100);
    bar.className = gpu.temp >= 80 ? 'hot' : gpu.temp >= 70 ? 'warm' : '';
    $('#bd-mem').textContent = formatMem(gpu.memUsed);
    $('#bd-mem-bar').style.width = pct(gpu.memUsed, gpu.memTotal);
    $('#bd-mem-of').textContent = `of ${formatMem(gpu.memTotal)}${b?.vramPeak ? ` · training peak ${formatMem(b.vramPeak)}` : ''}`;
  } else {
    $('#bd-temp').textContent = '—';
    $('#bd-mem').textContent = '—';
    $('#bd-mem-of').textContent = 'No NVIDIA GPU found';
  }
  $('#bd-step').textContent = b && b.steps ? `${b.step.toLocaleString()} / ${b.steps.toLocaleString()}` : '—';
  $('#bd-step-bar').style.width = b ? pct(b.step, b.steps) : '0%';
  const trend = b?.loss != null && b.lossStart != null ? (b.loss < b.lossStart - 0.02 ? 'falling' : b.loss > b.lossStart + 0.02 ? 'rising' : 'steady') : '';
  $('#bd-loss').textContent = b?.loss != null ? `Loss ${b.loss.toFixed(2)} · ${trend}` : b?.secondsPerStep ? '' : 'Starts after the lessons';
  $('#bd-lessons-n').textContent = b ? `${b.lessonsDone.toLocaleString()}` : '—';
  $('#bd-lessons-bar').style.width = b ? pct(b.lessonsDone, b.lessonTarget) : '0%';
  $('#bd-lessons-note').textContent = b ? `of ${b.lessonTarget.toLocaleString()} · ${b.lessonsFromYou} from you, the rest written by the teacher` : 'Written by the teacher';
}

function renderLog() {
  const box = $('#bd-log');
  if (checks && (!build || !active())) {
    box.innerHTML =
      `<div class="bd-log-head">Requirements</div>` +
      checks
        .map((c) => `<div class="bd-check-row" data-ok="${c.ok}"><span class="bd-check-mark">${c.ok ? icons.check : icons.close}</span><span class="strong">${escapeText(c.label)}</span><span class="muted">${escapeText(c.detail)}</span></div>`)
        .join('');
    return;
  }
  if (!build || !build.log.length) {
    box.innerHTML = '<div class="muted">The build log appears here: lessons, training steps, checkpoints and test results.</div>';
    return;
  }
  const stick = box.scrollHeight - box.scrollTop - box.clientHeight < 40;
  box.innerHTML = build.log
    .slice(-60)
    .map((l) => `<div class="${l.alert ? 'bd-log-alert' : ''}"><span class="bd-log-time">${clock(l.ts)}</span>${escapeText(l.text)}</div>`)
    .join('');
  if (stick) box.scrollTop = box.scrollHeight;
}

/** The capabilities card shows the newest finished build (a real one if any). */
function shownBuild(): BuildSummary | null {
  const list = overview?.finished ?? [];
  return list.find((b) => !b.testRun) ?? list[0] ?? null;
}

function renderCaps() {
  const s = shownBuild();
  $('#bd-cap-name').textContent = s?.name ?? '';
  const note = $('#bd-cap-note');
  const bars = $('#bd-cap-bars');
  if (!s) {
    note.textContent = 'No finished build yet. After a build, its test scores appear here, compared with the teacher.';
    bars.innerHTML = '';
  } else {
    const when = s.finishedAt ? new Date(s.finishedAt).toLocaleDateString([], { month: 'short', day: 'numeric' }) : '';
    note.textContent = `${s.testRun ? 'Test build' : 'Last finished build'}, tested ${when}. Scores compared with the teacher (${s.teacher}).`;
    const rows: [string, number][] = s.scores
      ? [
          ['Knows who it is', s.scores.identity],
          ['Follows your rules', s.scores.rules],
          ['Your everyday tasks', s.scores.everyday],
          ['Coding', s.scores.coding],
        ]
      : [];
    bars.innerHTML = rows
      .map(
        ([label, v]) =>
          `<div class="bd-cap-row"><div class="with-action small"><span>${label}</span><span class="strong">${v}%</span></div>` +
          `<div class="bd-bar big"><div class="${v < 80 ? 'soft' : ''}" style="width:${v}%"></div></div></div>`,
      )
      .join('');
  }
  $('#bd-cap-tps').textContent = s?.scores?.studentTps ? String(Math.round(s.scores.studentTps)) : '—';
  $('#bd-cap-speed').textContent = s?.scores?.speedup ? `${s.scores.speedup >= 10 ? Math.round(s.scores.speedup) : s.scores.speedup.toFixed(1)}×` : '—';
  const makeDefault = $<HTMLButtonElement>('#bd-make-default');
  makeDefault.disabled = !s || s.testRun;
  makeDefault.title = s?.testRun ? 'A test build is for checking the stages, not for everyday use.' : '';
  $<HTMLButtonElement>('#bd-compare').disabled = !s?.scores;
}

function render() {
  renderStages();
  renderStatus();
  renderStats();
  renderLog();
  renderCaps();
}

// ---------- your examples (6D) ----------

async function loadExamples() {
  try {
    examples = await api.trainingExamples();
    const status = await api.datasetStatus();
    $('#bd-ex-identity').textContent = String(status.identityExamples);
    const l = status.latest;
    $('#bd-ex-latest').textContent = l
      ? `Last export ${l.exportedAt}: ${l.rated} rated, ${l.edited} edited, ${l.identity} about Democritus${l.excluded ? `, ${l.excluded} left out` : ''} · ${status.folder}\\${l.file}`
      : `Not exported yet. Exports go to ${status.folder}. A build exports them by itself when it starts.`;
  } catch (e) {
    toast(errorText(e), 'error');
  }
  renderExamples();
}

function renderExamples() {
  const used = examples.filter((e) => !e.excluded).length;
  $('#bd-ex-count').textContent = examples.length ? String(used) : '';
  $('#bd-ex-summary').textContent = `${used} of ${examples.length} included · ${examples.filter((e) => e.edited).length} edited`;
  const list = $('#bd-ex-items');
  if (!examples.length) {
    list.innerHTML =
      '<li class="bd-ex-empty muted">No examples yet. In chat, rate good answers with a thumbs up, or press <strong>Edit</strong> under a reply to write the answer you wanted.</li>';
    return;
  }
  list.innerHTML = examples
    .map(
      (e) =>
        `<li class="bd-ex-item${e.excluded ? ' excluded' : ''}" data-id="${e.messageId}">` +
        `<div class="bd-ex-meta"><span class="badge bd-ex-badge">${e.edited ? `${icons.edit}Edited by you` : `${icons.thumbUp}Rated`}</span><span class="mono small muted">${escapeText(e.model)}</span><span class="small muted">${escapeText(e.conversationTitle)}</span>` +
        `<span class="spacer"></span><label class="toggle-chip small">Include<input type="checkbox" class="switch" data-id="${e.messageId}" ${e.excluded ? '' : 'checked'} /></label></div>` +
        `<p class="bd-ex-q"><span class="label">You asked</span> ${escapeText(e.prompt.slice(0, 400))}${e.prompt.length > 400 ? '…' : ''}</p>` +
        `<p class="bd-ex-a"><span class="label">Answer</span> ${escapeText(e.answer.slice(0, 600))}${e.answer.length > 600 ? '…' : ''}</p></li>`,
    )
    .join('');
}

// ---------- actions ----------

async function refresh() {
  try {
    overview = await api.buildOverview();
    build = overview.current;
    const sel = $<HTMLSelectElement>('#bd-teacher');
    if (!sel.options.length) {
      const models = (await api.listModels().catch(() => [])).filter((m) => !/embed/i.test(m.id) && !m.id.startsWith('democritus:'));
      sel.replaceChildren(...models.map((m) => new Option(`${m.id}${m.parameterSize ? ` · ${m.parameterSize}` : ''}`, m.id)));
      sel.value = models.some((m) => m.id === overview!.defaultTeacher) ? overview.defaultTeacher : (models[0]?.id ?? '');
    }
  } catch (e) {
    toast(errorText(e), 'error');
  }
  render();
}

async function pollGpu() {
  gpu = await api.buildGpu().catch(() => null);
  renderStats();
}

async function runCheck(): Promise<BuildCheck[] | null> {
  const teacher = $<HTMLSelectElement>('#bd-teacher').value;
  checks = [{ id: 'wait', label: 'Checking…', ok: true, detail: 'Python tools, student weights, teacher, GPU and disk' }];
  renderLog();
  try {
    checks = await api.buildPreflight(teacher, false);
  } catch (e) {
    toast(errorText(e), 'error');
    checks = null;
  }
  renderLog();
  return checks;
}

async function begin(testRun: boolean) {
  const teacher = $<HTMLSelectElement>('#bd-teacher').value;
  const lessons = testRun ? 20 : Number($<HTMLInputElement>('#bd-lessons').value) || 1000;
  const useHistory = $<HTMLInputElement>('#bd-history').checked;
  if (!teacher) {
    toast('Pick a teacher model first.', 'error');
    return;
  }
  const found = await runCheck();
  const failed = (found ?? []).filter((c) => !c.ok && c.id !== 'converter');
  if (!found || failed.length) {
    toast(failed.length ? `Not ready: ${failed.map((c) => c.label).join(', ')}.` : 'The requirements check failed.', 'error');
    return;
  }
  const noConverter = found.some((c) => c.id === 'converter' && !c.ok);
  const text = testRun
    ? `Start a test run? ${teacher} writes 20 lessons, then the student trains for a few steps, and the build is packaged as democritus:4b-test and tested. It takes roughly 20–40 minutes and uses the GPU the whole time.`
    : `Begin a full build with ${lessons.toLocaleString()} lessons from ${teacher}? It runs for many hours (best overnight): the PC is kept awake, training pauses by itself above 83°C, and you can pause, resume or cancel at any time. The default brain does not change.`;
  const go = await choose(text + (noConverter ? ' Note: packaging needs the GGUF converter, which is not installed yet; the build stops before packaging and can resume once it is.' : ''), [
    { value: 'cancel', label: 'Not now' },
    { value: 'go', label: testRun ? 'Start test run' : 'Begin training', kind: 'primary' },
  ]);
  if (go !== 'go') return;
  busy = true;
  try {
    build = await api.buildStart(teacher, testRun, lessons, useHistory);
    checks = null;
  } catch (e) {
    toast(errorText(e), 'error');
  } finally {
    busy = false;
  }
  render();
}

async function act(name: string) {
  try {
    if (name === 'check') await runCheck();
    else if (name === 'test') await begin(true);
    else if (name === 'begin') await begin(false);
    else if (name === 'pause') {
      await api.buildPause();
      toast('Pausing: the current step finishes and a checkpoint is saved.');
    } else if (name === 'resume') await api.buildResume();
    else if (name === 'cancel') {
      const c = await choose('Cancel this build? Lessons and checkpoints stay in its folder, but the build stops for good.', [
        { value: 'keep', label: 'Keep building' },
        { value: 'cancel', label: 'Cancel build', kind: 'danger' },
      ]);
      if (c === 'cancel') await api.buildCancel();
    } else if (name === 'dismiss') {
      await api.buildDismiss();
      build = null;
      await refresh();
    }
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

async function compare() {
  const s = shownBuild();
  if (!s) return;
  try {
    const r = await api.buildReport(s.id);
    $('#compare-title').textContent = `Compare answers · ${s.name} and ${s.teacher}`;
    $('#compare-items').innerHTML = r.items
      .map(
        (i) =>
          `<section class="compare-item"><div class="label">${escapeText(i.category)} · ${escapeText(i.prompt)}</div>` +
          `<div class="compare-cols"><div class="inset-box"><div class="with-action small"><span class="mono">${escapeText(s.name)}</span><span class="strong">${i.studentScore.toFixed(1)}</span></div><p>${escapeText(i.student || '(no answer)')}</p></div>` +
          `<div class="inset-box"><div class="with-action small"><span class="mono">${escapeText(s.teacher)}</span><span class="strong">${i.teacherScore.toFixed(1)}</span></div><p>${escapeText(i.teacher || '(no answer)')}</p></div></div></section>`,
      )
      .join('');
    $<HTMLDialogElement>('#compare-dialog').showModal();
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

async function makeDefault() {
  const s = shownBuild();
  if (!s || s.testRun) return;
  const c = await choose(`Make ${s.name} the default brain for new conversations? You can switch back at any time; ${s.name} stays installed either way.`, [
    { value: 'cancel', label: 'Cancel' },
    { value: 'ok', label: 'Make default', kind: 'primary' },
  ]);
  if (c !== 'ok') return;
  try {
    await api.buildMakeDefault(s.name);
    await deps.onDefaultChanged();
    toast(`${s.name} is now the default brain.`);
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

function showTab(t: 'build' | 'examples') {
  tab = t;
  $('#bd-tabs')
    .querySelectorAll<HTMLButtonElement>('[data-tab]')
    .forEach((b) => b.setAttribute('aria-checked', String(b.dataset.tab === t)));
  $('#bd-build').hidden = t !== 'build';
  $('#bd-examples').hidden = t !== 'examples';
  if (t === 'examples') void loadExamples();
}

export function initBuildPage(d: Deps) {
  deps = d;
  void listen<Build>('build-changed', (e) => {
    build = e.payload;
    if (build.status === 'done' || build.status === 'failed') void refresh();
    else if (currentPage() === 'build') {
      renderStages();
      renderStatus();
      renderStats();
      renderLog();
    } else {
      $('#build-badge').hidden = build.status !== 'running';
    }
  });
  onShow('build', () => {
    void refresh();
    void pollGpu();
    if (tab === 'examples') void loadExamples();
    else void api.trainingExamples().then((x) => {
      examples = x;
      $('#bd-ex-count').textContent = x.length ? String(x.filter((e) => !e.excluded).length) : '';
    });
  });
  onOperatorChange(() => {
    if (currentPage() === 'build') renderStatus();
  });
  window.setInterval(() => {
    if (currentPage() === 'build' && !document.hidden) {
      void pollGpu();
      if (build?.status === 'running') renderStatus(); // "checkpoint saved N min ago"
    }
  }, 3000);
  $('#bd-actions').addEventListener('click', (e) => {
    const b = (e.target as HTMLElement).closest<HTMLButtonElement>('[data-act]');
    if (b && !b.disabled) void act(b.dataset.act!);
  });
  $('#bd-tabs').addEventListener('click', (e) => {
    const b = (e.target as HTMLElement).closest<HTMLButtonElement>('[data-tab]');
    if (b) showTab(b.dataset.tab as 'build' | 'examples');
  });
  $('#bd-ex-items').addEventListener('change', async (e) => {
    const input = e.target as HTMLInputElement;
    const id = Number(input.dataset.id);
    if (!id) return;
    try {
      await api.setExampleExcluded(id, !input.checked);
      const ex = examples.find((x) => x.messageId === id);
      if (ex) ex.excluded = !input.checked;
      renderExamples();
    } catch (err) {
      toast(errorText(err), 'error');
      input.checked = !input.checked;
    }
  });
  $('#bd-export').addEventListener('click', async () => {
    try {
      const m = await api.exportDataset();
      toast(`Exported ${m.rated + m.edited + m.identity} examples to ${m.file}.`);
      void loadExamples();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  });
  $('#bd-compare').addEventListener('click', () => void compare());
  $('#bd-make-default').addEventListener('click', () => void makeDefault());
  // A build may be paused from an earlier session: show the dot at once.
  void api
    .buildOverview()
    .then((o) => {
      overview = o;
      build = o.current;
      $('#build-badge').hidden = build?.status !== 'running';
    })
    .catch(() => {});
}
