// Studio's state and the actions that change it. The engine owns all logic; this only mirrors
// what it reports and remembers what the user asked for in this session.

import { createClient, EngineError, isAbort, UNREACHABLE } from '../api/client';
import type { Aspect, AutoAnswer, DiskSpace, Gpu, Hardware, Health, Info, Job, JobEvent, JobRequest, Model, ModelList, Paused, Preset, Progress, PullProgress, Task } from '../api/types';
import { ENGINE_PORT, ENGINE_URL } from '../config';
import { aspectFor } from '../lib/format';
import { autoFor, chosenModel } from '../lib/models';
import { createStore, useStore } from '../lib/store';
import { inTauri, startEngine } from '../lib/tauri';

export const api = createClient(ENGINE_URL);

export type Page = 'create' | 'library' | 'models' | 'settings';
export type PrintStatus = 'sending' | 'waiting' | 'queued' | 'running' | 'completed' | 'failed' | 'cancelled';

/** One image in the Create stream, from request to finished print. */
export interface Print {
  key: string;
  request: JobRequest;
  status: PrintStatus;
  jobId: string | null;
  job: Job | null;
  progress: Progress | null;
  /** Latest step preview as a data URL. */
  preview: string | null;
  error: string | null;
  errorCode: string | null;
  /** When it started running here (for the elapsed counter). */
  runningSince: number | null;
  /** Stop was pressed; the engine stops it at its next checkpoint (longer while a model is loading). */
  stopping: boolean;
}

/** An image going into an edit: the one to edit, or a reference. Stored in the engine by sha256. */
export interface InputImage {
  key: string;
  /** Set once the engine has stored it. */
  sha256: string | null;
  width: number;
  height: number;
  /** What to show: an object URL, or the Library thumbnail. */
  src: string;
  name: string;
  /** The Library image it is, if it is one ("edited from"). */
  imageId: string | null;
  status: 'loading' | 'ready' | 'failed';
}

/** One Develop press: a prompt and its prints. */
export interface Group {
  id: string;
  task: Task;
  prompt: string;
  /** "Auto · Z-Image Turbo · 1:1" */
  summary: string;
  /** Edits: the image that was edited, then the references. */
  inputs: InputImage[];
  width: number;
  height: number;
  /** Set while the group waits for this model to finish downloading. */
  waitingFor: string | null;
  prints: Print[];
  createdAt: number;
}

export interface Draft {
  task: Task;
  prompt: string;
  /** Generate's shape. */
  aspect: string;
  /** Edit's shape: `original` keeps the input's. */
  editAspect: string;
  count: number;
  /** The chosen model per tab; null = Auto. */
  models: Record<Task, string | null>;
  /** Edit: the image to edit and up to 9 references. */
  image: InputImage | null;
  refs: InputImage[];
  /** More. Empty strings mean "use the default". */
  seed: string;
  seedLocked: boolean;
  steps: string;
  cfg: string;
  negative: string;
  customSize: boolean;
  width: string;
  height: string;
}

export interface Notice {
  text: string;
  code: string;
}

export interface AppState {
  conn: 'connecting' | 'ok' | 'down';
  /** Why the shell couldn't start the engine, if it tried and failed. */
  startError: string | null;
  /** The engine answers, but it's a build from before this Studio (it lacks endpoints Studio needs). */
  outdated: boolean;
  health: Health | null;
  gpu: Gpu | null;
  /** This machine as the engine sees it: GPUs, the runtime it picked, driver advice. */
  hardware: Hardware | null;
  disk: DiskSpace | null;
  runtimeNeeded: number;
  /** The image runtime's paused download, if any (GET /api/models `runtime_paused`). */
  runtimePaused: Paused | null;
  models: Model[];
  /** One row per model with its size tags and the default tag for this machine. */
  families: ModelList['families'];
  /** False until the first model list arrives. */
  loaded: boolean;
  presets: Preset[];
  aspects: Aspect[];
  /** Auto's pick and reason per task (GET /api/auto); null on an engine without Auto. */
  auto: AutoAnswer | null;
  info: Info | null;
  /** Live progress of pulls this window is attached to. */
  pulls: Record<string, PullProgress | null>;
  /** Models to pull after the current one. */
  pullQueue: string[];
  groups: Group[];
  draft: Draft;
  notice: Notice | null;
  page: Page;
  /** The image open in the Library's full view. */
  viewing: string | null;
  /** The Advanced panel (seed, steps, guidance, negative prompt, size) is open. */
  advancedOpen: boolean;
  /** Bumped to move focus to the prompt. */
  focusPrompt: number;
  /** Bumped when the library's contents change. */
  libraryRev: number;
  motion: 'system' | 'reduced';
  theme: Theme;
}

