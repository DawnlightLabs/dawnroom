import type { Aspect, Job, Progress, PullProgress, Model, Paused, PullWait } from '../api/types';

/** 6696835812 → "6.7 GB". Decimal units, the way download sizes are usually quoted. */
export function bytes(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(n >= 1e10 ? 0 : 1)} GB`;
  if (n >= 1e6) return `${Math.round(n / 1e6)} MB`;
  if (n >= 1e3) return `${Math.round(n / 1e3)} kB`;
  return `${n} B`;
}

/** Film-style frame number: 247 → "0247". */
export function frameNo(frame: number): string {
  return String(frame).padStart(4, '0');
}

/** 24812 → "24.8 s"; over two minutes → "2 min 5 s". */
export function duration(ms: number): string {
  const s = ms / 1000;
  if (s < 120) return `${s.toFixed(1)} s`;
  return `${Math.floor(s / 60)} min ${Math.round(s % 60)} s`;
}

/** How long the job spent on the GPU, or null if it never ran to an end. */
export function jobMs(job: Pick<Job, 'started_ms' | 'finished_ms'>): number | null {
  return job.started_ms != null && job.finished_ms != null ? job.finished_ms - job.started_ms : null;
}

/** The film-edge caption: "0247 ▸ seed 190442 ▸ 4 steps ▸ 11.2 s". */
export function filmEdge(job: Job): string {
  const ms = jobMs(job);
  const parts = [frameNo(job.frame), `seed ${job.seed}`, `${job.params.steps} steps`];
  if (ms != null) parts.push(duration(ms));
  return parts.join(' ▸ ');
}

/** "about 40 s left", "about 3 min left". */
export function eta(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return '';
  if (seconds < 50) return `about ${Math.max(5, Math.round(seconds / 5) * 5)} s left`;
  return `about ${Math.round(seconds / 60)} min left`;
}

/** "NVIDIA GeForce RTX 5060 Laptop GPU" → "RTX 5060 Laptop". */
export function shortGpu(name: string): string {
  return name.replace(/^NVIDIA\s+/i, '').replace(/^GeForce\s+/i, '').replace(/\s+GPU$/i, '');
}

/** 8151 MiB → "8 GB" (cards are sold by the rounded number). */
export function vram(mib: number): string {
  const gb = mib / 1024;
  return `${gb >= 10 || Math.abs(gb - Math.round(gb)) < 0.06 ? Math.round(gb) : gb.toFixed(1)} GB`;
}

/** The aspect chip a size belongs to, if any. */
export function aspectFor(width: number, height: number, aspects: Aspect[]): string | null {
  return aspects.find((a) => a.width === width && a.height === height)?.id ?? null;
}

/** What the engine is doing, in a few plain words. */
export function stageLabel(p: Progress | null): string {
  switch (p?.stage) {
    case 'installing_runtime':
      return 'Installing the image runtime';
    case 'starting':
      return 'Loading the model';
    case 'switching_model':
      return 'Switching models';
    case 'conditioning':
      return 'Reading the prompt';
    case 'sampling':
      return p.step != null && p.total != null ? `Developing ${p.step}/${p.total}` : 'Developing';
    case 'vae_decode':
    case 'encoding':
      return 'Finishing';
    default:
      return 'Developing';
  }
}

/** 0..1 through the sampling steps. Stages before sampling are 0, after it 1. */
export function stageProgress(p: Progress | null): number {
  if (!p) return 0;
  if (p.stage === 'vae_decode' || p.stage === 'encoding') return 1;
  if (p.stage === 'sampling' && p.step != null && p.total) return Math.min(1, p.step / p.total);
  return 0;
}

/** Bytes done and still to go across every file of a pull, from the engine's per-file progress. */
/** A pull installs the image runtime first (`file` = `runtime/<zip>`); those bytes aren't the model's. */
export function isRuntimeStep(p: PullProgress | null): boolean {
  return !!p && p.file.startsWith('runtime/');
}

/** What a runtime step is doing, for the progress line. */
export function runtimeStepLabel(p: PullProgress): string {
  if (p.stage === 'unpacking') return 'Unpacking the image runtime…';
  if (p.stage === 'verifying') return 'Checking the image runtime…';
  return `Image runtime · ${bytes(p.bytes)} of ${bytes(p.total)}`;
}

/** "Network error, trying again in 12 s" for a pull that waits to retry. */
export function waitLabel(w: PullWait, now = Date.now()): string {
  const secs = Math.max(0, Math.ceil((w.retry_at_ms - now) / 1000));
  const what = w.kind === 'connection' ? 'Network error' : w.reason;
  return secs > 0 ? `${what}, trying again in ${secs} s` : `${what}, trying again now`;
}

/** "Your download stopped at 4.1 of 13.0 GB." */
export function pausedLabel(p: Paused): string {
  const gb = (n: number) => (n / 1e9).toFixed(1);
  return p.total >= 1e9 ? `Your download stopped at ${gb(p.bytes)} of ${gb(p.total)} GB.` : `Your download stopped at ${bytes(p.bytes)} of ${bytes(p.total)}.`;
}

export function pullTotals(model: Pick<Model, 'files' | 'size_bytes'>, p: PullProgress | null): { done: number; total: number; fraction: number } {
  const total = model.size_bytes;
  if (!p || isRuntimeStep(p)) return { done: 0, total, fraction: 0 };
  const index = model.files.findIndex((f) => f.path === p.file);
  const before = model.files.slice(0, Math.max(0, index)).reduce((n, f) => n + f.size, 0);
  // Verifying a file that's already complete counts it as done.
  const current = p.stage === 'downloading' || p.stage === 'waiting' ? p.bytes : p.total;
  const done = Math.min(total, before + current);
  return { done, total, fraction: total ? done / total : 0 };
}

/**
 * The recipe as text: what was asked, the settings that reproduce it, and the CLI line that
 * runs it again.
 */
export function recipeText(job: Job, bin = 'dawnroom'): string {
  const p = job.params;
  const lines = [job.prompt];
  if (p.negative_prompt) lines.push(`negative: ${p.negative_prompt}`);
  lines.push(`${job.model} · seed ${job.seed} · ${p.steps} steps · guidance ${p.cfg} · ${p.width}×${p.height}`);
  lines.push(`${bin} run ${JSON.stringify(job.prompt)} --model ${job.model} --seed ${job.seed}`);
  return lines.join('\n');
}
