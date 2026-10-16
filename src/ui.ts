// Small UI helpers shared by every page.

export function $<T extends HTMLElement = HTMLElement>(selector: string): T {
  const el = document.querySelector<T>(selector);
  if (!el) throw new Error(`Missing element ${selector}`);
  return el;
}

export const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export function errorText(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}

export function newRequestId(): string {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

/** File sizes in decimal units, matching `ollama list` and Hugging Face. */
export function formatBytes(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)} MB`;
  if (n >= 1e3) return `${Math.round(n / 1e3)} KB`;
  return `${n} B`;
}

/** RAM/VRAM amounts in binary GB, matching Task Manager ("32 GB RAM", "12 GB VRAM"). */
export function formatMem(n: number): string {
  const gib = n / 1024 ** 3;
  return gib >= 1 ? `${gib.toFixed(1)} GB` : `${Math.round(n / 1024 ** 2)} MB`;
}

export function relativeTime(ms: number): string {
  const d = new Date(ms);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  if (now.getTime() - ms < 6 * 86_400_000) return d.toLocaleDateString([], { weekday: 'short' });
  return d.toLocaleDateString([], { month: 'short', day: 'numeric' });
}

export function escapeText(s: string): string {
  const d = document.createElement('div');
  d.textContent = s;
  return d.innerHTML;
}

export function toast(text: string, kind: 'info' | 'error' = 'info', ms = 4500) {
  const t = document.createElement('div');
  t.className = `toast ${kind}`;
  t.textContent = text;
  $('#toasts').append(t);
  setTimeout(() => t.classList.add('leaving'), ms);
  setTimeout(() => t.remove(), ms + 300);
}

export interface Choice {
  value: string;
  label: string;
  kind?: 'primary' | 'danger';
}

/** Modal with custom buttons; resolves to the chosen value ('cancel' on Esc). */
export function choose(text: string, choices: Choice[]): Promise<string> {
  const dialog = $<HTMLDialogElement>('#confirm-dialog');
  $('#confirm-text').textContent = text;
  $('#confirm-actions').replaceChildren(
    ...choices.map((c) => {
      const b = document.createElement('button');
      b.type = 'submit';
      b.value = c.value;
      b.className = `btn ${c.kind ?? ''}`;
      b.textContent = c.label;
      return b;
    }),
  );
  dialog.returnValue = '';
  dialog.showModal();
  return new Promise((resolve) => dialog.addEventListener('close', () => resolve(dialog.returnValue || 'cancel'), { once: true }));
}

export async function askConfirm(text: string, action = 'Delete'): Promise<boolean> {
  const c = await choose(text, [
    { value: 'cancel', label: 'Cancel' },
    { value: 'ok', label: action, kind: 'danger' },
  ]);
  return c === 'ok';
}
