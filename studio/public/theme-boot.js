// Sets the theme before the first paint, so a Light window never flashes ink. Same rule as applyTheme()
// in src/app/state.ts: the saved choice, else the OS.
(function () {
  var t = 'system';
  try {
    t = JSON.parse(localStorage.getItem('studio.prefs') || '{}').theme || 'system';
  } catch (e) {}
  var light = t === 'light' || (t === 'system' && matchMedia('(prefers-color-scheme: light)').matches);
  document.documentElement.dataset.theme = light ? 'light' : 'dark';
  document.documentElement.style.colorScheme = light ? 'light' : 'dark';
})();
