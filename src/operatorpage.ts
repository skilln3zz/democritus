// The Operator page: permissions (all off by default), Lockdown, the download
// allowlist and the two-step approval flow for downloads.
import { api, type Approval, type OperatorSettings } from './api';
import { icons } from './icons';
import { currentPage, onShow } from './pages';
import { $, askConfirm, errorText, escapeText, formatBytes, toast } from './ui';

let operator: OperatorSettings | null = null;
const listeners: ((o: OperatorSettings) => void)[] = [];

export function getOperator(): OperatorSettings | null {
  return operator;
}

export function onOperatorChange(fn: (o: OperatorSettings) => void) {
  listeners.push(fn);
  if (operator) fn(operator);
}

function emit() {
  if (operator) for (const fn of listeners) fn(operator);
}

export async function loadOperator() {
  operator = await api.getOperator();
  emit();
}

async function update(patch: Partial<OperatorSettings>) {
  if (!operator) return;
  try {
    operator = await api.saveOperator({ ...operator, ...patch });
    emit();
  } catch (e) {
    toast(errorText(e), 'error');
    emit(); // re-render to the real state
  }
}

interface Toggle {
  key: keyof OperatorSettings;
  label: string;
  desc: string;
  disabled?: string;
}

const TOGGLES: Toggle[] = [
  { key: 'lockdown', label: 'Lockdown', desc: 'Block all internet access and pause all learning and memory writing. Chatting with the local model still works.' },
  { key: 'allowDownloads', label: 'Allow model downloads', desc: 'Lets you download model files from the allowlist below. Every download still needs your approval twice.' },
  { key: 'autoApplyMemory', label: 'Auto-apply memory updates', desc: "Apply Democritus's proposed memory changes without asking. When off, you review each change first." },
  { key: 'memoryPaused', label: 'Pause memory', desc: 'Stop proposing and writing memory updates. Existing memory is still used.' },
  { key: 'allowAutoModelSelect', label: 'Automatic brain choice', desc: 'Let Democritus switch to the model that has worked best for a kind of task. Memory and conversation stay the same.' },
  {
    key: 'allowTraining',
    label: 'Allow fine-tuning',
    desc: 'Allow local training runs.',
    disabled: 'Not available yet: training needs PyTorch, Unsloth and Qwen base weights, which you have not approved for download.',
  },
];

function renderToggles() {
  if (!operator) return;
  $('#op-toggles').innerHTML = TOGGLES.map((t, i) => {
    const on = Boolean(operator![t.key]);
    const id = `op-t-${t.key}`;
    return (
      `<div class="toggle-row${t.disabled ? ' disabled' : ''}"><span class="mono muted toggle-num">${String(i + 1).padStart(2, '0')}</span>` +
      `<label class="toggle-text" for="${id}"><span class="toggle-label">${escapeText(t.label)}</span>` +
      `<span class="muted small">${escapeText(t.desc)}${t.disabled ? `<br><em>${escapeText(t.disabled)}</em>` : ''}</span></label>` +
      `<input id="${id}" type="checkbox" role="switch" class="switch" data-key="${t.key}" ${on ? 'checked' : ''} ${t.disabled ? 'disabled' : ''} /></div>`
    );
  }).join('');
  $<HTMLTextAreaElement>('#op-allowlist').value = operator.downloadAllowlist.join('\n');
  $('#op-allow-chips').innerHTML =
    operator.downloadAllowlist.map((u) => `<span class="chip-ink mono">${escapeText(u.replace(/^https:\/\//, '').replace(/\/$/, ''))}</span>`).join('') +
    '<span class="chip-outline mono">.gguf · .safetensors</span><span class="chip-refused mono">.bin .pt .pkl</span>';
  const canDownload = operator.allowDownloads && !operator.lockdown;
  $<HTMLButtonElement>('#op-download-btn').disabled = !canDownload;
  $<HTMLInputElement>('#op-download-url').disabled = !canDownload;
  $('#op-download-hint').textContent = operator.lockdown
    ? 'Lockdown is on, so downloads are blocked.'
    : operator.allowDownloads
      ? ''
      : 'Turn on "Allow model downloads" above to use this.';
}

function renderLockdownButton(o: OperatorSettings) {
  const btn = $<HTMLButtonElement>('#lockdown-btn');
  btn.classList.toggle('on', o.lockdown);
  btn.setAttribute('aria-pressed', String(o.lockdown));
  btn.querySelector('.rail-icon')!.innerHTML = o.lockdown ? icons.lock : icons.unlock;
  btn.querySelector('.lockdown-label')!.textContent = o.lockdown ? 'Locked' : 'Lockdown';
  btn.title = o.lockdown ? 'Lockdown is ON. Click to turn it off.' : 'Lockdown: block all internet access and pause learning';
  $('#lockdown-banner').hidden = !o.lockdown;
  // The Settings & safety band inverts while locked.
  $('#op-band').classList.toggle('locked', o.lockdown);
  $('#op-band-title').textContent = o.lockdown ? 'LOCKED' : 'LOCKDOWN';
  $('#op-band-text').textContent = o.lockdown
    ? 'Internet blocked, learning paused. Chat still works.'
    : 'One switch blocks all internet access and pauses learning.';
  const big = $<HTMLButtonElement>('#op-lock-btn');
  big.textContent = o.lockdown ? 'LIFT LOCKDOWN' : 'LOCK DOWN';
  big.setAttribute('aria-pressed', String(o.lockdown));
}

