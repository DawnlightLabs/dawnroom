//! The CLI: a thin HTTP client of the engine (except `serve`, which runs it).
//! Human output goes to stderr (progress) and stdout (results); `--json` prints exactly one
//! JSON document on stdout and nothing else.

mod doctor;
mod mcp;
mod storage;
mod style;
mod update;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use engine::config::{Config, DEFAULT_PORT, PRODUCT_NAME};
use style::{Style, gb, rate};

const BIN: &str = env!("CARGO_BIN_NAME");

#[derive(Parser)]
#[command(name = BIN, version, about = "Dawnroom, by Dawnlight Labs: local image generation on your own GPU.")]
struct Cli {
    /// Engine port.
    #[arg(long, global = true, default_value_t = DEFAULT_PORT)]
    port: u16,
    /// Print one JSON document on stdout instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the engine (Ctrl-C to stop).
    Serve {
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Use this sd-server exe instead of the managed runtime.
        #[arg(long)]
        sd_server: Option<PathBuf>,
        /// Force a runtime build, e.g. windows-vulkan (to try the AMD/Intel path on NVIDIA).
        #[arg(long)]
        runtime: Option<String>,
        /// Use the GPU whose name contains this, e.g. "arc" for a hybrid laptop's integrated GPU.
        #[arg(long)]
        gpu: Option<String>,
        #[arg(long)]
        registry_dir: Option<PathBuf>,
        /// Free the GPU after this many idle seconds.
        #[arg(long)]
        idle_secs: Option<u64>,
        /// Download models from here instead of https://huggingface.co (a mirror, or a test server).
        #[arg(long)]
        hf_endpoint: Option<String>,
    },
    /// Download a model: `pull z-image-turbo` (the tag that suits this machine), `pull z-image-turbo:q8`,
    /// or a custom one from Hugging Face: `pull hf.co/<user>/<repo>[:<file>]` (checked first, then a test image).
    Pull {
        model: String,
        /// Accept the model's licence without asking (for models whose official repo is gated).
        #[arg(long)]
        accept_licence: bool,
        /// Custom models: download without asking after the check passes (required with --json).
        #[arg(short, long)]
        yes: bool,
    },
    /// Add a model file from this computer (.gguf or .safetensors): checked, linked into the models
    /// folder, then one small test image. No path: lists image models already downloaded (Hugging Face
    /// cache, ComfyUI, A1111/Forge, Stability Matrix, Downloads); a folder: lists the ones in it.
    Import {
        path: Option<PathBuf>,
        /// Import without asking after the check passes (required with --json).
        #[arg(short, long)]
        yes: bool,
    },
    /// Show installed models and the presets; `--all` shows every model in the library.
    List {
        /// Every model: tasks, licence, size and fit for the tag that suits this machine.
        #[arg(long)]
        all: bool,
    },
    /// Delete a model's files (files another installed model uses are kept).
    Rm { model: String },
    /// Generate an image, or edit one: `run "make it night" --image in.png [--ref a.png …]`.
    /// The model is Auto unless you pick one (it says what it picked and why).
    Run {
        /// What to make, or for an edit, what to change.
        prompt: String,
        /// Edit this image (PNG, JPEG or WebP) instead of making a new one.
        #[arg(long)]
        image: Option<PathBuf>,
        /// A reference image for the edit ("the hat from image 2"); repeat for more (at most 9).
        #[arg(long = "ref", requires = "image")]
        refs: Vec<PathBuf>,
        /// Fast preset (FLUX.2 klein 4B), an alias kept for old scripts.
        #[arg(long, conflicts_with_all = ["quality", "model"])]
        fast: bool,
        /// Quality preset (Z-Image Turbo), an alias kept for old scripts.
        #[arg(long, conflicts_with = "model")]
        quality: bool,
        /// A specific model id (see `list`), or auto (the default).
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        seed: Option<i64>,
        /// Also copy the PNG here.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Download a missing model without asking (required with --json).
        #[arg(short, long)]
        yes: bool,
    },
    /// Engine, GPU, loaded model, and what Auto would pick now for each task.
    Status,
    /// Stop the engine: frees the GPU and memory (a download in progress keeps what it has)
    Stop,
    /// Update a CLI-only or Linux tarball install to the latest release (checked against its
    /// SHA256SUMS; models and images stay). Installs that came with Studio update from Studio
    Update,
    /// What Auto picks now for generate and edit, and why (or which model to pull).
    Auto,
    /// Everything a bug report needs, as Markdown for a GitHub issue (or --json).
    Doctor {
        /// Also develop one Fast image and report its time and peak GPU memory.
        #[arg(long)]
        test: bool,
    },
    /// The data folder: usage, Clear cache, paused downloads, move, link (with --json).
    Storage {
        #[command(subcommand)]
        cmd: Option<storage::StorageCmd>,
    },
    /// Image runtimes (the sd.cpp builds the engine runs).
    Runtime {
        #[command(subcommand)]
        cmd: RuntimeCmd,
    },
    /// MCP server on stdio for agents (Claude Code, Codex). Starts the engine if needed.
    Mcp,
}

#[derive(Subcommand)]
enum RuntimeCmd {
    /// Install runtime zips already on this machine (matched by sha256, whatever their names).
    Import { files: Vec<PathBuf> },
    /// List runtime builds and which are installed.
    List,
}

/// A failure to show the user: plain words plus a machine code for --json.
struct Fail {
    code: String,
    message: String,
}

impl Fail {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }
}

type R<T> = Result<T, Fail>;

struct Client {
    http: reqwest::Client,
    base: String,
    json: bool,
    st: Style,
}