export type Theme = 'system' | 'dark' | 'light';

const PREFS = 'studio.prefs';
const prefs: Partial<Pick<AppState, 'motion' | 'theme'> & Pick<Draft, 'aspect' | 'count' | 'task'>> = (() => {
  try {
    return JSON.parse(localStorage.getItem(PREFS) ?? '{}');
  } catch {
    return {};
  }
})();

export const emptyAdvanced = { seed: '', seedLocked: false, steps: '', cfg: '', negative: '', customSize: false, width: '1024', height: '1024' };

export const store = createStore<AppState>({
  conn: 'connecting',
  startError: null,
  outdated: false,
  health: null,
  gpu: null,
  hardware: null,
  disk: null,
  runtimeNeeded: 0,
  runtimePaused: null,
  models: [],
  families: [],
  loaded: false,
  presets: [],
  aspects: [],
  auto: null,
  info: null,
  pulls: {},
  pullQueue: [],
  groups: [],
  draft: { task: prefs.task ?? 'generate', prompt: '', aspect: prefs.aspect ?? '1:1', editAspect: 'original', count: prefs.count ?? 1, models: { generate: null, edit: null }, image: null, refs: [], ...emptyAdvanced },
  notice: null,
  page: 'create',
  viewing: null,
  advancedOpen: false,
  focusPrompt: 0,
  libraryRev: 0,
  motion: prefs.motion ?? 'system',
  theme: prefs.theme ?? 'system',
});

export const useApp = <R>(select: (s: AppState) => R) => useStore(store, select);

const patch = (p: Partial<AppState>) => store.set((s) => ({ ...s, ...p }));

function savePrefs() {
  const { motion, theme, draft } = store.get();
  try {
    localStorage.setItem(PREFS, JSON.stringify({ motion, theme, task: draft.task, aspect: draft.aspect, count: draft.count }));
  } catch {
    // private mode: preferences just don't persist
  }
}

// ---------- Navigation and small setters ----------

export const go = (page: Page) => patch({ page, viewing: null });
export const view = (id: string | null) => patch({ page: 'library', viewing: id });
export const dismissNotice = () => patch({ notice: null });
export const toggleAdvanced = () => store.set((s) => ({ ...s, advancedOpen: !s.advancedOpen }));

export function setDraft(d: Partial<Draft>) {
  store.set((s) => ({ ...s, draft: { ...s.draft, ...d } }));
  if ('task' in d || 'aspect' in d || 'count' in d) savePrefs();
}

export function fail(e: unknown) {
  if (isAbort(e)) return;
  const err = e instanceof EngineError ? e : new EngineError(String(e), 'unknown', 0);
  patch({ notice: { text: err.message, code: err.code } });
}

// ---------- Motion ----------

const reducedQuery = typeof matchMedia === 'undefined' ? null : matchMedia('(prefers-reduced-motion: reduce)');

/** Every animation in Studio hangs off `<html data-motion>`: the system setting, or the override in Settings. */
export function applyMotion() {
  const off = store.get().motion === 'reduced' || !!reducedQuery?.matches;
  document.documentElement.dataset.motion = off ? 'off' : 'on';
}

export function setMotion(motion: AppState['motion']) {
  patch({ motion });
  savePrefs();
  applyMotion();
}

// ---------- Theme ----------

const darkQuery = typeof matchMedia === 'undefined' ? null : matchMedia('(prefers-color-scheme: dark)');

/** `<html data-theme>` is always the resolved theme; System follows the OS live. */
export function applyTheme() {
  const t = store.get().theme;
  const resolved = t === 'system' ? (darkQuery?.matches === false ? 'light' : 'dark') : t;
  document.documentElement.dataset.theme = resolved;
  document.documentElement.style.colorScheme = resolved;
}

export function setTheme(theme: Theme) {
  patch({ theme });
  savePrefs();
  applyTheme();
}

/** Re-applies the theme when the OS switches (System). Returns the unsubscribe. */
export function followSystemTheme(): () => void {
  applyTheme();
  darkQuery?.addEventListener('change', applyTheme);
  return () => darkQuery?.removeEventListener('change', applyTheme);
}

// ---------- Engine polling ----------

/** Dev only: `?simulate=no-gpu` shows the no-GPU screen on a machine that has one. */
const SIMULATE_NO_GPU = import.meta.env.DEV && new URLSearchParams(location.search).get('simulate') === 'no-gpu';
/** Dev only: `?simulate=slow-start` holds the first health check for 3 s, as a cold engine start would (shows the opening). */
const SIMULATE_SLOW_START = import.meta.env.DEV && new URLSearchParams(location.search).get('simulate') === 'slow-start';

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
let triedStart = false;

