<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="studio/public/brand/dawnroom-lockup-paper.svg">
    <img src="studio/public/brand/dawnroom-lockup-ink.svg" alt="Dawnroom" height="56">
  </picture>
</p>

<p align="center">
  <b>Make and edit images on your own GPU. No credits, no cloud, no node graphs.</b><br>
  An open-source, local-first image studio by <a href="https://dawnlightlabs.com">Dawnlight Labs</a>.
</p>

<p align="center">
  <a href="https://dawnroom.dawnlightlabs.com">Website</a> ·
  <a href="https://dawnroom.dawnlightlabs.com/search">Model library</a> ·
  <a href="https://dawnroom.dawnlightlabs.com/docs">Docs</a> ·
  <a href="engine/API.md">API reference</a>
</p>

---

## What is Dawnroom?

ChatGPT and Gemini made image generation effortless. Running open models on your own hardware still means tools built around nodes, samplers and checkpoints. Dawnroom is the middle ground: install it, describe what you want, get an image. It runs on your own GPU and nothing leaves your machine.

- **Studio**: a desktop app (Windows; Linux is experimental) with a chat-style Create view, a gallery of everything you've made, a model library and storage tools.
- **CLI**: `dawnroom run "a lighthouse at first light"` and you have a PNG.
- **MCP server**: agents like Claude Code and Codex can generate and edit images for you.
- **OpenAI-compatible API** on `localhost`, so existing scripts and tools work as they are.

