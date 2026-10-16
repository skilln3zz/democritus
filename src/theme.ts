// Light / Dark / Follow system. public/theme-boot.js applies the cached choice
// before the first paint; this module keeps it current and in sync with the
// app's settings (the source of truth).

export type ThemePref = 'system' | 'light' | 'dark';

const KEY = 'democritus.theme';
const media = window.matchMedia('(prefers-color-scheme: dark)');
const root = document.documentElement;

function readPref(): ThemePref {
  const p = root.dataset.themePref;
  return p === 'light' || p === 'dark' ? p : 'system';
}

let pref: ThemePref = readPref();
const listeners: ((p: ThemePref) => void)[] = [];

export function themePref(): ThemePref {
  return pref;
}

/** Applies a choice now and caches it for the next start. */
export function applyTheme(next: string) {
  pref = next === 'light' || next === 'dark' ? next : 'system';
  const dark = pref === 'dark' || (pref === 'system' && media.matches);
  root.dataset.theme = dark ? 'dark' : 'light';
  root.dataset.themePref = pref;
  try {
    localStorage.setItem(KEY, pref);
  } catch {
    // not fatal: the setting is still saved in the app's database
  }
  for (const fn of listeners) fn(pref);
}

// Follow Windows live while "Follow system" is chosen.
media.addEventListener('change', () => {
  if (pref === 'system') applyTheme('system');
});

/**
 * Wires a radiogroup of buttons with data-theme-choice. Arrow keys move the
 * choice, as in a native radio group.
 */
export function initThemeSwitch(group: HTMLElement, onChoose: (p: ThemePref) => void) {
  const buttons = [...group.querySelectorAll<HTMLButtonElement>('[data-theme-choice]')];
  const render = (p: ThemePref) =>
    buttons.forEach((b) => {
      const on = b.dataset.themeChoice === p;
      b.setAttribute('aria-checked', String(on));
      b.tabIndex = on ? 0 : -1;
    });
  listeners.push(render);
  render(pref);
  const choose = (b: HTMLButtonElement) => {
    const p = b.dataset.themeChoice as ThemePref;
    if (p !== pref) onChoose(p);
    b.focus();
  };
  buttons.forEach((b, i) => {
    b.addEventListener('click', () => choose(b));
    b.addEventListener('keydown', (e) => {
      const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
      if (!step) return;
      e.preventDefault();
      choose(buttons[(i + step + buttons.length) % buttons.length]);
    });
  });
}