impl Client {
    fn not_running(&self) -> Fail {
        Fail::new("engine_not_running", format!("The engine isn't running. Start it in another terminal with: {BIN} serve"))
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> R<reqwest::Response> {
        let resp = req.send().await.map_err(|e| if e.is_connect() { self.not_running() } else { Fail::new("http_error", e.to_string()) })?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        Err(Fail::new(v["code"].as_str().unwrap_or("http_error"), v["error"].as_str().map(String::from).unwrap_or_else(|| format!("The engine answered HTTP {status}."))))
    }

    /// Every POST goes through here: the engine requires Content-Type: application/json on POST,
    /// even without a body.
    fn post(&self, path: &str, body: &Value) -> reqwest::RequestBuilder {
        self.http.post(format!("{}{path}", self.base)).json(body)
    }

    async fn get(&self, path: &str) -> R<Value> {
        let resp = self.send(self.http.get(format!("{}{path}", self.base))).await?;
        resp.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))
    }

    /// Calls `on_event(name, data)` for each server-sent event until it returns false or the stream ends.
    async fn sse(&self, mut resp: reqwest::Response, mut on_event: impl FnMut(&str, Value) -> bool) -> R<()> {
        // Buffer raw bytes: a multi-byte character can be split across chunks.
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let chunk = resp.chunk().await.map_err(|e| Fail::new("stream_broken", format!("Lost the connection to the engine ({e}).")))?;
            let Some(chunk) = chunk else { return Ok(()) };
            buf.extend_from_slice(&chunk);
            while let Some(end) = buf.windows(2).position(|w| w == b"\n\n") {
                let raw: Vec<u8> = buf.drain(..end + 2).collect();
                let block = String::from_utf8_lossy(&raw);
                let (mut name, mut data) = ("message".to_string(), String::new());
                for line in block.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        name = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push_str(v.trim_start());
                    }
                }
                if data.is_empty() {
                    continue; // keep-alive comment
                }
                let v: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
                if !on_event(&name, v) {
                    return Ok(());
                }
            }
        }
    }

    async fn models(&self) -> R<Value> {
        self.get("/api/models").await
    }

    /// A full id (`z-image-turbo:q4`) or a bare one (`z-image-turbo`: the tag the engine picks
    /// for this machine).
    pub(crate) async fn find_model(&self, id: &str) -> R<Value> {
        let all = self.models().await?;
        let id = all["families"].as_array().into_iter().flatten().find(|f| f["id"] == id).and_then(|f| f["default"].as_str()).unwrap_or(id).to_string();
        all["models"].as_array().and_then(|ms| ms.iter().find(|m| m["id"] == id.as_str()).cloned()).ok_or_else(|| {
            let tags = match id.split_once(':') {
                Some((fam, _)) => all["families"].as_array().into_iter().flatten().find(|f| f["id"] == fam).map(|f| {
                    let v: Vec<&str> = f["variants"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect();
                    format!(" It comes in: {}.", v.join(", "))
                }),
                None => None,
            };
            Fail::new("unknown_model", format!("There's no model called {id:?}.{} Run `{BIN} list --all` to see the models.", tags.unwrap_or_default()))
        })
    }

    /// Gated originals: show the licence and ask (or take --accept-licence), then record the yes.
    async fn licence_gate(&self, m: &Value, accept: bool) -> R<()> {
        if m["licence"]["accept_required"] != true || !m["licence_accepted_ms"].is_null() {
            return Ok(());
        }
        let name = m["licence"]["name"].as_str().or(m["licence"]["spdx"].as_str()).unwrap_or("its licence");
        let url = m["licence"]["url"].as_str().unwrap_or("");
        let model = m["name"].as_str().unwrap_or("This model");
        if !accept {
            use std::io::{IsTerminal, Write};
            if self.json || !std::io::stdin().is_terminal() {
                return Err(Fail::new(
                    "licence_not_accepted",
                    format!("{model} is under the {name} ({url}). Read it, then pull again with --accept-licence to accept it. Nothing was downloaded."),
                ));
            }
            eprintln!("{} {} is under the {}.", self.st.accent("●"), self.st.bold(model), self.st.bold(name));
            eprintln!("  {url}");
            if let Some(note) = m["licence"]["note"].as_str() {
                eprintln!("  {}", self.st.muted(note));
            }
            eprint!("  Its official repo asks you to accept this licence. Accept it and download? [y/N] ");
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            if !matches!(line.trim(), "y" | "Y" | "yes" | "Yes") {
                return Err(Fail::new("licence_not_accepted", "Not accepted, so nothing was downloaded."));
            }
        }
        let id = m["id"].as_str().unwrap_or_default();
        self.send(self.post(&format!("/api/models/{}/licence/accept", enc(id)), &json!({}))).await?;
        Ok(())
    }

    // ---------- pull ----------

    async fn pull(&self, id: &str, accept_licence: bool) -> R<Value> {
        let m = self.find_model(id).await?;
        let id = m["id"].as_str().unwrap_or(id).to_string();
        let id = id.as_str();
        self.licence_gate(&m, accept_licence).await?;
        if !self.json {
            eprintln!(
                "{} {}  {}  {}",
                self.st.accent("●"),
                self.st.bold(m["name"].as_str().unwrap_or(id)),
                self.st.muted(&licence_label(&m)),
                self.st.muted(&{
                    let (model, all) = (m["download_bytes"].as_u64().unwrap_or(0), m["needed_bytes"].as_u64().unwrap_or(0));
                    if all > model { format!("{} to download, {} on disk with the image runtime", gb(model), gb(all)) } else { format!("{} to download", gb(model)) }
                })
            );
        }
        self.stream_pull(id).await
    }

    /// Follows a pull's event stream to the end (download, then a custom model's test image).
    async fn stream_pull(&self, id: &str) -> R<Value> {
        let resp = self.send(self.post(&format!("/api/models/{}/pull", enc(id)), &json!({}))).await?;
        let mut outcome: R<Value> = Err(Fail::new("stream_broken", "The engine closed the download stream early. Pull again; it resumes."));
        let mut current = String::new();
        let mut hinted = false;
        let watch = self.sse(resp, |name, v| match name {
            "progress" => {
                if self.json {
                    return true;
                }
                // A custom model's test image reports as files `test:<stage>`: one line for all of it.
                let testing = v["stage"] == "testing";
                let file = if testing { "test".to_string() } else { v["file"].as_str().unwrap_or("").to_string() };
                let short = if testing { "test image".to_string() } else { file.rsplit('/').next().unwrap_or(&file).to_string() };
                if file != current {
                    if !current.is_empty() {
                        self.st.end_line();
                    }
                    current = file.clone();
                    // The name on its own line keeps the live line short enough not to wrap.
                    eprintln!("  {short}");
                }
                let (bytes, total) = (v["bytes"].as_u64().unwrap_or(0), v["total"].as_u64().unwrap_or(1).max(1));
                let frac = bytes as f64 / total as f64;
                let text = match v["stage"].as_str() {
                    Some("present") => format!("  {} {}", self.st.bar(1.0, 16), self.st.muted("already here")),
                    Some("verifying") => format!("  {} {}", self.st.bar(frac, 16), self.st.muted(&format!("Checking {:.0}%", frac * 100.0))),
                    Some("unpacking") => format!("  {} {}", self.st.bar(1.0, 16), self.st.muted("Unpacking the image runtime")),
                    Some("testing") => {
                        let stage = v["file"].as_str().unwrap_or("").trim_start_matches("test:");
                        match stage {
                            "sampling" => format!("  {} {}", self.st.bar(frac, 16), self.st.muted(&format!("Developing {bytes}/{total}"))),
                            "starting" | "switching_model" => format!("  {} {}", self.st.bar(0.0, 16), self.st.muted("Loading the model for a small test image…")),
                            other => format!("  {} {}", self.st.bar(0.0, 16), self.st.muted(&stage_words(other, ""))),
                        }
                    }
                    Some("waiting") => {
                        // Once per pull: stopping is safe, the next pull carries on from here.
                        if !hinted && !self.json {
                            hinted = true;
                            self.st.end_line();
                            eprintln!("  {}", self.st.muted("Ctrl+C stops it; run the same command again and it continues from here."));
                        }
                        format!("  {} {}", self.st.bar(frac, 16), self.st.muted(&wait_words(&v["wait"])))
                    }
                    _ => format!(
                        "  {} {}",
                        self.st.bar(frac, 16),
                        self.st.muted(&format!("{} / {}  {}", gb(bytes), gb(total), rate(v["speed_bps"].as_u64().unwrap_or(0))))
                    ),
                };
                self.st.line(&text);
                true
            }
            "done" => {
                outcome = Ok(json!({ "model": id, "status": "installed" }));
                false
            }
            "error" => {
                outcome = Err(Fail::new(v["code"].as_str().unwrap_or("pull_failed"), v["message"].as_str().unwrap_or("The pull failed.")));
                false
            }
            _ => true,
        });
        // The engine owns the pull, so leaving would let it carry on: Ctrl-C cancels it explicitly.
        let interrupted = tokio::select! {
            r = watch => { r?; false }
            _ = tokio::signal::ctrl_c() => true,
        };
        if !self.json && !current.is_empty() {
            self.st.end_line();
        }
        if interrupted {
            self.send(self.post(&format!("/api/models/{}/pull/cancel", enc(id)), &json!({}))).await?;
            return Err(Fail::new("cancelled", "Stopped. Pull again to resume; the downloaded part is kept."));
        }
        outcome
    }

    // ---------- custom models ----------

    /// Prints a check report: one line per step.
    fn print_check(&self, r: &Value) {
        for s in r["steps"].as_array().into_iter().flatten() {
            let (mark, text) = match s["status"].as_str() {
                Some("ok") => (self.st.accent("ok"), s["message"].as_str().unwrap_or("").to_string()),
                Some("failed") => (self.st.error("no"), s["message"].as_str().unwrap_or("").to_string()),
                _ => (self.st.muted("--"), self.st.muted("not checked")),
            };
            eprintln!("  {mark}  {}  {text}", pad(s["step"].as_str().unwrap_or(""), 12));
        }
    }

    /// A repo's sizes, grouped by bits like Hugging Face's panel, numbered; returns the files in
    /// number order (the ones that can load).
    fn print_sizes(&self, r: &Value) -> Vec<String> {
        let files: Vec<&Value> = r["files"].as_array().into_iter().flatten().collect();
        let mut picks = vec![];
        if files.is_empty() {
            return picks;
        }
        let ok: Vec<&&Value> = files.iter().filter(|f| f["loadable"] == true).collect();
        let mut bits: Vec<Option<u64>> = ok.iter().map(|f| f["bits"].as_u64()).collect();
        bits.sort_by_key(|b| b.unwrap_or(u64::MAX));
        bits.dedup();
        let qw = ok.iter().map(|f| f["quant"].as_str().unwrap_or("").chars().count() + 14).max().unwrap_or(20);
        eprintln!("  {}", self.st.muted(&format!("{} model files; fit is an estimate for this machine:", files.len())));
        for b in bits {
            let head = b.map(|b| format!("{b}-bit")).unwrap_or_else(|| "other".into());
            for (i, f) in ok.iter().filter(|f| f["bits"].as_u64() == b).enumerate() {
                picks.push(f["file"].as_str().unwrap_or("").to_string());
                let fit = match f["fit"].as_str() {
                    Some("fits") => "fits",
                    Some("offload") => "RAM offload",
                    Some("needs_more_memory") => "too big",
                    Some("no_gpu") => "needs a GPU",
                    _ => "",
                };
                let rec = if f["recommended"] == true { self.st.accent("  recommended for this machine") } else { String::new() };
                let quant = format!("{} {}", f["quant"].as_str().unwrap_or(""), if f["format"] == "gguf" { "GGUF" } else { "safetensors" });
                eprintln!(
                    "    {}  {}  {}  {}  {}{rec}",
                    self.st.muted(&pad(if i == 0 { &head } else { "" }, 6)),
                    self.st.bold(&format!("{:>2}", picks.len())),
                    pad(&quant, qw),
                    pad(&gb(f["size"].as_u64().unwrap_or(0)), 9),
                    pad(fit, 12)
                );
            }
        }
        let no: Vec<&&Value> = files.iter().filter(|f| f["loadable"] != true).collect();
        if !no.is_empty() {
            let names: Vec<&str> = no.iter().map(|f| f["quant"].as_str().unwrap_or("")).collect();
            eprintln!("  {}", self.st.muted(&format!("{} files in formats Dawnroom can't run yet: {}.", no.len(), names.join(", "))));
        }
        picks
    }

    /// `import` without a file: image models already on this computer, numbered; in a terminal the
    /// user picks one, which then goes through the normal import.
    async fn import_found(&self, dir: Option<&std::path::Path>, yes: bool) -> R<Value> {
        let dir = dir.map(|d| std::path::absolute(d).unwrap_or_else(|_| d.to_path_buf()));
        let body = json!({ "dir": dir.as_ref().map(|d| d.to_string_lossy()) });
        if !self.json {
            let where_ = dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "the usual places".into());
            eprintln!("{} {}", self.st.accent("●"), self.st.muted(&format!("Looking for image models in {where_} (headers only)…")));
        }
        let r: Value = self.send(self.post("/api/models/custom/local", &body)).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
        if self.json {
            return Ok(r);
        }
        if dir.is_none() {
            let looked: Vec<String> = r["roots"].as_array().into_iter().flatten().filter(|x| x["exists"] == true).map(|x| format!("{} ({})", x["label"].as_str().unwrap_or(""), x["path"].as_str().unwrap_or(""))).collect();
            eprintln!("  {}", self.st.muted(&format!("Looked in: {}", if looked.is_empty() { "nothing (none of the usual folders exist)".into() } else { looked.join(", ") })));
        }
        let found: Vec<&Value> = r["found"].as_array().into_iter().flatten().collect();
        let skipped = r["skipped"].as_u64().unwrap_or(0);
        let skip_note = match skipped {
            0 => String::new(),
            1 => " (1 other model file there isn't an image model: an LLM, a text encoder, a VAE…)".into(),
            n => format!(" ({n} other model files there aren't image models: LLMs, text encoders, VAEs…)"),
        };
        if found.is_empty() {
            return Err(Fail::new("nothing_found", format!("No image models found{skip_note}. Point at a file or folder: {BIN} import <path>")));
        }
        let mut picks = vec![];
        let fw = found.iter().map(|f| f["file"].as_str().unwrap_or("").chars().count() + 2).max().unwrap_or(30).min(48);
        for f in &found {
            let state = if let Some(id) = f["added"].as_str() {
                format!("already added as {id}")
            } else if let Some(id) = f["library"].as_str() {
                format!("same file as {id} in the library")
            } else if f["ok"] == true {
                let fit = match f["fit"].as_str() {
                    Some("fits") => "fits",
                    Some("offload") => "RAM offload",
                    _ => "",
                };
                let dl = f["download_bytes"].as_u64().unwrap_or(0);
                format!("{fit}{}", if dl > 0 { format!(", {} to download", gb(dl)) } else { String::new() })
            } else {
                f["message"].as_str().unwrap_or("can't be added").to_string()
            };
            let can = f["ok"] == true && f["added"].is_null() && f["library"].is_null();
            let n = if can {
                picks.push(f["path"].as_str().unwrap_or("").to_string());
                self.st.bold(&format!("{:>2}", picks.len()))
            } else {
                "  ".into()
            };
            eprintln!(
                "  {n}  {}  {}  {}  {}",
                pad(f["file"].as_str().unwrap_or(""), fw),
                pad(&gb(f["size"].as_u64().unwrap_or(0)), 9),
                pad(f["architecture"].as_str().unwrap_or(""), 22),
                if can { state } else { self.st.muted(&state) }
            );
            eprintln!("      {}", self.st.muted(&format!("{} · {}", f["place"].as_str().unwrap_or(""), f["path"].as_str().unwrap_or(""))));
        }
        if !skip_note.is_empty() {
            eprintln!("  {}", self.st.muted(skip_note.trim()));
        }
        if picks.is_empty() {
            return Err(Fail::new("nothing_new", "Everything found is already in Dawnroom."));
        }
        if yes || !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            return Err(Fail::new("pick_a_file", format!("Import one with: {BIN} import <path>")));
        }
        eprint!("  Which one? [1-{}, Enter to stop] ", picks.len());
        let _ = std::io::Write::flush(&mut std::io::stderr());
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        let Some(path) = line.trim().parse::<usize>().ok().and_then(|n| picks.get(n.wrapping_sub(1))) else { return Err(Fail::new("pick_a_file", "Nothing picked, so nothing was imported.")) };
        self.add_custom(path, true, yes).await
    }

    /// Asks y/N on the terminal (false without one).
    fn confirm(&self, question: &str) -> bool {
        use std::io::{IsTerminal, Write};
        eprint!("{question} [y/N] ");
        if !std::io::stdin().is_terminal() {
            eprintln!("(no terminal to answer in: pass --yes)");
            return false;
        }
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        matches!(line.trim(), "y" | "Y" | "yes" | "Yes")
    }

    /// `pull hf.co/…` for a model not added yet, and `import <path>`: the compatibility check, the
    /// size and licence, a y/N (or --yes), then the pull with its test image.
    async fn add_custom(&self, source: &str, local: bool, yes: bool) -> R<Value> {
        let body = if local { json!({ "path": source }) } else { json!({ "source": source }) };
        if !self.json {
            eprintln!("{} {}  {}", self.st.accent("●"), self.st.bold(source), self.st.muted("checking (only the file's header is read)"));
        }
        let r: Value = self.send(self.post("/api/models/custom/check", &body)).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
        let picks = if self.json { vec![] } else { self.print_check(&r); self.print_sizes(&r) };
        // No file named and several sizes: in a terminal the user picks (Enter = the recommended one).
        let named = engine::custom::parse_hf(source).is_some_and(|(_, f)| f.is_some());
        if !local && !named && picks.len() > 1 && !yes && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            let checked = r["file"].as_str().unwrap_or("");
            let def = picks.iter().position(|f| f == checked).map(|i| i + 1).unwrap_or(1);
            eprint!("  Which size? [1-{}, Enter = {def}] ", picks.len());
            let _ = std::io::Write::flush(&mut std::io::stderr());
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            let n = if line.trim().is_empty() { Some(def) } else { line.trim().parse::<usize>().ok() };
            let Some(f) = n.and_then(|n| picks.get(n.wrapping_sub(1))) else { return Err(Fail::new("pick_a_file", "Nothing picked, so nothing was downloaded.")) };
            if f != checked {
                let repo = r["repo"]["id"].as_str().unwrap_or_default();
                return Box::pin(self.add_custom(&format!("hf.co/{repo}:{f}"), false, yes)).await;
            }
        }
        if r["ok"] != true {
            if self.json {
                return Ok(json!({ "ok": false, "check": r }));
            }
            // The failed step above already says why.
            return Err(Fail::new(r["code"].as_str().unwrap_or("check_failed"), "Nothing was downloaded."));
        }
        let id = r["id"].as_str().unwrap_or_default().to_string();
        let size = r["download_bytes"].as_u64().unwrap_or(0);
        if !self.json {
            eprintln!("  {}", self.st.muted(&format!("{} will be {id}. {}: its memory fit is an estimate.", r["name"].as_str().unwrap_or(""), r["label"].as_str().unwrap_or(""))));
        }
        if !yes {
            if self.json {
                return Err(Fail::new("confirm_required", format!("The check passed: {} to download. Pass --yes to go ahead.", gb(size))));
            }
            let what = if local { format!("Import it ({} into the models folder) and develop one small test image?", gb(size)) } else { format!("Download {} and develop one small test image?", gb(size)) };
            if !self.confirm(&format!("  {what}")) {
                return Err(Fail::new("not_confirmed", "Not confirmed, so nothing was downloaded."));
            }
        }
        let id = if local {
            let m: Value = self.send(self.post("/api/models/custom/import", &json!({ "path": source }))).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
            m["id"].as_str().unwrap_or(&id).to_string()
        } else {
            id
        };
        self.stream_pull(&id).await?;
        self.custom_result(&id).await
    }

    /// `pull`: a custom source not added yet goes through the check; anything else pulls as usual.
    async fn pull_any(&self, model: &str, accept_licence: bool, yes: bool) -> R<Value> {
        if !engine::custom::is_custom_id(model) {
            return self.pull(model, accept_licence).await;
        }
        let m = match self.find_model(model).await {
            Ok(m) => m,
            Err(_) if engine::custom::parse_hf(model).is_some() => return self.add_custom(model, false, yes).await,
            Err(e) => return Err(e),
        };
        let id = m["id"].as_str().unwrap_or(model).to_string();
        self.pull(&id, accept_licence).await?;
        self.custom_result(&id).await
    }

    /// After a custom model's pull: what its test image said.
    async fn custom_result(&self, id: &str) -> R<Value> {
        let m = self.find_model(id).await?;
        let t = &m["custom"]["test"];
        if !self.json {
            eprintln!("  {}  {}", self.st.accent("ok"), t["message"].as_str().unwrap_or("The test image developed."));
            if let Some(img) = t["image"].as_str() {
                eprintln!("  {}", self.st.muted(&format!("Test image: {img}")));
            }
        }
        Ok(json!({ "model": id, "status": m["state"], "label": m["label"], "test": t }))
    }

    // ---------- run ----------

    /// Stores a local image as an edit input; returns its id (sha256).
    async fn upload(&self, path: &std::path::Path) -> R<Value> {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let v: Value = self.send(self.post("/api/inputs", &json!({ "path": abs.to_string_lossy() }))).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
        Ok(v)
    }

    #[allow(clippy::too_many_arguments)]
    async fn run(&self, prompt: &str, preset: Option<&str>, model: Option<&str>, seed: Option<i64>, output: Option<PathBuf>, yes: bool, image: Option<&std::path::Path>, refs: &[PathBuf]) -> R<Value> {
        let task = if image.is_some() { "edit" } else { "generate" };
        let auto = preset.is_none() && model.is_none_or(|m| m == "auto");
        // Resolve which model this will use, so a missing one can be pulled first (like `ollama run`).
        let model_id = match (model, preset) {
            (Some(m), _) if m != "auto" => m.to_string(),
            (_, Some(id)) => {
                let presets = self.get("/api/presets").await?;
                let list = presets["presets"].as_array().cloned().unwrap_or_default();
                list.iter().find(|p| p["id"] == id).and_then(|p| p["model"].as_str()).map(String::from).ok_or_else(|| Fail::new("unknown_preset", "The engine has no such preset."))?
            }
            _ => {
                // Auto: what it would pick now; if nothing installed qualifies, the model to pull.
                let a = self.get("/api/auto").await?;
                let a = &a[task];
                if a["ok"] == true {
                    a["model"].as_str().unwrap_or_default().to_string()
                } else {
                    let msg = a["error"].as_str().unwrap_or("Auto found no model.").to_string();
                    match a["suggest"]["model"].as_str() {
                        Some(sug) if !self.json || yes => {
                            if !self.json {
                                eprintln!("{}", self.st.muted(&msg));
                            }
                            sug.to_string()
                        }
                        Some(_) => return Err(Fail::new("needs_pull", format!("{msg} Pass --yes to download it."))),
                        None => return Err(Fail::new("no_model_for_task", msg)),
                    }
                }
            }
        };
        // Say what's wrong with the machine before any download, not after.
        let hw = self.get("/api/hardware").await?;
        let choice = &hw["choice"];
        if choice["status"] != "ok" {
            let msg = choice["message"].as_str().or(choice["reason"].as_str()).unwrap_or("This machine can't run images yet.");
            let link = choice["link"].as_str().map(|l| format!(" {l}")).unwrap_or_default();
            return Err(Fail::new(choice["status"].as_str().unwrap_or("no_gpu"), format!("{msg}{link}")));
        }
        let mut m = self.find_model(&model_id).await?;
        while m["state"] == "verifying" {
            if !self.json {
                self.st.line(&self.st.muted(&format!("  Checking {} files…", m["name"].as_str().unwrap_or(""))));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            m = self.find_model(&model_id).await?;
        }
        if m["state"] != "installed" {
            if self.json && !yes {
                return Err(Fail::new(
                    "needs_pull",
                    format!("{model_id} isn't downloaded ({} under {}). Pass --yes to download it, or run `{BIN} pull {model_id}`.", gb(m["download_bytes"].as_u64().unwrap_or(0)), licence_label(&m)),
                ));
            }
            let id = m["id"].as_str().unwrap_or(&model_id).to_string();
            self.pull(&id, false).await?;
        }

        let mut body = json!({ "prompt": prompt, "model": if auto { "auto".to_string() } else { model_id.clone() }, "task": task });
        if let Some(s) = seed {
            body["seed"] = json!(s);
        }
        if let Some(img) = image {
            let v = self.upload(img).await?;
            if !self.json {
                let from = v["image_id"].as_str().map(|id| format!(", Library image {}", &id[..8.min(id.len())])).unwrap_or_default();
                eprintln!("{}", self.st.muted(&format!("  image  {} ({}x{}{from})", img.display(), v["width"], v["height"])));
            }
            body["image"] = v["sha256"].clone();
            let mut ids = Vec::new();
            for r in refs {
                let v = self.upload(r).await?;
                if !self.json {
                    eprintln!("{}", self.st.muted(&format!("  ref    {} ({}x{})", r.display(), v["width"], v["height"])));
                }
                ids.push(v["sha256"].clone());
            }
            body["refs"] = json!(ids);
        }
        let job: Value = self.send(self.post("/api/jobs", &body)).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
        let id = job["id"].as_str().unwrap_or_default().to_string();
        if !self.json {
            match job["params"]["auto"].as_str() {
                Some(reason) => eprintln!("{} {}{}", self.st.accent("●"), self.st.muted("Auto · "), self.st.bold(reason)),
                None => {
                    let label = preset.map(|p| format!("{}{} · ", p[..1].to_uppercase(), &p[1..])).unwrap_or_default();
                    eprintln!("{} {}{}", self.st.accent("●"), self.st.muted(&label), self.st.bold(job["model"].as_str().unwrap_or(&model_id)));
                }
            }
        }

        let events = self.send(self.http.get(format!("{}/api/jobs/{id}/events", self.base))).await?;
        let mut finished: Option<Value> = None;
        let watch = self.sse(events, |name, v| {
            match name {
                "progress" if !self.json => {
                    let stage = v["stage"].as_str().unwrap_or("");
                    let text = match stage {
                        "sampling" => {
                            let (n, t) = (v["step"].as_u64().unwrap_or(0), v["total"].as_u64().unwrap_or(1).max(1));
                            format!("  {} Developing {n}/{t}", self.st.bar(n as f64 / t as f64, 24))
                        }
                        other => format!("  {}", self.st.muted(&stage_words(other, m["name"].as_str().unwrap_or("")))),
                    };
                    self.st.line(&text);
                }
                "finished" => {
                    finished = Some(v["job"].clone());
                    return false;
                }
                "snapshot" if matches!(v["status"].as_str(), Some("completed" | "failed" | "cancelled")) => {
                    finished = Some(v.clone());
                    return false;
                }
                _ => {}
            }
            true
        });
        // Ctrl-C cancels the job in the engine, then we wait for its final state.
        tokio::select! {
            r = watch => r?,
            _ = tokio::signal::ctrl_c() => {
                if !self.json { self.st.line(&self.st.muted("  Cancelling…")); }
                let _ = self.send(self.post(&format!("/api/jobs/{id}/cancel"), &json!({}))).await;
                for _ in 0..120 {
                    let j = self.get(&format!("/api/jobs/{id}")).await?;
                    if matches!(j["status"].as_str(), Some("completed" | "failed" | "cancelled")) { finished = Some(j); break; }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
        if !self.json {
            self.st.end_line();
        }
        let job = finished.ok_or_else(|| Fail::new("stream_broken", "Lost track of the job. Check the engine's terminal."))?;
        match job["status"].as_str() {
            Some("completed") => {}
            Some("cancelled") => return Err(Fail::new("cancelled", "Cancelled.")),
            _ => return Err(Fail::new("generation_failed", job["error"].as_str().unwrap_or("Generation failed. Try again."))),
        }
        let mut path = job["output_path"].as_str().unwrap_or_default().to_string();
        if let Some(out) = output {
            std::fs::copy(&path, &out).map_err(|e| Fail::new("copy_failed", format!("Made the image but couldn't write {} ({e}). It's at {path}.", out.display())))?;
            path = out.to_string_lossy().into_owned();
        }
        let secs = (job["finished_ms"].as_i64().unwrap_or(0) - job["started_ms"].as_i64().unwrap_or(0)) as f64 / 1000.0;
        Ok(json!({
            "id": job["id"], "status": "completed", "path": path, "prompt": job["prompt"], "model": job["model"],
            "task": job["params"]["task"], "auto": job["params"]["auto"], "inputs": job["params"]["inputs"], "parent_id": job["parent_id"],
            "seed": job["seed"], "steps": job["params"]["steps"], "width": job["params"]["width"], "height": job["params"]["height"],
            "seconds": (secs * 10.0).round() / 10.0,
        }))
    }
}

/// "Network error, trying again in 12 s" from a pull event's `wait`.
fn wait_words(w: &Value) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
    let secs = ((w["retry_at_ms"].as_i64().unwrap_or(now) - now) as f64 / 1000.0).ceil().max(0.0) as u64;
    let what = match w["kind"].as_str() {
        Some("connection") => "Network error".to_string(),
        _ => w["reason"].as_str().unwrap_or("Waiting").to_string(),
    };
    format!("{what}, trying again in {secs} s")
}

fn stage_words(stage: &str, model: &str) -> String {
    match stage {
        "installing_runtime" => "Installing the image runtime…".into(),
        "starting" => "Warming up…".into(),
        "switching_model" => format!("Switching model to {model}…"),
        "conditioning" => "Reading the prompt…".into(),
        "preparing" | "vae_encode" => "Reading the input image…".into(),
        "vae_decode" => "Finishing…".into(),
        "encoding" => "Saving…".into(),
        other => format!("{other}…"),
    }
}

fn licence_label(m: &Value) -> String {
    let spdx = m["licence"]["spdx"].as_str().unwrap_or("?");
    // Custom models: the card's own words ("No licence stated", "Other (see the model card)").
    if m["custom"].is_object() && matches!(spdx, "NOASSERTION" | "LicenseRef-other") {
        return m["licence"]["name"].as_str().unwrap_or(spdx).to_string();
    }
    let name = match spdx {
        "LicenseRef-CreativeML-OpenRAIL-PlusPlus-M" => "OpenRAIL++",
        other => other.strip_prefix("LicenseRef-").unwrap_or(other),
    };
    // "FLUX-1-dev-Non-Commercial (non-commercial)" says it twice.
    let name = name.trim_end_matches("-Non-Commercial");
    if m["licence"]["commercial"] != true && m["licence"]["use_restrictions"] == true {
        format!("{name} (restricted)")
    } else if m["licence"]["commercial"] != true {
        format!("{name} (non-commercial)")
    } else if m["licence"]["use_restrictions"] == true {
        format!("{name} (use restrictions)")
    } else {
        name.to_string()
    }
}

/// "gen", "edit", "gen+edit".
fn tasks_label(m: &Value) -> String {
    let t: Vec<&str> = m["tasks"].as_array().into_iter().flatten().filter_map(|t| t.as_str()).collect();
    t.iter().map(|x| if *x == "generate" { "gen" } else { x }).collect::<Vec<_>>().join("+")
}

/// The warm (else cold) time per task on this GPU: "10 s", "10 s, edit 18 s", or "" when we have none for this machine.
fn time_label(m: &Value) -> String {
    let parts: Vec<String> = m["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str())
        .filter_map(|t| {
            let x = &m["timing"][t];
            if x["here"] != true {
                return None;
            }
            let s = x["warm_s"].as_f64().or(x["cold_s"].as_f64())?;
            let tag = if t == "edit" && m["tasks"].as_array().is_some_and(|a| a.len() > 1) { "edit " } else { "" };
            Some(format!("{tag}{s:.0} s"))
        })
        .collect();
    parts.join(", ")
}

fn fit_label(m: &Value) -> String {
    let need = m["fit_detail"]["need_mib"].as_u64().map(|v| format!(" {:.1} GB", v as f64 / 1024.0)).unwrap_or_default();
    // An estimate (nobody measured it here) is marked "est." and never reads as measured.
    let est = if m["fit_detail"]["estimate"] == true { " est." } else { "" };
    match m["fit"].as_str() {
        Some("fits") => format!("fits{need}{est}"),
        Some("offload") => format!("RAM offload{need}{est}"),
        Some("needs_more_memory") => format!("needs more{need}{est}"),
        Some("no_gpu") => "no GPU".into(),
        _ => "?".into(),
    }
}

/// A model id as one URL path segment (custom ids have slashes: `hf.co/user/repo:file`).
pub(crate) fn enc(id: &str) -> String {
    let mut out = String::new();
    for b in id.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub(crate) fn pad(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(s.chars().count())))
}

/// `POST /api/shutdown`, then waits (up to 15 s) for the port to close. Not running is fine too.
async fn stop(c: &Client) -> R<Value> {
    match c.send(c.post("/api/shutdown", &json!({}))).await {
        Err(f) if f.code == "engine_not_running" => {
            if !c.json {
                println!("The engine isn't running.");
            }
            return Ok(json!({ "stopped": false, "running": false }));
        }
        Err(f) => return Err(f),
        Ok(_) => {}
    }
    for _ in 0..60 {
        if c.http.get(format!("{}/api/health", c.base)).send().await.is_err() {
            if !c.json {
                println!("Engine stopped.");
            }
            return Ok(json!({ "stopped": true, "running": false }));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(Fail::new("still_running", "Asked the engine to stop, but it's still answering after 15 s."))
}

async fn list(c: &Client, all: bool) -> R<Value> {
    let v = c.models().await?;
    if c.json {
        return Ok(v);
    }
    let gpu = match v["gpu"]["name"].as_str() {
        Some(n) => format!("{n}, {} MiB", v["gpu"]["vram_total_mib"]),
        None => "none found".into(),
    };
    println!("{}", c.st.muted(&format!("GPU: {gpu}")));
    let models: Vec<&Value> = v["models"].as_array().into_iter().flatten().collect();
    let state_of = |m: &Value| match m["state"].as_str().unwrap_or("?") {
        "needs_test" => c.st.muted("files here, no test image yet (pull to run it)"),
        "test_failed" => c.st.muted("test image failed (pull to retry, or rm)"),
        "installed" if m["loaded"] == true => c.st.accent("installed, loaded"),
        "installed" => c.st.accent("installed"),
        "partial" if m["paused"].is_object() => {
            let p = &m["paused"];
            c.st.muted(&format!("paused at {} of {} (pull to resume)", gb(p["bytes"].as_u64().unwrap_or(0)), gb(p["total"].as_u64().unwrap_or(0))))
        }
        "partial" => c.st.muted(&format!("partial ({} left)", gb(m["download_bytes"].as_u64().unwrap_or(0)))),
        "not_installed" => c.st.muted("not installed"),
        other => c.st.muted(other),
    };
    // The licence column fits the longest label (plus " *" and a gap), so it never runs into FIT.
    let lw = models.iter().map(|m| licence_label(m).chars().count() + 4).max().unwrap_or(0).max(12);
    // MODEL fits the longest id shown (custom ids are long).
    let mw = models.iter().filter(|m| m["custom"].is_object() || m["state"] != "not_installed" || all).map(|m| m["id"].as_str().unwrap_or("").chars().count() + 2).max().unwrap_or(0).max(26);
    let any_custom = models.iter().any(|m| m["custom"].is_object());
    if all {
        // One row per model, for the tag a bare id means here; the other tags are listed.
        let head = format!("{}{}{}{}{}{}{}{}", pad("MODEL", mw), pad("TAGS", 11), pad("TASKS", 10), pad("SIZE", 9), pad("LICENCE", lw), pad("FIT", 24), pad("TIME", 22), "STATE");
        println!("{}", c.st.muted(&head));
        for f in v["families"].as_array().into_iter().flatten() {
            let Some(m) = models.iter().find(|m| m["id"] == f["default"]) else { continue };
            let tags: Vec<&str> = f["variants"].as_array().into_iter().flatten().filter_map(|x| x.as_str()?.split(':').nth(1)).collect();
            let gate = if m["licence"]["accept_required"] == true && m["licence_accepted_ms"].is_null() { " *" } else { "" };
            println!(
                "{}{}{}{}{}{}{}{}",
                pad(m["id"].as_str().unwrap_or(""), mw),
                pad(&tags.join(","), 11),
                pad(&tasks_label(m), 10),
                pad(&gb(m["size_bytes"].as_u64().unwrap_or(0)), 9),
                pad(&format!("{}{gate}", licence_label(m)), lw),
                pad(&fit_label(m), 24),
                pad(&time_label(m), 22),
                if m["custom"].is_object() { format!("{} · custom", state_of(m)) } else { state_of(m) }
            );
        }
        println!("{}", c.st.muted("est. = estimated from file sizes. * = you accept its licence before the download."));
        println!("{}", c.st.muted("TASKS: gen = makes images, edit = edits them. TIME = one image on this GPU, shown only where we have one."));
        if any_custom {
            println!("{}", c.st.muted("custom = Custom, not reviewed by Dawnroom (added with pull hf.co/… or import)."));
        }
        println!("{}", c.st.muted(&format!("Pull one with `{BIN} pull <model>` (the tag shown) or `{BIN} pull <model>:<tag>`.")));
        return Ok(Value::Null);
    }
    // Installed (or partly downloaded) variants, plus whatever the presets use.
    let head = format!("{}{}{}{}{}{}", pad("MODEL", mw), pad("PRESET", 9), pad("SIZE", 9), pad("LICENCE", lw), pad("FIT", 24), "STATE");
    println!("{}", c.st.muted(&head));
    for m in models.iter().filter(|m| m["state"] != "not_installed" || m["presets"].as_array().is_some_and(|p| !p.is_empty())) {
        let presets: Vec<&str> = m["presets"].as_array().into_iter().flatten().filter_map(|p| p.as_str()).collect();
        println!(
            "{}{}{}{}{}{}",
            pad(m["id"].as_str().unwrap_or(""), mw),
            pad(&if m["custom"].is_object() { "custom".to_string() } else { presets.join(",") }, 9),
            pad(&gb(m["size_bytes"].as_u64().unwrap_or(0)), 9),
            pad(&licence_label(m), lw),
            pad(&fit_label(m), 24),
            state_of(m)
        );
    }
    if any_custom {
        println!("{}", c.st.muted("custom = Custom, not reviewed by Dawnroom (added with pull hf.co/… or import; its fit is an estimate)."));
    }
    let curated = v["families"].as_array().into_iter().flatten().filter(|f| f["custom"] != true).count();
    println!("{}", c.st.muted(&format!("{curated} models in the library: `{BIN} list --all`.")));
    Ok(Value::Null)
}

async fn status(c: &Client) -> R<Value> {
    let h = c.get("/api/health").await?;
    if c.json {
        return Ok(h);
    }
    let model = h["model"].as_str().unwrap_or("none loaded");
    println!("{} {} {}  {} {}  {}", c.st.tile(), c.st.bold(PRODUCT_NAME), h["version"].as_str().unwrap_or(""), c.st.orb(), c.st.muted("by Dawnlight Labs"), c.st.muted(&c.base));
    println!("  backend  {} ({model}){}", h["backend"].as_str().unwrap_or("?"), if h["busy"] == true { ", busy" } else { "" });
    if let Some(n) = h["gpu"]["name"].as_str() {
        println!("  gpu      {n}, {} / {} MiB used", h["gpu"]["vram_used_mib"], h["gpu"]["vram_total_mib"]);
    }
    println!("  runtime  {}{}", h["runtime"].as_str().unwrap_or("none"), if h["runtime_ready"] == true { "" } else { " (not installed yet)" });
    print_auto(c, &c.get("/api/auto").await?);
    Ok(Value::Null)
}

fn print_auto(c: &Client, a: &Value) {
    for (task, label) in [("generate", "generate"), ("edit", "edit    ")] {
        let x = &a[task];
        if x["ok"] == true {
            println!("  auto     {label} → {}", x["reason"].as_str().unwrap_or(""));
        } else {
            println!("  auto     {label} → {}", c.st.muted(x["error"].as_str().unwrap_or("")));
        }
    }
}

async fn auto(c: &Client) -> R<Value> {
    let a = c.get("/api/auto").await?;
    if c.json {
        return Ok(a);
    }
    println!("{}", c.st.muted(&format!("loaded: {}", a["loaded"].as_str().unwrap_or("nothing"))));
    print_auto(c, &a);
    Ok(Value::Null)
}

async fn runtime_import(c: &Client, files: &[PathBuf]) -> R<Value> {
    let abs: Vec<String> = files.iter().map(|f| std::path::absolute(f).unwrap_or_else(|_| f.clone()).to_string_lossy().into_owned()).collect();
    if !c.json {
        eprintln!("{}", c.st.muted("Checking the files' sha256 against the runtime registry…"));
    }
    let v: Value = c.send(c.post("/api/runtimes/import", &json!({ "paths": abs }))).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
    if !c.json {
        for f in v["files"].as_array().into_iter().flatten() {
            match f["matched"].as_str() {
                Some(m) => println!("  {}  {} = {m}", c.st.accent("ok"), f["path"].as_str().unwrap_or("")),
                None => println!("  {}  {} matches no runtime file", c.st.muted("--"), f["path"].as_str().unwrap_or("")),
            }
        }
        println!("{}", if v["active_installed"] == true { c.st.accent("This machine's runtime is installed.") } else { c.st.muted("This machine's runtime still isn't installed; pulling a model installs it.") });
    }
    Ok(v)
}

async fn runtime_list(c: &Client) -> R<Value> {
    let v = c.get("/api/runtimes").await?;
    if !c.json {
        for r in v["runtimes"].as_array().into_iter().flatten() {
            let active = if r["id"] == v["active"] { c.st.accent(" (this machine)") } else { String::new() };
            let state = if r["installed"] == true { "installed" } else { "not installed" };
            println!("  {}{active}  {}  {}{}", pad(r["build"].as_str().unwrap_or(""), 16), pad(&gb(r["download_bytes"].as_u64().unwrap_or(0)), 9), state, if r["experimental"] == true { ", experimental" } else { "" });
        }
    }
    Ok(v)
}

async fn rm(c: &Client, id: &str) -> R<Value> {
    let v: Value = c.send(c.http.delete(format!("{}/api/models/{}", c.base, enc(id)))).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
    if !c.json {
        for p in v["deleted"].as_array().into_iter().flatten() {
            println!("  deleted  {}", p.as_str().unwrap_or(""));
        }
        for p in v["kept"].as_array().into_iter().flatten() {
            println!("  {}", c.st.muted(&format!("kept     {} (another installed model uses it)", p.as_str().unwrap_or(""))));
        }
    }
    Ok(v)
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Cmd::Serve { data_dir, sd_server, runtime, gpu, registry_dir, idle_secs, hf_endpoint } = cli.cmd {
        let mut cfg = Config { port: cli.port, runtime, gpu, ..Config::resolved() };
        if let Some(d) = data_dir { cfg.data_dir = d; }
        if sd_server.is_some() { cfg.sd_server = sd_server; }
        if let Some(r) = registry_dir { cfg.registry_dir = r; }
        if let Some(s) = idle_secs { cfg.idle_timeout = Duration::from_secs(s); }
        if let Some(e) = hf_endpoint { cfg.hf_endpoint = e.trim_end_matches('/').to_string(); }
        return match engine::serve(cfg).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }

    let c = Client {
        http: reqwest::Client::builder().no_proxy().build().expect("http client"),
        base: format!("http://127.0.0.1:{}", cli.port),
        json: cli.json || matches!(cli.cmd, Cmd::Mcp), // MCP: stdout is JSON-RPC only, never text
        st: Style::detect(),
    };
    if let Cmd::Mcp = cli.cmd {
        return match mcp::run(c, cli.port).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let result = match &cli.cmd {
        Cmd::Serve { .. } | Cmd::Mcp => unreachable!(),
        Cmd::Pull { model, accept_licence, yes } => c.pull_any(model, *accept_licence, *yes).await,
        Cmd::Import { path: Some(path), yes } if !path.is_dir() => {
            let abs = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            c.add_custom(&abs.to_string_lossy(), true, *yes).await
        }
        Cmd::Import { path, yes } => c.import_found(path.as_deref(), *yes).await,
        Cmd::List { all } => list(&c, *all).await,
        Cmd::Rm { model } => rm(&c, model).await,
        Cmd::Status => status(&c).await,
        Cmd::Stop => stop(&c).await,
        Cmd::Update => update::update(&c).await,
        Cmd::Auto => auto(&c).await,
        Cmd::Doctor { test } => doctor::doctor(&c, *test, &Config::resolved()).await,
        Cmd::Runtime { cmd: RuntimeCmd::Import { files } } => runtime_import(&c, files).await,
        Cmd::Runtime { cmd: RuntimeCmd::List } => runtime_list(&c).await,
        Cmd::Storage { cmd } => storage::run(&c, cmd.as_ref()).await,
        Cmd::Run { prompt, image, refs, fast, quality, model, seed, output, yes } => {
            let preset = if *fast { Some("fast") } else if *quality { Some("quality") } else { None };
            c.run(prompt, preset, model.as_deref(), *seed, output.clone(), *yes, image.as_deref(), refs).await
        }
    };
    match result {
        Ok(v) => {
            if c.json {
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            } else if let Cmd::Run { .. } = cli.cmd {
                println!("{} {}", c.st.accent("Ready"), v["path"].as_str().unwrap_or(""));
                println!(
                    "{}",
                    c.st.muted(&format!("{} ▸ seed {} ▸ {} steps ▸ {}s ▸ {}x{}", v["model"].as_str().unwrap_or(""), v["seed"], v["steps"], v["seconds"], v["width"], v["height"]))
                );
                if let Some(parent) = v["parent_id"].as_str() {
                    println!("{}", c.st.muted(&format!("edited from Library image {parent}")));
                }
            } else if let Cmd::Pull { .. } | Cmd::Import { .. } = &cli.cmd {
                println!("{} {} is ready", c.st.accent("Ready"), v["model"].as_str().unwrap_or(""));
            }
            ExitCode::SUCCESS
        }
        Err(f) => {
            if c.json {
                println!("{}", serde_json::to_string_pretty(&json!({ "error": f.message, "code": f.code })).unwrap());
            } else {
                eprintln!("{}", c.st.error(&f.message));
            }
            ExitCode::FAILURE
        }
    }
}