> **Status:** v0.1.0 is out (2026-11-20). Windows is supported; Linux is experimental and needs glibc 2.43 or newer. Download it from the [latest release](https://github.com/DawnlightLabs/dawnroom/releases/latest) or use the install commands below. You can also build from source (see [Build from source](#build-from-source)).

## Install

**Installer.** From the [latest release](https://github.com/DawnlightLabs/dawnroom/releases/latest):

- Windows: `Dawnroom_<version>_x64-setup.exe` (one click, per user, puts `dawnroom` on your PATH)
- Linux (experimental): `Dawnroom_<version>_amd64.deb` or `Dawnroom_<version>_amd64.AppImage`

**From a terminal.** One command installs Studio and the CLI, checking every download against the release's `SHA256SUMS`:

```powershell
irm https://dawnroom.dawnlightlabs.com/install.ps1 | iex
```

```sh
curl -fsSL https://dawnroom.dawnlightlabs.com/install.sh | sh
```

For the CLI and engine only (no Studio), add `--cli`:

```powershell
& ([scriptblock]::Create((irm https://dawnroom.dawnlightlabs.com/install.ps1))) --cli
```

```sh
curl -fsSL https://dawnroom.dawnlightlabs.com/install.sh | sh -s -- --cli
```

Nothing else is bundled: on first run Dawnroom checks your GPU, shows what the downloads need against your free disk, and fetches the right image runtime and a starter model.

## Features

### Generate and edit
- **Text to image** with the best model your machine can run.
- **Edit by instruction**: "make it night", "change the sign to OPEN LATE". Add **reference images** ("put the hat from image 2 on the cat"), up to 9.
- Edits remember where they came from: every edit links back to the image it was made from.
- **Every image carries its recipe** (prompt, model, seed, steps, time) inside the PNG, so any result can be reproduced exactly.

### Auto: the right model, every time
You don't need to know what a checkpoint is. **Auto** picks the model for you, separately for Generate and Edit:
- Only models that are installed and fit your machine.
- It keeps the model that's already loaded when it qualifies, so it never switches for nothing.
- Same inputs, same pick. It never downloads anything without asking.
- It always says what it picked and why, in one line:
  `Z-Image Turbo: already loaded, best text, about 19 s here`

You can still pick any model yourself.

### A model library that answers "will it run on my machine?"
- **34 well-known models** that run on the pinned stable-diffusion.cpp runtime: Z-Image, FLUX.1 / FLUX.2 / FLUX.3, Qwen-Image and Qwen-Image-Edit, Stable Diffusion 1.5 / SDXL / 3.5, Chroma, HiDream, LongCat, ERNIE, PixArt-Σ and more.
- **Fit for your hardware.** Each model shows whether it fits your GPU, runs with **RAM offload** (slower), or needs more memory.
- **Size tags**, like Ollama: `z-image-turbo:q4`, `:q8`. A bare name picks the tag that suits your machine.
- **Licences shown plainly.** Commercial-use, restricted and non-commercial models are labelled. For models whose official repo is gated, Dawnroom shows the original licence and asks you to accept it before downloading.
- **Shared files are downloaded once.** Text encoders and VAEs used by several models are stored once and reused.

### Custom models from Hugging Face or your disk
```sh
dawnroom pull hf.co/<user>/<repo>:<file>.gguf
dawnroom import ./my-model.safetensors      # no path: lists models already on this computer
```
Before anything is downloaded, Dawnroom checks the model by reading only its header:
1. **Source**: a public, ungated repo.
2. **Format**: GGUF or safetensors only. Pickle formats (`.ckpt`, `.bin`, `.pt`) are refused, because they can run code.
3. **Architecture**: identified from the file itself and matched to a supported family.
4. **Companions**: the text encoder and VAE it needs, reused from disk where you already have them.
5. **Size and memory**: checked against your free disk and your GPU.
6. **Licence**: read from the model card.

If it can't run, you get the reason in plain words and nothing more is downloaded. If it passes, Dawnroom downloads it and makes one small test image before calling it ready. Custom models are labelled *Custom, not reviewed by Dawnroom*.

### Downloads that survive real life
Model files are big. Dawnroom's downloads:
- **retry on their own** through Wi-Fi drops, sleep, and busy or failing servers;
- **resume where they stopped** after a crash, a power cut or a restart, without starting over;
- **verify every file** with sha256;
- **refuse captive-portal pages** and redirects to unexpected hosts;
- keep your laptop awake while they run (Windows).

### Your library and your disk
- **Gallery**: browse every image and every session, open a print full-screen with its recipe and its edit history, favourite, delete, or send it back to Create to edit.
- **Storage manager**: see what takes space (models, images, runtimes, cache), delete models, clear rebuildable cache safely, and move the whole data folder to another drive.
- **Dark, Light and System themes.**

### Private by default
Everything runs locally. The engine listens on `127.0.0.1` only. Your prompts, input images and results stay on your machine.

## Hardware

| Platform | GPU | Status |
|---|---|---|
| Windows | NVIDIA (GTX 10 series and newer), CUDA | Supported |
| Windows | AMD Radeon, Intel Arc, built-in graphics, Vulkan | Experimental |
| Linux x86_64 | Any Vulkan GPU | Experimental |
| macOS | | Planned after v0.1 |

- NVIDIA on Windows needs driver **572.61 or newer**. Dawnroom detects an old or missing driver and tells you what to update.
- System RAM counts too: models bigger than your GPU can run with RAM offload, more slowly.
- Built-in graphics work through shared memory and are slow, but they work.
- Bigger GPUs and unified-memory machines run bigger models.

## Models

The full library, with fit for your own card, is at [dawnroom.dawnlightlabs.com/search](https://dawnroom.dawnlightlabs.com/search).

| Model | Task | Licence | Download | Memory |
|---|---|---|---|---|
| Chroma1-HD | generate | Apache-2.0 | 10.8 GB | 12.1 GB est. |
| Chroma1-Radiance | generate | Apache-2.0 | 23.9 GB | 24.3 GB est. |
| ERNIE-Image | generate | Apache-2.0 | 7.5 GB | 9.0 GB est. |
| ERNIE-Image Turbo | generate | Apache-2.0 | 7.5 GB | 9.0 GB est. |
| Fibo 2.0 dev | generate | CC-BY-NC-4.0 (non-commercial) \* | 11.2 GB | 12.5 GB est. |
| Fibo 2.0 Edit Turbo | edit | CC-BY-NC-4.0 (non-commercial) \* | 11.7 GB | 12.9 GB est. |
| FLUX.1 dev | generate | FLUX-1-dev (non-commercial) \* | 12.4 GB | 13.5 GB est. |
| FLUX.1 Kontext dev | edit | FLUX-1-dev (non-commercial) \* | 12.4 GB | 13.6 GB est. |
| FLUX.1 schnell | generate | Apache-2.0 \* | 12.4 GB | 13.5 GB |
| FLUX.2 dev | generate, edit | FLUX (non-commercial) \* | 34.8 GB | 34.4 GB est. |
| FLUX.2 klein 4B | generate, edit | Apache-2.0 | 7.1 GB | 5.1 GB |
| FLUX.2 klein 9B | generate, edit | FLUX (non-commercial) \* | 11.0 GB | 12.2 GB est. |
| FLUX.3 Image klein | generate, edit | FLUX-Kommunity (restricted) \* | 6.3 GB | 7.9 GB est. |
| FLUX.3 Image klein base | generate, edit | FLUX-Kommunity (restricted) \* | 6.3 GB | 7.9 GB est. |
| HiDream-O1-Image Dev | generate | MIT | 16.4 GB | 17.2 GB est. |
| Ideogram 4 | generate | Ideogram (non-commercial) \* | 16.7 GB | 17.5 GB est. |
| Krea 2 Turbo | generate | Krea-2-Community (restricted) \* | 10.0 GB | 11.3 GB est. |
| LongCat-Image | generate | Apache-2.0 | 8.7 GB | 9.5 GB |
| MOSS-Image | generate | Apache-2.0 | 9.4 GB | 10.7 GB est. |
| Ovis-Image 7B | generate | Apache-2.0 | 9.7 GB | 11.0 GB est. |
| PixArt-Σ | generate | OpenRAIL++ (use restrictions) | 7.7 GB | 9.1 GB est. |
| Qwen-Image | generate | Apache-2.0 | 18.0 GB | 18.3 GB |
| Qwen-Image 2.1 | generate | Qwen-Research (non-commercial) | 9.9 GB | 11.2 GB est. |
| Qwen-Image 2.1 Turbo | generate | Qwen-Research (non-commercial) | 19.9 GB | 20.6 GB est. |
| Qwen-Image-Edit 2511 | edit | Apache-2.0 | 18.6 GB | 18.9 GB |
| RealVisXL V5.0 | generate | OpenRAIL++ (use restrictions) | 7.3 GB | 5.5 GB |
| SD Turbo | generate | Stability-AI-Community (restricted) | 5.2 GB | 6.9 GB est. |
| Stable Diffusion 1.5 | generate | CreativeML-OpenRAIL-M (use restrictions) | 2.1 GB | 2.3 GB |
| Stable Diffusion 3.5 Large | generate | Stability-AI-Community (restricted) \* | 11.0 GB | 12.2 GB est. |
| Stable Diffusion 3.5 Medium | generate | Stability-AI-Community (restricted) \* | 8.0 GB | 9.4 GB |
| Stable Diffusion XL 1.0 | generate | OpenRAIL++ (use restrictions) | 7.3 GB | 8.8 GB est. |
| SDXL Turbo | generate | SAI-NC-Community (non-commercial) | 7.3 GB | 8.8 GB est. |
| Z-Image | generate | Apache-2.0 | 6.7 GB | 8.2 GB est. |
| Z-Image Turbo | generate | Apache-2.0 | 6.7 GB | 4.3 GB |

\* You accept the original licence before the download (the official repo is gated). Download is the default size tag; memory is VRAM plus system RAM at that tag, and "est." means estimated from file sizes. `dawnroom list --all` shows the same on your machine.

## Quick start (CLI)

```sh
# Generate: Auto picks the model and says why
dawnroom run "a lighthouse at first light, long exposure, film grain"

# Edit an image, optionally with references
dawnroom run "make it night" --image photo.png
dawnroom run "put the hat from image 2 on the cat" --image cat.png --ref hat.png

# Models
dawnroom list            # installed models
dawnroom list --all      # the whole library
dawnroom pull z-image-turbo
dawnroom pull flux2-klein-4b:q8

# What Auto would pick right now, and why
dawnroom auto
```

If no suitable model is installed yet, `run` tells you which one to get and its size, then asks before downloading.

## CLI reference

| Command | What it does |
|---|---|
| `run <prompt>` | Generate an image, or edit one with `--image` (+ `--ref`, up to 9). Auto unless you pass `--model`. Also `--seed`, `-o <file>`, `-y` |
| `pull <model>[:<tag>]` | Download a library model, or a custom one with `hf.co/<user>/<repo>[:<file>]` (checked first, then a test image). `--accept-licence` for gated originals, `-y` for custom models |
| `import [path]` | Add a `.gguf` / `.safetensors` file from your computer (checked, then a test image). No path: lists image models already downloaded; a folder: lists the ones in it. `-y` skips the question |
| `list [--all]` | Installed models and presets, or the whole library |
| `rm <model>` | Delete a model's files (files another installed model uses are kept) |
| `auto` | What Auto picks now for generate and edit, and why |
| `status` | Engine, GPU, loaded model, Auto's picks |
| `stop` | Stop the engine and free the GPU (a download in progress keeps what it has) |
| `storage` | Usage (`show`), `clear-cache`, `delete-partial`, `move <folder>`, `link` |
| `doctor` | Everything a bug report needs, as Markdown for a GitHub issue |
| `serve` | Run the engine (Studio and the CLI start it for you when needed) |
| `mcp` | MCP server on stdio for agents |
| `runtime` | The image runtimes the engine runs: `list`, `import` |

Every command takes `--json` for scripts, and `--port` (default `8420`).

## Use it from an agent (MCP)

Dawnroom ships an MCP server, so Claude Code, Codex and other MCP clients can make and edit images on your GPU.

```sh
claude mcp add dawnroom -- dawnroom mcp
```

For other clients, register a stdio server with the command `dawnroom mcp`.

Tools: `generate_image`, `edit_image`, `get_job`, `cancel_job`, `list_images`, `list_models`, `pull_model`, `check_model`.

Agents can't download a model you haven't asked for, accept a licence for you, or add custom models on their own. When a model is missing, they get the name and size to ask you about.

## Use it from code (API)

The engine serves an OpenAI-compatible images API on `http://127.0.0.1:8420`:

```sh
curl http://127.0.0.1:8420/v1/images/generations \
  -H "Content-Type: application/json" \
  -d '{"prompt": "a ceramic mug on linen, soft window light"}'

curl http://127.0.0.1:8420/v1/images/edits \
  -F image=@photo.png -F prompt="make it night"
```

Unknown model names fall back to Auto. The native API (`/api/jobs` with live progress and previews, `/api/models`, `/api/auto`, `/api/images`, storage, custom models) is documented in [`engine/API.md`](engine/API.md). Every error has the same shape: `{"error": "<plain words>", "code": "<code>"}`.

## Where your data lives

| | Windows | Linux |
|---|---|---|
| Models, images, history | `%LOCALAPPDATA%\Dawnroom` | `~/.local/share/dawnroom` |
| Config | `%APPDATA%\Dawnroom\config.toml` | `~/.config/dawnroom/config.toml` |

You can move the data folder to another drive from Studio (Settings → Storage) or with `dawnroom storage move <folder>`.

## Build from source

You need a stable Rust toolchain and Node.js (LTS). For the desktop app you also need the [Tauri prerequisites](https://tauri.app/start/prerequisites/) (WebView2 on Windows).

```sh
# Engine and CLI
cd engine
cargo build --release
./target/release/dawnroom serve

# Studio (desktop app)
cd studio
npm install
npm run tauri dev        # or `npm run dev` for the browser at http://localhost:5173

# Website
cd site
npm install
npm run build
```

On first run the engine downloads the image runtime (a pinned [stable-diffusion.cpp](https://github.com/leejet/stable-diffusion.cpp) build) for your GPU and verifies it. Model weights and binaries are never committed to this repo.

## How it works

```
Studio (Tauri + React) ─┐
CLI ────────────────────┼──▶  Engine (Rust, Axum, 127.0.0.1:8420) ──▶ sd-server (stable-diffusion.cpp)
MCP (agents) ───────────┤        job queue · SQLite history · downloads
OpenAI-compatible API ──┘        Auto · fit · model registry · storage
```

- **Engine** (`engine/`): owns all logic. It detects hardware on any vendor, picks and installs the runtime, downloads and verifies models, queues jobs, keeps history and serves the API.
- **Studio** (`studio/`): a thin client of the engine's API.
- **Registry** (`registry/`): one TOML manifest per model (files pinned by revision and sha256, licence, defaults, measurements), generated from stable-diffusion.cpp's own recipes and the Hugging Face API.
- **Site** (`site/`): the website and model library, built from the same registry.

## Roadmap

- **v0.1** (released): local generate and edit, Studio, CLI, MCP, installers for Windows and Linux.
- **v0.2 Director**: an agent that plans prompts, makes variations, reviews them and refines, with your own API keys or a local model. Plus open, shareable Recipes for multi-step workflows.
- **v0.3 Video**: local video generation on small GPUs, and cloud video with your own keys.
- **Running tracks**: macOS, more GPUs, more models, community measurements with `dawnroom bench`.

More at [dawnroom.dawnlightlabs.com/vision](https://dawnroom.dawnlightlabs.com/vision).

## Contributing

Bug reports are welcome. Run `dawnroom doctor` and paste its output into the issue: it has everything we need about your machine. See [CONTRIBUTING.md](CONTRIBUTING.md). Security problems: [SECURITY.md](SECURITY.md).

## Licence

Dawnroom is licensed under the [Apache License 2.0](LICENSE).

Models are not part of Dawnroom and keep their own licences, which Dawnroom shows before you download. Some are non-commercial or restricted; check before using results commercially.

Dawnroom runs [stable-diffusion.cpp](https://github.com/leejet/stable-diffusion.cpp) (MIT) for inference.

---

<p align="center">by <a href="https://dawnlightlabs.com">Dawnlight Labs</a></p>
