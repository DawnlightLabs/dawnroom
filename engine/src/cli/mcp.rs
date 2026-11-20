//! `dawnroom mcp`: an MCP server on stdio so agents (Claude Code, Codex) can make images.
//! Like the rest of the CLI it is a thin client of the engine's HTTP API. Stdout carries
//! JSON-RPC only; every log line goes to stderr. If the engine isn't running, the first tool
//! call starts it as a detached, windowless daemon that logs to `.data/logs/daemon.log`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine as _;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ProgressNotificationParam, RequestMetaObject};
use rmcp::{ErrorData, Peer, RoleServer, ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};

use super::{Client, Fail, R};
use engine::config::{Config, PRODUCT_NAME};

/// generate_image returns within this long (engine start included), handing back a job id if the
/// image isn't ready. Codex's default tool timeout is 60 s.
const WAIT: Duration = Duration::from_secs(45);
const PREVIEW_PX: u32 = 512;

pub struct Mcp {
    c: Client,
    port: u16,
    /// Serialises "is the engine up? if not, start it" so two calls can't start two daemons.
    starting: Mutex<()>,
}

pub async fn run(c: Client, port: u16) -> Result<(), String> {
    eprintln!("[mcp] {PRODUCT_NAME} MCP server on stdio (engine port {port})");
    let service = Mcp { c, port, starting: Mutex::new(()) }.serve(rmcp::transport::stdio()).await.map_err(|e| e.to_string())?;
    service.waiting().await.map_err(|e| e.to_string())?;
    eprintln!("[mcp] client closed the connection");
    Ok(())
}

// ---------- tool arguments ----------

#[derive(Deserialize, schemars::JsonSchema, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Preset {
    /// FLUX.2 klein 4B: about 10-18 s.
    Fast,
    /// Z-Image Turbo: about 20-30 s, best at legible text.
    Quality,
}

#[derive(Deserialize, schemars::JsonSchema, Clone, Copy)]
enum Aspect {
    #[serde(rename = "1:1")]
    Square,
    #[serde(rename = "4:3")]
    Landscape4x3,
    #[serde(rename = "3:4")]
    Portrait3x4,
    #[serde(rename = "3:2")]
    Landscape3x2,
    #[serde(rename = "2:3")]
    Portrait2x3,
    #[serde(rename = "16:9")]
    Wide,
    #[serde(rename = "9:16")]
    Tall,
}

