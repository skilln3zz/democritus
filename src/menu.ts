// The burger menu (design-neumorphic/NEU-SPEC.md, "Burger menu"). One menu,
// moved into the header of whichever page is showing. It holds the page links
// that used to sit at the bottom of the sidebar, the Lockdown switch and the
// "In memory" readout. Every row calls what its old button called: the links
// open the same pages (class rail-btn, see pages.ts), the switch is wired to
// toggleLockdown in operatorpage.ts, and the readout is drawn by main.ts.
import { onShow } from './pages';
import { $ } from './ui';

/** Pages with a header and a .menu-slot in it. */
const PAGES = ['chat', 'code', 'build', 'memory', 'log', 'operator'];

const btn = () => $<HTMLButtonElement>('#menu-btn');
const menu = () => $('#app-menu');
const openListeners: (() => void)[] = [];

/** Runs whenever the menu opens (main.ts closes the Brain picker and refreshes the readout). */
export function onMenuOpen(fn: () => void) {
  openListeners.push(fn);
}

export function menuOpen(): boolean {
  return !menu().hidden;
}

function items(): HTMLElement[] {
  return [...menu().querySelectorAll<HTMLElement>('[role=menuitem], [role=menuitemcheckbox], .menu-mem button')].filter(
    (el) => !(el as HTMLButtonElement).disabled && el.offsetParent !== null,
  );
}

/** Opens the menu; from the keyboard, focus moves to the first row. */
export function openMenu(fromKeyboard = false) {
  if (menuOpen()) return;
  menu().hidden = false;
  btn().setAttribute('aria-expanded', 'true');
  btn().classList.add('open');
  for (const fn of openListeners) fn();
  if (fromKeyboard) items()[0]?.focus({ preventScroll: true });
}

export function closeMenu(focusButton = false) {
  if (!menuOpen()) return;
  menu().hidden = true;
  btn().setAttribute('aria-expanded', 'false');
  btn().classList.remove('open');
  if (focusButton) btn().focus();
}

/** The accent dot on the button: something inside has a badge. */
function renderDot() {
  const memory = $('#memory-badge');
  const build = $('#build-badge');
  $('#menu-dot').hidden = memory.hidden && build.hidden;
  const n = Number(memory.textContent) || 0;
  $('#memory-note').textContent = memory.hidden
    ? 'Your notes and what Democritus remembers'
    : `${n} update${n === 1 ? '' : 's'} waiting for your review`;
}

/** Moves the menu into the header of the page now showing. */
function place(page: string) {
  closeMenu();
  const slot = document.querySelector(`#page-${page} .menu-slot`);
  const tools = $('#header-tools');
  if (slot && tools.parentElement !== slot) slot.append(tools);
}

export function initMenu() {
  for (const page of PAGES) onShow(page, () => place(page));
  // detail is 0 when the button was pressed with Enter or Space.
  btn().addEventListener('click', (e) => (menuOpen() ? closeMenu(true) : openMenu(e.detail === 0)));
  btn().addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown' && !menuOpen()) {
      e.preventDefault();
      openMenu(true);
    }
  });
  // Choosing a page or the settings dialog closes the menu; the switch and Unload keep it open.
  menu().addEventListener('click', (e) => {
    if ((e.target as HTMLElement).closest('[role=menuitem]')) closeMenu();
  });
  menu().addEventListener('keydown', (e) => {
    const list = items();
    const i = list.indexOf(document.activeElement as HTMLElement);
    const go = (n: number) => {
      e.preventDefault();
      list[(n + list.length) % list.length]?.focus();
    };
    if (e.key === 'ArrowDown') go(i + 1);
    else if (e.key === 'ArrowUp') go(i < 0 ? list.length - 1 : i - 1);
    else if (e.key === 'Home') go(0);
    else if (e.key === 'End') go(list.length - 1);
    else if (e.key === 'Tab') closeMenu();
  });
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && menuOpen() && !document.querySelector('dialog[open]')) {
      e.preventDefault();
      e.stopImmediatePropagation(); // Escape closes the menu, not a running reply
      closeMenu(true);
    }
  }, true);
  document.addEventListener('pointerdown', (e) => {
    if (menuOpen() && !(e.target as HTMLElement).closest('.menu-picker')) closeMenu();
  });
  const watch = new MutationObserver(renderDot);
  for (const id of ['#memory-badge', '#build-badge']) watch.observe($(id), { attributes: true, attributeFilter: ['hidden'], childList: true, characterData: true, subtree: true });
  renderDot();
}
