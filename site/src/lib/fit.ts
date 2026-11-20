/**
 * "Will it run on my machine?": one set of rules for the build-time tables and the in-browser check.
 * No Node imports here; this module ships to the browser.
 */

export type Support = 'now' | 'experimental' | 'planned' | 'not planned';
export type Vendor = 'nvidia' | 'amd' | 'intel' | 'apple' | 'other';

export interface Device {
  id: string;
  name: string;
  family: string;
  vendor: Vendor;
  match: string[];
  memory_gb: number[];
  unified: boolean;
  /** Integrated graphics: `memory_gb` is system RAM and `share` the part the GPU may use. */
  shared: boolean;
  /** Share of memory the GPU may use (1 for dedicated VRAM). */
  share: number;
  supported: Support;
  notes: string;
}

/** What the check needs from a model: its measured peak and, where measured, the speed and device. */
export interface FitModel {
  id: string;
  name: string;
  peak_mib: number | null;
  /** `peak_mib` isn't measured: it's the files + COMPUTE_RESERVE_MIB (engine/src/fit.rs). */
  estimate?: boolean;
  /** The size tag (q4, q8, bf16…), for messages. */
  tag?: string;
  /** Measured on this device id at this memory size (only there do we show speed). */
  measured?: { device: string; gb: number; cold_s?: number; warm_s?: number };
  /** Peak measured on integrated graphics (adapter dedicated + shared use), when there is one. */
  igpu_peak_mib?: number | null;
  igpu_measured?: { gpu: string; cold_s?: number; warm_s?: number };
}

export type Verdict = 'works' | 'offload' | 'experimental' | 'later' | 'needs' | 'unsupported' | 'unknown';
export interface Fit {
  verdict: Verdict;
  label: string;
  /** Plain words: what this means for you. */
  help: string;
  /** Seconds an image takes, only where we timed it on this exact device and size. */
  secs?: number;
}

/**
 * One formula with the engine (engine/src/fit.rs):
 *   fits     need + HEADROOM <= VRAM
 *   offload  need + HEADROOM <= VRAM + (RAM - RAM_RESERVE)    discrete cards only
 *   needs    otherwise
 */
/** Room left for Windows, the browser and the driver, same as the engine's estimate. */
export const HEADROOM_MIB = 512;
/** System RAM kept for Windows and other apps when offloading (measured; see engine/src/fit.rs). */
export const RAM_RESERVE_MIB = 6 * 1024;
/** Working memory on top of the weights for an untested model's estimate (same as the engine). */
export const COMPUTE_RESERVE_MIB = 2048;
export const estimateMib = (bytes: number) => Math.ceil(bytes / 1024 / 1024) + COMPUTE_RESERVE_MIB;

/** fitFor, marked as an estimate when nobody measured the model: never shown as measured. */
export function fitFor(m: FitModel, d: Device, gb: number, ramGb?: number): Fit {
  const f = fitMeasured(m, d, gb, ramGb);
  if (!m.estimate || f.verdict === 'unsupported' || f.verdict === 'unknown') return f;
  const est = 'Estimated from its file sizes plus 2 GB of working memory.';
  const label = f.verdict === 'offload' ? 'Runs with RAM offload (estimate)' : f.verdict === 'needs' ? 'Needs more memory (estimate)' : f.verdict === 'works' ? 'Should fit (estimate)' : `${f.label} (estimate)`;
  return { verdict: f.verdict, label, help: `${f.help} ${est}`.replace(/It needs /, 'It should need ') };
}

/** The tag a bare id means here, as the engine picks it (engine/src/models.rs default_variant): the largest that fits, else the smallest. */
export function bestFit(variants: FitModel[], d: Device, gb: number, ramGb?: number): { m: FitModel; f: Fit } | null {
  const all = variants.map((m) => ({ m, f: fitFor(m, d, gb, ramGb) }));
  const runs = (v: Verdict) => v === 'works' || v === 'experimental' || v === 'later';
  return [...all].reverse().find((x) => runs(x.f.verdict)) ?? all[0] ?? null;
}

