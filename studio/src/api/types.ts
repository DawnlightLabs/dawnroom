// Shapes from engine/API.md. Keep the two in step.

export type JobStatus = 'queued' | 'running' | 'completed' | 'failed' | 'cancelled' | 'deleted';

export type Task = 'generate' | 'edit';

export interface JobParams {
  prompt: string;
  negative_prompt: string;
  width: number;
  height: number;
  seed: number;
  steps: number;
  cfg: number;
  /** Missing on jobs from before edits: generate. */
  task?: Task;
  /** Auto's one-line reason, when Auto picked the model. */
  auto?: string;
  /** An edit's input hashes: the image, then the references. */
  inputs?: string[];
}

export interface Job {
  id: string;
  kind: string;
  status: JobStatus;
  prompt: string;
  params: JobParams;
  seed: number;
  model: string;
  created_ms: number;
  started_ms: number | null;
  finished_ms: number | null;
  output_path: string | null;
  error: string | null;
  frame: number;
  favourite: boolean;
  /** "Edited from": the Library image an edit's input was. */
  parent_id?: string | null;
}

export type Stage = 'installing_runtime' | 'starting' | 'switching_model' | 'preparing' | 'vae_encode' | 'conditioning' | 'sampling' | 'vae_decode' | 'encoding';

export interface Progress {
  stage: Stage | (string & {});
  step: number | null;
  total: number | null;
}

export interface JobView extends Job {
  progress: Progress | null;
}

export interface ImageItem extends Job {
  url: string;
}

export interface ImagePage {
  items: ImageItem[];
  total: number;
  limit: number;
  offset: number;
}

/** What `POST /api/jobs` accepts. Only `prompt` is required. */
export interface JobRequest {
  prompt: string;
  negative_prompt?: string;
  width?: number;
  height?: number;
  aspect?: string;
  seed?: number;
  preset?: string;
  /** `auto` (the default when missing), a model id, or a preset id. */
  model?: string;
  steps?: number;
  cfg?: number;
  task?: Task;
  /** sha256 of the stored input (POST /api/inputs). */
  image?: string;
  refs?: string[];
}

/** POST /api/inputs. */
export interface StoredInput {
  sha256: string;
  format: string;
  width: number;
  height: number;
  bytes: number;
  /** The Library image with exactly these bytes, if it is one. */
  image_id: string | null;
}

export type AutoPick =
  | { ok: true; model: string; name: string; reason: string; loaded: boolean }
  | { ok: false; code: string; error: string; suggest: { model: string; name: string; download_bytes: number } | null };

/** GET /api/auto. */
export interface AutoAnswer {
  loaded: string | null;
  generate: AutoPick;
  edit: AutoPick;
}

/** A measured time to show for one task (`timing` in GET /api/models). */
export interface Timing {
  cold_s?: number | null;
  warm_s?: number | null;
  gpu: string;
  size?: string;
  /** Measured on this GPU (same name, same kind). */
  here: boolean;
  label: string;
}

export type JobEvent =
  | { type: 'snapshot'; job: JobView }
  | { type: 'started' }
  | ({ type: 'progress' } & Progress)
  | { type: 'preview'; step: number; total: number; b64_png: string }
  | { type: 'finished'; job: Job };

export interface Gpu {
  name: string;
  vram_total_mib: number;
  vram_used_mib: number;
}

export type ModelState = 'installed' | 'pulling' | 'verifying' | 'partial' | 'not_installed' | 'needs_test' | 'test_failed';
/** One engine formula (engine/src/fit.rs): fits in VRAM, runs with RAM offload (slower), or needs more memory. */
export type Fit = 'fits' | 'offload' | 'needs_more_memory' | 'no_gpu';
/** How much a size tag has been tried: measured by Dawnroom, by the community (6d), or not yet. */
export type Tier = 'tested' | 'community' | 'untested';

/** The numbers behind `fit`. `gpu_mib` is VRAM, or an integrated GPU's shared budget. */
export interface FitDetail {
  tier: Fit;
  need_mib: number | null;
  gpu_mib: number | null;
  usable_ram_mib: number | null;
  /** Not measured on this kind of GPU: file sizes + compute reserve, or a discrete card's peak for an iGPU. */
  estimate?: boolean;
}

export interface Licence {
  spdx: string;
  /** The licence's own name from the model card. */
  name?: string;
  url: string;
  /** The official repo is gated: the user accepts the licence in Dawnroom before the download. */
  accept_required?: boolean;
  commercial: boolean;
  use_restrictions: boolean;
  note?: string;
}

