# Contributing to Dawnroom

Thanks for helping. Small, focused changes are easiest to review. If you're planning something big, open an issue first.

## Layout

| Folder | What it is |
| --- | --- |
| `engine/` | Rust: the engine (HTTP API, job queue, downloads, hardware detection), the `dawnroom` CLI and the MCP server. API reference: `engine/API.md`. |
| `studio/` | Studio: Vite + React + TypeScript, wrapped in Tauri 2 (`studio/src-tauri/`). |
| `site/` | The website and docs: Astro, static. |
| `registry/` | One TOML manifest per model (`registry/*.toml`), the runtimes (`registry/runtimes/`) and the hardware table (`registry/hardware/devices.toml`). |

## Build and run

You need Rust (stable), Node 22 or newer, and on Linux the Tauri build libraries
(`libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev patchelf`).

Engine and CLI:

```
cd engine
cargo build --release --bin dawnroom
cargo run --bin dawnroom -- serve        # the engine, on port 8420
cargo run --bin dawnroom -- --help
```

Studio, in a browser against a running engine (http://localhost:5173, the engine only allows that origin):

```
cd studio
npm ci
npm run dev
```

The desktop app: `npm run tauri dev`. It starts the engine itself; for a release build, copy the engine to
`studio/src-tauri/binaries/dawnroom-<target triple>` (`rustc -vV` shows yours; add `.exe` on Windows) and run `npx tauri build`.

Site (http://localhost:4322 with `npx astro preview`):

```
cd site
npm ci
npm run build
```

## Tests

```
cd engine && cargo test && cargo clippy --all-targets -- -D warnings
cd studio && npm run typecheck && npm test
cd site && npm run build
```

CI runs the same on Windows and Linux. Please keep clippy clean.

## Never commit weights or binaries

Model weights, runtimes (sd.cpp builds), installers and anything else big are downloaded at run time and live in
gitignored folders (`.data/`, `.models/`, `target/`). If a change needs a new binary, it goes in a
runtime manifest with a URL and a sha256, never in the repo.

## Proposing a model

The library only holds models that run on the pinned sd.cpp build, with a licence we can show you before you download.

1. Open a "feature request" issue with the Hugging Face repo, the files you'd use, the licence and what it does best.
2. Or send the change: add an entry to `registry/sources/models.toml`, run `python engine/scripts/gen_models.py <id>`
   (it reads the live Hugging Face API, pins revisions and sha256, and writes `registry/<id>.toml`), and commit both.
3. Fine-tunes, merges and uncensored models aren't added to the registry (Dawnroom isn't a model host). They work as
   custom models: `dawnroom pull hf.co/<user>/<repo>`.

Only add a measurement to a manifest if you ran it yourself, through the engine, and say on which GPU.

## Reporting a bug

Run `dawnroom doctor` and paste the result into a new issue (the templates ask for it), with your OS, GPU, and what
happened. For a model that doesn't work, add the model id and tag. Security problems: see `SECURITY.md`.

## Licence

By contributing you agree your work is released under the Apache License 2.0.