export function usableMib(d: Device, gb: number) {
  return gb * 1024 * d.share;
}

export function fits(peakMib: number, d: Device, gb: number) {
  return peakMib + HEADROOM_MIB <= usableMib(d, gb);
}

/** `ramGb` is the system RAM the visitor told us (the check's RAM row); without it there's no offload verdict. */
function fitMeasured(m: FitModel, d: Device, gb: number, ramGb?: number): Fit {
  if (d.supported === 'not planned') return { verdict: 'unsupported', label: 'Not supported', help: d.notes };
  if (d.shared) return fitShared(m, d, gb);
  if (m.peak_mib == null) return { verdict: 'unknown', label: 'Not known yet', help: 'We don’t know how much memory it needs yet, so we won’t guess.' };
  if (!fits(m.peak_mib, d, gb)) {
    const usableRam = ramGb ? Math.max(0, ramGb * 1024 - RAM_RESERVE_MIB) : 0;
    if (!d.unified && ramGb && m.peak_mib + HEADROOM_MIB <= usableMib(d, gb) + usableRam)
      return {
        verdict: 'offload',
        label: 'Runs with RAM offload (slower)',
        help: `It needs ${gbStr(m.peak_mib)}: ${gb} GB on the card, the rest from your ${ramGb} GB of system RAM (keeping 6 GB for Windows and other apps). Slower than fitting on the card.${d.supported === 'experimental' ? ' Your GPU is experimental (Vulkan).' : ''}`,
      };
    const need = Math.ceil((m.peak_mib + HEADROOM_MIB) / 1024 / d.share);
    return {
      verdict: 'needs',
      label: 'Needs more memory',
      help: `It needs ${gbStr(m.peak_mib)} while developing; ${d.unified ? `about ${need} GB of unified memory` : `a ${need} GB GPU`} fits it on the card.${d.unified ? '' : ramGb ? ` Your ${ramGb} GB of RAM isn't enough to make up the rest.` : ' Tell us your system RAM: with enough of it, it can run with RAM offload.'} You can still download it: it may run very slowly or fail.`,
    };
  }
  const spare = (usableMib(d, gb) - m.peak_mib - HEADROOM_MIB) / 1024;
  const room = `About ${spare.toFixed(1)} GB to spare.`;
  const secs = m.measured && m.measured.device === d.id && m.measured.gb === gb ? (m.measured.warm_s ?? m.measured.cold_s) : undefined;
  if (d.supported === 'now')
    return {
      verdict: 'works',
      label: 'Fits · works now',
      help: secs != null ? `${room} About ${secs} s an image on this card.` : `${room}${d.notes ? ` ${d.notes}` : ''}`,
      secs,
    };
  if (d.supported === 'experimental')
    return { verdict: 'experimental', label: 'Fits · experimental', help: 'Runs through Vulkan, which is experimental in this version.' };
  return { verdict: 'later', label: 'Fits · support planned', help: d.notes };
}

/** Integrated graphics: fits or not in the GPU's share of system RAM; always experimental and slow. */
function fitShared(m: FitModel, d: Device, gb: number): Fit {
  const peak = m.igpu_peak_mib ?? m.peak_mib;
  if (peak == null) return { verdict: 'unknown', label: 'Not known yet', help: 'We don’t know how much memory it needs yet, so we won’t guess.' };
  if (!fits(peak, d, gb)) {
    const need = Math.ceil((peak + HEADROOM_MIB) / 1024 / d.share);
    return { verdict: 'needs', label: 'Needs more memory', help: `On built-in graphics it peaks at ${gbStr(peak)}, which needs about ${need} GB of system RAM. You can still download it: it may run very slowly or fail.` };
  }
  return { verdict: 'experimental', label: 'Fits · experimental, slow', help: 'Runs through Vulkan on built-in graphics, from system memory. Expect minutes per image.' };
}

export const gbStr = (mib: number) => `${(mib / 1024).toFixed(1)} GB`;

/* ---------------- Detection ---------------- */