export interface Measured {
  gpu: string;
  /** Missing = generate. */
  task?: Task;
  /** "1024x1024". */
  size?: string;
  vram_total_mib: number;
  peak_vram_mib: number;
  cold_s?: number;
  warm_s?: number;
  note?: string;
  /** Runtime build it ran on (e.g. windows-vulkan); missing = windows-cuda12. */
  runtime?: string;
  /** Measured on integrated graphics (shared memory). */
  integrated?: boolean;
  /** Memory needed with nothing offloaded, when measured with offload. */
  need_mib?: number;
  peak_ram_mib?: number;
  offload?: 'auto-fit' | 'off';
}

export interface ModelFile {
  path: string;
  role: string;
  size: number;
  status: { state: string; bytes?: number; reason?: string };
}

export interface PullProgress {
  type: 'progress';
  file: string;
  file_index: number;
  file_count: number;
  /** `unpacking` only for runtime files (`file` = `runtime/<zip>`), which come first in a pull. */
  stage: 'present' | 'verifying' | 'downloading' | 'waiting' | 'unpacking' | 'testing';
  bytes: number;
  total: number;
  speed_bps: number;
  /** Set while `stage` is `waiting`: network trouble, the engine retries by itself. */
  wait?: PullWait | null;
}

export interface PullWait {
  kind: 'connection' | 'busy' | 'server';
  /** Plain words: "No connection", "Hugging Face is busy". */
  reason: string;
  /** When the next attempt starts (ms since the epoch). */
  retry_at_ms: number;
  attempt: number;
}

/** A download that stopped (cancel, crash, reboot) and waits for the user to resume it. */
export interface Paused {
  bytes: number;
  total: number;
  updated_ms: number | null;
}

export type PullEvent = PullProgress | { type: 'done'; model: string } | { type: 'error'; code: string; message: string };

export interface Model {
  /** `<family>:<tag>`, e.g. `z-image-turbo:q4`. */
  id: string;
  family: string;
  tag: string | null;
  name: string;
  tagline: string;
  tags: string[];
  licence: Licence;
  /** When the user accepted the licence here; null = not yet (only matters with `accept_required`). */
  licence_accepted_ms: number | null;
  original: string | null;
  tier: Tier;
  /** What it can do; missing on older engines = generate. */
  tasks?: Task[];
  /** Tested / community / untested per task. */
  tiers?: Partial<Record<Task, Tier>>;
  /** Per task, the measured time to show, or null ("no timing yet"). */
  timing?: Partial<Record<Task, Timing | null>>;
  /** Fit per task (an edit can peak higher than a generate). */
  task_fit?: Partial<Record<Task, FitDetail>>;
  /** Set for custom models: "Custom, not reviewed by Dawnroom". */
  label?: string | null;
  custom?: { source: string; architecture?: string; sd_version?: string; base?: string; added_ms?: number; test?: TestResult | null } | null;
  fixed_steps?: boolean;
  state: ModelState;
  fit: Fit;
  fit_detail: FitDetail;
  size_bytes: number;
  download_bytes: number;
  /** What a pull still puts on disk: the model plus the runtime if it isn't installed yet. */
  needed_bytes: number;
  presets: string[];
  loaded: boolean;
  defaults: { steps: number; cfg: number; width: number; height: number };
  measured: Measured[];
  recipe?: string;
  files: ModelFile[];
  pull: PullProgress | null;
  pull_error: { type: 'error'; code: string; message: string } | null;
  /** Never resumed on its own: the user may be on a metered connection now. */
  paused?: Paused | null;
}

export interface DiskSpace {
  free_bytes: number;
  total_bytes: number;
}

export interface ModelList {
  gpu: Gpu | null;
  /** Free space on the data drive (null if the engine can't tell). */
  disk: DiskSpace | null;
  /** Bytes the image runtime still needs on disk (0 once installed). */
  runtime_needed_bytes: number;
  /** The image runtime's own paused download (null when none). */
  runtime_paused?: Paused | null;
  /** One row per model: its tags (smallest first) and the one a bare id means on this machine. */
  families: { id: string; default: string | null; variants: string[] }[];
  models: Model[];
}

export type ChoiceStatus = 'ok' | 'driver_too_old' | 'no_driver' | 'no_gpu' | 'unsupported_os' | 'glibc_too_old';

/** GET /api/hardware, the parts Studio uses. */
export interface Hardware {
  os: string;
  gpus: { name: string; vendor: string; integrated: boolean; vram_mib: number; shared_mib: number; driver: string | null }[];
  ram: { total_bytes: number; available_bytes: number };
  choice: {
    gpu_index: number | null;
    build: string | null;
    experimental: boolean;
    status: ChoiceStatus;
    reason: string;
    message?: string;
    link?: string;
  };
}