impl Aspect {
    fn as_str(self) -> &'static str {
        match self {
            Aspect::Square => "1:1",
            Aspect::Landscape4x3 => "4:3",
            Aspect::Portrait3x4 => "3:4",
            Aspect::Landscape3x2 => "3:2",
            Aspect::Portrait2x3 => "2:3",
            Aspect::Wide => "16:9",
            Aspect::Tall => "9:16",
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
struct GenerateArgs {
    /// What to draw, in plain words: subject, setting, style, light. Words in quotes are drawn as text.
    prompt: String,
    /// Leave out to let Auto pick (the default: the best installed, tested model that fits this
    /// GPU, preferring the one already loaded; the result says what it picked and why).
    /// "fast" / "quality" pin FLUX.2 klein 4B / Z-Image Turbo.
    preset: Option<Preset>,
    /// Aspect ratio, about 1 megapixel (default 1:1).
    aspect: Option<Aspect>,
    /// Fix the seed to reproduce or vary an image. Omit for a random one; the result reports it.
    seed: Option<i64>,
    /// Things to keep out of the image. Optional; most prompts don't need it.
    negative_prompt: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct EditArgs {
    /// What to change, in plain words ("make it night", "replace the sign's text with 'OPEN'"). For
    /// references, refer to them as image 2, image 3… (the edited image is image 1).
    instruction: String,
    /// Full path of the image to edit (PNG, JPEG or WebP) on this computer. A Dawnroom output's path
    /// links the edit to it in the Library.
    image: String,
    /// Full paths of reference images (optional, at most 9).
    refs: Option<Vec<String>>,
    /// A model id from list_models that can edit (tasks includes "edit"). Leave out for Auto.
    model: Option<String>,
    /// Fix the seed to reproduce an edit. Omit for a random one; the result reports it.
    seed: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct PullArgs {
    /// Model id from list_models, e.g. "z-image-turbo".
    id: String,
    /// Must be true. Pulling downloads several GB, so ask the user first.
    confirm: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct CheckArgs {
    /// A Hugging Face model, `hf.co/<user>/<repo>` or `hf.co/<user>/<repo>:<file>`, or the full path of a model file on this computer.
    source: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct JobArgs {
    /// Job id from generate_image or list_images.
    id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ListImagesArgs {
    /// How many of the newest images to list (default 10, at most 50).
    limit: Option<u32>,
}

// ---------- results ----------

fn text(v: Value) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(serde_json::to_string_pretty(&v).unwrap())])
}

fn fail(f: Fail) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(serde_json::to_string_pretty(&json!({ "error": f.message, "code": f.code })).unwrap())])
}

fn ok(r: R<CallToolResult>) -> Result<CallToolResult, ErrorData> {
    Ok(r.unwrap_or_else(fail))
}

fn gb(bytes: u64) -> f64 {
    (bytes as f64 / 1e9 * 10.0).round() / 10.0
}

fn licence(m: &Value) -> Value {
    let l = &m["licence"];
    json!({ "spdx": l["spdx"], "commercial": l["commercial"], "use_restrictions": l["use_restrictions"], "note": l["note"], "url": l["url"] })
}

/// Downscales a PNG to a JPEG whose longest side is at most `PREVIEW_PX`, as base64.
fn preview_jpeg(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {path} ({e})."))?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("Couldn't decode {path} ({e})."))?;
    let img = if img.width().max(img.height()) > PREVIEW_PX { img.thumbnail(PREVIEW_PX, PREVIEW_PX) } else { img };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img.to_rgb8()).map_err(|e| format!("Couldn't encode the preview ({e})."))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(out))
}

/// A completed job as the agent sees it: recipe + path as text, plus the small preview image.
async fn completed(job: &Value) -> CallToolResult {
    let path = job["output_path"].as_str().unwrap_or_default().to_string();
    let secs = (job["finished_ms"].as_i64().unwrap_or(0) - job["started_ms"].as_i64().unwrap_or(0)) as f64 / 1000.0;
    let recipe = json!({
        "status": "completed",
        "job_id": job["id"],
        "path": path,
        "prompt": job["prompt"],
        "model": job["model"],
        "seed": job["seed"],
        "steps": job["params"]["steps"],
        "width": job["params"]["width"],
        "height": job["params"]["height"],
        "seconds": (secs * 10.0).round() / 10.0,
        "task": job["params"]["task"],
        // Why Auto picked the model (null when a model or preset was named).
        "auto": job["params"]["auto"],
        "edited_from": job["parent_id"],
        "note": "The full-size PNG is at path (recipe embedded). The image below is a small preview."
    });
    let mut content = vec![ContentBlock::text(serde_json::to_string_pretty(&recipe).unwrap())];
    let p = path.clone();
    match tokio::task::spawn_blocking(move || preview_jpeg(&p)).await.map_err(|e| e.to_string()).and_then(|r| r) {
        Ok(b64) => content.push(ContentBlock::image(b64, "image/jpeg")),
        Err(e) => eprintln!("[mcp] preview failed: {e}"),
    }
    CallToolResult::success(content)
}

fn job_summary(job: &Value) -> Value {
    json!({
        "job_id": job["id"], "status": job["status"], "prompt": job["prompt"], "model": job["model"],
        "seed": job["seed"], "progress": job["progress"], "error": job["error"],
    })
}

/// Maps engine progress to 0-100 plus a few words, for MCP progress notifications.
fn progress_of(ev: &Value) -> (f64, String) {
    match ev["stage"].as_str().unwrap_or("") {
        "sampling" => {
            let (n, t) = (ev["step"].as_f64().unwrap_or(0.0), ev["total"].as_f64().unwrap_or(1.0).max(1.0));
            (5.0 + 90.0 * n / t, format!("Developing {n}/{t}"))
        }
        "starting" => (1.0, "Warming up the model".into()),
        "switching_model" => (1.0, "Switching model".into()),
        "conditioning" => (4.0, "Reading the prompt".into()),
        "preparing" | "vae_encode" => (3.0, "Reading the input image".into()),
        "vae_decode" => (96.0, "Finishing".into()),
        "encoding" => (98.0, "Saving".into()),
        other => (1.0, other.to_string()),
    }
}