export function normalise(s: string) {
  return s.toLowerCase().replace(/\(r\)|\(tm\)|™|®/g, '').replace(/\s+/g, ' ');
}

/** The device whose longest match fragment appears in the renderer string, ending at a word boundary. */
export function matchRenderer(renderer: string, devices: Device[]): Device | null {
  const r = normalise(renderer);
  let best: Device | null = null;
  let bestLen = 0;
  for (const d of devices)
    for (const m of d.match) {
      const i = r.indexOf(m);
      if (i < 0 || m.length <= bestLen) continue;
      const next = r.charAt(i + m.length);
      if (next && /[a-z0-9]/.test(next)) continue;
      best = d;
      bestLen = m.length;
    }
  return best;
}

export type Detection =
  | { kind: 'device'; device: Device; gb?: number } // gb set when there's only one size
  | { kind: 'integrated'; device: Device }
  | { kind: 'apple'; device?: Device }
  | { kind: 'mobile' }
  | { kind: 'unknown'; reason: string };

const SOFTWARE = /swiftshader|llvmpipe|softpipe|basic render|software|mesa offscreen/;

/** Read what the browser says; anything unsure goes to the picker. */
export function detect(ua: string, touchPoints: number, renderer: string | null, devices: Device[]): Detection {
  const isIpad = /macintosh/i.test(ua) && touchPoints > 1;
  if (/android|iphone|ipad|ipod|mobile/i.test(ua) || isIpad) return { kind: 'mobile' };
  // Firefox no longer exposes the real renderer; it reports a stand-in card.
  if (/firefox\//i.test(ua)) return { kind: 'unknown', reason: 'Firefox hides the graphics card from websites.' };
  if (!renderer) return { kind: 'unknown', reason: 'Your browser doesn’t say which graphics card you have.' };
  const r = normalise(renderer);
  if (SOFTWARE.test(r)) return { kind: 'unknown', reason: 'Your browser is drawing in software, so it can’t see your graphics card.' };
  const d = matchRenderer(renderer, devices);
  if (/apple/.test(r)) return d && d.vendor === 'apple' ? { kind: 'apple', device: d } : { kind: 'apple' };
  if (!d) return { kind: 'unknown', reason: 'We don’t recognise the graphics card your browser reports.' };
  if (d.family === 'integrated') return { kind: 'integrated', device: d };
  return { kind: 'device', device: d, gb: d.memory_gb.length === 1 ? d.memory_gb[0] : undefined };
}

/* ---------------- Remembered choice (this browser only) ---------------- */

export const STORE_KEY = 'dawnroom.hardware';
export interface Choice {
  id: string;
  gb: number;
  how: 'auto' | 'picked';
  /** System RAM in GB, if the visitor told us. Not used in the verdict yet (RAM offload comes with the engine work). */
  ram?: number;
}

/** RAM sizes offered as chips. */
export const RAM_SIZES = [8, 16, 32, 64, 128];
/** navigator.deviceMemory: Chromium only, rounded down and capped (8 GB historically; Chromium 1243 reported 16), so it's a hint, never an answer. */
export function ramHint(): number | null {
  const m = (navigator as Navigator & { deviceMemory?: number }).deviceMemory;
  return typeof m === 'number' && m > 0 ? m : null;
}

export function loadChoice(): Choice | null {
  try {
    const c = JSON.parse(localStorage.getItem(STORE_KEY) ?? 'null');
    if (!(c && typeof c.id === 'string' && typeof c.gb === 'number')) return null;
    if (c.ram !== undefined && !(typeof c.ram === 'number' && c.ram > 0)) delete c.ram;
    return c;
  } catch {
    return null;
  }
}
export function saveChoice(c: Choice | null) {
  try {
    if (c) localStorage.setItem(STORE_KEY, JSON.stringify(c));
    else localStorage.removeItem(STORE_KEY);
  } catch {
    /* private mode: the check still works, it just won't remember */
  }
  window.dispatchEvent(new CustomEvent('hardware-change', { detail: c }));
}
