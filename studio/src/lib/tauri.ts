// The few things Studio asks of its desktop shell. In a plain browser (Vite dev) each one
// has a harmless fallback, so the whole UI works without Tauri.

import { invoke } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { openUrl, revealItemInDir } from '@tauri-apps/plugin-opener';

export const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/** Starts `dawnroom serve` detached. Resolves with the log file path. */
export function startEngine(port: number): Promise<string> {
  return invoke<string>('start_engine', { port });
}

/** The engine binary the shell would start, or null if it can't find one. */
export function enginePath(): Promise<string | null> {
  return inTauri ? invoke<string | null>('engine_path') : Promise.resolve(null);
}

/** Shows a file or folder in the system file manager. Returns false in a browser. */
export async function reveal(path: string): Promise<boolean> {
  if (!inTauri) return false;
  await revealItemInDir(path);
  return true;
}

/** Lets the user pick a folder. Null if they cancel, or in a browser (Studio shows a text field there). */
export async function pickFolder(title: string): Promise<string | null> {
  if (!inTauri) return null;
  const picked = await openDialog({ directory: true, multiple: false, title });
  return typeof picked === 'string' ? picked : null;
}

/** Lets the user pick a model file (.gguf, .safetensors). Null if they cancel, or in a browser (a text field there). */
export async function pickModelFile(title: string): Promise<string | null> {
  if (!inTauri) return null;
  const picked = await openDialog({ multiple: false, title, filters: [{ name: 'Model files', extensions: ['gguf', 'safetensors'] }] });
  return typeof picked === 'string' ? picked : null;
}

/** Studio's web cache: bytes, and whether clearing it is scheduled for the next start. Null in a browser. */
export async function webCacheInfo(): Promise<{ bytes: number; scheduled: boolean } | null> {
  if (!inTauri) return null;
  const [bytes, scheduled] = await invoke<[number, boolean]>('web_cache_info');
  return { bytes, scheduled };
}

/** Clears Studio's web cache the next time it starts (WebView2 holds it open while running). */
export function clearWebCache(): Promise<void> {
  return invoke('clear_web_cache');
}

export function openExternal(url: string) {
  if (inTauri) void openUrl(url);
  else window.open(url, '_blank', 'noopener');
}

export const appWindow = {
  minimize: () => void getCurrentWindow().minimize(),
  toggleMaximize: () => void getCurrentWindow().toggleMaximize(),
};

/** The tray menu's middle lines: each download and the GPU's state. */
export function setTray(status: { downloads: string[]; gpu: string | null }): Promise<void> {
  return inTauri ? invoke('set_tray', { status }) : Promise.resolve();
}

/** Hides the window to the tray. */
export const hideWindow = () => (inTauri ? invoke<void>('hide_window') : Promise.resolve());

/** Quit Dawnroom: stops the engine and closes Studio. */
export const quitApp = () => (inTauri ? invoke<void>('quit_app') : Promise.resolve());

/** Calls back when the window is asked to close (title bar, Alt+F4, taskbar). Returns an unsubscribe. */
export function onCloseRequested(f: () => void): () => void {
  if (!inTauri) return () => {};
  const off = getCurrentWindow().listen('close-requested', f);
  return () => void off.then((u) => u());
}

/** Start at sign-in. Null in a browser (there's nothing to start). */
export const signInGet = (): Promise<boolean | null> => (inTauri ? invoke<boolean>('sign_in_get') : Promise.resolve(null));
export const signInSet = (on: boolean): Promise<boolean> => invoke<boolean>('sign_in_set', { on });

/** Checks for a newer Dawnroom and downloads it in the background. Resolves with its version once it's ready. */
export const updateCheck = (): Promise<string | null> => (inTauri ? invoke<string | null>('update_check') : Promise.resolve(null));
/** The version downloaded and waiting to be installed, if any. */
export const updateReady = (): Promise<string | null> => (inTauri ? invoke<string | null>('update_ready') : Promise.resolve(null));
/** Installs the waiting update and reopens Studio (the engine stops first). */
export const updateInstall = (): Promise<void> => invoke('update_install');

/**
 * Files dragged onto the desktop window, with their paths (Tauri's own drag and drop; the webview's
 * HTML drop carries no paths). `over` is true while files hover over the window. A no-op in a browser.
 */
export function onDesktopDrop(on: { over?: (over: boolean) => void; drop: (paths: string[]) => void }): () => void {
  if (!inTauri) return () => {};
  const off = getCurrentWebview().onDragDropEvent((e) => {
    const p = e.payload;
    if (p.type === 'enter' || p.type === 'over') on.over?.(true);
    else if (p.type === 'leave') on.over?.(false);
    else if (p.type === 'drop') {
      on.over?.(false);
      on.drop(p.paths);
    }
  });
  return () => void off.then((u) => u());
}
