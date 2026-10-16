// Left-rail navigation between pages. The chat page is the sidebar + #main;
// every other page is a <section id="page-NAME">.
import { icons } from './icons';
import { $ } from './ui';

const onShowHandlers = new Map<string, () => void>();
let current = 'chat';

export function onShow(page: string, fn: () => void) {
  onShowHandlers.set(page, fn);
}

export function currentPage(): string {
  return current;
}

export function showPage(name: string) {
  current = name;
  $('#app').dataset.page = name;
  document.querySelectorAll<HTMLElement>('.rail-btn[data-page]').forEach((b) => b.classList.toggle('active', b.dataset.page === name));
  document.querySelectorAll<HTMLElement>('section.page').forEach((s) => (s.hidden = s.id !== `page-${name}`));
  $('#sidebar').hidden = name !== 'chat';
  $('#main').hidden = name !== 'chat';
  onShowHandlers.get(name)?.();
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
