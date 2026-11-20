# Engine HTTP API

The engine listens on `http://127.0.0.1:8420` (`--port` to change). Localhost only, no auth.
Every response is JSON unless noted. Studio, the CLI and the MCP server are all clients of this API.

## Local security

No auth, so every request (all routes, including 404s and OPTIONS) passes three checks first:

| check | rule | refusal |
|---|---|---|
| Host | must be `127.0.0.1:<port>` or `localhost:<port>` (stops DNS rebinding) | 403 `forbidden_host` |
| Origin | absent (CLI, MCP, curl), or one of `tauri://localhost`, `http://tauri.localhost`, `https://tauri.localhost` (Studio) and `http://localhost:5173` (Vite dev) | 403 `forbidden_origin` |
| Content-Type | every POST needs `application/json` (parameters like `; charset=utf-8` are fine), **even with no body**; the one exception is `POST /v1/images/edits`, which also takes `multipart/form-data` (OpenAI's file upload) | 415 `unsupported_media_type` |

CORS: allowed origins get `Access-Control-Allow-Origin: <origin>` (and `Vary: Origin`) on every
response, errors included. A preflight (`OPTIONS` with `Access-Control-Request-Method`) from an
allowed origin gets 204 with `GET, POST, DELETE` and the `content-type` header allowed.

## Errors

Every engine endpoint (except the OpenAI one) fails with the same shape:

```json
{ "error": "Z-Image Turbo isn't downloaded yet. Pull z-image-turbo first.", "code": "model_not_installed" }
```

`error` is plain words ending in what to do next; branch on `code`.

| code | HTTP | meaning |
|---|---|---|
| `invalid_json` | 400 | body isn't valid JSON for the endpoint (includes unknown fields) |
| `invalid_request` | 400 | e.g. empty prompt, bad size |
| `unknown_model` | 400 | no such model id or preset |
| `task_not_supported` | 400 | the model can't do this task (an edit on a generate-only model, or text-to-image on an edit-only one); `error` lists the edit models |
| `invalid_image` | 400 | an input that isn't a PNG, JPEG or WebP, is over 50 MB, or a side over 8192 px |
| `input_not_found` | 404 | a job names an input sha256 that `POST /api/inputs` never stored |
| `no_model_for_task` | 409 | Auto found no installed, tested model for the task that runs here; `error` names the model to pull and its size (see Auto) |
| `not_found` | 404 | no such job, image or model |
| `model_not_installed` | 409 | pull the model first |
| `model_not_ready` | 409 | a custom model whose test image hasn't run or failed: pull it (again) to run the test |
| check codes (`gated`, `unsupported_quant`, `pickle`, `not_image_model`, `lora`, …) | 409 | a pull or import of a custom model whose compatibility check failed; the body carries the whole `check` report (see Custom models) |
| `licence_not_accepted` | 409 | the model's official repo is gated: the user accepts its licence first (see Licence acceptance) |
| `model_busy` | 409 | e.g. delete while jobs run or the model is downloading |
| `already_finished` | 409 | cancel on a finished job |
| `not_pulling` | 409 | cancel-pull when nothing is downloading |
| `needs_more_memory` | 409 | a job for a model this machine can't hold even with RAM offload; `error` gives both numbers |
| `moving`, `busy` | 409 | the data folder is moving (every write is refused until it's done), or a move was asked for while jobs or downloads run |
| `folder_not_empty` | 400 | a move to a folder that already holds files |
| `forbidden_host`, `forbidden_origin` | 403 | see Local security |
| `unsupported_media_type` | 415 | POST without `Content-Type: application/json` |
| `no_gpu` | 503 | no usable GPU found (see `GET /api/hardware`) |
| `driver_too_old`, `no_driver` | 503 | the NVIDIA driver is older than the CUDA runtime needs, or isn't answering; `error` ends with the driver page link |
| `unsupported_os`, `no_runtime` | 503 | there's no runtime for this OS (macOS in v0.1) |
| `not_enough_space` | 507 | a download (model and/or runtime) won't fit; `error` gives both numbers |
| `checksum_mismatch`, `download_failed`, `download_refused`, `network_intercepted`, `disk_full`, `disk_error`, `runtime_broken` | 500 | a runtime install failed (same codes as pulls, see Downloads) |
| anything else | 500 | e.g. `db_error`, `file_missing` (410) |

## Jobs

### `POST /api/jobs` → 202

```json
{ "prompt": "a lighthouse at dawn", "preset": "fast", "seed": 42 }
```

All fields but `prompt` are optional: `negative_prompt`, `width`, `height` (multiples of 8, 256–2048),
`aspect` (`1:1` 1024×1024, `4:3` 1152×864, `3:4` 864×1152, `3:2` 1248×832, `2:3` 832×1248, `16:9` 1360×768,
`9:16` 768×1360; about 1 MP, what the models were measured at; not together with `width`/`height`),
`seed` (missing or negative = random), `model` (`auto`, the default: see Auto; a model id, `z-image-turbo:q4`,
or a bare one, `z-image-turbo`, which means the default tag for this machine, see Model ids; or a preset id),
`preset` (`fast` | `quality`: aliases kept for old clients, each pinned to one model; `model` wins),
`task` (`generate` | `edit`; missing = `edit` when `image` is set, else `generate`), `image` and `refs` (edits,
below), `steps` (1–50) and `cfg` (guidance, 0–20). Steps and guidance
default to the model's manifest, which is what it was tuned and measured at; the sampler, scheduler,
distilled guidance, flow shift and any fixed sigma schedule always come from the manifest (from sd-server's
built-in recipe for that model). The job's `model` is always the full id it ran on.

**Edits.** Store each input image with `POST /api/inputs` first, then send its `sha256`:

```json
{ "prompt": "change the text on the chalkboard signs to 'OPEN LATE'", "image": "7e7e5004…", "refs": ["e81ffe6c…"] }
```

`image` is the image to edit; `refs` are up to 9 reference images ("put the hat from image 2 on the cat": the
edited image is image 1). Every edit model in the library takes them all as sd-server reference images
(`ref_images`). Without `width`/`height`/`aspect` the output keeps the input's aspect ratio at about the model's
own area (multiples of 16). The input hashes are recorded in `params.inputs` and in the PNG's recipe (`task`,
`inputs`); the files stay in `<data>/inputs/`. If the input's bytes are a Library image's PNG (same sha256 as an
output), the job's `parent_id` is that image: "edited from".

Not supported (sd-server at the pinned build has them, but none of the library's edit models uses them):
`mask_image` (inpainting) and `init_image` + `strength` (img2img).

Response: the job record.

```json
{ "id": "826085f4-…", "kind": "image", "status": "queued", "prompt": "a lighthouse at dawn",
  "params": { "prompt": "…", "negative_prompt": "", "width": 1024, "height": 1024, "seed": 42, "steps": 4, "cfg": 1.0 },
  "seed": 42, "model": "flux2-klein-4b:q8", "created_ms": 1763050000000,
  "started_ms": null, "finished_ms": null, "output_path": null, "error": null,
  "frame": 248, "favourite": false, "parent_id": null }
```

`params.task` is `generate` or `edit`; `params.auto` is Auto's one-line reason when Auto picked the model
(absent when a model or preset was named); `params.inputs` lists an edit's input hashes.

`frame` counts every Library job ever submitted, like frames on a roll of film; it is never reused, so the
library has gaps where jobs failed or images were deleted. A custom model's hidden test image takes no number
(`frame: 0`). `favourite` is set with the endpoint below.

`status`: `queued` → `running` → `completed` | `failed` | `cancelled`. One job runs at a time (FIFO).

### `GET /api/jobs/{id}`

The job record plus `progress` (`null` unless running):
`{ …job, "progress": { "stage": "sampling", "step": 3, "total": 8 } }`

### `GET /api/jobs/{id}/events` (SSE)

| event | data |
|---|---|
| `snapshot` | the `GET /api/jobs/{id}` body; the stream ends here if the job is already finished |
| `started` | `{ "type": "started" }` |
| `progress` | `{ "type": "progress", "stage": "sampling", "step": 3, "total": 8 }` |
| `preview` | `{ "type": "preview", "step": 3, "total": 8, "b64_png": "…" }` (128×128) |
| `finished` | `{ "type": "finished", "job": { …job } }`, then the stream closes |

Stages, in order: `installing_runtime` (only if the runtime this machine needs isn't installed yet;
normally the pull installed it), `starting` (cold start) or `switching_model` (another model was loaded; the engine
restarts sd-server with the new files), then `conditioning`, `sampling` (step n of total),
`vae_decode`, `encoding`. Edits add `preparing` and `vae_encode` (reading the input images) before
`conditioning`. A step-`total` sampling event may be skipped.

### `POST /api/inputs` → 201

An edit input: `{ "path": "C:\\photos\\cat.png" }` (a file on this computer: CLI, MCP) or `{ "b64": "…" }` (base64, a
`data:` URL prefix is fine). PNG, JPEG or WebP, at most 50 MB and 8192 px a side. Stored once, by the sha256 of
its bytes, in `<data>/inputs/` (never sent anywhere). Response:

```json
{ "sha256": "7e7e5004…", "format": "png", "width": 1024, "height": 1024, "bytes": 1512345, "image_id": "ed46854b-…" }
```

`image_id`: the Library image with exactly these bytes, if it is one (an edit of it gets that `parent_id`).
Errors: 400 `invalid_image`, 404 `not_found` (no such file).

### `GET /api/inputs/{sha256}` → 200

The stored input exactly as it was sent (`image/png`, `image/jpeg` or `image/webp`, cached for a year since the
id is its hash). Studio shows a dropped file's thumbnail from it. 404 `input_not_found`.

### Auto: `GET /api/auto`

Auto is the default model choice for every job (`model` missing or `auto`). Per task it picks only models
that are installed, curated (never custom), **tested by Dawnroom for that task** (a `measured` entry with that
`task`), able to do it, and that fit or run with RAM offload here. It never downloads. Then:

1. the model that will be loaded when the job runs (the last queued job's model, else sd-server's) if it
   qualifies and fits on the GPU (or nothing that fits qualifies): no needless switch, but a loaded
   RAM-offload model never blocks a much faster one that fits;
2. otherwise fits before RAM offload, commercially clean before restricted, the ranking in `engine/src/auto.rs`
   (`RANK`: generate = Z-Image Turbo, FLUX.2 klein 4B, …; edit = FLUX.2 klein 4B, then the rest by measured speed), the bigger tag, the id.

Same inputs and state, same pick. The answer, for both tasks:

```json
{
  "loaded": "flux2-klein-4b:q8",
  "generate": { "ok": true, "model": "flux2-klein-4b:q8", "name": "FLUX.2 klein 4B", "reason": "FLUX.2 klein 4B: already loaded, fastest, about 10 s here", "loaded": true },
  "edit": { "ok": false, "code": "no_model_for_task", "error": "No tested model for editing is installed. Pull FLUX.2 klein 4B (7.1 GB): `dawnroom pull flux2-klein-4b:q8`.", "suggest": { "model": "flux2-klein-4b:q8", "name": "FLUX.2 klein 4B", "download_bytes": 7134122064 } }
}
```

`suggest`: the best tested, commercially clean, ungated model that runs here, or `null`. A job sent with
Auto when nothing qualifies fails with 409 `no_model_for_task` and the same `error`.

### `POST /api/jobs/{id}/cancel` → 202

`{ "id": "…", "cancel_requested": true }`. A queued job is dropped before it runs; a running one stops
at sd-server's next checkpoint (about a second). Watch the events or poll for `status: "cancelled"`.
A finished job returns 409 `already_finished`.

## Images

### `GET /api/images?limit=50&offset=0&q=lighthouse&favourite=true`

Completed images, newest first: `{ "items": [ { …job, "url": "/api/images/{id}/file" } ], "total": 12, "limit": 50, "offset": 0 }`

`q` finds text in the prompt (case-insensitive); `favourite=true` keeps only favourites. `total`
counts the matches. `limit` is capped at 200.

### `POST /api/images/{id}/favourite`

`{ "favourite": true }` → `{ "id": "…", "favourite": true }`. 404 `not_found` if there's no such image.

### `DELETE /api/images/{id}`

`{ "id": "…", "deleted": true }`. Removes the PNG from the outputs folder and the image from the
library; its job record stays (status `deleted`) so the frame number isn't handed out again.
404 `not_found` if there's no such image; 500 `disk_error` if the file can't be removed.

### `GET /api/images/{id}/file?w=512`

The PNG (`image/png`), with the recipe in iTXt chunks (`prompt`, `model`, `seed`, `steps`, `time`, `size`).
With `w` (64–1024): a JPEG thumbnail no larger than `w` on its longest side, for grids. Thumbnails
are made on first request and kept in `<data>/thumbs/`; that folder is safe to delete. Both are
served with a long `Cache-Control`, since an image never changes once made.

## Models

### Model ids

A model comes in size tags (`q4`, `q8`, `bf16`, `fp16`, `int8`…), each its own download with its own
measurements and fit. The full id is `<model>:<tag>` (`z-image-turbo:q4`); every endpoint below also takes
the bare model id (`z-image-turbo`), which means the **default tag for this machine**: the largest tag already
installed that runs here (so a bare id never starts a surprise download), else the largest tag that `fits`
by the fit formula, else the smallest. Tags share text encoders and VAEs, so files already on disk count
for every tag that uses them. Jobs recorded before size tags have bare ids and resolve the same way.

Custom models (see Custom models) have ids `hf.co/<user>/<repo>:<file>` (the repo in Hugging Face's own
spelling) and `local/<name>` for imported files. In a URL path the id is **one percent-encoded segment**
(`encodeURIComponent`): `POST /api/models/hf.co%2Funsloth%2FZ-Image-Turbo-GGUF%3Az-image-turbo-Q3_K_M.gguf/pull`.
A bare `hf.co/<user>/<repo>` means the custom file from that repo, when there's one.

### `GET /api/models`

```json
{
  "gpu": { "name": "NVIDIA GeForce RTX 5060 Laptop GPU", "vram_total_mib": 8151, "vram_used_mib": 0 },
  "disk": { "free_bytes": 255194259456, "total_bytes": 472534478848 },
  "runtime_needed_bytes": 2179219548,
  "runtime_paused": null,
  "families": [ { "id": "z-image-turbo", "default": "z-image-turbo:q4", "variants": ["z-image-turbo:q4", "z-image-turbo:q8"] } ],
  "models": [
    {
      "id": "z-image-turbo:q4", "family": "z-image-turbo", "tag": "q4",
      "name": "Z-Image Turbo", "tagline": "…", "tags": ["quality", "text", "photo"],
      "original": "Tongyi-MAI/Z-Image-Turbo", "recipe": "z-image-turbo",
      "licence": { "spdx": "Apache-2.0", "name": "Apache License 2.0", "url": "https://huggingface.co/Tongyi-MAI/Z-Image-Turbo", "commercial": true, "use_restrictions": false, "accept_required": false },
      "licence_accepted_ms": null,
      "tier": "tested",
      "tasks": ["generate"],
      "tiers": { "generate": "tested" },
      "timing": { "generate": { "cold_s": 27.4, "warm_s": 19.0, "gpu": "NVIDIA GeForce RTX 5060 Laptop GPU", "size": "1024x1024", "here": true, "label": "on your GPU" } },
      "state": "installed",
      "fit": "fits",
      "fit_detail": { "tier": "fits", "need_mib": 4376, "gpu_mib": 8151, "usable_ram_mib": 17815 },
      "task_fit": { "generate": { "tier": "fits", "need_mib": 4376, "gpu_mib": 8151, "usable_ram_mib": 17815 } },
      "size_bytes": 6696835812, "download_bytes": 0, "needed_bytes": 0,
      "presets": ["quality"], "loaded": true,
      "defaults": { "steps": 8, "cfg": 1.0, "width": 1024, "height": 1024 },
      "fixed_steps": false,
      "measured": [ { "gpu": "NVIDIA GeForce RTX 5060 Laptop GPU", "vram_total_mib": 8151, "peak_vram_mib": 4376, "cold_s": 27.4, "warm_s": 19.0, "note": "…" } ],
      "files": [ { "path": "diffusion_models/z_image_turbo-Q4_K.gguf", "role": "diffusion_model", "size": 3864250304, "status": { "state": "verified" } } ],
      "pull": null,
      "pull_error": null,
      "paused": null
    }
  ]
}
```

- `families`: one row per model, its tags smallest first, and `default`, the tag a bare id means here.
  `models` has one entry per tag, in the same order.
- `tier`: `tested` (Dawnroom measured it: a `measured` entry without `source`), `community` (only contributed
  measurements, `source: "community"`; Slice 6d) or `untested`. Derived from `measured`, never written by hand.
- `original`: the official repo; the files may come from mirrors (`files[].path` is local). `recipe`: the
  sd-server built-in recipe its defaults come from.
- `tasks`: `generate` and/or `edit`. `tiers`: tested / community / untested per task, from that task's
  measurements only (`measured[].task`, missing = `generate`); Auto uses only `tested`.
- `timing`: per task, the measured time to show here, or `null` ("no timing yet"; never estimated):
  `cold_s`, `warm_s`, the `gpu` and `size` (`1024x1024`) it was measured at, and `here: true` with
  `label: "on your GPU"` when it was this GPU (same name, same kind), else `label: "on <gpu>"`. A measurement on
  the same kind of GPU (discrete or integrated) is preferred, ours before community ones.
- `task_fit`: the fit per task. `fit`/`fit_detail` are the model's main task's (generate if it can, else
  edit): an edit holds input images too and can peak higher, so each task's fit uses its own measurements.
- `fixed_steps: true`: the model has a fixed sigma schedule (Qwen-Image 2.1 Turbo), so `steps` is ignored.
- `gpu` is the GPU the engine uses (see `GET /api/hardware`), `null` without one. For an integrated GPU,
  `vram_total_mib` is its shared budget.
- `disk`: free space on the data drive (`null` if it can't be read). `runtime_needed_bytes`: what the image
  runtime still needs on disk at its peak (zip + unpacked; 0 once installed or with `--sd-server`).
- `state`: `installed` | `pulling` | `verifying` (hashing after start-up) | `partial` | `not_installed`.
- `fit`: `fits` | `offload` (runs with part of the weights in system RAM, slower) | `needs_more_memory` |
  `no_gpu`. One formula, shared with the site (`engine/src/fit.rs`, `site/src/lib/fit.ts`): need = the
  tag's measured `need_mib` (measured with offload) or else its peak VRAM; for a tag nobody measured, an
  **estimate**: the sum of its file sizes + 2048 MiB compute reserve. `fits` if need + 512 MiB ≤ VRAM,
  `offload` if need + 512 MiB ≤ VRAM + (system RAM − 6 GiB), else `needs_more_memory`. An integrated GPU's
  "VRAM" is its shared budget and it has no `offload` tier; it uses integrated measurements, else the discrete
  need, else the file estimate.
- `fit_detail`: the numbers behind `fit` (`need_mib`, `gpu_mib`, `usable_ram_mib`) and `estimate: true` when
  the need isn't measured on this kind of GPU. Show estimates as estimates ("Should fit (estimate)",
  "Runs with RAM offload (estimate)"), never as measured.
- Every model starts sd-server with `--auto-fit on`: sd.cpp places weights on the GPU, then RAM, then disk by
  the memory that's free, so the driver never spills on its own. `offload` models use exactly this.
- `download_bytes`: what a pull would still download (resumable `.part` bytes count as done).
- `needed_bytes`: what a pull still puts on disk: `download_bytes` plus the runtime if it isn't installed.
  A pull refuses with 507-style `not_enough_space` (as a stream `error` event) when this plus 500 MB
  spare doesn't fit.
- `measured[]` entries may carry `runtime` (the build, e.g. `windows-vulkan`; missing = `windows-cuda12`),
  `integrated: true` (an integrated GPU: `vram_total_mib` is its shared budget; never used for a discrete card),
  and for runs that offloaded: `need_mib` (memory needed with nothing offloaded), `peak_ram_mib` (how far free
  RAM dropped) and `offload` (`auto-fit` or `off`).
- File `status.state`: `missing` | `partial` (`bytes`) | `unverified` | `verifying` | `verified` | `corrupt` (`reason`).
- `licence.spdx` is an SPDX id, or `LicenseRef-…` when the licence isn't on the SPDX list; `name` is the
  licence's own name from the model card; `note` is optional. `commercial: true` only when commercial use is
  allowed for everyone; revenue-capped licences (Stability AI Community, Krea 2 Community, FLUX Kommunity)
  are `commercial: false` with `use_restrictions: true` and the cap in `note`. `use_restrictions: true` means
  the licence limits use even where `commercial` is true (RealVisXL's OpenRAIL++); `note` says what. Show it
  next to the licence.
- `licence.accept_required: true`: the official repo is gated (Dawnroom downloads from an ungated mirror), so
  the user accepts the licence in Dawnroom first. `licence_accepted_ms` says when they did (`null` = not yet).
- `pull`: while pulling, the latest pull `progress` event (below), `null` until the first one and when not pulling.
  Poll this to show progress without holding a stream open.
- `pull_error`: how the last pull ended badly, `{ "type": "error", "code": "cancelled", "message": "…" }`,
  until the next pull starts; `null` otherwise.
- `paused`: a download that stopped (cancel, failure, engine crash, power cut, reboot) and waits for the user,
  `{ "bytes": 4100000000, "total": 13000000000, "updated_ms": 1794857077948 }`: bytes kept (what a resume
  starts from), of the model's total (plus the runtime's while that isn't installed), and when the `.part`
  was last written. Set while `state` is `partial` and nothing is pulling. **Never resumed automatically**
  (the user may be on a metered connection now): `POST …/pull` resumes, `POST /api/storage/partials/delete`
  discards. `runtime_paused` (top level) is the same for the image runtime's own `.part` zips.
- File `status.partial.bytes` and `download_bytes` count what a resume keeps (see Downloads), not the raw `.part` length.

### `POST /api/models/{id}/pull` (SSE)

| event | data |
|---|---|
| `progress` | `{ "type": "progress", "file": "vae/flux2-vae.safetensors", "file_index": 2, "file_count": 3, "stage": "downloading", "bytes": 120000000, "total": 336211292, "speed_bps": 41000000, "wait": null }` |
| `done` | `{ "type": "done", "model": "flux2-klein-4b" }` |
| `error` | `{ "type": "error", "code": "checksum_mismatch", "message": "…" }` |

A pull first installs the image runtime this machine needs, if it isn't installed: those events have
`file` = `runtime/<zip name>` and come first in `file_index`; they add the stage `unpacking`. Model files follow.

`stage` is `present` (already verified), `verifying` (hashing), `downloading`, `waiting` (network trouble; the
engine retries by itself) or `unpacking`. While `waiting`, `wait` is
`{ "kind": "connection", "reason": "No connection", "retry_at_ms": 1794856798911, "attempt": 2 }` (`kind`:
`connection` | `busy` | `server`), re-sent once a second so a countdown can follow it; `bytes` is what's kept.
Files download to `<name>.part`, resume with an HTTP Range request, and are renamed into place only after
their sha256 matches the manifest. Unknown model: 400 `unknown_model` (plain JSON, no stream). A model whose
licence hasn't been accepted: 409 (plain JSON, no stream, nothing downloaded):

```json
{ "error": "FLUX.1 dev is under the FLUX.1 [dev] Non-Commercial License (https://…/LICENSE.md). …",
  "code": "licence_not_accepted", "model": "flux1-dev:q4",
  "licence": { "name": "FLUX.1 [dev] Non-Commercial License", "spdx": "LicenseRef-FLUX-1-dev-Non-Commercial", "url": "https://…", "commercial": false, "note": "…" } }
```

### Licence acceptance

Some models' official Hugging Face repos are gated (you click "Agree" there). Dawnroom downloads those
models from reputable ungated mirrors and never uses an HF token, but it doesn't skip the terms: the user
accepts the original licence in Dawnroom before the first download.

`POST /api/models/{id}/licence/accept` → `{ "model": "flux1-dev", "licence_url": "https://…", "accepted_ms": 1794900000000 }`.
Stored in SQLite (`licence_acceptances`: model, licence URL, time). One acceptance covers every size tag of
the model; if the manifest's licence URL changes, the user is asked again. 400 `invalid_request` for a model
that doesn't need it, 404 for an unknown one.

**Only the user accepts.** Studio shows a licence sheet (name, link, note; "Accept and download"; Cancel
downloads nothing) and calls this after the click; the CLI's `pull` prints the licence and asks `[y/N]`
(`--accept-licence` answers yes; without a terminal or with `--json` it refuses). The MCP server never
calls this endpoint: `pull_model` returns `licence_not_accepted` in plain words, and its description tells
agents to ask the user to accept in Studio or the CLI.

**The engine owns the pull.** Closing the stream only detaches: the download carries on (a Studio
reload doesn't kill a 7 GB pull). A POST during a running pull joins it, so that is also how to
re-attach. Only the cancel endpoint below stops it. Error codes: `download_failed` (connection interrupted
for 15 minutes; says how much is saved), `download_refused` (401/403/404 from the server), `network_intercepted`
(a captive portal, proxy or firewall answered instead of the file), `checksum_mismatch`, `disk_full` (the
message gives what's still needed and what's free; the `.part` is kept), `disk_error`, `verify_failed`,
`cancelled`; each also shows as `pull_error` in `GET /api/models`.

### Downloads

One downloader serves model pulls (Hugging Face) and runtime installs (GitHub), `engine/src/download.rs`:

- **Retry.** Connect errors, read timeouts (30 s without a byte; Windows keeps a socket open through a Wi-Fi drop), a body that breaks off, a download that ends
  early, 5xx and 429 are retried after 2 s, 4 s, 8 s… capped at 60 s; 429 and 503 honour `Retry-After`
  (seconds). Every attempt resumes from the `.part` with a Range request. It gives up with `download_failed`
  after 15 minutes of failures without a byte of progress (progress resets the clock and the backoff):
  "Connection interrupted (no connection for 15 minutes). Pull again to continue the download; 76% of
  ae.safetensors is saved (255 MB of 335 MB)."
  401/403/404, checksum mismatches, a full disk and refused responses fail at once.
- **Response check, before a byte is written.** Refused with `network_intercepted`, leaving the `.part`
  untouched: `Content-Type: text/html`; a 200 whose `Content-Length`, or a 206 whose `Content-Range` total,
  isn't the manifest's size; a redirect that ends on a host other than the manifest URL's,
  `huggingface.co`, `hf.co`, `github.com`, `githubusercontent.com` or their subdomains. 416 (the `.part` is
  longer than the file) and a 206 at the wrong offset start that file from zero.
- **Power-cut-safe `.part`.** Next to each `<file>.part` sits `<file>.part.json`:
  `{ "url", "size", "sha256", "synced_bytes", "updated_ms" }`. The `.part` is `sync_all`ed every 64 MB or 5 s,
  and on cancel, error and finish; the sidecar is updated after each sync (written to a temp file, synced,
  renamed). A resume trusts only `synced_bytes`: the `.part` is truncated to it before the Range request.
  No sidecar (a `.part` from before sidecars) keeps its length minus the last 64 MB; a sidecar for another
  url, size or sha256, or an unreadable one, starts from zero.
- **Keep awake (Windows).** While any download runs or waits, the engine holds
  `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)` (the display may still sleep). Linux: nothing in
  v0.1; the retry covers a sleep.

### `POST /api/models/{id}/pull/cancel` → 202

`{ "id": "flux2-klein-4b:q8", "cancel_requested": true }`. The pull stops within about 100 ms (also
mid-wait) and syncs and keeps the `.part`; pulling again resumes it. Attached streams get `error` with code `cancelled`.
409 `not_pulling` if nothing is downloading; 404 `not_found` for an unknown model. The CLI's `pull`
calls this on Ctrl-C.

### `DELETE /api/models/{id}`

`{ "id": "flux2-klein-4b:q8", "deleted": ["diffusion_models/flux-2-klein-4b-Q8_0.gguf"], "kept": [] }`

Deletes one tag (a bare id means the default tag). Files another installed model or tag also uses are kept. 409 `model_busy` while jobs run or the model downloads.
Stops sd-server first if it holds this model.

### Custom models

Ollama style: any public, ungated Hugging Face repo or a model file on this computer. A source can be any
form people copy (`custom::parse_source`): `user/repo` (Hugging Face's copy button), the page
`https://huggingface.co/user/repo` (± `www.`, `/tree/<rev>`, a query), a file link `…/blob/<rev>/<file>` or
`…/resolve/<rev>/<file>` (the revision is ignored: the repo's current commit is pinned),
`hf.co/user/repo[:<file>]`, or a path (an existing file wins, so a relative `models/x.gguf` stays a path). Nothing is marked ready
before a **compatibility check** passes. Its steps run in this order and stop at the first failure, each
with a plain-words reason (`engine/src/custom.rs`, `arch.rs`, `header.rs`):

1. `source`: the repo is public and ungated (gated: "This repo needs a Hugging Face login, which Dawnroom
   doesn't support yet."); one weight file is checked. A repo with several model files lists them in `files`
   (below) and, without a named file, the **recommended** one is checked: the largest that fits on the GPU,
   else the smallest that runs with RAM offload. A named file in a format sd.cpp can't load fails `format`
   with `unsupported_quant`, suggesting the recommended file; a repo with nothing loadable fails
   `no_loadable_file`. A file that's byte-identical to one in the curated library (same LFS sha256) is
   refused with `in_library`, naming the curated id to pull instead.
2. `format`: `.gguf` or `.safetensors` only. `.ckpt`, `.bin`, `.pt`, `.pth`, `.pkl` are refused (`pickle`:
   loading them can run code); so are split models (`split_model`) and files whose bytes aren't what the
   extension says (`bad_header`).
3. `architecture`: from the **header only** (GGUF key/values and tensor infos, or the safetensors JSON
   header) read over HTTP Range, 256 KB first and more only if the header needs it (at most 64 MB), or from
   disk. The tensor names go through a line-by-line port of sd.cpp's `get_sd_version` at the pinned build,
   then to the curated family the model borrows from. Refusals: `not_image_model` (an LLM, a ControlNet, an
   upscaler), `text_encoder`, `vae_only`, `lora`, `video_model`, `unsupported_family` (inpainting, control and
   layered models; image families sd.cpp knows but the library has no model of), `unknown_architecture`.
4. `companions`: text encoders and VAE from that family's curated manifest (the variant whose companions are
   most on disk). Files already on disk are shared, not downloaded again; an all-in-one checkpoint keeps what
   it carries (an SDXL checkpoint still gets the library's fp16-fixed VAE).
5. `size`: new bytes (model, missing companions, runtime) against free disk (`not_enough_space`); memory fit
   **always an estimate** (files + 2048 MiB through the fit formula); `needs_more_memory` or `no_gpu` fail it.
6. `licence`: from the model card (`cardData.license`, `license_name`, `license_link`), shown as is; none
   stated: "No licence stated". Never a reason to refuse. Local files: the safetensors metadata's
   `modelspec.license`, else none.

Then, on the user's yes, the pull: the normal downloader (retry, resume, sidecar ownership, Discard rules)
and one small **test image** (at most 512 px a side, at most 8 steps, seed 42, "a red apple on a wooden
table, soft daylight") through the queue as a job of kind `test`, kept out of the Library. It passes when the
image develops and isn't one flat colour; the result (message, seconds, image path, size, steps) is saved in
the manifest. Pass: `state` `installed`. Fail: `test_failed`, files kept; pull again to retry, or delete.
Until it has passed, jobs for it are refused with `model_not_ready` (only the test runs).

Custom manifests are written to `<data>/custom/*.toml` (never `registry/`), loaded at start (a broken one is
left out with a log line, never fatal). Custom models are labelled **"Custom, not reviewed by Dawnroom"**
(`label`), are always tier `untested`, and are never used by presets (or Auto, 6b-3). In `GET /api/models`
they carry `custom` (`source`, `architecture`, `sd_version`, `base`, `added_ms`, `test`) and `label`;
`families[].custom` is true for them. No Hugging Face tokens: gated repos are refused.

#### `POST /api/models/custom/check`

Body: `{ "source": "<any form above, or a path>" }` (`{ "path": … }` still works). Runs steps 1–6,
downloading no weights. Always 200 with the report; 400 `invalid_source` for text that is neither a Hugging
Face model nor a path. Besides the steps, the report carries `repo` (`id`, `author`, `base_model`, `licence`
(the card's `license_name` for "other"), `downloads`, `likes`, `url`), `file` (the file checked),
`architecture`, `companions` (`file`, `role`, `size`, `on_disk`), and for a repo with more than one model file
`files`, smallest first: `{ file, quant, bits, format, size, loadable, reason?, fit, recommended, selected }`.
`quant` is the short name from the file name ("Q4_K_M", "BF16", "INT8 ConvRot comfy"; else the part the names
don't share); `bits` 1–32 or null; `loadable` is judged from the name for the pinned sd.cpp (not loadable:
bitsandbytes/NF4, NVFP4, torchao, ComfyUI-scaled FP8/MXFP8; the header check has the last word); `fit` is the
estimate with that file's size swapped in (null for files that can't load, or when the check stopped before
sizing). Text encoders, VAEs, LoRAs and shards beside the model are left out of `files`. A custom model's
saved `test` also carries `job`: its image is served at `/api/images/{job}/file`.

```json
{ "ok": true, "source": "hf.co/unsloth/Z-Image-Turbo-GGUF:z-image-turbo-Q3_K_M.gguf",
  "id": "hf.co/unsloth/Z-Image-Turbo-GGUF:z-image-turbo-Q3_K_M.gguf", "name": "Z-Image-Turbo-GGUF · z-image-turbo-Q3_K_M",
  "label": "Custom, not reviewed by Dawnroom",
  "steps": [
    { "step": "source", "status": "ok", "message": "unsloth/Z-Image-Turbo-GGUF, public. File z-image-turbo-Q3_K_M.gguf (2.7 GB).", "detail": { … } },
    { "step": "format", "status": "ok", "message": "GGUF: its header reads as one (453 tensors).", "detail": { … } },
    { "step": "architecture", "status": "ok", "message": "Z-Image (sd.cpp: z_image), read from the first 1 MB of the file.", "detail": { "architecture": "Z-Image", "sd_version": "z_image", "base": "z-image-turbo", "role": "diffusion_model", "header_bytes": … } },
    { "step": "companions", "status": "ok", "message": "From Z-Image Turbo: Qwen3-4B-Instruct-2507-Q4_K_M.gguf (2.5 GB), ae.safetensors (336 MB); all already on disk, shared.", "detail": { "files": [ … ], "missing_bytes": 0 } },
    { "step": "size", "status": "ok", "message": "2.7 GB to download (186.0 GB free). Should fit (estimate): about 7.1 GB of 8.0 GB VRAM.", "detail": { … } },
    { "step": "licence", "status": "ok", "message": "apache-2.0 (from the model card).", "detail": { "licence": { … } } }
  ],
  "code": null, "message": null, "download_bytes": …, "size_bytes": …, "fit": { "tier": "fits", "estimate": true, … }, "licence": { … } }
```

A failure has `"ok": false`, the failed step's `status: "failed"` with its `code`, the steps after it
`skipped`, and top-level `code` + `message`.

#### Pull: `POST /api/models/{hf.co id}/pull` (SSE)

An `hf.co/…` id that isn't added yet runs the check first: failed → 409 `{ "error", "code", "check": <report> }`
and nothing is written; passed → the manifest is added and the pull starts (the usual stream). After the
files, the stream sends `progress` with `stage: "testing"` (`file` is `test:<job stage>`; while sampling,
`bytes`/`total` are the step and step count), then `done`, or `error` with code `test_failed`. A custom model
that already passed isn't tested again. Clients ask the user before this call (the CLI asks y/N).

#### `POST /api/models/custom/import`

Body: `{ "path": "<a .gguf or .safetensors file>" }`. Runs the check, hashes the file, and hard-links it into
the models folder (a copy on another drive); returns the new model (`GET /api/models` shape), state
`needs_test`. Then `POST /api/models/{local id}/pull` fetches any companions and runs the test image. 409 with
the report if the check fails, or `in_library` when the same bytes are already a model here. Two different
files with the same name get `local/<name>`, `local/<name>-2`.
A path that's already a custom model's source fails the check's `source` step with `already_added`
(`detail.model` names it). A Hugging Face cache snapshot (a link into `blobs/`) is linked from the real file.

#### `POST /api/models/custom/local`

Body: `{}` or `{ "dir": "<a folder>" }`. Image models already on this computer: without `dir`, the usual
places other tools keep them (Hugging Face cache via `HF_HUB_CACHE` / `HF_HOME` / `~/.cache/huggingface/hub`,
ComfyUI (`~/ComfyUI/models`, `~/Documents/ComfyUI/models`, `C:` and `D:\ComfyUI\models`), A1111/Forge
`models/Stable-diffusion`, Stability Matrix `Models`, and `Downloads` two levels deep); with `dir`, only that
folder (400 `not_a_folder` if it isn't one). Files of 300 MB and up with a weight extension, six folders deep,
skipping Dawnroom's own models folder and `loras`/`controlnet`/`upscale_models`/`embeddings`; each goes through
the same check as an import (headers only, nothing copied), and only image models are listed:

```json
{ "roots": [{ "path": "C:\\Users\\you\\.cache\\huggingface\\hub", "label": "Hugging Face cache", "exists": true }, …],
  "found": [{ "path": "…\\sdxl\\RealVisXL_V5.0_fp16.safetensors", "file": "RealVisXL_V5.0_fp16.safetensors", "place": "sdxl",
              "size": 6938040682, "architecture": "Stable Diffusion XL", "ok": true, "message": null, "fit": "offload",
              "download_bytes": 334641164, "added": null, "library": "realvisxl-v5:fp16" }],
  "skipped": 5 }
```

`place` says where (`ComfyUI · checkpoints`, `Hugging Face cache · user/repo`, or the subfolder of `dir`);
`added` is the custom model it's already imported as; `library` the curated model with the same file (name and
size); `skipped` counts model-sized files that aren't image models (LLMs, encoders, VAEs, LoRAs). Ready to add
first, then by size. CLI: `dawnroom import` (the usual places) or `dawnroom import <folder>` list them numbered
and import the one picked.

`DELETE /api/models/{custom id}` deletes its files (keeping any another installed model uses) and its
manifest.

### `GET /api/presets`

```json
{ "presets": [
  { "id": "fast", "label": "Fast", "model": "flux2-klein-4b:q8", "default": false, "state": "installed", "fit": "fits" },
  { "id": "quality", "label": "Quality", "model": "z-image-turbo:q4", "default": true, "state": "installed", "fit": "fits" }
],
  "aspects": [ { "id": "1:1", "width": 1024, "height": 1024 }, { "id": "16:9", "width": 1360, "height": 768 } ] }
```

`aspects` is the full list `POST /api/jobs` accepts as `aspect`, with the size each one renders at.
Presets point at pinned tags: the measured ones. Only tested, commercially clean models without a licence
gate can be a preset (the engine refuses to start otherwise).

## Info and health

### `GET /api/info`

```json
{ "name": "Dawnroom", "version": "0.1.0", "port": 8420,
  "exe": "C:\…\dawnroom.exe",
  "data_dir": "…", "models_dir": "…", "outputs_dir": "…", "logs_dir": "…",
  "models_bytes": 21103664506, "outputs_bytes": 39412213 }
```

Where things live on disk and how big they are (the sizes walk the folders on every call, so don't
poll it). `exe` is the running executable: the path an agent's MCP config should point at.

### `GET /api/health`

`{ "name": "Dawnroom", "version": "0.1.0", "backend": "ready", "model": "z-image-turbo", "busy": false, "gpu": { …live nvidia-smi… }, "runtime": "windows-cuda12", "runtime_ready": true }`

`backend`: `stopped` | `loading` | `ready` | `busy`. `model` is the loaded model or `null`. `gpu` is live
nvidia-smi (`null` without an NVIDIA driver). `runtime` is the build this machine uses; `runtime_ready` says
whether it's installed.

### `POST /api/shutdown`

`202 { "ok": true }`, then the engine stops: running jobs and pulls end (a pull keeps its `.part` and
resumes on the next pull), sd-server is stopped and the process exits, freeing the GPU and memory.
Used by Studio's Quit, `dawnroom stop` and the uninstaller. Refused with 409 `moving` during a data-folder move.

### `GET /api/hardware`

Detected once at start (RAM availability is read per request). `dawnroom doctor` prints the same report.

```json
{
  "os": "windows", "os_version": "Microsoft Windows [Version 10.0.26300.9711]", "cpu": "Intel(R) Core(TM) Ultra 7 255H",
  "gpus": [
    { "name": "Intel(R) Graphics", "vendor": "intel", "integrated": true, "vram_mib": 128, "shared_mib": 13657,
      "driver": "32.0.101.8625", "compute_cap": null, "source": "dxgi", "luid": "0x00000000_0x000135b1" },
    { "name": "NVIDIA GeForce RTX 5060 Laptop GPU", "vendor": "nvidia", "integrated": false, "vram_mib": 8151, "shared_mib": 0,
      "driver": "596.36", "compute_cap": "12.0", "source": "nvidia-smi", "luid": "0x00000000_0x00013b5d" }
  ],
  "ram": { "total_bytes": 25123938304, "available_bytes": 8100000000 },
  "choice": { "gpu_index": 1, "runtime": "sd-cpp-master-1086-465adf3-windows-cuda12", "build": "windows-cuda12",
              "experimental": false, "status": "ok", "reason": "NVIDIA GeForce RTX 5060 Laptop GPU is an NVIDIA GPU: CUDA, the tested path." },
  "runtime": { "id": "sd-cpp-master-1086-465adf3-windows-cuda12", "build": "windows-cuda12", "gpu": "cuda", "experimental": false,
               "installed": true, "path": "…\\runtimes\\sd-cpp-master-1086-465adf3-windows-cuda12",
               "download_bytes": 930603493, "unpacked_bytes": 1248616055, "needed_bytes": 0, "min_driver": "572.61" },
  "sd_server_override": false,
  "devices": [ { "name": "CUDA0", "description": "NVIDIA GeForce RTX 5060 Laptop GPU" }, { "name": "CPU", "description": "Intel(R) Core(TM) Ultra 7 255H" } ],
  "runtime_sees_gpu": true,
  "disk": { "path": "…\\.data", "space": { "free_bytes": 255194259456, "total_bytes": 472534478848 } }
}
```

- `gpus`: every adapter. Windows lists them with DXGI (NVIDIA numbers come from nvidia-smi); Linux uses
  nvidia-smi, `vulkaninfo --summary` and `/sys/class/drm` (amdgpu VRAM + GTT; an Intel iGPU's budget is a
  guess of half the RAM, marked `memory_estimated: true`). `integrated`: little or no dedicated memory;
  its real budget is `vram_mib + shared_mib` (DXGI's SharedSystemMemory, about half of RAM).
- `choice`: discrete before integrated, then NVIDIA, then most memory. NVIDIA on Windows → `windows-cuda12`;
  AMD, Intel, integrated graphics, NVIDIA on Linux and NVIDIA older than compute 6.1 → Vulkan (`experimental: true`).
  `status`: `ok` | `driver_too_old` (NVIDIA driver below the runtime's `min_driver`) | `no_driver` (an NVIDIA
  card, but nvidia-smi doesn't answer) | `no_gpu` | `unsupported_os`. When it isn't `ok`, `message` says what to
  do and `link` is the vendor's driver page. Jobs are refused with that code; pulls still work.
- `devices`: what the installed runtime itself can use (`sd-cli --list-devices`); empty until it's installed.
  With more than one GPU listed (Vulkan on a hybrid laptop), the engine starts sd-server with
  `--backend <device>` for the chosen GPU. `runtime_sees_gpu: false` usually means the graphics driver is too old.

### `GET /api/runtimes`

```json
{ "active": "sd-cpp-master-1086-465adf3-windows-cuda12",
  "runtimes": [ { "id": "…-windows-vulkan", "build": "windows-vulkan", "os": "windows", "gpu": "vulkan", "experimental": true,
                  "download_bytes": 30023026, "unpacked_bytes": 89566289, "installed": true } ],
  "installed": [ { "id": "…-windows-cuda12", "last_used_ms": 1794832850323 } ],
  "partial": [ { "file": "sd-master-465adf3-bin-win-vulkan-x64.zip.part", "bytes": 29342840 } ] }
```

Runtimes are the sd.cpp builds in `registry/runtimes/*.toml` (generated from the GitHub release API by
`engine/scripts/gen_runtimes.py`; never edited by hand). Each model manifest names its runtime in `backend`.
Installs live in `<data>/runtimes/<id>/`: the zip(s) download to `<data>/runtimes/.downloads/*.part` with
resume, are checked against their sha256, unpacked into a temp folder that's renamed into place (an
interrupted unpack never looks installed), and deleted. Only the runtime in use and the one used before it
are kept. `partial` lists resumable runtime downloads.

### `POST /api/runtimes/import`

`{ "paths": ["D:\\spike\\bin\\sd-cuda12.zip", "D:\\spike\\bin\\cudart.zip"] }` →
`{ "files": [ { "path": "…", "matched": "sd-master-465adf3-bin-win-cuda12-x64.zip" } ], "active_installed": true }`

Installs runtime zips already on this machine instead of downloading them: each file is hashed and matched
to a runtime file by size and sha256 (names don't matter), copied in, and any runtime whose zips are now all
present is installed. `matched: null` for a file that matches nothing. `dawnroom runtime import <files>` calls it.

## Storage

The data folder, for Studio Settings → Storage and `dawnroom storage`. Every size counts a hardlinked file
once. The CLI calls the same library functions (`engine/src/storage.rs`) when the engine is stopped, so its
numbers match.

### `GET /api/storage`

```json
{ "data_dir": "D:\\…\\.data",
  "drive": { "free_bytes": 229100000000, "total_bytes": 472534478848 },
  "usage": { "models": 39106290240, "images": 87525271, "runtimes": 1248616338, "cache": 2116069, "partial": 0, "other": 3367546656, "total": 43812094574 },
  "models": [ { "id": "qwen-image:q4", "name": "Qwen-Image", "complete": true, "bytes": 18002625734, "shared": [], "last_used_ms": 1794840000000 } ],
  "partials": [ { "file": "models/vae/Qwen_Image-VAE.safetensors.part", "owner": "qwen-image", "bytes": 1000000, "total": 253806246 } ],
  "cache": [ { "kind": "thumbnails", "path": "thumbs", "bytes": 2116069 } ],
  "move": { "state": "idle", "bytes": 0, "total": 0, "from": null, "to": null, "error": null } }
```

- `usage`: every file lands in exactly one bucket. `cache` is what Clear cache would remove; `partial` is paused
  downloads; `other` is the history database, current logs and anything else.
- `models`: models with files on disk; `last_used_ms` is when a job last loaded it (`null`: never). `shared`: files
  another model on disk also uses (`DELETE /api/models/{id}` keeps them).
- `cache` kinds: `thumbnails` (also capped at about 500 MB automatically, oldest first), `rotated_log` (`*.log.N`),
  `old_runtime` (installed runtimes other than the current and the previous one), `unpack_leftover`,
  `runtime_zip` (a finished zip an interrupted install left). Never models, images, the database, the current
  and previous runtime, or paused downloads.

### `POST /api/storage/clear-cache`

`{}` → `{ "freed_bytes": 2116069, "removed": ["thumbs"] }`. Removes exactly the `cache` items (recomputed at that
moment). Show `GET /api/storage`'s list first and ask.

### `POST /api/storage/partials/delete`

`{ "file": "models/vae/Qwen_Image-VAE.safetensors.part" }` or `{ "all": true }` →
`{ "deleted": ["…"], "freed_bytes": 1000000 }`. Only files `partials` lists can be named. 409 `model_busy` for a
`.part` any running pull includes (shared encoders and VAEs too; for runtime zips, any download); `all` skips
those instead. 404 `not_found` for anything else. Clear cache never touches these; Studio and the CLI ask first.
Each `.part`'s sidecar (`.part.json`) goes with it. A `.part`'s `owner` is the model whose pull wrote it (recorded
in the sidecar), else the model it's the weights of; a shared file another model's pull started never makes a
model paused or lands in its Discard. Studio's Discard on a paused download deletes every `.part` whose `owner`
is that model.

### `GET /api/storage/drives`

Fixed drives (no removable, network or optical ones) for first run's "Where models live":

```json
{ "data_dir": "C:\Users\me\AppData\Local\Dawnroom",
  "drives": [{ "root": "C:\\", "label": "Windows", "free_bytes": 188884156416, "total_bytes": 550534901760,
               "path": "C:\Users\me\AppData\Local\Dawnroom", "current": true },
             { "root": "D:\\", "label": "Data", "free_bytes": 144430264320, "total_bytes": 472534478848,
               "path": "D:\Dawnroom", "current": false }] }
```

`path` is where the data folder would go on that drive: the current folder on its own drive, the default folder on
the default drive, else `Dawnroom` at the drive's top. Linux lists `/` and real file systems mounted under
`/home`, `/mnt`, `/media`, `/run/media` and `/data`. Choosing one is a move (below); on a first run there's almost
nothing to copy.

### `POST /api/storage/move` → 202, `GET /api/storage/move`

`{ "to": "E:\\Dawnroom" }` → the move's progress:
`{ "state": "copying", "bytes": 0, "total": 8384689428, "from": "…", "to": "E:\\Dawnroom", "error": null }`.

`state`: `idle` → `copying` → `verifying` → `switching` → `restarting`, or `failed` (with `error`). Checked first:
`to` must be a full path, not inside or around the current folder (400 `invalid_request`), empty or an unfinished
earlier move (400 `folder_not_empty`), with room for everything (507 `not_enough_space`), and nothing may be
developing, downloading or installing (409 `busy`). While it runs, every POST and DELETE except the move itself,
and new thumbnails, get 409 `moving`.

How it works: sd-server is stopped and the database checkpointed; the folder is copied into `to` (hardlinked
files stay hardlinked; a `.dawnroom-moving` marker says it's unfinished) and every file's size checked; then
the engine's config file is written atomically (`data_dir = to`, `delete_after_move = old`), the marker is
removed, and the engine starts a fresh copy of itself (same arguments, on the same port) and exits. The new
engine deletes the old folder before it opens anything, and repoints stored image paths into the new
`outputs`. A kill at any point is safe: before the switch the old folder is still in use and the next move
clears the unfinished copy; after it, the next start finishes the delete. Clients poll `GET /api/storage/move`
until it answers `restarting`, then `GET /api/info` until `data_dir` changes.

The config file: `%APPDATA%\Dawnroom\config.toml` (Windows), `~/.config/dawnroom/config.toml` (Linux);
`DAWNROOM_CONFIG` replaces the path. Studio's shell passes nothing; the engine reads it.

## OpenAI-compatible

### `POST /v1/images/generations`

Synchronous. `{ "prompt": "…", "model": "fast", "size": "1024x1024", "response_format": "b64_json", "seed": 42 }`
→ `{ "created": 1763050000, "data": [ { "b64_json": "…" } ] }`.
`model` may be `auto`, a model id or a preset id; any other value (e.g. `gpt-image-1`) uses Auto,
so stock OpenAI clients work (the engine logs that fallback once). Only `n = 1`. `response_format: "url"` returns `/api/images/{id}/file`.
Errors use OpenAI's shape: `{ "error": { "message": "…", "type": "invalid_request_error", "code": null } }`.
The Local security refusals (403/415) come before the route, so they use the engine's shape here too.

### `POST /v1/images/edits`

Synchronous, `multipart/form-data` like OpenAI's: `image` (or `image[]`, repeated; the first is the image to
edit, the others references), `prompt` (required), and optional `model` (as above: Auto by default), `size`,
`response_format`, `seed`, `negative_prompt`, `n` (only 1). Fields OpenAI has that don't apply here (`quality`,
`background`, `user`, …) are ignored. `mask` is refused (400): none of the edit models uses one. Same response and
error shapes as generations. Uploads are stored like `POST /api/inputs`.

```
curl -F image=@cat.png -F "prompt=turn the cat into a black cat" -F response_format=url http://127.0.0.1:8420/v1/images/edits
```

## Engine flags

`serve [--port N] [--data-dir DIR] [--runtime BUILD] [--gpu NAME] [--sd-server PATH] [--registry-dir DIR] [--idle-secs N] [--hf-endpoint URL]`.
`--runtime` forces a build (`windows-vulkan` runs the AMD/Intel path on an NVIDIA card); `--gpu` picks the GPU
whose name contains NAME (`--gpu intel` uses a hybrid laptop's integrated GPU). `--sd-server` uses that exe as
is: no runtime is installed or chosen.

Dev and test only: `DAWNROOM_SIMULATE=no-gpu | old-driver | no-driver | igpu-only` changes what detection
reports (the report says `simulated`), and `DAWNROOM_FAKE_FREE_BYTES=N` replaces the data drive's free space.
`--hf-endpoint` (or the `HF_ENDPOINT` environment variable, the Hugging Face convention) replaces
`https://huggingface.co` as the download base: a mirror, or a local fixture server in tests.

## CLI: models

`dawnroom list` shows installed (or partly downloaded) tags and the preset models; `list --all` shows every
model once, for its default tag here: its other tags, tasks (`gen`, `edit`), size, licence (`*` = asks for
licence acceptance), fit (`est.` = estimated from file sizes), the measured warm time (only for this GPU; empty
when nobody measured it) and state (custom models are marked there).

`dawnroom run "<prompt>"` uses Auto and prints its reason (`● Auto · Z-Image Turbo: already loaded, best text,
about 19 s here`); `--model <id>` or `--fast`/`--quality` name one. `dawnroom run "<instruction>" --image in.png
[--ref a.png …]` edits. If Auto finds nothing installed it names the model to pull and, in a terminal (or with
`--yes`), pulls it. `dawnroom auto` (and `dawnroom status`) print what Auto picks now for generate and edit. `dawnroom pull <model>[:<tag>]` pulls the
default tag or the one named; for a gated original it prints the licence and asks `[y/N]` first
(`--accept-licence` answers yes; `--json` or no terminal refuses with `licence_not_accepted`).

Custom models: `dawnroom pull hf.co/<user>/<repo>[:<file>]` runs the check and prints each step, then the
download size, licence and the "Custom, not reviewed by Dawnroom" label, and asks `[y/N]` (`--yes`/`-y`
answers yes; required with `--json`; without a terminal it says so and downloads nothing). Several files and
a terminal: it lists them numbered and asks which. Then the pull and the test image, with its result and the
test image's path. `dawnroom import <path> [--yes]` does the same for a local file; with no path it lists the
image models already downloaded in the usual places (Hugging Face cache, ComfyUI, A1111/Forge, Stability Matrix,
Downloads), with a folder it lists the ones in it. `list` marks custom models and explains the label under the
table; `rm <id>` deletes one.

## CLI: engine and installs

`dawnroom stop` stops the engine and its sd-server (`POST /api/shutdown`); a download in progress keeps its
`.part`, and "isn't running" is fine. `dawnroom update` updates a CLI-only or Linux tarball install from the
latest GitHub release, after checking it against the release's `SHA256SUMS`: engine stopped, files swapped,
models and images untouched; Studio installs update from Studio and say so. `dawnroom doctor [--test]` prints a
bug report as Markdown (`--test` also develops one image). `dawnroom runtime list` and
`dawnroom runtime import <zip>…` show the image runtimes and install zips you already have (matched by sha256).

## CLI: `dawnroom storage`

`dawnroom storage [show]`, `storage clear-cache [--yes]`, `storage delete-partial <file> | --all [--yes]`,
`storage move <folder>`, `storage link <source>`, each with `--json` (which needs `--yes` where the command asks).
With the engine running they use the endpoints above; with it stopped, the same functions in-process (what
the installer's hooks use). `link` hardlinks registry model files found under `<source>` into the models folder after
checking each one's sha256 (same drive, engine stopped): the CLI form of `engine/scripts/link-models.ps1`, and
how a dev checkout gets its `.models/` links back after a move.

## MCP

`dawnroom mcp` is an MCP server on stdio and a client of this API like the CLI. If the engine isn't
answering, the first tool call starts `dawnroom serve` as a detached, windowless daemon (stdio to
`logs/daemon.log` in the data folder). Tools: `generate_image`, `list_models`, `pull_model` (needs `confirm: true`),
`get_job`, `cancel_job`, `list_images`, `check_model`, `edit_image` (`instruction`, `image` path, optional `refs`
paths, `model`, `seed`; returns the output path, Auto's reason, `edited_from` and a preview). `generate_image`
uses Auto unless a preset is given; with nothing installed for the task both return `needs_pull` with the
suggested model's size and licence. `list_models` returns one entry per model (its default tag here, plus
installed tags) with `tags`, `tier`, `tasks`, `tiers`, `timing`, `fit_is_estimate`, `licence_acceptance` and `custom` (the label, for custom models). `pull_model` can't accept a
licence: for a gated original it fails with `licence_not_accepted` and plain words asking the user to accept
it in Studio or the CLI. `check_model` (read-only) runs the custom-model check for `hf.co/…` or a local path
and returns the steps; `pull_model` refuses custom sources with `custom_needs_user` (only the user adds them,
in a terminal).