export async function refresh() {
  try {
    const [health, list] = await Promise.all([api.health(), api.models()]);
    const first = store.get().conn !== 'ok';
    patch({ conn: 'ok', startError: null, health, gpu: SIMULATE_NO_GPU ? null : list.gpu, disk: list.disk ?? null, runtimeNeeded: list.runtime_needed_bytes ?? 0, runtimePaused: list.runtime_paused ?? null, models: list.models, families: list.families ?? [], loaded: true });
    if (first || !store.get().presets.length) await loadPresets();
    // Detected once by the engine; an older engine without /api/hardware just leaves it null.
    if (first || !store.get().hardware) api.hardware().then((hardware) => patch({ hardware }), () => {});
    // Auto's pick changes with what's installed and loaded; an engine without Auto leaves it null.
    api.auto().then((auto) => patch({ auto }), () => patch({ auto: null }));
    afterModels();
  } catch (e) {
    if (!(e instanceof EngineError) || e.code !== UNREACHABLE) return;
    patch({ conn: store.get().conn === 'connecting' ? 'connecting' : 'down' });
    if (inTauri && !triedStart) {
      triedStart = true;
      startEngine(ENGINE_PORT).catch((err) => patch({ startError: String(err) }));
    } else if (store.get().conn === 'connecting' && (!inTauri || store.get().startError)) {
      patch({ conn: 'down' });
    }
  }
}

async function loadPresets() {
  const { presets, aspects } = await api.presets();
  // `aspects` arrived with the Studio-era engine; without it this is an older build.
  if (!Array.isArray(aspects)) return patch({ outdated: true });
  patch({ outdated: false });
  const draft = store.get().draft;
  patch({ presets, aspects });
  setDraft({ aspect: aspects.some((a) => a.id === draft.aspect) ? draft.aspect : (aspects[0]?.id ?? '1:1') });
}

export async function loadInfo() {
  try {
    patch({ info: await api.info() });
  } catch (e) {
    fail(e);
  }
}

/** Starts the polling loop. Faster while something is downloading or the engine is away. */
export function startPolling() {
  let stopped = false;
  (async () => {
    if (SIMULATE_SLOW_START) await sleep(3000);
    let giveUp = Date.now() + 15000;
    while (!stopped) {
      await refresh();
      const s = store.get();
      if (s.conn === 'connecting' && Date.now() > giveUp) patch({ conn: 'down' });
      if (s.conn === 'ok') giveUp = Date.now() + 15000;
      const busy = s.conn !== 'ok' || s.models.some((m) => m.state === 'pulling' || m.state === 'verifying');
      await sleep(busy ? 1000 : 2500);
    }
  })();
  return () => {
    stopped = true;
  };
}

/** The engine is about to restart itself (a data folder move): wait for it instead of starting another. */
export function awaitRestart() {
  triedStart = true;
  patch({ conn: 'connecting' });
}

/** Ask the shell to start the engine again (the "Try again" button). */
export function retryEngine() {
  triedStart = false;
  patch({ conn: 'connecting', startError: null });
}

// ---------- Pulls (the engine owns them; this window only attaches) ----------

const attached = new Set<string>();
/** Pulls the user just cancelled. The engine reports them as pulling for a moment longer; don't re-join (a POST would restart them). */
const cancelling = new Set<string>();

/** Starts a pull or joins the one already running, and follows it until it ends. */
export function attachPull(id: string) {
  if (attached.has(id)) return;
  attached.add(id);
  store.set((s) => ({ ...s, pulls: { ...s.pulls, [id]: s.pulls[id] ?? null } }));
  let failed = false;
  api
    .pull(id, (e) => {
      if (e.type === 'progress') store.set((s) => ({ ...s, pulls: { ...s.pulls, [id]: e } }));
      if (e.type === 'error') {
        failed = true;
        // A cancel is the user's own doing; anything else needs saying.
        if (e.code !== 'cancelled') patch({ notice: { text: e.message, code: e.code } });
      }
    })
    .catch((e) => {
      failed = true;
      fail(e);
    })
    .finally(async () => {
      attached.delete(id);
      store.set((s) => {
        const { [id]: _, ...pulls } = s.pulls;
        return { ...s, pulls, pullQueue: failed ? [] : s.pullQueue };
      });
      await refresh();
      const next = store.get().pullQueue[0];
      if (next) {
        store.set((s) => ({ ...s, pullQueue: s.pullQueue.slice(1) }));
        attachPull(next);
      }
    });
}

