// Navigation between pages. Every page is a <section id="page-NAME">; any
// button with class rail-btn and data-page="NAME" opens that page.
import { icons } from './icons';
import { $ } from './ui';

const onShowHandlers = new Map<string, (() => void)[]>();
let current = 'chat';

export function onShow(page: string, fn: () => void) {
  onShowHandlers.set(page, [...(onShowHandlers.get(page) ?? []), fn]);
}

export function currentPage(): string {
  return current;
}

export function showPage(name: string) {
  current = name;
  $('#app').dataset.page = name;
  document.querySelectorAll<HTMLElement>('.rail-btn[data-page]').forEach((b) => {
    const on = b.dataset.page === name;
    b.classList.toggle('active', on);
    if (on) b.setAttribute('aria-current', 'page');
    else b.removeAttribute('aria-current');
  });
  document.querySelectorAll<HTMLElement>('section.page').forEach((s) => (s.hidden = s.id !== `page-${name}`));
  for (const fn of onShowHandlers.get(name) ?? []) fn();
}

export function initRail() {
  document.querySelectorAll<HTMLElement>('.rail-icon[data-icon]').forEach((el) => {
    el.innerHTML = (icons as Record<string, string>)[el.dataset.icon ?? ''] ?? '';
  });
  document.querySelectorAll<HTMLButtonElement>('.rail-btn[data-page]').forEach((b) =>
    b.addEventListener('click', () => showPage(b.dataset.page ?? 'chat')),
  );
  $('#app').dataset.page = 'chat';
}
