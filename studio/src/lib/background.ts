// Studio in the background: the tray's status lines, closing to the tray, start at sign-in and
// updates. Once a day Studio asks the latest release whether there's a newer Dawnroom (the only
// network call Dawnroom makes on its own) and downloads it, signed, in the background. It installs
// on "Restart to update" (never while an image develops or a model downloads) or at the next Quit.

import type { AppState } from '../app/state';
import { pullTotals } from './format';
import { createStore, useStore } from './store';
import { hideWindow, inTauri, signInGet, signInSet, updateCheck, updateInstall, updateReady } from './tauri';

interface Saved {
  /** Update automatically (on unless turned off in Settings). */
  updates: boolean;
  /** When the last check answered (ms). */
  lastCheck: number;
  /** The one-time "Dawnroom keeps running in the tray" line has been shown. */
  trayNoted: boolean;
}

interface Live {
  /** A newer version, downloaded and waiting to install. */
  ready: string | null;
  checking: boolean;
  checkFailed: boolean;
  installing: boolean;
  closeAsk: boolean;
  signIn: boolean | null;
}

const KEY = 'studio.background';
const DAY = 24 * 3600 * 1000;

function load(): Saved {
  const base: Saved = { updates: true, lastCheck: 0, trayNoted: false };
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? '{}');
    return { updates: saved.updates ?? base.updates, lastCheck: saved.lastCheck ?? 0, trayNoted: saved.trayNoted ?? false };
  } catch {
    return base;
  }
}

export const bg = createStore<Saved & Live>({ ...load(), ready: null, checking: false, checkFailed: false, installing: false, closeAsk: false, signIn: null });
export const useBg = <R>(select: (s: ReturnType<typeof bg.get>) => R) => useStore(bg, select);

function save(p: Partial<Saved & Live>) {
  bg.set((s) => ({ ...s, ...p }));
  const { updates, lastCheck, trayNoted } = bg.get();
  try {
    localStorage.setItem(KEY, JSON.stringify({ updates, lastCheck, trayNoted }));
  } catch {
    // private mode: forgotten at the next start
  }
}

// ---------- Updates ----------

/** Checks (and downloads) when a day has passed since the last answer, or now with `force`. */
export async function checkForUpdate(force = false) {
  const s = bg.get();
  if (!inTauri || s.checking || s.ready || (!force && (!s.updates || Date.now() - s.lastCheck < DAY))) return;
  save({ checking: true });
  try {
    const ready = await updateCheck();
    save({ ready, lastCheck: Date.now(), checking: false, checkFailed: false });
  } catch {
    // Offline or GitHub away: nothing in the app; Settings says so, and the next hour tries again.
    save({ checking: false, checkFailed: true });
  }
}

export const setUpdates = (updates: boolean) => (save({ updates }), updates && void checkForUpdate(true));

/** Picks up an update already downloaded this session, then checks now and every hour (each time only if a day has passed). */
export function startUpdateChecks(): () => void {
  void updateReady().then((ready) => ready && save({ ready }), () => {});
  const first = setTimeout(() => void checkForUpdate(), 5000);
  const hourly = setInterval(() => void checkForUpdate(), 3600 * 1000);
  return () => (clearTimeout(first), clearInterval(hourly));
}

/** Why "Restart to update" has to wait, or null when it can go now. */
export function restartBlocker(s: Pick<AppState, 'models' | 'pulls' | 'health' | 'groups'>): string | null {
  const developing = s.health?.busy || s.groups.some((g) => g.prints.some((p) => ['sending', 'queued', 'running'].includes(p.status)));
  if (developing) return 'after this image develops';
  if (s.models.some((m) => m.state === 'pulling' || m.id in s.pulls)) return 'after the download finishes';
  return null;
}

export async function restartToUpdate() {
  save({ installing: true });
  try {
    await updateInstall(); // Studio exits inside (Windows) or restarts (Linux)
  } catch {
    save({ installing: false, ready: null, checkFailed: true });
  }
}

// ---------- Start at sign-in ----------

export async function loadSignIn() {
  save({ signIn: await signInGet() });
}

export async function setSignIn(on: boolean) {
  save({ signIn: await signInSet(on) });
}

// ---------- Closing to the tray ----------

/** The window's close button (and Alt+F4): hide to the tray; the first time, say so once. */
export function requestClose() {
  if (bg.get().trayNoted) void hideWindow();
  else save({ closeAsk: true });
}

/** The answer to the one-time line. */
export function closeNoted(hide: boolean) {
  save({ trayNoted: true, closeAsk: false });
  if (hide) void hideWindow();
}

export const cancelClose = () => save({ closeAsk: false });

// ---------- Tray lines ----------

/** What the tray menu shows: each download in progress and what the GPU is doing. */
export function trayLines(s: Pick<AppState, 'models' | 'pulls' | 'health' | 'conn'>): { downloads: string[]; gpu: string | null } {
  const downloads = s.models
    .filter((m) => m.state === 'pulling' || m.id in s.pulls)
    .map((m) => {
      const p = s.pulls[m.id] ?? m.pull;
      if (p?.stage === 'waiting') return `${m.name} · waiting for the network`;
      if (p?.stage === 'testing') return `${m.name} · test image`;
      return `${m.name} · ${Math.round(pullTotals(m, p).fraction * 100)}%`;
    });
  if (s.conn !== 'ok' || !s.health) return { downloads, gpu: null };
  const loaded = s.models.find((m) => m.id === s.health?.model);
  const gpu = s.health.busy ? 'Developing…' : loaded ? `${loaded.name} loaded` : 'GPU idle';
  return { downloads, gpu };
}
