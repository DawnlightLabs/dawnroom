//! One FIFO worker, exactly one job in flight. The worker also owns the idle timer: after
//! `idle` with no jobs it stops the backend, so a stop can never race a running job.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::backend::{Backend, BackendError, BackendState, GenRequest, Progress};
use crate::config::PRODUCT_NAME;
use crate::db::{Db, JobRecord};
use crate::models::Models;
use crate::auto;
use crate::png_meta;
use crate::registry::{self, Manifest, Task, TestResult};

/// Edits take the image plus at most this many references (FLUX.3 klein's documented limit is 10 images).
pub const MAX_REFS: usize = 9;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageParams {
    pub prompt: String,
    #[serde(default)]
    pub negative_prompt: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// "1:1", "16:9", … (see `registry::ASPECTS`). Can't be combined with width/height.
    pub aspect: Option<String>,
    /// Missing or negative means random; the chosen seed is always recorded.
    pub seed: Option<i64>,
    /// "fast" | "quality": aliases for old clients (each names one pinned model). Ignored when
    /// `model` is set. Neither set means Auto.
    pub preset: Option<String>,
    /// A registry model id, "auto" (the default: see auto.rs), or a preset id.
    pub model: Option<String>,
    /// "generate" | "edit". Missing: edit when `image` is set, else generate.
    pub task: Option<String>,
    /// Edits: the image to edit, as the sha256 `POST /api/inputs` returned.
    pub image: Option<String>,
    /// Edits: reference images (sha256s), e.g. "put the hat from image 2 on the cat". At most 9.
    #[serde(default)]
    pub refs: Vec<String>,
    /// Sampling steps, 1 to 50. Default: the model's own.
    pub steps: Option<u32>,
    /// Guidance (CFG) scale, 0 to 20. Default: the model's own.
    pub cfg: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub job_id: String,
    pub kind: EventKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    Started,
    Progress { stage: String, step: Option<u32>, total: Option<u32> },
    Preview { step: u32, total: u32, b64_png: String },
    // Boxed: a job record is much bigger than the other events.
    Finished { job: Box<JobRecord> },
}

struct Pending {
    id: String,
    req: GenRequest,
    done: oneshot::Sender<JobRecord>,
}

pub struct Engine {
    backend: Arc<dyn Backend>,
    pub db: Arc<Db>,
    pub models: Arc<Models>,
    outputs_dir: PathBuf,
    /// `<data>/inputs`: edit inputs by sha256.
    pub inputs_dir: PathBuf,
    /// The model of the last job submitted: what will be loaded once the queue drains (Auto
    /// prefers it, so it never switches needlessly).
    queued_model: Mutex<Option<String>>,
    tx: mpsc::UnboundedSender<Pending>,
    pub events: broadcast::Sender<Event>,
    /// Latest progress of the running job (no preview frames).
    live: Mutex<HashMap<String, Progress>>,
    /// Cancel flags of jobs that are queued or running.
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Jobs submitted and not yet finished.
    in_flight: AtomicUsize,
    shutting_down: AtomicBool,
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

fn bad(code: &str, message: impl Into<String>) -> BackendError {
    BackendError::with_message(code, message)
}

impl Engine {
    pub fn start(backend: Arc<dyn Backend>, db: Arc<Db>, models: Arc<Models>, outputs_dir: PathBuf, idle: Duration) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(256);
        let engine = Arc::new(Self {
            backend,
            db,
            models,
            inputs_dir: outputs_dir.with_file_name("inputs"),
            outputs_dir,
            queued_model: Mutex::new(None),
            tx,
            events,
            live: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
            in_flight: AtomicUsize::new(0),
            shutting_down: AtomicBool::new(false),
        });
        tokio::spawn(worker(engine.clone(), rx, idle));
        // Custom models finish their pull with one small test image through this queue.
        let weak = Arc::downgrade(&engine);
        engine.models.set_tester(Arc::new(move |id, on| {
            let weak = weak.clone();
            Box::pin(async move {
                match weak.upgrade() {
                    Some(e) => e.test_model(&id, on).await,
                    None => TestResult { ok: false, at_ms: now_ms(), message: "The engine is shutting down.".into(), seconds: None, image: None, job: None, width: 0, height: 0, steps: 0 },
                }
            })
        }));
        engine
    }

    /// Validates, records the job as queued, and puts it on the queue. The receiver
    /// resolves with the final record; drop it for fire-and-forget.
    pub fn submit(&self, p: ImageParams) -> Result<(JobRecord, oneshot::Receiver<JobRecord>), BackendError> {
        self.submit_kind(p, "image")
    }

