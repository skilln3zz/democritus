// Runs before the first paint (a classic, blocking script in <head>) so the
// window never flashes the wrong theme. The choice itself lives in the app's
// settings; src/theme.ts mirrors it into localStorage for this script.
(function () {
  var pref = 'system';
  try {
    pref = localStorage.getItem('democritus.theme') || 'system';
  } catch (e) {
    // storage unavailable: follow the system
  }
  if (pref !== 'light' && pref !== 'dark') pref = 'system';
  var dark = pref === 'dark' || (pref === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
  document.documentElement.setAttribute('data-theme', dark ? 'dark' : 'light');
  document.documentElement.setAttribute('data-theme-pref', pref);
})();