/** Downloads models one after another, in order. */
export function pullModels(ids: string[]) {
  const [first, ...rest] = ids;
  if (!first) return;
  ids.forEach((id) => cancelling.delete(id));
  store.set((s) => ({ ...s, pullQueue: [...s.pullQueue, ...rest.filter((id) => !s.pullQueue.includes(id))] }));
  attachPull(first);
}

export async function cancelPull(id: string) {
  cancelling.add(id);
  // Cancelling one download stops the ones lined up behind it too.
  store.set((s) => ({ ...s, pullQueue: [] }));
  try {
    await api.cancelPull(id);
  } catch (e) {
    if (!(e instanceof EngineError && e.code === 'not_pulling')) fail(e);
  }
  await refresh();
}

/** Throws away a paused download: every `.part` owned by this model (or `runtime`). The UI asks first. */
export async function discardPaused(owner: string) {
  try {
    const r = await api.storage();
    for (const p of r.partials.filter((x) => x.owner === owner)) await api.deletePartials({ file: p.file });
  } catch (e) {
    fail(e);
  }
  await refresh();
}

export async function deleteModel(id: string) {
  try {
    await api.deleteModel(id);
  } catch (e) {
    fail(e);
  }
  await refresh();
}

/** Runs after every model list: re-attach to pulls started elsewhere, release groups whose model arrived. */
function afterModels() {
  const s = store.get();
  for (const m of s.models) {
    if (m.state !== 'pulling') cancelling.delete(m.id);
    else if (!cancelling.has(m.id)) attachPull(m.id);
  }
  for (const g of s.groups) {
    if (g.waitingFor && s.models.find((m) => m.id === g.waitingFor)?.state === 'installed') {
      updateGroup(g.id, (x) => ({ ...x, waitingFor: null }));
      void sendGroup(g.id);
    }
  }
}

// ---------- Create: groups, prints and the job watcher ----------

let counter = 0;
const uid = () => `${Date.now().toString(36)}-${(counter++).toString(36)}`;

const updateGroup = (id: string, f: (g: Group) => Group) => store.set((s) => ({ ...s, groups: s.groups.map((g) => (g.id === id ? f(g) : g)) }));
const updatePrint = (key: string, p: Partial<Print>) =>
  store.set((s) => ({ ...s, groups: s.groups.map((g) => (g.prints.some((x) => x.key === key) ? { ...g, prints: g.prints.map((x) => (x.key === key ? { ...x, ...p } : x)) } : g)) }));
const findPrint = (key: string) => store.get().groups.flatMap((g) => g.prints).find((p) => p.key === key);

const newPrint = (request: JobRequest, status: PrintStatus): Print => ({ key: uid(), request, status, jobId: null, job: null, progress: null, preview: null, error: null, errorCode: null, runningSince: null, stopping: false });

const taskOf = (req: Pick<JobRequest, 'task' | 'image'>): Task => req.task ?? (req.image ? 'edit' : 'generate');

/** The model a request will run on: the one it names, or Auto's pick for its task. */
export function modelFor(req: Pick<JobRequest, 'model' | 'preset' | 'task' | 'image'>, s: AppState = store.get()): Model | undefined {
  const named = req.model && req.model !== 'auto' ? req.model : req.preset ? s.presets.find((p) => p.id === req.preset)?.model : null;
  if (named) return findModel(named, s);
  return chosenModel(s.models, s.auto, taskOf(req), null);
}

/** A full id, or a bare model id (old history rows) resolved to its default tag here. */
export function findModel(id: string | null | undefined, s: AppState = store.get()): Model | undefined {
  if (!id) return undefined;
  const full = s.families.find((f) => f.id === id)?.default ?? id;
  return s.models.find((m) => m.id === full);
}

/** About a megapixel at this width:height, multiples of 16 (what the engine does for an edit that keeps its input's shape). */
function keepShape(width: number, height: number, area = 1024 * 1024): { width: number; height: number } {
  const k = Math.sqrt(area / (width * height));
  return { width: Math.max(16, Math.round((width * k) / 16) * 16), height: Math.max(16, Math.round((height * k) / 16) * 16) };
}

/** The size a request will render at (an estimate for edits that keep their input's shape). */
export function sizeFor(req: Omit<JobRequest, 'prompt'>, s: AppState = store.get(), input?: { width: number; height: number } | null): { width: number; height: number } {
  const a = s.aspects.find((x) => x.id === req.aspect);
  if (a) return { width: a.width, height: a.height };
  if (req.width && req.height) return { width: req.width, height: req.height };
  if (input) return keepShape(input.width, input.height);
  const d = modelFor(req, s)?.defaults;
  return { width: d?.width ?? 1024, height: d?.height ?? 1024 };
}