/** The newest activity-log lines, beside the permissions. */
async function renderLogPreview() {
  const box = $('#op-log-preview');
  try {
    const entries = await api.readActivity(8, '', '');
    box.innerHTML = entries.length
      ? entries
          .map((e) => {
            const alert = e.kind === 'security' && /refus|block|denied|fail|lockdown on/i.test(e.action);
            return `<li class="${alert ? 'alert' : ''}"><span class="mono">${escapeText(e.time.slice(-8, -3) || e.time)}</span><span>${escapeText(e.action)}</span></li>`;
          })
          .join('')
      : '<li class="muted">Nothing logged yet.</li>';
  } catch (e) {
    box.innerHTML = `<li class="muted">${escapeText(errorText(e))}</li>`;
  }
}

export async function toggleLockdown() {
  if (!operator) return;
  if (!operator.lockdown) {
    await update({ lockdown: true });
    toast('Lockdown on: internet blocked, learning and memory writing paused.');
  } else if (await askConfirm('Turn Lockdown off? Internet access stays limited to downloads you approve, and learning resumes.', 'Turn off')) {
    await update({ lockdown: false });
    toast('Lockdown off.');
  }
}

/** Shows exactly what will happen; resolves true only on an explicit Approve. */
function askApproval(a: Approval): Promise<boolean> {
  const dialog = $<HTMLDialogElement>('#approval-dialog');
  $('#approval-what').textContent =
    a.kind === 'lookup'
      ? 'Democritus will send ONE request to look up the size and checksum of this file. Nothing is downloaded yet.'
      : 'Democritus will download this file and verify its checksum.';
  const rows: [string, string][] = [
    ['Address', a.url],
    ['File', a.fileName],
    ['Size', a.sizeBytes != null ? `${formatBytes(a.sizeBytes)} (${a.sizeBytes.toLocaleString()} bytes)` : a.kind === 'lookup' ? 'Unknown until looked up' : 'Not reported by the server'],
  ];
  if (a.kind === 'download') {
    rows.push(['Published SHA-256', a.sha256 ?? 'None published (the file will be checked for type, but not against a checksum)']);
    if (a.servedFrom) rows.push(['Served from', a.servedFrom]);
  }
  $('#approval-details').innerHTML = rows.map(([k, v]) => `<dt>${escapeText(k)}</dt><dd>${escapeText(v)}</dd>`).join('');
  dialog.returnValue = '';
  dialog.showModal();
  return new Promise((resolve) => dialog.addEventListener('close', () => resolve(dialog.returnValue === 'approve'), { once: true }));
}

async function startDownload() {
  const status = $('#op-download-status');
  const url = $<HTMLInputElement>('#op-download-url').value.trim();
  if (!url) return;
  status.hidden = false;
  status.className = 'small';
  let approval: Approval;
  try {
    approval = await api.requestDownload(url);
  } catch (e) {
    status.className = 'small form-error';
    status.textContent = errorText(e);
    return;
  }
  for (;;) {
    const ok = await askApproval(approval);
    status.className = 'small';
    status.textContent = ok ? (approval.kind === 'lookup' ? 'Looking up…' : 'Downloading…') : 'Denied. Nothing was sent.';
    try {
      const outcome = await api.respondApproval(approval.id, ok, (p) => {
        const pct = p.total ? ` (${Math.floor((p.done / p.total) * 100)}%)` : '';
        status.textContent = `Downloading… ${formatBytes(p.done)}${p.total ? ` of ${formatBytes(p.total)}` : ''}${pct}`;
      });
      if (outcome.type === 'denied') return;
      if (outcome.type === 'needsApproval') {
        approval = outcome.approval;
        continue;
      }
      const r = outcome.result;
      const check =
        r.checksumMatches === true ? '✓ Checksum matches the published SHA-256.' : r.checksumMatches === false ? '✗ Checksum mismatch.' : 'No published checksum to compare.';
      status.className = 'small ok-box';
      status.innerHTML = `Downloaded <code>${escapeText(r.path)}</code> (${formatBytes(r.sizeBytes)}).<br>SHA-256 <code>${escapeText(r.sha256)}</code><br>${escapeText(check)}`;
      return;
    } catch (e) {
      status.className = 'small form-error';
      status.textContent = errorText(e);
      return;
    }
  }
}

export function initOperatorPage() {
  onOperatorChange(renderToggles);
  onOperatorChange(renderLockdownButton);
  onOperatorChange(() => {
    if (currentPage() === 'operator') void renderLogPreview();
  });
  onShow('operator', () => {
    void loadOperator();
    void renderLogPreview();
    void api.appPaths().then((p) => ($('#op-downloads-path').textContent = p.downloads));
  });
  window.setInterval(() => {
    if (currentPage() === 'operator' && !document.hidden) void renderLogPreview();
  }, 5000);
  $('#op-toggles').addEventListener('change', (e) => {
    const input = e.target as HTMLInputElement;
    const key = input.dataset.key as keyof OperatorSettings | undefined;
    if (!key) return;
    if (key === 'lockdown') {
      input.checked = !input.checked; // let toggleLockdown decide (it may ask first)
      void toggleLockdown();
      return;
    }
    void update({ [key]: input.checked } as Partial<OperatorSettings>);
  });
  $('#op-allowlist-save').addEventListener('click', () => {
    const list = $<HTMLTextAreaElement>('#op-allowlist').value.split('\n').map((s) => s.trim()).filter(Boolean);
    void update({ downloadAllowlist: list }).then(() => toast('Allowlist saved'));
  });
  $('#op-download-btn').addEventListener('click', () => void startDownload());
  $('#lockdown-btn').addEventListener('click', () => void toggleLockdown());
  $('#op-lock-btn').addEventListener('click', () => void toggleLockdown());
}
