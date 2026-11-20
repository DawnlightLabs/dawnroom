# Dawnroom

Dawnroom is an open-source, local-first image generation and editing studio by Dawnlight Labs (Apache-2.0). It runs open
models on the user's own GPU through a pinned stable-diffusion.cpp runtime, with a desktop app (Studio), a CLI
(`dawnroom`) and an MCP server for agents. Nothing leaves the machine except downloads the user starts and an update check.

## Layout

| Folder | What it is |
| --- | --- |
| `engine/` | Rust (Axum). The engine, the `dawnroom` CLI and the MCP server. It owns all logic: hardware detection, runtime and model downloads, job queue, SQLite history, Auto model choice, storage. API reference: `engine/API.md`. |
| `studio/` | Studio: Vite + React + TypeScript in a Tauri 2 shell (`studio/src-tauri/`). A thin client of the engine's HTTP API. |
| `site/` | The website and docs: Astro, static. Built from the same `registry/`. |
| `registry/` | One TOML manifest per model (`registry/*.toml`, generated), runtime builds (`registry/runtimes/`) and the hardware table (`registry/hardware/devices.toml`). |

## Build and test

```
cd engine && cargo build --release --bin dawnroom        # target/release/dawnroom
cd engine && cargo test --all-targets && cargo clippy --all-targets -- -D warnings
cd studio && npm ci && npm run typecheck && npm test      # npm run dev (http://localhost:5173), npm run tauri dev
cd site && npm ci && npm run build                        # also checks every internal link and anchor; 0 dead is required
```

The engine listens on `127.0.0.1:8420` by default. Studio in a browser only works from `http://localhost:5173`
(the engine allows that origin). For a release build of Studio, copy the engine to
`studio/src-tauri/binaries/dawnroom-<target triple>` first. CI is `.github/workflows/ci.yml`; releases are built by
`.github/workflows/release.yml` on a `v*` tag.

## Conventions

- Keep code simple, direct and modular. Prefer the obvious solution; no speculative abstractions. The one planned seam is the inference `Backend` trait.
- The engine owns logic; Studio, the CLI and MCP are clients. Every engine error is `{"error": "<plain words>", "code": "<code>"}`; add new codes to `engine/API.md`.
- The product name lives in config constants (`engine`, `studio/src/config.ts`, `site/src/config.ts`, Tauri config), never scattered.
- Never commit model weights or binaries. They live in gitignored folders and are downloaded at run time, with a sha256 in a manifest.
- `registry/*.toml` are generated: edit `registry/sources/models.toml` and run `python engine/scripts/gen_models.py <id>`; runtimes come from `engine/scripts/gen_runtimes.py`. Only `[[variant.measured]]` blocks are written by hand, and only from real measurements.
- User-facing text is short, warm and plain; errors say what to do next. Show a model's time only where it was measured on that GPU; never invent numbers. Check each model's licence from its live model card before adding it.
- Shell scripts (`*.sh`) keep LF line endings; `install.sh` is piped into `sh`.
- UI: two themes (Dark, Light), each with its own tokens; motion is 150–250 ms and off under `prefers-reduced-motion`. Studio uses Geist + DM Mono; the site is a plain, full-width layout.
- Windows PowerShell 5.1: send JSON request bodies from a file written without a BOM.

## Platforms

Windows (NVIDIA via CUDA; AMD, Intel Arc and built-in graphics via Vulkan, experimental) and Linux x86_64 (Vulkan, experimental; the pinned Linux runtime needs glibc 2.43 or newer). macOS is not in v0.1.