function summaryFor(req: JobRequest, s: AppState, size: { width: number; height: number }): string {
  const model = modelFor(req, s);
  const auto = !req.model || req.model === 'auto';
  const shape = req.aspect ?? (req.width ? `${size.width} × ${size.height}` : (aspectFor(size.width, size.height, s.aspects) ?? 'original shape'));
  return [auto ? 'Auto' : null, model?.name ?? req.model, shape].filter(Boolean).join(' · ');
}

/** Turns the composer into a request, or says what's missing. */
export function buildRequest(d: Draft): JobRequest | string {
  const req: JobRequest = { prompt: d.prompt.trim(), task: d.task };
  const model = d.models[d.task];
  if (model) req.model = model;
  if (d.task === 'edit') {
    if (!d.image) return 'Add the image to edit first: drop it here, paste it, or attach one.';
    if (d.image.status !== 'ready' || d.refs.some((r) => r.status === 'loading')) return 'The image is still being read. One moment.';
    req.image = d.image.sha256!;
    const refs = d.refs.filter((r) => r.status === 'ready' && r.sha256).map((r) => r.sha256!);
    if (refs.length) req.refs = refs;
  }
  if (d.customSize) {
    const [w, h] = [Number(d.width), Number(d.height)];
    if (![w, h].every((v) => Number.isInteger(v) && v >= 256 && v <= 2048 && v % 8 === 0)) return 'Custom size needs a width and height that are multiples of 8, from 256 to 2048.';
    req.width = w;
    req.height = h;
  } else if (d.task === 'generate') req.aspect = d.aspect;
  else if (d.editAspect !== 'original') req.aspect = d.editAspect;
  if (d.negative.trim()) req.negative_prompt = d.negative.trim();
  if (d.steps.trim()) req.steps = Number(d.steps);
  if (d.cfg.trim()) req.cfg = Number(d.cfg);
  if (d.seedLocked && d.seed.trim()) {
    const seed = Number(d.seed);
    if (!Number.isInteger(seed) || seed < 0) return 'The seed must be a whole number, 0 or more.';
    req.seed = seed;
  }
  return req;
}

/** Adds a group of `count` prints for a request and sends them (or parks them until the model has downloaded). */
function addGroup(request: JobRequest, count: number, inputs: InputImage[] = []) {
  const s = store.get();
  let model = modelFor(request, s);
  // Auto has nothing installed yet, but a model for this task is on its way (first run): wait for that one.
  if (!model && (!request.model || request.model === 'auto')) {
    const task = taskOf(request);
    const id = [...Object.keys(s.pulls), ...s.pullQueue, ...s.models.filter((m) => m.state === 'pulling').map((m) => m.id)].find((x) => {
      const m = s.models.find((y) => y.id === x);
      return m && (m.tasks ?? ['generate']).includes(task);
    });
    model = s.models.find((m) => m.id === id);
    if (model) request = { ...request, model: model.id };
  }
  // A model that's on its way: hold the prompt here and submit when the download is done.
  const coming = model && model.state !== 'installed' && (model.state === 'pulling' || model.state === 'verifying' || attached.has(model.id) || s.pullQueue.includes(model.id));
  const prints = Array.from({ length: count }, (_, i) => newPrint(request.seed != null ? { ...request, seed: request.seed + i } : request, coming ? 'waiting' : 'sending'));
  const size = sizeFor(request, s, inputs[0]);
  const group: Group = { id: uid(), task: taskOf(request), prompt: request.prompt, summary: summaryFor(request, s, size), inputs, ...size, waitingFor: coming && model ? model.id : null, prints, createdAt: Date.now() };
  store.set((x) => ({ ...x, groups: [...x.groups, group], notice: null, page: 'create', viewing: null }));
  if (!coming) void sendGroup(group.id);
}

/** Submits a group's unsent prints in order, so they queue in the order shown. */
async function sendGroup(groupId: string) {
  const group = store.get().groups.find((g) => g.id === groupId);
  for (const p of group?.prints ?? []) {
    if (findPrint(p.key)?.status === 'waiting' || findPrint(p.key)?.status === 'sending') await sendPrint(p.key);
  }
}