// ---------- engine lifecycle ----------

impl Mcp {
    async fn healthy(&self) -> bool {
        let req = self.c.http.get(format!("{}/api/health", self.c.base)).timeout(Duration::from_secs(5));
        matches!(req.send().await, Ok(r) if r.status().is_success())
    }

    /// Makes sure the engine answers, starting it as a daemon if it doesn't.
    async fn engine(&self) -> R<()> {
        if self.healthy().await {
            return Ok(());
        }
        let _guard = self.starting.lock().await;
        if self.healthy().await {
            return Ok(());
        }
        let log = spawn_daemon(self.port)?;
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(30) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if self.healthy().await {
                eprintln!("[mcp] started the engine in {:.1}s (log: {})", t0.elapsed().as_secs_f32(), log.display());
                return Ok(());
            }
        }
        Err(Fail::new("engine_start_failed", format!("Started the engine but it didn't answer within 30 s. Its log is at {}.", log.display())))
    }

    async fn model(&self, id: &str) -> R<Value> {
        self.c.find_model(id).await
    }

    /// The model a job will use, or the answer to give instead (not downloaded, still checking…).
    /// `named`: a model id or preset id; `None` = Auto for this task.
    async fn ready_model(&self, task: &str, named: Option<String>, deadline: tokio::time::Instant) -> R<Result<String, CallToolResult>> {
        let model_id = match named {
            Some(id) if id == "fast" || id == "quality" => {
                let presets = self.c.get("/api/presets").await?;
                presets["presets"].as_array().into_iter().flatten().find(|p| p["id"] == id.as_str()).and_then(|p| p["model"].as_str()).unwrap_or_default().to_string()
            }
            Some(id) => id,
            None => {
                let a = self.c.get("/api/auto").await?;
                let a = &a[task];
                if a["ok"] == true {
                    a["model"].as_str().unwrap_or_default().to_string()
                } else if let Some(sug) = a["suggest"]["model"].as_str() {
                    let m = self.model(sug).await?;
                    return Ok(Err(text(json!({
                        "status": "needs_pull",
                        "model": sug,
                        "name": m["name"],
                        "message": a["error"],
                        "download_gb": gb(m["download_bytes"].as_u64().unwrap_or(0)),
                        "licence": licence(&m),
                        "fit": m["fit"],
                        "next": "No tested model for this is installed. Tell the user the size and licence and ask before calling pull_model with this id and confirm: true.",
                    }))));
                } else {
                    return Err(Fail::new("no_model_for_task", a["error"].as_str().unwrap_or("No model for this runs here.")));
                }
            }
        };
        let mut m = self.model(&model_id).await?;
        while m["state"] == "verifying" && tokio::time::Instant::now() + Duration::from_secs(1) < deadline {
            tokio::time::sleep(Duration::from_secs(1)).await; // hashing files after start-up
            m = self.model(&model_id).await?;
        }
        match m["state"].as_str() {
            Some("installed") => {}
            Some("pulling") => {
                return Ok(Err(text(json!({ "status": "pulling", "model": model_id, "pull": m["pull"], "next": "The model is downloading. Check list_models and try again when its state is installed." }))));
            }
            Some("verifying") => return Ok(Err(text(json!({ "status": "verifying", "model": model_id, "next": "The engine is still checking model files after start-up. Try again in a minute." })))),
            _ => {
                return Ok(Err(text(json!({
                    "status": "needs_pull",
                    "model": model_id,
                    "name": m["name"],
                    "download_gb": gb(m["download_bytes"].as_u64().unwrap_or(0)),
                    "licence": licence(&m),
                    "fit": m["fit"],
                    "next": "Not downloaded. Tell the user the size and licence and ask before calling pull_model with this id and confirm: true.",
                }))));
            }
        }

        Ok(Ok(model_id))
    }

    async fn generate(&self, a: GenerateArgs, peer: Peer<RoleServer>, meta: RequestMetaObject) -> R<CallToolResult> {
        // The whole call (engine start, file checks, generation) fits in WAIT.
        let deadline = tokio::time::Instant::now() + WAIT;
        self.engine().await?;
        let preset = a.preset.map(|p| match p {
            Preset::Fast => "fast",
            Preset::Quality => "quality",
        });
        let model_id = match self.ready_model("generate", preset.map(String::from), deadline).await? {
            Ok(id) => id,
            Err(answer) => return Ok(answer),
        };
        let mut body = match preset {
            Some(p) => json!({ "prompt": a.prompt, "preset": p }),
            None => json!({ "prompt": a.prompt, "model": "auto" }),
        };
        if let Some(x) = a.aspect {
            body["aspect"] = json!(x.as_str());
        }
        if let Some(s) = a.seed {
            body["seed"] = json!(s);
        }
        if let Some(n) = a.negative_prompt {
            body["negative_prompt"] = json!(n);
        }
        self.run_job(body, &model_id, peer, meta, deadline).await
    }

    async fn edit(&self, a: EditArgs, peer: Peer<RoleServer>, meta: RequestMetaObject) -> R<CallToolResult> {
        let deadline = tokio::time::Instant::now() + WAIT;
        self.engine().await?;
        // Inputs first: a wrong path is the most likely mistake and costs nothing to report.
        let upload = |path: String| async move {
            let v: Value = self.c.send(self.c.post("/api/inputs", &json!({ "path": path }))).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
            Ok::<Value, Fail>(v)
        };
        let image = upload(a.image.clone()).await?;
        let mut refs = Vec::new();
        for r in a.refs.unwrap_or_default() {
            refs.push(upload(r).await?["sha256"].clone());
        }
        let model_id = match self.ready_model("edit", a.model.clone(), deadline).await? {
            Ok(id) => id,
            Err(answer) => return Ok(answer),
        };
        let mut body = json!({ "prompt": a.instruction, "task": "edit", "image": image["sha256"], "refs": refs, "model": a.model.unwrap_or_else(|| "auto".into()) });
        if let Some(s) = a.seed {
            body["seed"] = json!(s);
        }
        self.run_job(body, &model_id, peer, meta, deadline).await
    }

    /// Submits a job and waits (until `deadline`) for its result, with progress notifications.
    async fn run_job(&self, body: Value, model_id: &str, peer: Peer<RoleServer>, meta: RequestMetaObject, deadline: tokio::time::Instant) -> R<CallToolResult> {
        let job: Value = self.c.send(self.c.post("/api/jobs", &body)).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
        let id = job["id"].as_str().unwrap_or_default().to_string();
        eprintln!("[mcp] job {id} ({model_id}) queued");

        // Progress notifications only if the client asked for them (sent a progressToken).
        let (tx, mut rx) = mpsc::unbounded_channel::<(f64, String)>();
        if let Some(token) = meta.get_progress_token() {
            tokio::spawn(async move {
                while let Some((p, msg)) = rx.recv().await {
                    let n = ProgressNotificationParam::new(token.clone(), p).with_total(100.0).with_message(msg);
                    if peer.notify_progress(n).await.is_err() {
                        break;
                    }
                }
            });
        }

        let events = self.c.send(self.c.http.get(format!("{}/api/jobs/{id}/events", self.c.base))).await?;
        let mut finished: Option<Value> = None;
        let watch = self.c.sse(events, |name, v| match name {
            "progress" => {
                let _ = tx.send(progress_of(&v));
                true
            }
            "finished" => {
                finished = Some(v["job"].clone());
                false
            }
            "snapshot" if matches!(v["status"].as_str(), Some("completed" | "failed" | "cancelled")) => {
                finished = Some(v);
                false
            }
            _ => true,
        });
        let timed_out = tokio::time::timeout_at(deadline, watch).await.is_err();
        drop(tx);
        if timed_out {
            let now = self.c.get(&format!("/api/jobs/{id}")).await?;
            return Ok(text(json!({ "status": now["status"], "job_id": id, "progress": now["progress"], "next": "Still developing (queued behind other jobs or a cold start). Call get_job with this job_id; it returns the image when done." })));
        }
        let job = finished.ok_or_else(|| Fail::new("stream_broken", "Lost track of the job. Call get_job with its id."))?;
        self.finished(&job).await
    }

    async fn finished(&self, job: &Value) -> R<CallToolResult> {
        match job["status"].as_str() {
            Some("completed") => Ok(completed(job).await),
            Some("cancelled") => Ok(text(job_summary(job))),
            Some("failed") => Err(Fail::new("generation_failed", job["error"].as_str().unwrap_or("Generation failed. Try again."))),
            _ => Ok(text(job_summary(job))),
        }
    }

    async fn models(&self) -> R<CallToolResult> {
        self.engine().await?;
        let all = self.c.models().await?;
        // One entry per model, for the size tag that suits this GPU (installed variants too), so
        // the list stays short; `tags` names the others (pull `<id>:<tag>` for one of those).
        let defaults: Vec<&str> = all["families"].as_array().into_iter().flatten().filter_map(|f| f["default"].as_str()).collect();
        let models: Vec<Value> = all["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| defaults.contains(&m["id"].as_str().unwrap_or("")) || m["state"] != "not_installed")
            .map(|m| {
                let tags: Vec<Value> = all["families"].as_array().into_iter().flatten().find(|f| f["id"] == m["family"]).map(|f| f["variants"].as_array().cloned().unwrap_or_default()).unwrap_or_default();
                let measured = &m["measured"][0];
                json!({
                    "id": m["id"], "name": m["name"], "tagline": m["tagline"], "presets": m["presets"], "tags": tags,
                    // Custom models (added by the user): "Custom, not reviewed by Dawnroom".
                    "custom": m["label"],
                    "tier": m["tier"],
                    // generate and/or edit, with how tested each is (Auto only uses tested ones).
                    "tasks": m["tasks"], "tiers": m["tiers"],
                    // Measured times per task, with where ("on your GPU" or "on <gpu>"); null = no timing yet.
                    "timing": m["timing"],
                    "state": m["state"], "loaded": m["loaded"],
                    "size_gb": gb(m["size_bytes"].as_u64().unwrap_or(0)),
                    "download_gb": gb(m["download_bytes"].as_u64().unwrap_or(0)),
                    "fit": m["fit"],
                    "fit_is_estimate": m["fit_detail"]["estimate"] == true,
                    "licence_acceptance": if m["licence"]["accept_required"] != true { json!("not needed") } else if m["licence_accepted_ms"].is_null() { json!("needed: only the user can accept, in Dawnroom Studio or the CLI") } else { json!("accepted") },
                    "measured": { "gpu": measured["gpu"], "peak_vram_mib": measured["peak_vram_mib"], "cold_s": measured["cold_s"], "warm_s": measured["warm_s"] },
                    "licence": licence(m),
                    "pull": m["pull"], "pull_error": m["pull_error"], "paused": m["paused"],
                })
            })
            .collect();
        Ok(text(json!({ "gpu": all["gpu"], "models": models })))
    }

    async fn pull(&self, a: PullArgs) -> R<CallToolResult> {
        self.engine().await?;
        // Custom models are the user's call: they see the check and answer in a terminal.
        if engine::custom::is_custom_id(&a.id) {
            let ready = self.model(&a.id).await.is_ok_and(|m| m["state"] == "installed");
            if ready {
                return Ok(text(json!({ "status": "installed", "model": a.id })));
            }
            return Err(Fail::new(
                "custom_needs_user",
                format!(
                    "Custom models aren't reviewed by Dawnroom, so only the user adds them: ask them to run `dawnroom pull {}` in a terminal, which shows the compatibility check, the size and the licence and asks them to confirm. You can call check_model first to see whether it would work. Nothing was downloaded.",
                    a.id
                ),
            ));
        }
        let m = self.model(&a.id).await?;
        let id = m["id"].as_str().unwrap_or(&a.id).to_string();
        // Gated originals: the user accepts the licence themselves. An agent can't, by design.
        if m["licence"]["accept_required"] == true && m["licence_accepted_ms"].is_null() {
            let name = m["licence"]["name"].as_str().or(m["licence"]["spdx"].as_str()).unwrap_or("its licence");
            return Err(Fail::new(
                "licence_not_accepted",
                format!(
                    "{} is under the {name} ({}). Its official repo asks people to accept that licence, so only the user can accept it: ask them to open Dawnroom Studio (Models) or run `dawnroom pull {id}` in a terminal and answer yes. You can't accept it for them. Nothing was downloaded.",
                    m["name"].as_str().unwrap_or(&id),
                    m["licence"]["url"].as_str().unwrap_or("")
                ),
            ));
        }
        let summary = json!({ "model": id, "name": m["name"], "download_gb": gb(m["download_bytes"].as_u64().unwrap_or(0)), "licence": licence(&m), "fit": m["fit"], "fit_is_estimate": m["fit_detail"]["estimate"] == true, "tier": m["tier"] });
        if !a.confirm {
            return Err(Fail::new("confirm_required", format!("Pulling {id} downloads {} GB. Ask the user, then call again with confirm: true. Details: {summary}", summary["download_gb"])));
        }
        if m["state"] == "installed" {
            return Ok(text(json!({ "status": "installed", "model": id })));
        }
        // Starts the pull (or joins one); the engine owns it, so dropping the stream is fine.
        drop(self.c.send(self.c.post(&format!("/api/models/{id}/pull"), &json!({}))).await?);
        let mut out = summary;
        out["status"] = json!("pulling");
        out["next"] = json!("Downloading in the engine; it carries on if this session ends. Check list_models for progress; generate_image works once state is installed.");
        Ok(text(out))
    }

    async fn check(&self, source: &str) -> R<CallToolResult> {
        self.engine().await?;
        // The engine works out what it is: user/repo, a Hugging Face link, hf.co/…, or a path.
        let body = json!({ "source": source });
        let r: Value = self.c.send(self.c.post("/api/models/custom/check", &body)).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
        let steps: Vec<Value> = r["steps"].as_array().into_iter().flatten().map(|s| json!({ "step": s["step"], "status": s["status"], "message": s["message"] })).collect();
        Ok(text(json!({
            "ok": r["ok"], "id": r["id"], "name": r["name"], "label": r["label"], "code": r["code"], "message": r["message"], "steps": steps,
            "repo": r["repo"], "file": r["file"], "architecture": r["architecture"],
            // Each: file, quant, bits, size, loadable (+ reason), fit (an estimate), recommended, selected.
            "files": r["files"],
            "download_gb": gb(r["download_bytes"].as_u64().unwrap_or(0)), "fit": r["fit"]["tier"], "fit_is_estimate": true, "licence": r["licence"],
            "next": if r["ok"] == true { "It would work. Only the user can add it: ask them to run `dawnroom pull <source>` in a terminal (it asks them to confirm and runs one test image)." } else { "It can't be added; tell the user the message." },
        })))
    }

    async fn job(&self, id: &str) -> R<CallToolResult> {
        self.engine().await?;
        let job = self.c.get(&format!("/api/jobs/{id}")).await?;
        self.finished(&job).await
    }

    async fn cancel(&self, id: &str) -> R<CallToolResult> {
        self.engine().await?;
        let v: Value = self.c.send(self.c.post(&format!("/api/jobs/{id}/cancel"), &json!({}))).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
        Ok(text(v))
    }

    async fn images(&self, limit: Option<u32>) -> R<CallToolResult> {
        self.engine().await?;
        let limit = limit.unwrap_or(10).clamp(1, 50);
        let page = self.c.get(&format!("/api/images?limit={limit}")).await?;
        let items: Vec<Value> = page["items"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|j| json!({ "job_id": j["id"], "path": j["output_path"], "prompt": j["prompt"], "model": j["model"], "seed": j["seed"], "width": j["params"]["width"], "height": j["params"]["height"], "created_ms": j["created_ms"] }))
            .collect();
        Ok(text(json!({ "total": page["total"], "items": items })))
    }
}

