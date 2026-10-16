// The Memory page: browse/search/edit memory files, review proposed updates,
// history and undo, pause, meaning-search setup and "reset everything learned".
import { listen } from '@tauri-apps/api/event';
import { api, type MemoryFile, type MemoryOverview, type Proposal } from './api';
import { diffLines } from './diff';
import { onOperatorChange } from './operatorpage';
import { currentPage, onShow } from './pages';
import { $, askConfirm, errorText, escapeText, relativeTime, toast } from './ui';

let overview: MemoryOverview | null = null;
let currentPath = '';
let loadedText = '';
const pendingListeners: ((n: number) => void)[] = [];

export function onPendingChange(fn: (n: number) => void) {
  pendingListeners.push(fn);
}

function setBadge(n: number) {
  const badge = $('#memory-badge');
  badge.hidden = n === 0;
  badge.textContent = String(n);
  for (const fn of pendingListeners) fn(n);
}

function dirty(): boolean {
  return currentPath !== '' && $<HTMLTextAreaElement>('#mem-text').value !== loadedText;
}

async function refresh() {
  try {
    overview = await api.memoryOverview();
  } catch (e) {
    toast(`Couldn't open memory: ${errorText(e)}`, 'error');
    return;
  }
  const o = overview;
  $('#mem-path').textContent = o.memoryDir;
  $<HTMLInputElement>('#mem-pause').checked = o.paused;
  const bits = [];
  if (o.lockdown) bits.push('Lockdown: automatic memory writing paused');
  else if (o.paused) bits.push('Memory paused');
  else bits.push(o.autoApply ? 'Updates apply automatically' : 'You review each update');
  if (o.queued) bits.push(`${o.queued} exchange${o.queued === 1 ? '' : 's'} being read`);
  $('#mem-status').textContent = bits.join(' · ');
  setBadge(o.pending);
  renderSemantic(o);
  if (!$<HTMLInputElement>('#mem-search').value.trim()) renderFiles(o.files);
  if (!currentPath && o.files.length) await openFile(o.files.find((f) => f.path === 'current_state.md')?.path ?? o.files[0].path);
  await renderProposals();
}

function renderSemantic(o: MemoryOverview) {
  const el = $('#mem-semantic');
  if (o.semanticInstalled) {
    el.innerHTML = `Meaning search is on (${o.embeddedChunks} of ${o.chunks} sections indexed) · keyword search always on.`;
  } else if (o.semanticFileDownloaded) {
    el.innerHTML = `Keyword search is on. <button id="mem-install" class="btn small" type="button">Turn on meaning search</button> (uses the downloaded Qwen3-Embedding model)`;
  } else {
    el.innerHTML =
      'Keyword search is on. For meaning search, download <code>Qwen3-Embedding-0.6B-Q8_0.gguf</code> from huggingface.co/Qwen on the Operator page.';
  }
}

function fileRow(f: MemoryFile): string {
  return (
    `<button type="button" class="mem-file${f.path === currentPath ? ' active' : ''}" data-path="${escapeText(f.path)}">` +
    `<span class="mem-file-name">${escapeText(f.path)}</span>` +
    `<span class="muted small">${escapeText(f.description || f.title)}</span>` +
    `<span class="muted small">${relativeTime(f.modifiedMs)}</span></button>`
  );
}

function renderFiles(files: MemoryFile[]) {
  const groups: [string, MemoryFile[]][] = [
    ['Core', files.filter((f) => !f.path.includes('/'))],
    ['Projects', files.filter((f) => f.path.startsWith('projects/'))],
    ['Sessions', files.filter((f) => f.path.startsWith('sessions/'))],
  ];
  $('#mem-files').innerHTML = groups
    .filter(([, list]) => list.length)
    .map(([name, list]) => `<div class="mem-group">${name}</div>${list.map(fileRow).join('')}`)
    .join('');
}