async function sendPrint(key: string) {
  const p = findPrint(key);
  if (!p) return;
  updatePrint(key, { status: 'sending', error: null, errorCode: null, job: null, jobId: null, preview: null, progress: null, stopping: false });
  try {
    const job = await api.createJob(p.request);
    updatePrint(key, { status: 'queued', jobId: job.id, job });
    void watch();
  } catch (e) {
    const err = e instanceof EngineError ? e : new EngineError(String(e), 'unknown', 0);
    updatePrint(key, { status: 'failed', error: err.message, errorCode: err.code });
  }
}

/** The Develop button. */
export function develop() {
  const d = store.get().draft;
  if (!d.prompt.trim()) return;
  const req = buildRequest(d);
  if (typeof req === 'string') return patch({ notice: { text: req, code: 'invalid_request' } });
  addGroup(req, d.count, d.task === 'edit' && d.image ? [d.image, ...d.refs.filter((r) => r.status === 'ready')] : []);
  // Like a chat: what was sent leaves the composer (the group keeps its inputs; "Edit this" chains the next edit).
  setDraft({ prompt: '', image: null, refs: [] });
}

/** Same prompt and settings, new seed. */
export function vary(p: Print, group?: Group) {
  const { seed: _, ...request } = p.request;
  addGroup(request, 1, group?.inputs ?? []);
}

/** Keeps this print's seed for the next image, so only the prompt changes the result. */
export function sameSeed(p: Print) {
  if (!p.job) return;
  setDraft({ prompt: p.job.prompt, seed: String(p.job.seed), seedLocked: true });
  store.set((s) => ({ ...s, advancedOpen: true, focusPrompt: s.focusPrompt + 1 }));
}

export const retry = (p: Print) => void sendPrint(p.key);

// ---------- Edit inputs ----------

const readDataUrl = (file: Blob) =>
  new Promise<string>((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error ?? new Error("Couldn't read that file."));
    r.readAsDataURL(file);
  });

export type Slot = 'image' | 'ref';

function putInput(slot: Slot, input: InputImage) {
  store.set((s) => {
    const d = s.draft;
    if (slot === 'image') return { ...s, draft: { ...d, task: 'edit', image: input } };
    return { ...s, draft: { ...d, task: 'edit', refs: [...d.refs.filter((r) => r.key !== input.key), input].slice(0, 9) } };
  });
}

function patchInput(key: string, p: Partial<InputImage>) {
  store.set((s) => {
    const d = s.draft;
    return { ...s, draft: { ...d, image: d.image?.key === key ? { ...d.image, ...p } : d.image, refs: d.refs.map((r) => (r.key === key ? { ...r, ...p } : r)) } };
  });
}

async function storeInto(slot: Slot, input: InputImage, body: { path: string } | { b64: string }) {
  putInput(slot, input);
  try {
    const stored = await api.storeInput(body);
    patchInput(input.key, { sha256: stored.sha256, width: stored.width, height: stored.height, imageId: stored.image_id ?? input.imageId, status: 'ready' });
  } catch (e) {
    removeInput(input.key);
    fail(e);
  }
}

/** An image file from disk, a drop or the clipboard. */
export async function addInputFile(slot: Slot, file: File) {
  if (!/^image\/(png|jpeg|webp)$/.test(file.type)) return patch({ notice: { text: `${file.name || 'That file'} isn't a PNG, JPEG or WebP image. Those are the ones the models can read.`, code: 'invalid_image' } });
  const src = URL.createObjectURL(file);
  const input: InputImage = { key: uid(), sha256: null, width: 1024, height: 1024, src, name: file.name || 'Pasted image', imageId: null, status: 'loading' };
  let b64: string;
  try {
    b64 = await readDataUrl(file);
  } catch (e) {
    return fail(e);
  }
  await storeInto(slot, input, { b64 });
}

const IMAGE_PATH = /\.(png|jpe?g|webp)$/i;
/** A transparent pixel while a dropped file is being stored (its thumbnail then comes from the engine). */
const BLANK = 'data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==';

/** An image file dropped on the desktop window: the engine reads it by path, then serves it back as the thumbnail. */
export async function addInputPath(slot: Slot, path: string) {
  const name = path.split(/[\\/]/).pop() ?? path;
  if (!IMAGE_PATH.test(path)) return patch({ notice: { text: `${name} isn't a PNG, JPEG or WebP image. Those are the ones the models can read.`, code: 'invalid_image' } });
  const input: InputImage = { key: uid(), sha256: null, width: 1024, height: 1024, src: BLANK, name, imageId: null, status: 'loading' };
  putInput(slot, input);
  try {
    const stored = await api.storeInput({ path });
    patchInput(input.key, { sha256: stored.sha256, width: stored.width, height: stored.height, imageId: stored.image_id ?? null, src: `${ENGINE_URL}/api/inputs/${stored.sha256}`, status: 'ready' });
  } catch (e) {
    removeInput(input.key);
    fail(e);
  }
}

