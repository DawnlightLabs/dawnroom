/**
 * Reads registry/*.toml, the same manifests the engine loads (schema: engine/src/registry.rs).
 * Paths resolve from the site folder (process.cwd()), not from this module, which gets bundled.
 */
import { readdirSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { parse } from 'smol-toml';
import samples from '../samples.json';
import type { Device, FitModel, Support, Vendor } from './fit';
import { estimateMib, matchRenderer } from './fit';
import { BIN as BIN_NAME } from '../config';

export const REGISTRY_DIR = resolve(process.cwd(), '..', 'registry');

export interface ModelFile {
  role: 'diffusion_model' | 'uncond_diffusion_model' | 'checkpoint' | 'llm' | 'llm_vision' | 'clip_l' | 'clip_g' | 't5xxl' | 'vae' | 'tokenizer';
  category: string;
  filename: string;
  repo: string;
  revision: string;
  path: string;
  size: number;
  sha256: string;
  /** Where the bytes come from (official, identical to an official file, quantised or repacked by an uploader). */
  provenance?: string;
}
export interface Measured {
  gpu: string;
  vram_total_mib: number;
  peak_vram_mib: number;
  cold_s?: number;
  warm_s?: number;
  note?: string;
  /** Runtime build it ran on (e.g. windows-vulkan); missing = windows-cuda12. */
  runtime?: string;
  /** Measured on integrated graphics (shared memory): never used for a discrete card's fit. */
  integrated?: boolean;
  /** Memory needed with nothing offloaded, when measured with offload (engine/src/fit.rs). */
  need_mib?: number;
  peak_ram_mib?: number;
  offload?: 'auto-fit' | 'off';
  /** Missing = measured by Dawnroom ("tested"); `community` = a contributed measurement. */
  source?: string;
  /** What was measured: missing = generate. */
  task?: Task;
  /** The output size it was measured at, `1024x1024`. */
  size?: string;
}
/** generate = text to image; edit = an instruction applied to an input image. */
export type Task = 'generate' | 'edit';
export const TASK_LABELS: Record<Task, string> = { generate: 'Generate', edit: 'Edit' };
/** The task a model's fit is shown for, as the engine does it (engine/src/fit.rs main_task). */
export const mainTask = (tasks?: Task[]): Task => (!tasks || tasks.includes('generate') ? 'generate' : 'edit');
export type Tier = 'tested' | 'community' | 'untested';

/** One size tag of a model (schema 2 `[[variant]]`), with its files resolved (shared + its own). */
export interface Variant {
  /** `<model>:<tag>`, the engine's id. */
  id: string;
  tag: string;
  files: ModelFile[];
  measured: Measured[];
  bytes: number;
  tier: Tier;
  name: string;
  tasks: Task[];
}

/** Schema 2 as written (engine/src/registry.rs): shared files, then per-tag files and measurements. */
export interface Manifest {
  schema: number;
  id: string;
  name: string;
  tagline: string;
  tags?: string[];
  /** Missing = generate only. */
  tasks?: Task[];
  /** The runtime it needs (registry/runtimes/), e.g. sd-cpp@master-1086-465adf3. */
  backend: string;
  /** The official repo; files come from the mirrors named in `files`. */
  original?: string;
  recipe?: string;
  licence: { spdx: string; name?: string; url: string; commercial: boolean; use_restrictions?: boolean; accept_required?: boolean; note?: string };
  files: ModelFile[];
  defaults: { steps: number; cfg: number; width: number; height: number; sampler?: string; scheduler?: string; negative_prompt?: string; server_flags?: string[] };
  /** Schema 1 only. */
  measured?: Measured[];
  variant?: { tag: string; files?: ModelFile[]; measured?: Measured[] }[];
}

export function tierOf(measured: Measured[]): Tier {
  if (measured.some((x) => !x.source)) return 'tested';
  return measured.length ? 'community' : 'untested';
}

/** Every size tag, smallest first, each with its full file list (the engine does the same). */
export function variantsOf(m: Manifest): Variant[] {
  const list = m.variant?.length ? m.variant : [{ tag: '', files: [], measured: m.measured ?? [] }];
  return list
    .map((v) => {
      const own = v.files ?? [];
      const files = [...m.files.filter((f) => !own.some((g) => g.role === f.role)), ...own];
      const measured = v.measured ?? [];
      return { id: v.tag ? `${m.id}:${v.tag}` : m.id, tag: v.tag, files, measured, bytes: files.reduce((n, f) => n + f.size, 0), tier: tierOf(measured), name: m.name, tasks: m.tasks ?? ['generate'] };
    })
    .sort((a, b) => a.bytes - b.bytes);
}

export interface Sample {
  model: string;
  file: string;
  frame: number;
  prompt: string;
  seed: number;
  steps: number;
  width: number;
  height: number;
  seconds: number;
}

/** Manifests exactly as written, sorted by id like the engine. */
export function loadManifests(): Manifest[] {
  const files = readdirSync(REGISTRY_DIR).filter((f) => f.endsWith('.toml')).sort();
  const models = files.map((f) => parse(readFileSync(join(REGISTRY_DIR, f), 'utf8')) as unknown as Manifest);
  for (const m of models) {
    if (m.schema !== 1 && m.schema !== 2) throw new Error(`${m.id}: schema ${m.schema} isn't supported`);
    m.files ??= [];
    if (m.schema === 2 && !m.variant?.length) throw new Error(`${m.id}: no [[variant]]`);
    if (m.schema === 1 && !m.files.length) throw new Error(`${m.id}: no files`);
  }
  return models.sort((a, b) => a.id.localeCompare(b.id));
}

/** Presets live in engine code (engine/src/registry.rs, PRESETS), pinned to a tag. Mirrored here for labels. */
export const PRESET_TAGS: Record<string, { label: string; tag: string }> = { 'flux2-klein-4b': { label: 'Fast', tag: 'q8' }, 'z-image-turbo': { label: 'Quality', tag: 'q4' } };
export const PRESETS: Record<string, string> = Object.fromEntries(Object.entries(PRESET_TAGS).map(([k, v]) => [k, v.label]));

/** Display order: the presets first, then the rest. */
const ORDER = ['z-image-turbo', 'flux2-klein-4b'];

/**
 * One model on the site. `files`, `measured`, `bytes` and `tag` describe its representative tag: the
 * preset's tag, else a tested one, else the smallest. `variants` has every tag.
 */
export interface Model extends Manifest {
  bytes: number;
  measured: Measured[];
  tag: string;
  variants: Variant[];
  /** The best tier among its tags. */
  tier: Tier;
  samples: Sample[];
  preset?: string;
  licenceLabel: string;
}

export function loadModels(): Model[] {
  return loadManifests()
    .map((m) => {
      const variants = variantsOf(m);
      const rep = variants.find((v) => v.tag === PRESET_TAGS[m.id]?.tag) ?? variants.find((v) => v.tier === 'tested') ?? variants[0];
      const tier: Tier = variants.some((v) => v.tier === 'tested') ? 'tested' : variants.some((v) => v.tier === 'community') ? 'community' : 'untested';
      return {
        ...m,
        files: rep.files,
        measured: rep.measured,
        bytes: rep.bytes,
        tag: rep.tag,
        variants,
        tier,
        samples: (samples as Sample[]).filter((s) => s.model === m.id),
        preset: PRESETS[m.id],
        licenceLabel: licenceLabel(m.licence.spdx, m.licence.name),
      };
    })
    .sort((a, b) => rank(a.id) - rank(b.id) || (a.tier === b.tier ? 0 : a.tier === 'tested' ? -1 : b.tier === 'tested' ? 1 : 0) || a.name.localeCompare(b.name));
}

export const TIER_LABELS: Record<Tier, string> = { tested: 'Tested', community: 'Community-tested', untested: 'Untested' };

function rank(id: string) {
  const i = ORDER.indexOf(id);
  return i < 0 ? ORDER.length : i;
}

export function licenceLabel(spdx: string, name?: string) {
  if (spdx === 'LicenseRef-CreativeML-OpenRAIL-PlusPlus-M') return 'OpenRAIL++-M';
  if (spdx === 'LicenseRef-CreativeML-OpenRAIL-M') return 'OpenRAIL-M';
  if (spdx.startsWith('LicenseRef-') && name) return name.replace(/ License( Agreement)?$/, '').replace(/ Agreement$/, '');
  return spdx.replace(/^LicenseRef-/, '');
}

/** Same rounding as the engine: 8151 MiB is the 8 GB tier. */
export function vramTierGb(mib: number) {
  return Math.floor((mib + 512) / 1024);
}

export const gb = (bytes: number) => `${(bytes / 1e9).toFixed(1)} GB`;
export const gib = (mib: number) => `${(mib / 1024).toFixed(1)} GB`;

export const shortGpu = (name: string) => name.replace(/^NVIDIA GeForce /, '');

export const TAG_LABELS: Record<string, string> = {
  fast: 'Fast',
  quality: 'Quality',
  text: 'Text in images',
  photo: 'Photo',
  photoreal: 'Photoreal',
  sdxl: 'SDXL',
  small: 'Small',
  big: 'Big',
  classic: 'Classic',
};

/** The recipe line, as on Studio's prints: frame ▸ seed ▸ steps ▸ time. */
export function recipeLine(s: Sample) {
  return `${String(s.frame).padStart(4, '0')} ▸ seed ${s.seed} ▸ ${s.steps} steps ▸ ${s.seconds.toFixed(1)} s`;
}

/* ---------------- Runtimes (registry/runtimes/*.toml) ---------------- */

/** One sd.cpp build per OS + GPU backend; generated from the GitHub release API (engine/scripts/gen_runtimes.py). */
export interface Runtime {
  id: string;
  runtime: string;
  build: string;
  os: 'windows' | 'linux';
  gpu: 'cuda' | 'vulkan';
  experimental: boolean;
  files: { name: string; url: string; size: number; sha256: string; unpacked_size: number }[];
  /** Download size and size once unpacked (the zips are deleted after unpacking). */
  download: number;
  installed: number;
}

export function loadRuntimes(): Runtime[] {
  const dir = join(REGISTRY_DIR, 'runtimes');
  return readdirSync(dir)
    .filter((f) => f.endsWith('.toml'))
    .sort()
    .map((f) => {
      const r = parse(readFileSync(join(dir, f), 'utf8')) as unknown as Omit<Runtime, 'download' | 'installed'>;
      return { ...r, download: r.files.reduce((n, x) => n + x.size, 0), installed: r.files.reduce((n, x) => n + x.unpacked_size, 0) };
    });
}

/* ---------------- Hardware (registry/hardware/devices.toml) ---------------- */


interface FamilyToml {
  id: string;
  name: string;
  vendor: Vendor;
  backend: string;
  supported: Support;
  notes: string;
  source: string;
  devices: { id: string; name: string; match: string[]; memory_gb: number[]; memory?: 'dedicated' | 'unified' | 'shared'; usable_share?: number }[];
}
export interface Family extends Omit<FamilyToml, 'devices'> {
  devices: Device[];
}

const SUPPORTED: Support[] = ['now', 'experimental', 'planned', 'not planned'];

export function loadFamilies(): Family[] {
  const t = parse(readFileSync(join(REGISTRY_DIR, 'hardware', 'devices.toml'), 'utf8')) as unknown as { schema: number; family: FamilyToml[] };
  if (t.schema !== 1) throw new Error(`hardware/devices.toml: schema ${t.schema} isn't supported`);
  return t.family.map((f) => {
    if (!SUPPORTED.includes(f.supported)) throw new Error(`${f.id}: supported = "${f.supported}"`);
    return {
      ...f,
      devices: f.devices.map((d) => ({
        id: d.id,
        name: d.name,
        family: f.id,
        vendor: f.vendor,
        match: d.match,
        memory_gb: d.memory_gb,
        unified: d.memory === 'unified',
        shared: d.memory === 'shared',
        share: d.memory === 'unified' || d.memory === 'shared' ? (d.usable_share ?? 0.5) : 1,
        supported: f.supported,
        notes: f.notes,
      })),
    };
  });
}

export const loadDevices = () => loadFamilies().flatMap((f) => f.devices);

/**
 * The slice of a model (or one size tag) the hardware check needs; measured speed is pinned to the
 * device it ran on. Nobody measured it: the estimate, its file sizes + COMPUTE_RESERVE_MIB.
 */
export function fitModel(m: { id: string; name: string; measured?: Measured[]; bytes: number; tag?: string; tasks?: Task[] }, devices: Device[]): FitModel {
  // Only the main task's measurements count (an edit holds its input images too, so it peaks higher).
  const main = mainTask(m.tasks);
  const all = (m.measured ?? []).filter((x) => (x.task ?? 'generate') === main);
  // Integrated-GPU points describe shared system memory, so they never set a card's peak.
  const meas = all.filter((x) => !x.integrated);
  const igpu = all.filter((x) => x.integrated);
  // The need, as the engine computes it: need_mib where measured with offload, else the peak.
  const need = (x: Measured) => x.need_mib ?? x.peak_vram_mib;
  const peak = meas.length ? Math.max(...meas.map(need)) : estimateMib(m.bytes);
  const first = meas[0];
  const dev = first ? matchRenderer(first.gpu, devices) : null;
  return {
    id: m.id,
    name: m.name,
    peak_mib: peak,
    estimate: !meas.length,
    tag: m.tag || undefined,
    measured: first && dev ? { device: dev.id, gb: vramTierGb(first.vram_total_mib), cold_s: first.cold_s, warm_s: first.warm_s } : undefined,
    igpu_peak_mib: igpu.length ? Math.max(...igpu.map(need)) : null,
    igpu_measured: igpu[0] ? { gpu: igpu[0].gpu, cold_s: igpu[0].cold_s, warm_s: igpu[0].warm_s } : undefined,
  };
}

/* ---------------- Reproduce a print ---------------- */

/** Engine aspects (GET /api/presets → aspects). Mirrored for the reproduce commands. */
const ASPECTS: Record<string, string> = {
  '1024x1024': '1:1', '1152x864': '4:3', '864x1152': '3:4', '1248x832': '3:2', '832x1248': '2:3', '1360x768': '16:9', '768x1360': '9:16',
};
const PRESET_IDS: Record<string, string> = { 'flux2-klein-4b': 'fast', 'z-image-turbo': 'quality' };
/** All tags of every model, fitted: the check picks the largest tag that runs, like the engine. */
export function fitFamilies(devices: Device[]) {
  return loadModels().map((m) => ({ id: m.id, variants: m.variants.map((v) => fitModel(v, devices)) }));
}

export interface Recipe {
  /** The exact command to paste into PowerShell or a terminal. */
  cmd: string;
  via: 'cli' | 'api';
  /** Why it's the API form, when it is. */
  why?: string;
  /** The same recipe in Studio's words. */
  studio: string;
}

/**
 * The exact command for a sample. The CLI has no size option yet and Windows PowerShell 5.1 mangles
 * double quotes passed to programs, so non-square prints and prompts with quotes use the engine's API
 * (Invoke-RestMethod works in PowerShell 5.1 and 7). The print lands in the library either way.
 */
export function recipe(s: { model: string; prompt: string; seed: number; width: number; height: number }, name = s.model, port = 8420): Recipe {
  const preset = PRESET_IDS[s.model];
  const aspect = ASPECTS[`${s.width}x${s.height}`] ?? `${s.width}x${s.height}`;
  const pick = preset ? `--${preset}` : `--model ${s.model}`;
  const studio = `${preset ? preset[0].toUpperCase() + preset.slice(1) : name} · ${aspect} · seed ${s.seed}`;
  const square = aspect === '1:1';
  if (square && !/["`$]/.test(s.prompt)) return { cmd: `${BIN_NAME} run ${pick} --seed ${s.seed} "${s.prompt}"`, via: 'cli', studio };
  const body = JSON.stringify({ prompt: s.prompt, ...(preset ? { preset } : { model: s.model }), seed: s.seed, aspect });
  return {
    cmd: `Invoke-RestMethod -Method Post -Uri http://127.0.0.1:${port}/api/jobs -ContentType application/json -Body '${body.replace(/'/g, "''")}'`,
    via: 'api',
    why: square ? 'The prompt has quotes, which Windows PowerShell mangles on the command line, so this goes to the engine’s API.' : `The CLI can’t set the size yet, so this asks the engine’s API for ${aspect}.`,
    studio,
  };
}