    /// `kind` is "image", or "test" for a custom model's test image (kept out of the Library, and
    /// the only job a custom model that hasn't passed its test may run).
    fn submit_kind(&self, p: ImageParams, kind: &str) -> Result<(JobRecord, oneshot::Receiver<JobRecord>), BackendError> {
        let prompt = p.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(bad("invalid_request", "The prompt is empty. Describe the image you want."));
        }
        let task = match p.task.as_deref() {
            Some(t) => Task::parse(t).map_err(|e| bad("invalid_request", e))?,
            None if p.image.is_some() => Task::Edit,
            None => Task::Generate,
        };
        match task {
            Task::Edit if p.image.is_none() => return Err(bad("invalid_request", "An edit needs the image to edit: send its id from POST /api/inputs as image.")),
            Task::Generate if p.image.is_some() || !p.refs.is_empty() => {
                return Err(bad("invalid_request", "Input images are for edits. Leave out image and refs, or send task edit."));
            }
            _ => {}
        }
        if p.refs.len() > MAX_REFS {
            return Err(bad("invalid_request", format!("An edit takes at most {MAX_REFS} reference images (got {}).", p.refs.len())));
        }
        // Edit inputs: stored by sha256; the first is the image to edit.
        let mut inputs = Vec::new();
        for sha in p.image.iter().chain(&p.refs) {
            inputs.push(crate::inputs::get(&self.inputs_dir, sha).map_err(|e| bad("input_not_found", e))?);
        }
        // An explicit model wins (or "auto", or a preset id), then the preset, then Auto. A bare
        // model id (old history rows, "Same seed" on them) resolves to the default tag here.
        let (m, auto_reason) = self.choose(task, p.model.as_deref(), p.preset.as_deref())?;
        let m = &m;
        if !m.supports(task) {
            let reg = self.models.registry();
            let editors: Vec<&str> = reg.families().into_iter().filter(|f| reg.variants(f).iter().any(|v| v.supports(Task::Edit) && !v.is_custom())).collect();
            let msg = match task {
                Task::Edit => format!("{} can't edit images. Use model auto, or an edit model: {}.", m.name, editors.join(", ")),
                Task::Generate => format!("{} only edits images: send it an image to edit, or use model auto to make a new image.", m.name),
            };
            return Err(bad("task_not_supported", msg));
        }
        self.models.runtimes.usable()?;
        let fit = self.models.fit_for(m, task);
        if fit.tier == crate::fit::Tier::NeedsMoreMemory {
            let integrated = self.models.machine.is_some_and(|mc| mc.integrated);
            return Err(bad("needs_more_memory", crate::fit::needs_more_message(&m.name, &fit, integrated)));
        }
        if !self.models.is_installed(m) {
            let msg = match self.models.model_state(m) {
                "verifying" => format!("{} is still being checked after start-up. Try again in a minute.", m.name),
                "pulling" => format!("{} is still downloading. Try again when the pull finishes.", m.name),
                _ => format!("{} isn't downloaded yet. Pull {} first.", m.name, m.id),
            };
            return Err(bad("model_not_installed", msg));
        }
        if kind != "test" && !m.custom_ready() {
            let msg = match m.custom.as_ref().and_then(|c| c.test.as_ref()) {
                Some(t) => format!("{} failed its test image: {} Pull it again to retry the test, or delete it.", m.name, t.message),
                None => format!("{} ({}) hasn't made its test image yet. Run `dawnroom pull {}` to run it.", m.name, registry::CUSTOM_LABEL, m.id),
            };
            return Err(bad("model_not_ready", msg));
        }
        let d = &m.defaults;
        let parent_id = match inputs.first() {
            Some(i) => self.db.image_by_sha256(&i.sha256).map_err(|e| bad("db_error", format!("Couldn't read history ({e}).")))?,
            None => None,
        };
        let (width, height) = match &p.aspect {
            Some(_) if p.width.is_some() || p.height.is_some() => {
                return Err(bad("invalid_request", "Send either aspect or width/height, not both."));
            }
            Some(a) => registry::aspect_size(a).map_err(|e| bad("invalid_request", e))?,
            // An edit keeps the input's shape, at about the model's own size.
            None if p.width.is_none() && p.height.is_none() && !inputs.is_empty() => crate::inputs::edit_size(inputs[0].width, inputs[0].height, d.width * d.height),
            None => (p.width.unwrap_or(d.width), p.height.unwrap_or(d.height)),
        };
        for (name, v) in [("width", width), ("height", height)] {
            if !(256..=2048).contains(&v) || v % 8 != 0 {
                return Err(bad("invalid_request", format!("The {name} must be a multiple of 8 between 256 and 2048 (got {v}).")));
            }
        }
        let steps = p.steps.unwrap_or(d.steps);
        if !(1..=50).contains(&steps) {
            return Err(bad("invalid_request", format!("Steps must be between 1 and 50 (got {steps}). {} is tuned for {} steps.", m.name, d.steps)));
        }
        let cfg = p.cfg.unwrap_or(d.cfg);
        if !(0.0..=20.0).contains(&cfg) {
            return Err(bad("invalid_request", format!("Guidance must be between 0 and 20 (got {cfg}). {} is tuned for guidance {}.", m.name, d.cfg)));
        }
        let seed = p.seed.filter(|s| *s >= 0).unwrap_or_else(|| fastrand::u32(..) as i64);
        let negative_prompt = if p.negative_prompt.trim().is_empty() { d.negative_prompt.clone().unwrap_or_default() } else { p.negative_prompt };
        let cancel = Arc::new(AtomicBool::new(false));
        let req = GenRequest {
            model: self.models.spec(m),
            cancel: cancel.clone(),
            task,
            inputs: inputs.iter().map(|i| i.sha256.clone()).collect(),
            input_paths: inputs.iter().map(|i| i.path.clone()).collect(),
            prompt: prompt.clone(),
            negative_prompt,
            width,
            height,
            seed,
            steps,
            cfg,
            sampler: d.sampler.clone(),
            scheduler: d.scheduler.clone(),
            distilled_guidance: d.distilled_guidance,
            flow_shift: d.flow_shift,
            sigmas: d.sigmas.clone(),
        };
        let mut job = JobRecord {
            id: uuid::Uuid::new_v4().to_string(),
            kind: kind.into(),
            status: "queued".into(),
            prompt,
            params: {
                let mut v = serde_json::to_value(&req).unwrap_or_default();
                if let Some(r) = &auto_reason {
                    v["auto"] = serde_json::json!(r); // why Auto picked this model
                }
                v
            },
            seed,
            model: m.id.clone(),
            created_ms: now_ms(),
            started_ms: None,
            finished_ms: None,
            output_path: None,
            error: None,
            frame: 0,
            favourite: false,
            parent_id,
        };
        job.frame = self.db.insert(&job).map_err(|e| bad("db_error", format!("Couldn't save the job to history ({e}). Check the data folder is writable.")))?;
        self.cancels.lock().unwrap().insert(job.id.clone(), cancel);
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        *self.queued_model.lock().unwrap() = Some(m.id.clone());
        let (done, rx) = oneshot::channel();
        if self.tx.send(Pending { id: job.id.clone(), req, done }).is_err() {
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            return Err(bad("shutting_down", "The engine is shutting down. Start it again."));
        }
        Ok((job, rx))
    }

    /// The model for a job and, when Auto chose it, why.
    fn choose(&self, task: Task, model: Option<&str>, preset: Option<&str>) -> Result<(Manifest, Option<String>), BackendError> {
        let preset_id = |id: &str| registry::PRESETS.iter().any(|p| p.id == id);
        let id = match (model, preset) {
            (Some("auto"), _) | (None, None) => {
                return match self.auto(task) {
                    Ok(pick) => Ok((self.models.resolve(&pick.model)?, Some(pick.reason))),
                    Err(no) => Err(bad("no_model_for_task", no.message)),
                };
            }
            (Some(id), _) if preset_id(id) && self.models.resolve(id).is_err() => registry::preset_model(Some(id)).map_err(|e| bad("unknown_model", e))?,
            (Some(id), _) => id,
            (None, Some(p)) => registry::preset_model(Some(p)).map_err(|e| bad("unknown_model", e))?,
        };
        let m = self.models.resolve(id).map_err(|e| bad("unknown_model", e.message))?;
        Ok((m, None))
    }

    /// The model that will be loaded when a job submitted now runs: the last queued job's while
    /// the queue is busy, else what sd-server has loaded.
    pub fn expected_loaded(&self) -> Option<String> {
        if self.is_busy() { self.queued_model.lock().unwrap().clone() } else { self.backend.loaded_model() }
    }

    /// Auto's pick for this task on this machine, now (see auto.rs). Never downloads.
    pub fn auto(&self, task: Task) -> Result<auto::Pick, auto::NoPick> {
        let reg = self.models.registry();
        let cands: Vec<auto::Candidate> = reg
            .models
            .iter()
            .map(|m| auto::Candidate { m, installed: self.models.is_installed(m) && m.custom_ready(), fit: self.models.fit_for(m, task).tier })
            .collect();
        let loaded = self.expected_loaded();
        auto::pick(task, &cands, loaded.as_deref(), &|m| self.models.timing(m, task)).map_err(|mut no| {
            // Right after start-up the files are still being hashed: say so rather than "pull".
            let checking = reg.models.iter().any(|m| m.supports(task) && m.testing_for(task) == registry::Testing::Tested && self.models.model_state(m) == "verifying");
            if checking {
                no.message = "The engine is still checking model files after start-up. Try again in a minute.".into();
                no.suggest = None;
            }
            no
        })
    }

    /// One small real image from a custom model: the test that decides "ready". Small (at most
    /// 512 px a side), few steps (at most 8, or the model's fixed schedule), a fixed seed.
    /// Reports the job's stages to `on` and fails if the image comes out as one flat colour.
    pub async fn test_model(&self, id: &str, on: crate::models::TestProgress) -> TestResult {
        let failed = |msg: String, w: u32, h: u32, steps: u32| TestResult { ok: false, at_ms: now_ms(), message: msg, seconds: None, image: None, job: None, width: w, height: h, steps };
        let m = match self.models.resolve(id) {
            Ok(m) => m,
            Err(e) => return failed(e.message, 0, 0, 0),
        };
        let d = &m.defaults;
        let (w, h) = if d.width.max(d.height) <= 512 { (d.width, d.height) } else { (512, 512) };
        let steps = if d.sigmas.is_some() { d.steps } else { d.steps.min(8) };
        let p = ImageParams { prompt: TEST_PROMPT.into(), width: Some(w), height: Some(h), seed: Some(42), model: Some(m.id.clone()), steps: Some(steps), ..Default::default() };
        let mut events = self.events.subscribe();
        let (job, mut done) = match self.submit_kind(p, "test") {
            Ok(x) => x,
            Err(e) => return failed(e.message, w, h, steps),
        };
        let rec = loop {
            tokio::select! {
                r = &mut done => break r.ok(),
                ev = events.recv() => match ev {
                    Ok(Event { job_id, kind: EventKind::Progress { stage, step, total } }) if job_id == job.id => on(stage, step, total),
                    Err(broadcast::error::RecvError::Closed) => break (&mut done).await.ok(),
                    _ => {}
                },
            }
        };
        let Some(rec) = rec else { return failed("The test image was lost (the engine stopped).".into(), w, h, steps) };
        if rec.status != "completed" {
            let why = rec.error.unwrap_or_else(|| format!("The test image ended as {}.", rec.status));
            return failed(format!("The test image didn't develop: {why}"), w, h, steps);
        }
        let path = rec.output_path.clone().unwrap_or_default();
        let secs = (rec.finished_ms.unwrap_or(0) - rec.started_ms.unwrap_or(0)) as f64 / 1000.0;
        match flat_colour(std::path::Path::new(&path)) {
            Ok(false) => TestResult { ok: true, at_ms: now_ms(), message: format!("The test image developed in {secs:.1} s ({w}x{h}, {steps} steps)."), seconds: Some(secs), image: Some(path), job: Some(job.id.clone()), width: w, height: h, steps },
            Ok(true) => TestResult {
                ok: false,
                at_ms: now_ms(),
                message: "The test image came out as one flat colour, which usually means the model doesn't work with the text encoder and VAE Dawnroom gave it.".into(),
                seconds: Some(secs),
                image: Some(path),
                job: Some(job.id.clone()),
                width: w,
                height: h,
                steps,
            },
            Err(e) => failed(format!("Couldn't read the test image ({e})."), w, h, steps),
        }
    }

    /// Cancels a queued or running job. Returns false if it's already finished (or unknown).
    /// A running job stops at sd-server's next checkpoint; its `finished` event says `cancelled`.
    pub fn cancel(&self, id: &str) -> bool {
        match self.cancels.lock().unwrap().get(id) {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// True while any job is queued or running.
    pub fn is_busy(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst) > 0
    }

    pub fn progress(&self, id: &str) -> Option<Progress> {
        self.live.lock().unwrap().get(id).cloned()
    }

    pub fn backend_state(&self) -> BackendState {
        self.backend.state()
    }

    pub fn loaded_model(&self) -> Option<String> {
        self.backend.loaded_model()
    }

    /// Stop the backend if it holds this model (so its files can be deleted). Only call when idle.
    pub async fn unload(&self, model_id: &str) {
        if self.backend.loaded_model().as_deref() == Some(model_id) {
            self.backend.stop().await;
        }
    }

    /// Stops sd-server (frees the GPU and closes the model files) without shutting the engine down.
    pub async fn shutdown_backend(&self) {
        self.backend.stop().await;
    }

    /// Kill the backend child and wait for it to exit.
    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.backend.stop().await;
    }

    fn emit(&self, job_id: &str, kind: EventKind) {
        let _ = self.events.send(Event { job_id: job_id.to_string(), kind });
    }

    async fn run(&self, p: Pending) {
        let id = p.id;
        let started = now_ms();
        let outcome = if p.req.cancel.load(Ordering::SeqCst) {
            // Cancelled while queued: never touches the backend.
            Err(BackendError::new("cancelled"))
        } else {
            if let Err(e) = self.db.mark_running(&id, started) {
                eprintln!("[engine] db: {e}");
            }
            self.emit(&id, EventKind::Started);
            if let Err(e) = self.db.touch_model(&p.req.model.id, started) {
                eprintln!("[engine] db: {e}");
            }
            let on_progress = |pr: Progress| {
                if let Some(p) = pr.preview.clone() {
                    self.emit(&id, EventKind::Preview { step: p.step, total: p.total, b64_png: p.b64_png });
                }
                let lite = Progress { preview: None, ..pr };
                let changed = self.live.lock().unwrap().insert(id.clone(), lite.clone()).as_ref() != Some(&lite);
                if changed {
                    self.emit(&id, EventKind::Progress { stage: lite.stage, step: lite.step, total: lite.total });
                }
            };
            // The runtime is normally installed by the pull; this covers a runtime change since.
            let launch = self
                .models
                .runtimes
                .ensure(&p.req.cancel, &|_| on_progress(Progress { stage: "installing_runtime".into(), step: None, total: None, preview: None }))
                .await;
            let result = match launch {
                Ok(l) => {
                    let mut req = p.req.clone();
                    req.model.exe = l.exe;
                    req.model.args = l.args.into_iter().chain(req.model.args).collect();
                    self.backend.generate(&req, &on_progress).await
                }
                Err(e) => Err(e),
            };
            match result {
                Ok(png) => self.save(&id, &p.req, png, now_ms() - started).map_err(|m| bad("save_failed", m)),
                Err(e) if e.code == "cancelled" => Err(e),
                Err(_) if self.shutting_down.load(Ordering::SeqCst) => Err(bad("shut_down", "The engine was shut down before this job finished. Run it again.")),
                Err(e) => Err(e),
            }
        };
        let finished = now_ms();
        let db_result = match &outcome {
            Ok((path, sha)) => self.db.mark_completed(&id, finished, path, sha),
            Err(e) if e.code == "cancelled" => self.db.mark_cancelled(&id, finished),
            Err(e) => self.db.mark_failed(&id, finished, &e.message),
        };
        if let Err(e) = db_result {
            eprintln!("[engine] db: {e}");
        }
        self.live.lock().unwrap().remove(&id);
        self.cancels.lock().unwrap().remove(&id);
        self.in_flight.fetch_sub(1, Ordering::SeqCst);

        match self.db.get(&id) {
            Ok(Some(job)) => {
                eprintln!(
                    "[engine] job {id} ({}) {} in {:.1}s (waited {:.1}s){}",
                    job.model,
                    job.status,
                    (finished - started) as f64 / 1000.0,
                    (started - job.created_ms) as f64 / 1000.0,
                    job.error.as_deref().map(|e| format!(": {e}")).unwrap_or_default()
                );
                self.emit(&id, EventKind::Finished { job: Box::new(job.clone()) });
                let _ = p.done.send(job);
            }
            other => eprintln!("[engine] job {id} vanished from history: {other:?}"),
        }
    }

    /// Writes the PNG with its recipe; returns its path and sha256 (an edit of it links back by that).
    fn save(&self, id: &str, req: &GenRequest, png: Vec<u8>, gen_ms: i64) -> Result<(String, String), String> {
        let mut recipe = vec![
            ("prompt", req.prompt.clone()),
            ("model", req.model.id.clone()),
            ("seed", req.seed.to_string()),
            ("steps", req.steps.to_string()),
            ("time", format!("{:.1}s", gen_ms as f64 / 1000.0)),
            ("size", format!("{}x{}", req.width, req.height)),
        ];
        if req.task == Task::Edit {
            recipe.push(("task", "edit".into()));
            // The input images by sha256: the first was edited, the rest were references.
            recipe.push(("inputs", req.inputs.join(",")));
        }
        recipe.push(("Software", PRODUCT_NAME.to_string()));
        let png = png_meta::with_text_chunks(&png, &recipe).map_err(|e| format!("The backend returned a broken image ({e}). Try again."))?;
        std::fs::create_dir_all(&self.outputs_dir).map_err(|e| format!("Couldn't create the outputs folder ({e})."))?;
        let path = self.outputs_dir.join(format!("{id}.png"));
        let sha = crate::inputs::sha256_hex(&png);
        std::fs::write(&path, png).map_err(|e| format!("Couldn't save the image ({e}). Check free disk space."))?;
        Ok((path.to_string_lossy().into_owned(), sha))
    }
}