/** Chat-style attach for dropped paths: the first becomes the image to edit, the rest references. */
export function attachPaths(paths: string[]) {
  for (const path of paths) {
    const d = store.get().draft;
    if (d.image && d.refs.length >= 9) return patch({ notice: { text: 'That is the most an edit can take: the image and nine references. Remove one first.', code: 'too_many_inputs' } });
    void addInputPath(d.image ? 'ref' : 'image', path);
  }
}

/** A print that's already here (the Library, the stream): the engine reads it from its own folder. */
export async function addInputJob(slot: Slot, job: Job) {
  if (!job.output_path) return;
  const input: InputImage = { key: uid(), sha256: null, width: job.params.width, height: job.params.height, src: api.imageUrl(job.id, 512), name: `Frame ${String(job.frame).padStart(4, '0')}`, imageId: job.id, status: 'loading' };
  await storeInto(slot, input, { path: job.output_path });
}

/** Chat-style attach: the first image becomes the one to edit, the rest references (up to 9). */
export function attachFile(file: File) {
  const d = store.get().draft;
  if (d.image && d.refs.length >= 9) return patch({ notice: { text: 'That is the most an edit can take: the image and nine references. Remove one first.', code: 'too_many_inputs' } });
  void addInputFile(d.image ? 'ref' : 'image', file);
}

export function attachJob(job: Job) {
  const d = store.get().draft;
  if (d.image && d.refs.length >= 9) return;
  void addInputJob(d.image ? 'ref' : 'image', job);
}

export function removeInput(key: string) {
  store.set((s) => {
    const d = s.draft;
    const gone = d.image?.key === key ? d.image : d.refs.find((r) => r.key === key);
    if (gone?.src.startsWith('blob:') && !s.groups.some((g) => g.inputs.some((i) => i.key === key))) URL.revokeObjectURL(gone.src);
    return { ...s, draft: { ...d, image: d.image?.key === key ? null : d.image, refs: d.refs.filter((r) => r.key !== key) } };
  });
}

/** "Edit this": the print goes into the Edit tab's well, and the cursor into the instruction. */
export function editThis(job: Job) {
  store.set((s) => ({ ...s, page: 'create', viewing: null, draft: { ...s.draft, task: 'edit', prompt: '' }, focusPrompt: s.focusPrompt + 1 }));
  savePrefs();
  void addInputJob('image', job);
}

/** Vary from the Library: the same recipe (an edit keeps its inputs) with a new seed, developed on Create. */
export function varyJob(job: Job, parent?: Job | null) {
  const p = job.params;
  const task: Task = p.task ?? 'generate';
  const req: JobRequest = { prompt: job.prompt, task, model: p.auto ? 'auto' : job.model, width: p.width, height: p.height, steps: p.steps, cfg: p.cfg };
  if (p.negative_prompt) req.negative_prompt = p.negative_prompt;
  let inputs: InputImage[] = [];
  if (task === 'edit' && p.inputs?.length) {
    req.image = p.inputs[0];
    if (p.inputs.length > 1) req.refs = p.inputs.slice(1);
    // Show the original beside the result when it's a Library print.
    if (parent) inputs = [{ key: uid(), sha256: p.inputs[0], width: parent.params.width, height: parent.params.height, src: api.imageUrl(parent.id, 512), name: `Frame ${String(parent.frame).padStart(4, '0')}`, imageId: parent.id, status: 'ready' }];
  }
  addGroup(req, 1, inputs);
}

/** Loads an image's whole recipe into the composer. */
export function remix(job: Job) {
  const s = store.get();
  const model = s.models.find((m) => m.id === job.model);
  const p = job.params;
  const task: Task = p.task ?? 'generate';
  const aspect = aspectFor(p.width, p.height, s.aspects);
  setDraft({
    task,
    prompt: job.prompt,
    // Auto picked it then: let Auto pick again now (it may know a better model since).
    models: { ...s.draft.models, [task]: p.auto ? null : job.model },
    aspect: task === 'generate' ? (aspect ?? s.draft.aspect) : s.draft.aspect,
    customSize: task === 'generate' && !aspect,
    width: String(p.width),
    height: String(p.height),
    seed: String(job.seed),
    seedLocked: false,
    steps: model && p.steps !== model.defaults.steps ? String(p.steps) : '',
    cfg: model && p.cfg !== model.defaults.cfg ? String(p.cfg) : '',
    negative: p.negative_prompt ?? '',
  });
  store.set((x) => ({ ...x, page: 'create', viewing: null, focusPrompt: x.focusPrompt + 1 }));
  // An edit of a Library image: put that image back in the well.
  if (task === 'edit' && job.parent_id) api.job(job.parent_id).then((parent) => addInputJob('image', parent), () => {});
}