export interface Preset {
  id: string;
  label: string;
  model: string;
  default: boolean;
  state: ModelState | null;
  fit: Fit | null;
}

export interface Aspect {
  id: string;
  width: number;
  height: number;
}

export interface PresetList {
  presets: Preset[];
  aspects: Aspect[];
}

export interface Health {
  name: string;
  version: string;
  backend: 'stopped' | 'loading' | 'ready' | 'busy';
  model: string | null;
  busy: boolean;
  /** The image runtime in use, e.g. `windows-cuda12`. */
  runtime?: string;
  gpu: Gpu | null;
}

export interface Info {
  name: string;
  version: string;
  port: number;
  exe: string | null;
  data_dir: string;
  models_dir: string;
  outputs_dir: string;
  logs_dir: string;
  models_bytes: number;
  outputs_bytes: number;
}

/** GET /api/storage (engine/API.md). Sizes in bytes; hardlinked files counted once. */
export interface StorageReport {
  data_dir: string;
  drive: DiskSpace | null;
  usage: { models: number; images: number; runtimes: number; cache: number; partial: number; other: number; total: number };
  models: { id: string; name: string; complete: boolean; bytes: number; shared: string[]; last_used_ms: number | null }[];
  partials: { file: string; owner: string; bytes: number; total: number | null }[];
  cache: { kind: 'thumbnails' | 'rotated_log' | 'old_runtime' | 'unpack_leftover' | 'runtime_zip'; path: string; bytes: number }[];
  move: MoveProgress;
}

export interface MoveProgress {
  state: 'idle' | 'copying' | 'verifying' | 'switching' | 'restarting' | 'failed';
  bytes: number;
  total: number;
  from?: string | null;
  to?: string | null;
  error?: string | null;
}

/** POST /api/models/custom/check: the compatibility check, step by step. */
export interface CheckStep {
  step: 'source' | 'format' | 'architecture' | 'companions' | 'size' | 'licence' | (string & {});
  status: 'ok' | 'failed' | 'skipped';
  message: string;
  code?: string | null;
  detail?: { files?: { file: string; size?: number; bytes?: number }[] } & Record<string, unknown>;
}

export interface CheckReport {
  ok: boolean;
  source: string;
  id: string | null;
  name: string | null;
  label: string | null;
  steps: CheckStep[];
  code: string | null;
  message: string | null;
  download_bytes?: number;
  size_bytes?: number;
  /** The repo as Hugging Face describes it (absent for a local file). */
  repo?: { id: string; author: string | null; base_model: string | null; licence: string | null; downloads: number | null; likes: number | null; url: string } | null;
  /** The file this report checked. */
  file?: string | null;
  /** A repo's model files, when it has more than one. */
  files?: FileOption[];
  /** Read from the header ("Qwen-Image 2.1"). */
  architecture?: string | null;
  companions?: { file: string; role: string; size: number; on_disk: boolean }[];
  fit?: FitDetail | null;
  licence?: Licence | null;
}

/** A custom model's test image: `job` serves it at /api/images/{job}/file. */
export interface TestResult {
  ok: boolean;
  at_ms: number;
  message: string;
  seconds?: number;
  job?: string;
  width: number;
  height: number;
  steps: number;
}

/** POST /api/models/custom/local: image models already on this computer. */
export interface LocalScan {
  roots: { path: string; label: string; exists: boolean }[];
  found: {
    path: string;
    file: string;
    /** "ComfyUI", "Hugging Face cache · user/repo", "This folder"… */
    place: string;
    size: number;
    architecture: string | null;
    ok: boolean;
    message: string | null;
    fit: Fit | null;
    download_bytes: number;
    /** Already added as this custom model. */
    added: string | null;
    /** The same file as this library model. */
    library: string | null;
  }[];
  /** Model-sized files that aren't image models. */
  skipped: number;
}

/** One size in a repo with several model files. */
export interface FileOption {
  file: string;
  /** "Q4_K_M", "BF16", "INT8 ConvRot comfy"… */
  quant: string;
  bits: number | null;
  format: 'gguf' | 'safetensors';
  size: number;
  /** The pinned sd.cpp can load this format (by its name; the header check has the last word). */
  loadable: boolean;
  reason?: string;
  /** Estimated fit here, for files that can load. */
  fit: Fit | null;
  recommended: boolean;
  selected: boolean;
}

/** A drive the data folder could live on (GET /api/storage/drives). */
export interface Drive {
  root: string;
  label: string | null;
  free_bytes: number;
  total_bytes: number;
  /** Where the data folder would go on this drive. */
  path: string;
  /** The data folder is on this drive now. */
  current: boolean;
}

export interface DrivesAnswer {
  data_dir: string;
  drives: Drive[];
}