#[tool_router]
impl Mcp {
    #[tool(
        description = "Generate one image on the user's own GPU (local, no cloud, no credits). Waits for it, then returns the PNG's file path, its recipe (model, seed, steps, size, seconds) and a preview of at most 512 px. Without a preset Auto picks the model (installed, tested, fits; it prefers the one already loaded) and the result says which and why. Time on an 8 GB laptop GPU: fast 10-18 s, quality 20-30 s (the first image after a while also loads the model; switching presets adds about 2 s). Uses 4.5-5.5 GB of VRAM while generating; the engine frees it after 5 idle minutes. If it takes longer than 45 s it returns status \"running\" with a job_id: call get_job. If the model isn't downloaded it returns status \"needs_pull\" with the size and licence, and never downloads by itself.",
        annotations(title = "Generate image", read_only_hint = false, destructive_hint = false, open_world_hint = false)
    )]
    async fn generate_image(&self, Parameters(a): Parameters<GenerateArgs>, peer: Peer<RoleServer>, meta: RequestMetaObject) -> Result<CallToolResult, ErrorData> {
        ok(self.generate(a, peer, meta).await)
    }

    #[tool(
        description = "Edit an image on the user's own GPU (local, no cloud, no credits): give the full path of the image and an instruction (\"make it night\", \"change the sign to say OPEN\"), plus optional reference image paths. Returns the edited PNG's path, its recipe (model, seed, why Auto picked the model, which image it was edited from) and a small preview. Auto picks a tested edit model that's installed and fits (FLUX.2 klein 4B edits in about 10-20 s on an 8 GB laptop GPU). If none is installed it returns status \"needs_pull\" with the size and licence and never downloads by itself. Past 45 s it returns a job_id: call get_job. The images never leave the machine.",
        annotations(title = "Edit image", read_only_hint = false, destructive_hint = false, open_world_hint = false)
    )]
    async fn edit_image(&self, Parameters(a): Parameters<EditArgs>, peer: Peer<RoleServer>, meta: RequestMetaObject) -> Result<CallToolResult, ErrorData> {
        ok(self.edit(a, peer, meta).await)
    }

    #[tool(
        description = "List the image models: one entry per model for the size tag that suits this GPU (plus installed ones), with its other tags, installed or not, size, licence (commercial use, use restrictions, whether the user must accept it first), tier (tested = measured by Dawnroom, untested = not yet), whether it fits this GPU (fit_is_estimate: true means estimated from file sizes, not measured), its tasks (generate, edit) and how tested it is per task, measured times per task (\"on your GPU\" or the GPU it was measured on; null = no timing yet), which preset uses each, and download progress. Instant.",
        annotations(title = "List models", read_only_hint = true, open_world_hint = false)
    )]
    async fn list_models(&self) -> Result<CallToolResult, ErrorData> {
        ok(self.models().await)
    }

    #[tool(
        description = "Download a model by id (`z-image-turbo`, or `z-image-turbo:q8` for one size tag): several GB from Hugging Face, sha256-checked. Requires confirm: true; ask the user first and tell them the size and licence from list_models. Some models' official repos are gated: those need the user to accept the licence themselves, in Dawnroom Studio or the CLI, and this tool can't accept it for them; it returns licence_not_accepted with the licence name and link, so ask the user to do it. Custom models (hf.co/… or imported files) are refused with custom_needs_user: only the user adds them, in a terminal (use check_model to see whether one would work). Returns immediately; the download runs in the engine and carries on if this session ends. Check list_models for progress.",
        annotations(title = "Download model", read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = true)
    )]
    async fn pull_model(&self, Parameters(a): Parameters<PullArgs>) -> Result<CallToolResult, ErrorData> {
        ok(self.pull(a).await)
    }

    #[tool(
        description = "Check whether a custom model would run in Dawnroom, without downloading it: `hf.co/<user>/<repo>[:<file>]` from a public Hugging Face repo, or a model file on this computer. Reads only the file's header and reports each step (source, format, architecture, companion files, size and memory fit as an estimate, licence) in plain words, with the first failure's reason. Read-only: it never adds or downloads anything; only the user can add a custom model, with `dawnroom pull` in a terminal. A few seconds.",
        annotations(title = "Check custom model", read_only_hint = true, open_world_hint = true)
    )]
    async fn check_model(&self, Parameters(a): Parameters<CheckArgs>) -> Result<CallToolResult, ErrorData> {
        ok(self.check(&a.source).await)
    }

    #[tool(
        description = "Look up an image job. A completed job returns the same path, recipe and preview as generate_image; a running one returns its progress. Instant.",
        annotations(title = "Get job", read_only_hint = true, open_world_hint = false)
    )]
    async fn get_job(&self, Parameters(a): Parameters<JobArgs>) -> Result<CallToolResult, ErrorData> {
        ok(self.job(&a.id).await)
    }

    #[tool(
        description = "Cancel a queued or running image job. A running job stops within about a second.",
        annotations(title = "Cancel job", read_only_hint = false, destructive_hint = true, open_world_hint = false)
    )]
    async fn cancel_job(&self, Parameters(a): Parameters<JobArgs>) -> Result<CallToolResult, ErrorData> {
        ok(self.cancel(&a.id).await)
    }

    #[tool(
        description = "List the newest generated images, with file paths and recipes (no previews; use get_job for one). Instant.",
        annotations(title = "List images", read_only_hint = true, open_world_hint = false)
    )]
    async fn list_images(&self, Parameters(a): Parameters<ListImagesArgs>) -> Result<CallToolResult, ErrorData> {
        ok(self.images(a.limit).await)
    }
}

