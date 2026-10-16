// The Security check on the Settings & safety page. The backend reads the
// state; a fix only runs after the operator has seen its exact commands and
// pressed "Run commands". The window sends just the fix id: the backend
// rebuilds the commands itself.
import { api, type SecurityFix, type SecurityItem } from './api';
import { toggleLockdown } from './operatorpage';
import { onShow } from './pages';
import { $, errorText, escapeText, toast } from './ui';

const STATUS_TEXT: Record<SecurityItem['status'], string> = {
  ok: 'OK',
  bad: 'Needs attention',
  unknown: 'Unknown',
  info: 'Info',
};

let items: SecurityItem[] = [];
let running = false;
let checkedOnce = false;

function row(item: SecurityItem, i: number): string {
  const fix = item.fix ? `<button type="button" class="btn small ${item.status === 'bad' ? 'hex accent' : ''}" data-fix="${escapeText(item.fix.id)}">${escapeText(item.fix.label)}</button>` : '';
  return (
    `<li class="sec-row" data-status="${item.status}" data-id="${escapeText(item.id)}">` +
    `<span class="mono muted sec-num">${String(i + 1).padStart(2, '0')}</span>` +
    `<span class="sec-dot" aria-hidden="true"></span>` +
    `<div class="sec-text"><div class="sec-title">${escapeText(item.title)} <span class="sec-state">${STATUS_TEXT[item.status]}</span></div>` +
    `<div class="small muted">${escapeText(item.detail)}</div></div>${fix}</li>`
  );
}

async function run() {
  if (running) return;
  running = true;
  const btn = $<HTMLButtonElement>('#sec-run');
  btn.disabled = true;
  btn.textContent = 'Checking…';
  const list = $('#sec-list');
  if (!items.length) list.innerHTML = '<li class="small muted">Checking Ollama, the firewall and model files…</li>';
  try {
    items = await api.securityCheck();
    list.innerHTML = items.map(row).join('');
  } catch (e) {
    list.innerHTML = `<li class="form-error">${escapeText(errorText(e))}</li>`;
  } finally {
    running = false;
    checkedOnce = true;
    btn.disabled = false;
    btn.textContent = 'Check again';
  }
}

/** Shows the exact commands; resolves true only on "Run commands". */
function confirmFix(fix: SecurityFix): Promise<boolean> {
  const dialog = $<HTMLDialogElement>('#fix-dialog');
  $('#fix-title').textContent = `${fix.label}?`;
  $('#fix-what').textContent = fix.admin
    ? 'Democritus will run these commands in PowerShell with administrator rights. Windows will ask you to allow that.'
    : 'Democritus will run these commands in PowerShell for your user account.';
  $('#fix-commands').textContent = fix.commands.join('\n');
  $('#fix-note').textContent = fix.note;
  $('#fix-note').hidden = !fix.note;
  dialog.returnValue = '';
  dialog.showModal();
  return new Promise((resolve) => dialog.addEventListener('close', () => resolve(dialog.returnValue === 'run'), { once: true }));
}

async function applyFix(id: string, button: HTMLButtonElement) {
  const fix = items.find((i) => i.fix?.id === id)?.fix;
  if (!fix) return;
  if (fix.id === 'lockdown') {
    await toggleLockdown();
    await run();
    return;
  }
  if (!(await confirmFix(fix))) return;
  button.disabled = true;
  button.textContent = 'Running…';
  try {
    await api.securityFix(fix.id);
    toast(`Done: ${fix.label}`);
  } catch (e) {
    toast(errorText(e), 'error', 8000);
  }
  await run();
}

export function initSecurityCheck() {
  $('#sec-run').addEventListener('click', () => void run());
  $('#sec-list').addEventListener('click', (e) => {
    const b = (e.target as HTMLElement).closest<HTMLButtonElement>('[data-fix]');
    if (b?.dataset.fix) void applyFix(b.dataset.fix, b);
  });
  // The firewall query takes a few seconds, so run it once per session
  // automatically; "Check again" refreshes.
  onShow('operator', () => {
    if (!checkedOnce) void run();
  });
}