/** Auto's pick for the composer's tab, for the switcher. */
export const autoPick = (s: AppState) => autoFor(s.auto, s.draft.task);

const active = (p: Print) => p.status === 'queued' || p.status === 'running';

/** Stops everything in a group that hasn't finished. */
export async function stopGroup(group: Group) {
  for (const p of group.prints) {
    if (p.status === 'waiting' || p.status === 'sending') updatePrint(p.key, { status: 'cancelled' });
    else if (active(p) && p.jobId) {
      updatePrint(p.key, { stopping: true });
      try {
        await api.cancelJob(p.jobId);
      } catch (e) {
        if (!(e instanceof EngineError && e.code === 'already_finished')) fail(e);
      }
    }
  }
  updateGroup(group.id, (g) => ({ ...g, waitingFor: null }));
}

function applyEvent(key: string, e: JobEvent) {
  const p = findPrint(key);
  if (!p) return;
  switch (e.type) {
    case 'snapshot': {
      const { progress, ...job } = e.job;
      settle(key, job, progress);
      break;
    }
    case 'started':
      updatePrint(key, { status: 'running', runningSince: Date.now() });
      break;
    case 'progress':
      updatePrint(key, { status: 'running', runningSince: p.runningSince ?? Date.now(), progress: { stage: e.stage, step: e.step, total: e.total } });
      break;
    case 'preview':
      updatePrint(key, { preview: `data:image/png;base64,${e.b64_png}` });
      break;
    case 'finished':
      settle(key, e.job, null);
      break;
  }
}

/** Mirrors a job record onto its print. */
function settle(key: string, job: Job, progress: Progress | null) {
  const p = findPrint(key);
  if (!p) return;
  const status: PrintStatus = job.status === 'deleted' ? 'cancelled' : job.status;
  updatePrint(key, {
    job,
    status,
    progress: status === 'running' ? (progress ?? p.progress) : null,
    runningSince: status === 'running' ? (p.runningSince ?? job.started_ms ?? Date.now()) : p.runningSince,
    error: status === 'failed' ? (job.error ?? 'Something went wrong. Try again.') : null,
    errorCode: status === 'failed' ? 'job_failed' : null,
  });
  if (status === 'completed') store.set((s) => ({ ...s, libraryRev: s.libraryRev + 1 }));
}

let watching = false;

/**
 * Follows our unfinished jobs one at a time, in order. The engine runs one job at a time, so
 * one event stream is enough (and keeps well under the browser's six connections per host).
 */
async function watch() {
  if (watching) return;
  watching = true;
  try {
    for (;;) {
      const next = store.get().groups.flatMap((g) => g.prints).find((p) => active(p) && p.jobId);
      if (!next?.jobId) break;
      try {
        await api.jobEvents(next.jobId, (e) => applyEvent(next.key, e));
      } catch {
        // fall through to the check below
      }
      const now = findPrint(next.key);
      if (now && active(now)) {
        // The stream ended without a result (engine restart, network blip): ask once, then give up plainly.
        await sleep(1200);
        try {
          const { progress, ...job } = await api.job(next.jobId);
          settle(next.key, job, progress);
        } catch (e) {
          const err = e instanceof EngineError ? e : new EngineError(String(e), 'unknown', 0);
          updatePrint(next.key, { status: 'failed', error: err.code === UNREACHABLE ? 'The engine stopped before this image finished. Develop it again.' : err.message, errorCode: err.code });
        }
      }
    }
  } finally {
    watching = false;
  }
}

/** True while anything of ours is developing or about to. */
export const isDeveloping = (s: AppState) => s.groups.some((g) => g.prints.some((p) => p.status === 'running' || p.status === 'queued' || p.status === 'sending'));

// ---------- Library ----------

export async function setFavourite(id: string, favourite: boolean) {
  try {
    await api.setFavourite(id, favourite);
  } catch (e) {
    fail(e);
  }
}

export async function deleteImage(id: string): Promise<boolean> {
  try {
    await api.deleteImage(id);
    // It leaves the Create stream too, so nothing points at a file that's gone.
    store.set((s) => ({
      ...s,
      libraryRev: s.libraryRev + 1,
      groups: s.groups.map((g) => ({ ...g, prints: g.prints.filter((p) => p.jobId !== id) })).filter((g) => g.prints.length > 0),
    }));
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}
