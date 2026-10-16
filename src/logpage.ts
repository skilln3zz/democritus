// The Log page: the local activity log, newest first.
import { api, type ActivityEntry } from './api';
import { currentPage, onShow } from './pages';
import { $, errorText, escapeText, toast } from './ui';

function detailText(detail: unknown): string {
  if (detail == null || (typeof detail === 'object' && Object.keys(detail as object).length === 0)) return '';
  return JSON.stringify(detail);
}

function row(e: ActivityEntry): string {
  const detail = detailText(e.detail);
  return (
    `<tr data-kind="${escapeText(e.kind)}"><td class="nowrap">${escapeText(e.time)}</td>` +
    `<td><span class="kind kind-${escapeText(e.kind)}">${escapeText(e.kind)}</span></td>` +
    `<td>${escapeText(e.action)}</td>` +
    `<td class="detail" title="${escapeText(detail)}">${escapeText(detail)}</td></tr>`
  );
}

export async function refreshLog() {
  try {
    const entries = await api.readActivity(500, $<HTMLSelectElement>('#log-kind').value, $<HTMLInputElement>('#log-search').value);
    $('#log-rows').innerHTML = entries.map(row).join('');
    $('#log-empty').hidden = entries.length > 0;
  } catch (e) {
    toast(`Couldn't read the activity log: ${errorText(e)}`, 'error');
  }
}

export function initLogPage() {
  onShow('log', () => {
    void api.appPaths().then((p) => ($('#log-path').textContent = p.logs));
    void refreshLog();
  });
  $('#log-kind').addEventListener('change', () => void refreshLog());
  $('#log-refresh').addEventListener('click', () => void refreshLog());
  let t: number | undefined;
  $('#log-search').addEventListener('input', () => {
    window.clearTimeout(t);
    t = window.setTimeout(() => void refreshLog(), 250);
  });
  // Keep it live while visible.
  window.setInterval(() => {
    if (currentPage() === 'log' && !document.hidden) void refreshLog();
  }, 4000);
}