#[tool_handler(
    name = "dawnroom",
    instructions = "Local image generation and editing on the user's GPU. Call generate_image with a prompt (Auto picks the model; preset \"fast\" for quick drafts, \"quality\" for text), or edit_image with an image path and an instruction. Results are files on disk: give the user the path. Never pull a model without asking the user first."
)]
impl ServerHandler for Mcp {}

// ---------- daemon ----------

/// Starts `<this exe> serve` detached: no console window, its own process group (Ctrl-C in the
/// agent's terminal doesn't reach it) and, where allowed, outside the agent's Job Object, so it
/// outlives this MCP server. Its stdio goes to a log file, never ours (that would corrupt JSON-RPC).
fn spawn_daemon(port: u16) -> R<PathBuf> {
    let logs = Config::resolved().logs_dir();
    std::fs::create_dir_all(&logs).map_err(|e| Fail::new("engine_start_failed", format!("Couldn't create {} ({e}).", logs.display())))?;
    let log_path = logs.join("daemon.log");
    let open = || engine::logs::open_append(&log_path);
    let (out, err) = match (open(), open()) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return Err(Fail::new("engine_start_failed", format!("Couldn't open {} ({e}).", log_path.display()))),
    };
    let exe = std::env::current_exe().map_err(|e| Fail::new("engine_start_failed", e.to_string()))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--port", &port.to_string(), "serve"]).stdin(std::process::Stdio::null()).stdout(out).stderr(err);
    eprintln!("[mcp] engine not running; starting it (log: {})", log_path.display());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        keep_std_handles_private();
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if let Err(e) = cmd.spawn() {
            // The agent's Job Object may forbid breakaway; then the daemon lives as long as that job.
            eprintln!("[mcp] couldn't start the engine outside the client's job ({e}); starting it inside");
            cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
            cmd.spawn().map_err(|e| Fail::new("engine_start_failed", format!("Couldn't start the engine ({e}).")))?;
        }
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        cmd.spawn().map_err(|e| Fail::new("engine_start_failed", format!("Couldn't start the engine ({e}).")))?;
    }
    Ok(log_path)
}

/// Our stdin/stdout are the agent's pipes. If the daemon inherited them, the agent would never see
/// EOF after we exit. Mark them non-inheritable before spawning.
#[cfg(windows)]
fn keep_std_handles_private() {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        unsafe {
            let h = GetStdHandle(which);
            if !h.is_null() {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}