/// What the test image of a custom model asks for: simple, so any image model can draw it.
pub const TEST_PROMPT: &str = "a red apple on a wooden table, soft daylight";

/// True if the image is (nearly) one colour: every channel's standard deviation under 2 levels.
/// sd.cpp writes a black or grey image when the weights and companions don't match (NaNs).
fn flat_colour(path: &std::path::Path) -> Result<bool, String> {
    let img = image::open(path).map_err(|e| e.to_string())?.to_rgb8();
    let px: Vec<&image::Rgb<u8>> = img.pixels().step_by(7).collect();
    if px.is_empty() {
        return Ok(true);
    }
    let n = px.len() as f64;
    Ok((0..3).all(|c| {
        let mean = px.iter().map(|p| p[c] as f64).sum::<f64>() / n;
        let var = px.iter().map(|p| (p[c] as f64 - mean).powi(2)).sum::<f64>() / n;
        var.sqrt() < 2.0
    }))
}

async fn worker(engine: Arc<Engine>, mut rx: mpsc::UnboundedReceiver<Pending>, idle: Duration) {
    loop {
        match tokio::time::timeout(idle, rx.recv()).await {
            Ok(Some(p)) => engine.run(p).await,
            Ok(None) => break,
            Err(_) => {
                if engine.backend.state() != BackendState::Stopped {
                    eprintln!("[engine] idle for {}s, stopping sd-server to free the GPU", idle.as_secs());
                    engine.backend.stop().await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::Gpu;
    use crate::registry::Registry;
    use async_trait::async_trait;

    #[derive(Default)]
    struct FakeBackend {
        order: Mutex<Vec<String>>,
        stops: AtomicUsize,
        stopped: Mutex<bool>,
    }

    #[async_trait]
    impl Backend for FakeBackend {
        async fn generate(&self, req: &GenRequest, progress: &(dyn Fn(Progress) + Send + Sync)) -> Result<Vec<u8>, BackendError> {
            *self.stopped.lock().unwrap() = false;
            progress(Progress { stage: "sampling".into(), step: Some(1), total: Some(2), preview: None });
            tokio::time::sleep(Duration::from_millis(20)).await;
            self.order.lock().unwrap().push(req.prompt.clone());
            if req.cancel.load(Ordering::SeqCst) {
                return Err(BackendError::new("cancelled"));
            }
            if req.prompt == "fail" {
                return Err(BackendError::new("generation_failed"));
            }
            if req.prompt == TEST_PROMPT {
                // A decodable, flat grey image: what a mismatched model gives.
                let mut out = std::io::Cursor::new(Vec::new());
                image::RgbImage::from_pixel(8, 8, image::Rgb([90, 90, 90])).write_to(&mut out, image::ImageFormat::Png).unwrap();
                return Ok(out.into_inner());
            }
            // A real, decodable image (edits feed outputs back in as inputs).
            let mut out = std::io::Cursor::new(Vec::new());
            image::RgbImage::from_fn(8, 8, |x, y| image::Rgb([x as u8 * 30, y as u8 * 30, req.seed as u8])).write_to(&mut out, image::ImageFormat::Png).unwrap();
            Ok(out.into_inner())
        }
        fn state(&self) -> BackendState {
            if *self.stopped.lock().unwrap() { BackendState::Stopped } else { BackendState::Ready }
        }
        fn loaded_model(&self) -> Option<String> {
            None
        }
        async fn stop(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
            *self.stopped.lock().unwrap() = true;
        }
    }

    /// The real registry, with every file pretended verified.
    fn engine(fake: Arc<FakeBackend>, idle: Duration) -> Arc<Engine> {
        let dir = std::env::temp_dir().join(format!("engine-test-{}", uuid::Uuid::new_v4()));
        let reg = Registry::load_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("registry")).unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let gpu = Gpu { name: "test".into(), vram_total_mib: 8151, vram_used_mib: 0 };
        let rt = Arc::new(crate::runtimes::Runtimes::fixed(&dir, "sd-server.exe".into()));
        let machine = crate::fit::Machine { gpu_mib: gpu.vram_total_mib, integrated: false, ram_total_mib: 24 * 1024 };
        let models = Models::new(reg, Some(gpu), Some(machine), dir.join("models"), db.clone(), "http://unused", rt);
        models.pretend_all_verified();
        Engine::start(fake, db, models, dir.join("out"), idle)
    }

    /// A custom model must pass its test image before it runs anything else; a flat test image fails.
    #[tokio::test]
    async fn custom_models_run_only_their_test_until_it_passes() {
        let e = engine(Arc::new(FakeBackend::default()), Duration::from_secs(60));
        let mut m = e.models.registry().get("z-image-turbo:q4").unwrap().clone();
        m.id = "hf.co/a/b:x.gguf".into();
        m.family = "hf.co/a/b".into();
        m.tag = None;
        m.custom = Some(registry::Custom { source: m.id.clone(), architecture: "Z-Image".into(), sd_version: "z_image".into(), base: Some("z-image-turbo".into()), added_ms: 0, test: None });
        let (reg, skipped) = (*e.models.registry()).clone().with_custom(vec![m.clone()]);
        assert!(skipped.is_empty(), "{skipped:?}");
        e.models.set_registry(reg);
        let job = ImageParams { model: Some(m.id.clone()), ..params("x") };
        assert_eq!(e.submit(job.clone()).unwrap_err().code, "model_not_ready");
        // The fake backend's PNG is a single pixel: one flat colour, so the test fails, kept out of the Library.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let t = e.test_model(&m.id, Box::new(move |stage, _, _| s2.lock().unwrap().push(stage))).await;
        assert!(!t.ok && t.message.contains("one flat colour"), "{t:?}");
        assert!((t.width, t.height, t.steps) == (512, 512, 8), "small and few steps: {t:?}");
        assert_eq!(*seen.lock().unwrap(), ["sampling"], "progress reaches the pull");
        assert_eq!(e.db.list_images(10, 0, None, false).unwrap().1, 0, "test images aren't in the Library");
        crate::custom::save_test(&e.models, &m.id, TestResult { ok: true, ..t }).unwrap();
        assert_eq!(e.submit(job).unwrap().0.model, m.id, "ready after a passed test");
    }

    #[test]
    fn a_flat_image_is_told_apart_from_a_real_one() {
        let dir = std::env::temp_dir().join(format!("flat-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let flat = image::RgbImage::from_pixel(64, 64, image::Rgb([0, 0, 0]));
        let real = image::RgbImage::from_fn(64, 64, |x, y| image::Rgb([(x * 4) as u8, (y * 4) as u8, 128]));
        flat.save(dir.join("flat.png")).unwrap();
        real.save(dir.join("real.png")).unwrap();
        assert!(flat_colour(&dir.join("flat.png")).unwrap());
        assert!(!flat_colour(&dir.join("real.png")).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn input(e: &Engine, w: u32, h: u32) -> String {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_fn(w, h, |x, y| image::Rgb([x as u8, y as u8, 3])).write_to(&mut out, image::ImageFormat::Png).unwrap();
        crate::inputs::store(&e.inputs_dir, &out.into_inner()).unwrap().sha256
    }

    #[tokio::test]
    async fn edits_take_inputs_keep_their_shape_and_link_their_source() {
        let e = engine(Arc::new(FakeBackend::default()), Duration::from_secs(60));
        let img = input(&e, 1200, 800);
        let refi = input(&e, 64, 64);
        let (job, rx) = e.submit(ImageParams { image: Some(img.clone()), refs: vec![refi.clone()], model: Some("flux2-klein-4b:q8".into()), ..params("make it night") }).unwrap();
        assert_eq!(job.params["task"], "edit");
        assert_eq!(job.params["inputs"], serde_json::json!([img, refi]), "the recipe records the input hashes");
        let (w, h) = (job.params["width"].as_u64().unwrap(), job.params["height"].as_u64().unwrap());
        assert!(w % 16 == 0 && h % 16 == 0 && (w as f64 / h as f64 - 1.5).abs() < 0.05, "{w}x{h} keeps 3:2");
        assert_eq!(job.parent_id, None, "a fresh upload has no Library source");
        let done = rx.await.unwrap();
        assert_eq!(done.status, "completed");
        // Feed the result back in: the edit of it knows where it came from.
        let again = crate::inputs::store(&e.inputs_dir, &std::fs::read(done.output_path.unwrap()).unwrap()).unwrap();
        let (child, rx) = e.submit(ImageParams { image: Some(again.sha256), model: Some("flux2-klein-4b:q8".into()), ..params("add snow") }).unwrap();
        assert_eq!(child.parent_id.as_deref(), Some(done.id.as_str()));
        rx.await.unwrap();
    }

    #[tokio::test]
    async fn tasks_are_checked_before_anything_runs() {
        let e = engine(Arc::new(FakeBackend::default()), Duration::from_secs(60));
        let img = input(&e, 64, 64);
        let code = |p: ImageParams| e.submit(p).unwrap_err().code;
        assert_eq!(code(ImageParams { image: Some(img.clone()), preset: Some("quality".into()), ..params("x") }), "task_not_supported", "Z-Image can't edit");
        assert_eq!(code(ImageParams { model: Some("qwen-image-edit-2511:q4".into()), ..params("x") }), "task_not_supported", "an edit model needs an image");
        assert_eq!(code(ImageParams { task: Some("edit".into()), ..params("x") }), "invalid_request");
        assert_eq!(code(ImageParams { task: Some("generate".into()), image: Some(img.clone()), ..params("x") }), "invalid_request");
        assert_eq!(code(ImageParams { image: Some("0".repeat(64)), ..params("x") }), "input_not_found");
        assert_eq!(code(ImageParams { image: Some(img.clone()), refs: vec![img.clone(); MAX_REFS + 1], ..params("x") }), "invalid_request");
    }

    #[tokio::test]
    async fn auto_is_the_default_and_presets_still_work() {
        let e = engine(Arc::new(FakeBackend::default()), Duration::from_secs(60));
        let (job, rx) = e.submit(params("x")).unwrap();
        assert_eq!(job.model, "z-image-turbo:q4");
        assert!(job.params["auto"].as_str().unwrap().starts_with("Z-Image Turbo: "), "{}", job.params["auto"]);
        rx.await.unwrap();
        // Queue a klein job: Auto now prefers klein, which will be loaded, over switching back.
        let (_, rx) = e.submit(ImageParams { preset: Some("fast".into()), ..params("x") }).unwrap();
        let (job, rx2) = e.submit(ImageParams { model: Some("auto".into()), ..params("x") }).unwrap();
        assert_eq!(job.model, "flux2-klein-4b:q8");
        assert!(job.params["auto"].as_str().unwrap().contains("already loaded"));
        rx.await.unwrap();
        rx2.await.unwrap();
        // "fast" as a model id is the old alias.
        let (job, rx) = e.submit(ImageParams { model: Some("fast".into()), ..params("x") }).unwrap();
        assert_eq!(job.model, "flux2-klein-4b:q8");
        rx.await.unwrap();
    }

    fn params(prompt: &str) -> ImageParams {
        ImageParams { prompt: prompt.into(), seed: Some(7), ..Default::default() }
    }

    #[tokio::test]
    async fn jobs_run_one_at_a_time_in_fifo_order() {
        let fake = Arc::new(FakeBackend::default());
        let e = engine(fake.clone(), Duration::from_secs(60));
        let rxs: Vec<_> = ["a", "b", "c"].iter().map(|p| e.submit(params(p)).unwrap().1).collect();
        let mut jobs = Vec::new();
        for rx in rxs {
            jobs.push(rx.await.unwrap());
        }
        assert_eq!(*fake.order.lock().unwrap(), ["a", "b", "c"]);
        for pair in jobs.windows(2) {
            assert!(pair[1].started_ms.unwrap() >= pair[0].finished_ms.unwrap(), "jobs overlapped");
        }
        let first = &jobs[0];
        assert_eq!(first.status, "completed");
        assert_eq!(first.seed, 7);
        assert_eq!(first.model, "z-image-turbo:q4");
        assert!(std::path::Path::new(first.output_path.as_ref().unwrap()).is_file());
    }

    #[tokio::test]
    async fn presets_pick_the_model_and_its_defaults() {
        let e = engine(Arc::new(FakeBackend::default()), Duration::from_secs(60));
        let (job, rx) = e.submit(ImageParams { preset: Some("fast".into()), ..params("x") }).unwrap();
        assert_eq!(job.model, "flux2-klein-4b:q8");
        assert_eq!(job.params["steps"], 4);
        rx.await.unwrap();
        assert_eq!(e.submit(ImageParams { preset: Some("nope".into()), ..params("x") }).unwrap_err().code, "unknown_model");

        let (job, rx) = e.submit(ImageParams { aspect: Some("16:9".into()), ..params("x") }).unwrap();
        assert_eq!((job.params["width"].as_u64(), job.params["height"].as_u64()), (Some(1360), Some(768)));
        rx.await.unwrap();
        assert_eq!(e.submit(ImageParams { aspect: Some("5:4".into()), ..params("x") }).unwrap_err().code, "invalid_request");
        assert_eq!(e.submit(ImageParams { aspect: Some("1:1".into()), width: Some(512), ..params("x") }).unwrap_err().code, "invalid_request");
    }

    #[tokio::test]
    async fn steps_and_guidance_can_be_overridden_within_limits() {
        let e = engine(Arc::new(FakeBackend::default()), Duration::from_secs(60));
        let (job, rx) = e.submit(ImageParams { steps: Some(12), cfg: Some(2.5), ..params("x") }).unwrap();
        assert_eq!((job.params["steps"].as_u64(), job.params["cfg"].as_f64()), (Some(12), Some(2.5)));
        assert_eq!(job.frame, 1);
        rx.await.unwrap();
        for bad in [
            ImageParams { steps: Some(0), ..params("x") },
            ImageParams { steps: Some(51), ..params("x") },
            ImageParams { cfg: Some(-1.0), ..params("x") },
            ImageParams { cfg: Some(20.5), ..params("x") },
        ] {
            assert_eq!(e.submit(bad).unwrap_err().code, "invalid_request");
        }
    }

    #[tokio::test]
    async fn failures_and_cancels_are_recorded() {
        let e = engine(Arc::new(FakeBackend::default()), Duration::from_secs(60));
        let job = e.submit(params("fail")).unwrap().1.await.unwrap();
        assert_eq!(job.status, "failed");
        assert!(job.error.unwrap().contains("Try again"));
        assert!(e.submit(params("   ")).is_err());

        // Cancel the second job while the first is still running: it never reaches the backend.
        let (_, first) = e.submit(params("a")).unwrap();
        let (queued, second) = e.submit(params("b")).unwrap();
        assert!(e.cancel(&queued.id));
        assert_eq!(first.await.unwrap().status, "completed");
        assert_eq!(second.await.unwrap().status, "cancelled");
        assert!(!e.cancel(&queued.id), "finished jobs can't be cancelled");
        assert!(!e.is_busy());
    }

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_stops_the_backend_once() {
        let fake = Arc::new(FakeBackend::default());
        let e = engine(fake.clone(), Duration::from_secs(5));
        e.submit(params("a")).unwrap().1.await.unwrap();
        tokio::time::sleep(Duration::from_secs(16)).await;
        assert_eq!(fake.stops.load(Ordering::SeqCst), 1);
    }
}
