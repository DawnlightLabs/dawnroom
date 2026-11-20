// What the model switcher and the Models page say about a model, per task. Pure functions over
// the engine's `GET /api/models` and `GET /api/auto`: the engine decides, this only words it.

import type { AutoAnswer, AutoPick, FitDetail, Model, Task, Timing } from '../api/types';

export const TASKS: { id: Task; label: string }[] = [
  { id: 'generate', label: 'Generate' },
  { id: 'edit', label: 'Edit' },
];

export const tasksOf = (m: Pick<Model, 'tasks'>): Task[] => m.tasks ?? ['generate'];
export const canDo = (m: Pick<Model, 'tasks'>, task: Task) => tasksOf(m).includes(task);


/** 19 → "19 s", 750 → "13 min". */
export function seconds(s: number): string {
  return s < 90 ? `${Math.round(s)} s` : `${Math.round(s / 60)} min`;
}

/** Seconds an image takes on this machine, when we have a time for this GPU; else null (nothing shown). Warm first: that's what a second image takes. */
export function timeHere(t: Timing | null | undefined): number | null {
  if (!t?.here) return null;
  return t.warm_s ?? t.cold_s ?? null;
}

/** "about 19 s", or '' when there's no time for this machine. */
export function timingLabel(t: Timing | null | undefined): string {
  const s = timeHere(t);
  return s == null ? '' : `about ${seconds(s)}`;
}

export const timingFor = (m: Pick<Model, 'timing'>, task: Task) => m.timing?.[task] ?? null;

const gb = (mib: number) => `${(mib / 1024).toFixed(1)} GB`;

/** The engine keeps 512 MiB spare on the GPU (fit.rs); what doesn't fit goes to system RAM. */
const HEADROOM_MIB = 512;

/** "4.3 GB on the GPU", "9.5 GB: GPU + 2.0 GB RAM, slower", "est. 7.1 GB on the GPU". */
export function memoryLabel(f: FitDetail | null | undefined): string {
  if (!f) return 'memory not known';
  if (f.tier === 'no_gpu') return 'needs a GPU';
  if (f.need_mib == null) return 'memory not known';
  const need = `${f.estimate ? 'est. ' : ''}${gb(f.need_mib)}`;
  if (f.tier === 'fits') return `${need} on the GPU`;
  if (f.tier === 'offload') {
    const ram = Math.max(0, f.need_mib + HEADROOM_MIB - (f.gpu_mib ?? 0));
    return `${need}: GPU + ${gb(ram)} RAM, slower`;
  }
  return `needs ${need}, more than this machine has`;
}

/** The card's short form: "4.3 GB on the GPU", "9.5 GB · RAM offload", "est. 7.1 GB on the GPU", "needs 29 GB". */
export function memoryShort(f: FitDetail | null | undefined): string {
  if (!f || f.need_mib == null) return f?.tier === 'no_gpu' ? 'needs a GPU' : 'not known';
  const need = `${f.estimate ? 'est. ' : ''}${gb(f.need_mib)}`;
  if (f.tier === 'fits') return `${need} on the GPU`;
  if (f.tier === 'offload') return `${need} · RAM offload`;
  return f.tier === 'no_gpu' ? 'needs a GPU' : `needs ${need}`;
}

export const fitFor = (m: Pick<Model, 'task_fit' | 'fit_detail'>, task: Task): FitDetail => m.task_fit?.[task] ?? m.fit_detail;

/** Usable here at all (fits or with offload). */
export const runsHere = (f: FitDetail) => f.tier === 'fits' || f.tier === 'offload';

/**
 * The switcher's list for one task: installed models that can do it, library before custom, then the
 * ones that fit before offload, then by name. Models still downloading or not yet tested (custom)
 * aren't choosable, so they're left out; the Models page has the whole library.
 */
export function switcherOptions(models: Model[], task: Task): Model[] {
  const rank = (m: Model) => (m.custom ? 1 : 0);
  const fitRank = (m: Model) => (fitFor(m, task).tier === 'fits' ? 0 : fitFor(m, task).tier === 'offload' ? 1 : 2);
  return models
    .filter((m) => m.state === 'installed' && canDo(m, task))
    .sort((a, b) => rank(a) - rank(b) || fitRank(a) - fitRank(b) || a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
}

export const autoFor = (auto: AutoAnswer | null, task: Task): AutoPick | null => auto?.[task] ?? null;

/** The reason under "Auto": the engine's own line, minus the model name it starts with (shown beside it). */
export function autoReason(pick: AutoPick): string {
  if (!pick.ok) return pick.error;
  const prefix = `${pick.name}: `;
  const r = pick.reason.startsWith(prefix) ? pick.reason.slice(prefix.length) : pick.reason;
  return r.charAt(0).toUpperCase() + r.slice(1);
}

/** The model a draft will run on: the chosen one, or Auto's pick for the task. */
export function chosenModel(models: Model[], auto: AutoAnswer | null, task: Task, chosen: string | null): Model | undefined {
  if (chosen) return models.find((m) => m.id === chosen);
  const pick = autoFor(auto, task);
  return pick?.ok ? models.find((m) => m.id === pick.model) : undefined;
}

/** "q4 · 6.7 GB" */
export function tagLine(m: Pick<Model, 'tag' | 'size_bytes'>): string {
  const size = m.size_bytes >= 1e9 ? `${(m.size_bytes / 1e9).toFixed(1)} GB` : `${Math.round(m.size_bytes / 1e6)} MB`;
  return [m.tag, size].filter(Boolean).join(' · ');
}
