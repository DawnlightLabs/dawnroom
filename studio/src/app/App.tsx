import { useEffect } from 'react';
import { Safelight } from '../components/Moments';
import { CloseNote, Opening, Rail, Splash, TitleBar } from '../components/Shell';
import { loadSignIn, requestClose, startUpdateChecks, trayLines } from '../lib/background';
import { onCloseRequested, setTray } from '../lib/tauri';
import { PRODUCT_NAME } from '../config';
import { Create } from '../pages/Create';
import { EngineDown, EngineOutdated, FirstRun, NoGpu } from '../pages/FirstRun';
import { Library } from '../pages/Library';
import { Models } from '../pages/Models';
import { Settings } from '../pages/Settings';
import { applyMotion, followSystemTheme, loadInfo, retryEngine, startPolling, store, useApp } from './state';

export function App() {
  useEffect(() => {
    document.title = PRODUCT_NAME;
    applyMotion();
    const mq = matchMedia('(prefers-reduced-motion: reduce)');
    mq.addEventListener('change', applyMotion);
    const stopTheme = followSystemTheme();
    const stop = startPolling();
    const stopUpdates = startUpdateChecks();
    const offClose = onCloseRequested(requestClose);
    void loadSignIn();
    // The tray menu follows the store; it's only told when its words change.
    let last = '';
    const syncTray = () => {
      const lines = trayLines(store.get());
      const key = JSON.stringify(lines);
      if (key !== last) void setTray(lines).then(() => (last = key), () => {});
    };
    syncTray();
    const offTray = store.subscribe(syncTray);
    return () => {
      mq.removeEventListener('change', applyMotion);
      stopTheme();
      stop();
      stopUpdates();
      offClose();
      offTray();
    };
  }, []);

  return (
    <div className="flex h-full flex-col">
      <TitleBar />
      <Body />
      <Safelight />
      <CloseNote />
    </div>
  );
}

/** Which screen: the engine first, then the GPU, then first run, then the app itself. */
function Body() {
  const conn = useApp((s) => s.conn);
  const startError = useApp((s) => s.startError);
  const outdated = useApp((s) => s.outdated);
  const loaded = useApp((s) => s.loaded);
  const gpu = useApp((s) => s.gpu);
  const presets = useApp((s) => s.presets);
  const models = useApp((s) => s.models);
  const page = useApp((s) => s.page);
  const libraryRev = useApp((s) => s.libraryRev);

  // Keep the library's size on disk fresh without polling the folder walk.
  useEffect(() => {
    if (conn === 'ok') void loadInfo();
  }, [conn, libraryRev]);

  if (conn === 'down') return <EngineDown error={startError} onRetry={retryEngine} />;
  if (outdated) return <EngineOutdated />;
  if (conn === 'connecting' || !loaded || !presets.length)
    return (
      <Opening>
        <p className="edge m-0 text-muted">Starting the engine…</p>
      </Opening>
    );
  if (!gpu) return <NoGpu onRetry={() => location.reload()} />;

  // First run until any model is installed (Auto's suggestion, or anything else the user added).
  if (!models.some((m) => m.state === 'installed')) {
    // After start-up the engine re-checks files it hasn't hashed yet; don't call that a first run.
    if (models.some((m) => m.state === 'verifying'))
      return (
        <Splash>
          <p className="edge m-0 text-muted">Checking your models…</p>
        </Splash>
      );
    return <FirstRun />;
  }

  return (
    <div className="flex min-h-0 flex-1">
      <Rail />
      {page === 'create' && <Create />}
      {page === 'library' && <Library />}
      {page === 'models' && <Models />}
      {page === 'settings' && <Settings />}
    </div>
  );
}