async function search(q: string) {
  if (!q.trim()) {
    if (overview) renderFiles(overview.files);
    return;
  }
  try {
    const hits = await api.memorySearch(q);
    $('#mem-files').innerHTML =
      `<div class="mem-group">${hits.length} result${hits.length === 1 ? '' : 's'}</div>` +
      hits
        .map(
          (h) =>
            `<button type="button" class="mem-file mem-hit" data-path="${escapeText(h.path)}"><span class="mem-file-name">${escapeText(h.path)}${h.heading ? ` › ${escapeText(h.heading)}` : ''}</span>` +
            `<span class="muted small">${escapeText(h.text.slice(0, 180))}</span></button>`,
        )
        .join('');
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

async function openFile(path: string) {
  if (dirty() && !(await askConfirm(`Discard your unsaved changes to ${currentPath}?`, 'Discard'))) return;
  try {
    loadedText = await api.memoryRead(path);
  } catch (e) {
    toast(errorText(e), 'error');
    return;
  }
  currentPath = path;
  const f = overview?.files.find((x) => x.path === path);
  $('#mem-file-title').textContent = f?.title ?? path;
  $('#mem-file-path').textContent = path;
  const text = $<HTMLTextAreaElement>('#mem-text');
  text.value = loadedText;
  text.readOnly = path === 'index.md';
  $<HTMLButtonElement>('#mem-save').disabled = path === 'index.md';
  $<HTMLButtonElement>('#mem-delete').disabled = !!f?.core;
  $('#mem-history').hidden = true;
  document.querySelectorAll<HTMLElement>('.mem-file').forEach((b) => b.classList.toggle('active', b.dataset.path === path));
}

async function save() {
  if (!currentPath) return;
  try {
    await api.memoryWrite(currentPath, $<HTMLTextAreaElement>('#mem-text').value);
    loadedText = $<HTMLTextAreaElement>('#mem-text').value;
    toast(`Saved ${currentPath}`);
    await refresh();
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

async function remove() {
  if (!currentPath || !(await askConfirm(`Delete ${currentPath}? It stays in the history and can be restored.`))) return;
  try {
    await api.memoryDelete(currentPath);
    toast(`Deleted ${currentPath}`);
    currentPath = '';
    loadedText = '';
    await refresh();
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

async function showHistory() {
  const box = $('#mem-history');
  if (!box.hidden) {
    box.hidden = true;
    return;
  }
  const snaps = await api.memoryHistory(currentPath).catch(() => []);
  box.innerHTML = snaps.length
    ? `<div class="mem-group">Earlier versions of ${escapeText(currentPath)}</div>` +
      snaps
        .map(
          (s) =>
            `<div class="mem-snap"><span>${escapeText(s.time)}</span><span class="muted small">${s.size} bytes</span>` +
            `<button type="button" class="btn small" data-view="${escapeText(s.id)}">View</button>` +
            `<button type="button" class="btn small" data-restore="${escapeText(s.id)}">Restore</button></div>`,
        )
        .join('')
    : '<div class="muted small">No earlier versions yet.</div>';
  box.hidden = false;
}

// ---------- proposals (review panel) ----------

function diffHtml(before: string, after: string): string {
  return diffLines(before, after)
    .map((l) => `<div class="diff-line diff-${l.kind}">${l.kind === 'add' ? '+ ' : l.kind === 'del' ? '- ' : '  '}${escapeText(l.text)}</div>`)
    .join('');
}

async function proposalHtml(p: Proposal): Promise<string> {
  const previews = await api.memoryPreview(p.id).catch(() => []);
  const ops = p.ops
    .map(
      (op, i) =>
        `<label class="op-row"><input type="checkbox" checked data-op="${i}" /> <code>${escapeText(op.file)}</code> · ${escapeText(op.action.replace('_', ' '))}` +
        `${op.section ? ` · “${escapeText(op.section)}”` : ''}</label>`,
    )
    .join('');
  const rejected = p.rejected.length
    ? `<div class="muted small">Refused by the app: ${p.rejected.map((r) => `${escapeText(r.op.file)} (${escapeText(r.reason)})`).join('; ')}</div>`
    : '';
  const diffs = previews.map((f) => `<div class="diff-file"><div class="diff-name">${escapeText(f.path)}</div>${diffHtml(f.before, f.after)}</div>`).join('');
  return (
    `<div class="proposal" data-id="${p.id}"><div class="proposal-head"><div><strong>Proposed by ${escapeText(p.model)}</strong> ` +
    `<span class="muted small">from “${escapeText(p.conversationTitle || 'a conversation')}” · ${relativeTime(p.createdAt)}</span></div>` +
    `<div class="spacer"></div><button type="button" class="btn small" data-reject="${p.id}">Reject</button>` +
    `<button type="button" class="btn small primary" data-apply="${p.id}">Apply selected</button></div>` +
    `<div class="op-list">${ops}</div>${rejected}<div class="diffs">${diffs}</div></div>`
  );
}

async function renderProposals() {
  const box = $('#mem-proposals');
  const list = await api.memoryProposals('pending').catch(() => []);
  if (!list.length) {
    box.hidden = true;
    box.innerHTML = '';
    return;
  }
  box.innerHTML = `<div class="mem-group">Memory updates waiting for your review (${list.length})</div>` + (await Promise.all(list.map(proposalHtml))).join('');
  box.hidden = false;
}

export function initMemoryPage() {
  onShow('memory', () => void refresh());
  void api.memoryOverview().then((o) => setBadge(o.pending)).catch(() => {});
  void listen<{ pending: number }>('memory-changed', (e) => {
    setBadge(e.payload.pending);
    if (currentPage() === 'memory') void refresh();
  });
  onOperatorChange(() => {
    if (currentPage() === 'memory') void refresh();
  });

  $('#mem-files').addEventListener('click', (e) => {
    const b = (e.target as HTMLElement).closest<HTMLElement>('.mem-file');
    if (b?.dataset.path) void openFile(b.dataset.path);
  });
  let t: number | undefined;
  $('#mem-search').addEventListener('input', () => {
    window.clearTimeout(t);
    t = window.setTimeout(() => void search($<HTMLInputElement>('#mem-search').value), 250);
  });
  $('#mem-save').addEventListener('click', () => void save());
  $('#mem-delete').addEventListener('click', () => void remove());
  $('#mem-history-btn').addEventListener('click', () => void showHistory());
  $('#mem-text').addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      e.preventDefault();
      void save();
    }
  });
  $('#mem-history').addEventListener('click', async (e) => {
    const el = e.target as HTMLElement;
    const view = el.dataset.view;
    const restore = el.dataset.restore;
    if (view) {
      $<HTMLTextAreaElement>('#mem-text').value = await api.memorySnapshot(currentPath, view);
      toast('Showing an earlier version. Save to keep it, or Restore.');
    } else if (restore && (await askConfirm(`Put back the version from ${restore.slice(0, 15)}? The current version is kept in the history.`, 'Restore'))) {
      await api.memoryRestore(currentPath, restore).catch((err) => toast(errorText(err), 'error'));
      const path = currentPath;
      currentPath = '';
      await openFile(path);
      toast('Restored');
    }
  });
  $('#mem-pause').addEventListener('change', async (e) => {
    const on = (e.target as HTMLInputElement).checked;
    const op = await api.getOperator();
    await api.saveOperator({ ...op, memoryPaused: on }).catch((err) => toast(errorText(err), 'error'));
    toast(on ? 'Memory paused' : 'Memory resumed');
    void refresh();
  });
  $('#mem-semantic').addEventListener('click', async (e) => {
    if ((e.target as HTMLElement).id !== 'mem-install') return;
    (e.target as HTMLButtonElement).disabled = true;
    try {
      await api.memoryInstallSemantic();
      toast('Meaning search is on');
    } catch (err) {
      toast(errorText(err), 'error');
    }
    void refresh();
  });
  $('#mem-proposals').addEventListener('click', async (e) => {
    const el = e.target as HTMLElement;
    const apply = el.dataset.apply;
    const reject = el.dataset.reject;
    if (apply) {
      const box = el.closest('.proposal')!;
      const selected = [...box.querySelectorAll<HTMLInputElement>('input[data-op]')].filter((i) => i.checked).map((i) => Number(i.dataset.op));
      try {
        await api.memoryApply(Number(apply), selected);
        toast('Memory updated');
      } catch (err) {
        toast(errorText(err), 'error');
      }
    } else if (reject) {
      await api.memoryReject(Number(reject)).catch((err) => toast(errorText(err), 'error'));
      toast('Update rejected');
    } else return;
    const path = currentPath;
    currentPath = '';
    await refresh();
    if (path) await openFile(path);
  });
  $('#mem-reset').addEventListener('click', async () => {
    const ok = await askConfirm(
      'Reset everything learned? First the whole memory folder is backed up. Then all memory files go back to their starting templates, and the search index, pending updates and project links are cleared. Conversations and settings are kept.',
      'Back up and reset',
    );
    if (!ok) return;
    try {
      const backup = await api.resetLearning();
      toast(`Reset done. Backup: ${backup}`, 'info', 8000);
      currentPath = '';
      await refresh();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  });
}
