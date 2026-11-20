//! `SdServerBackend`: runs stable-diffusion.cpp's `sd-server.exe` (pinned release master-1086-465adf3)
//! as a supervised child on a free 127.0.0.1 port and talks to its `/sdcpp/v1/*` job API.
//! The model is given on the command line, so starting the child loads the model (weights load
//! lazily on the first job) and stopping the child frees all of its VRAM and RAM.

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{Value, json};
use tokio::process::{Child, Command};

use crate::backend::{Backend, BackendError, BackendState, GenRequest, ModelSpec, Preview, Progress};
use crate::config::Config;

const READY_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_EVERY: Duration = Duration::from_millis(250);

pub struct SdServerBackend {
    log_path: PathBuf,
    http: reqwest::Client,
    running: Mutex<Option<Running>>,
    state: Mutex<BackendState>,
    #[cfg(windows)]
    job: Option<crate::job_object::JobObject>,
}

struct Running {
    child: Child,
    base: String,
    model: String,
    /// The runtime it was started from; a different exe (runtime switch) means a restart.
    exe: PathBuf,
}

impl SdServerBackend {
    pub fn new(cfg: &Config) -> Self {
        Self {
            log_path: cfg.logs_dir().join("sd-server.log"),
            http: reqwest::Client::builder().no_proxy().build().expect("http client"),
            running: Mutex::new(None),
            state: Mutex::new(BackendState::Stopped),
            #[cfg(windows)]
            job: match crate::job_object::JobObject::kill_on_close() {
                Ok(j) => Some(j),
                Err(e) => {
                    eprintln!("[engine] warning: couldn't create a Job Object ({e}); a hard crash may leave sd-server running");
                    None
                }
            },
        }
    }

    fn set_state(&self, s: BackendState) {
        *self.state.lock().unwrap() = s;
    }

    /// Base URL of the child if it's still alive. Forgets a child that has exited.
    fn live_base(&self) -> Option<String> {
        self.live().map(|(base, _)| base)
    }

    /// (base URL, model id, exe) of the child if it's still alive.
    fn live_full(&self) -> Option<(String, String, PathBuf)> {
        let mut guard = self.running.lock().unwrap();
        let r = guard.as_mut()?;
        match r.child.try_wait() {
            Ok(None) => Some((r.base.clone(), r.model.clone(), r.exe.clone())),
            _ => {
                *guard = None;
                None
            }
        }
    }

    fn live(&self) -> Option<(String, String)> {
        self.live_full().map(|(b, m, _)| (b, m))
    }

    async fn start(&self, spec: &ModelSpec) -> Result<String, BackendError> {
        if !spec.exe.is_file() {
            return Err(BackendError::new("backend_missing"));
        }
        if let Some(f) = spec.files.iter().find(|f| !f.is_file()) {
            return Err(BackendError::with_message(
                "missing_model_files",
                format!("Model file {} is missing. Pull {} again.", f.display(), spec.id),
            ));
        }
        self.set_state(BackendState::Loading);
        let t0 = Instant::now();
        let result = self.spawn_and_wait(spec).await;
        match &result {
            Ok(_) => {
                eprintln!("[engine] sd-server ready with {} in {} ms", spec.id, t0.elapsed().as_millis());
                self.set_state(BackendState::Ready);
            }
            Err(e) => {
                eprintln!("[engine] sd-server failed to start: {e}");
                self.stop().await;
            }
        }
        result
    }

    async fn spawn_and_wait(&self, spec: &ModelSpec) -> Result<String, BackendError> {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .map_err(|e| BackendError::with_message("no_free_port", format!("Couldn't find a free local port ({e}). Try again.")))?;
        let log = open_log(&self.log_path, port)
            .map_err(|e| BackendError::with_message("log_failed", format!("Couldn't open {}: {e}. Check the data folder is writable.", self.log_path.display())))?;
        let log2 = log.try_clone().map_err(|e| BackendError::with_message("log_failed", e.to_string()))?;

        let mut cmd = Command::new(&spec.exe);
        // The runtime's DLLs sit beside the exe.
        if let Some(dir) = spec.exe.parent() {
            cmd.current_dir(dir);
        }
        cmd.arg("--listen-ip").arg("127.0.0.1")
            .arg("--listen-port").arg(port.to_string())
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log2)
            .kill_on_drop(true);
        // No console: Ctrl-C must reach only the engine, which then stops the child itself.
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000 /* CREATE_NO_WINDOW */);

        let child = cmd.spawn().map_err(|e| {
            BackendError::with_message("backend_missing", format!("Couldn't start {} ({e}). Pull a model again to reinstall the runtime, or check the --sd-server path.", spec.exe.display()))
        })?;
        #[cfg(windows)]
        if let (Some(job), Some(h)) = (&self.job, child.raw_handle())
            && let Err(e) = job.assign(h) {
                eprintln!("[engine] warning: couldn't add sd-server to the Job Object ({e})");
            }
        eprintln!("[engine] sd-server started (pid {:?}, port {port})", child.id());
        let base = format!("http://127.0.0.1:{port}");
        *self.running.lock().unwrap() = Some(Running { child, base: base.clone(), model: spec.id.clone(), exe: spec.exe.clone() });

        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if self.live_base().is_none() {
                return Err(BackendError::new("model_load_failed"));
            }
            if let Ok(resp) = self.http.get(format!("{base}/sdcpp/v1/models/status")).send().await
                && let Ok(v) = resp.json::<Value>().await {
                    match v["state"].as_str() {
                        Some("ready") => return Ok(base),
                        Some("error") => {
                            eprintln!("[engine] sd-server load error: {}", v["error"]);
                            return Err(BackendError::new("model_load_failed"));
                        }
                        _ => {}
                    }
                }
            if Instant::now() > deadline {
                return Err(BackendError::new("model_load_failed"));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// A request to the child failed: say whether it died or just didn't answer.
    fn transport_error(&self, e: reqwest::Error) -> BackendError {
        eprintln!("[engine] sd-server request failed: {e}");
        BackendError::new(if self.live_base().is_none() { "backend_crashed" } else { "backend_unreachable" })
    }

    async fn read_json(&self, resp: reqwest::Response) -> Result<Value, BackendError> {
        let status = resp.status();
        let v: Value = resp.json().await.map_err(|e| self.transport_error(e))?;
        if status.is_success() {
            return Ok(v);
        }
        eprintln!("[engine] sd-server HTTP {status}: {v}");
        Err(BackendError::new(v["code"].as_str().unwrap_or("generation_failed")))
    }

    async fn run_job(&self, base: &str, req: &GenRequest, progress: &(dyn Fn(Progress) + Send + Sync)) -> Result<Vec<u8>, BackendError> {
        let body = json!({
            "prompt": req.prompt,
            "negative_prompt": req.negative_prompt,
            "width": req.width,
            "height": req.height,
            "seed": req.seed,
            "batch_count": 1,
            "output_format": "png",
            "embed_image_metadata": false, // the engine writes its own recipe chunks
            "preview": "proj",
            "preview_interval": 1,
            "vae_tiling_params": { "enabled": true },
            "sample_params": { "sample_steps": req.steps, "guidance": { "txt_cfg": req.cfg } },
        });
        let mut body = body;
        if let Some(m) = &req.sampler {
            body["sample_params"]["sample_method"] = json!(m);
        }
        if let Some(m) = &req.scheduler {
            body["sample_params"]["scheduler"] = json!(m);
        }
        if let Some(g) = req.distilled_guidance {
            body["sample_params"]["guidance"]["distilled_guidance"] = json!(g);
        }
        if let Some(f) = req.flow_shift {
            body["sample_params"]["flow_shift"] = json!(f);
        }
        if let Some(s) = &req.sigmas {
            body["sample_params"]["custom_sigmas"] = json!(s);
        }
        // Edits: every edit model in the library (FLUX.2/3 klein, Qwen-Image-Edit, LongCat, Kontext,
        // Fibo) takes the image to edit and any references as reference images. sd-server also has
        // init_image + strength (img2img) and mask_image, which none of them use.
        if !req.input_paths.is_empty() {
            let mut refs = Vec::new();
            for p in &req.input_paths {
                refs.push(crate::inputs::b64_for_backend(p).map_err(|m| BackendError::with_message("input_missing", format!("{m} Send the image again.")))?);
            }
            body["ref_images"] = json!(refs);
        }
        if req.cancel.load(Ordering::SeqCst) {
            return Err(BackendError::new("cancelled"));
        }
        let resp = self.http.post(format!("{base}/sdcpp/v1/img_gen")).json(&body).send().await.map_err(|e| self.transport_error(e))?;
        let job = self.read_json(resp).await?;
        let id = job["id"].as_str().ok_or_else(|| BackendError::new("generation_failed"))?.to_string();

        let mut last: (String, Option<u32>, Option<u32>) = (String::new(), None, None);
        let mut last_preview_step: Option<u64> = None;
        let mut cancel_sent = false;
        loop {
            tokio::time::sleep(POLL_EVERY).await;
            if self.live_base().is_none() {
                return Err(BackendError::new("backend_crashed"));
            }
            if !cancel_sent && req.cancel.load(Ordering::SeqCst) {
                cancel_sent = true;
                // sd-server stops at its next checkpoint; keep polling until it says "cancelled".
                match self.http.post(format!("{base}/sdcpp/v1/jobs/{id}/cancel")).send().await {
                    Ok(r) => eprintln!("[engine] cancel requested for sd-server job {id}: HTTP {}", r.status()),
                    Err(e) => eprintln!("[engine] cancel request failed: {e}"),
                }
            }
            let resp = self.http.get(format!("{base}/sdcpp/v1/jobs/{id}")).send().await.map_err(|e| self.transport_error(e))?;
            let s = self.read_json(resp).await?;

            let p = &s["progress"];
            let stage = p["stage"].as_str().unwrap_or("").to_string();
            let (step, total) = if stage == "sampling" {
                (p["completed"].as_u64().map(|n| n as u32), p["total"].as_u64().map(|n| n as u32))
            } else {
                (None, None)
            };
            let pv = &s["preview"];
            let preview = match (pv["step"].as_u64(), pv["b64_json"].as_str()) {
                (Some(n), Some(b64)) if Some(n) != last_preview_step => {
                    last_preview_step = Some(n);
                    Some(Preview { step: n as u32, total: pv["total_steps"].as_u64().unwrap_or(0) as u32, b64_png: b64.to_string() })
                }
                _ => None,
            };
            let key = (stage.clone(), step, total);
            if !stage.is_empty() && (key != last || preview.is_some()) {
                last = key;
                progress(Progress { stage, step, total, preview });
            }

            match s["status"].as_str() {
                Some("completed") => {
                    let b64 = s["result"]["images"][0]["b64_json"].as_str().ok_or_else(|| BackendError::new("generation_failed"))?;
                    return base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .map_err(|_| BackendError::new("generation_failed"));
                }
                Some("cancelled") => return Err(BackendError::new("cancelled")),
                Some("failed") => {
                    eprintln!("[engine] sd-server job {id} failed: {}", s["error"]);
                    return Err(BackendError::new(s["error"]["code"].as_str().unwrap_or("generation_failed")));
                }
                _ => {}
            }
        }
    }
}

#[async_trait]
impl Backend for SdServerBackend {
    async fn generate(&self, req: &GenRequest, progress: &(dyn Fn(Progress) + Send + Sync)) -> Result<Vec<u8>, BackendError> {
        let base = match self.live_full() {
            Some((b, model, exe)) if model == req.model.id && exe == req.model.exe => b,
            Some((_, old, _)) => {
                // One model per child: switching = stop it, start a new one with the new files.
                eprintln!("[engine] switching model {old} -> {}", req.model.id);
                progress(Progress { stage: "switching_model".into(), step: None, total: None, preview: None });
                self.stop().await;
                self.start(&req.model).await?
            }
            None => {
                progress(Progress { stage: "starting".into(), step: None, total: None, preview: None });
                self.start(&req.model).await?
            }
        };
        self.set_state(BackendState::Busy);
        let result = self.run_job(&base, req, progress).await;
        self.set_state(if self.live_base().is_some() { BackendState::Ready } else { BackendState::Stopped });
        result
    }

    fn state(&self) -> BackendState {
        if *self.state.lock().unwrap() != BackendState::Stopped && self.live_base().is_none() {
            self.set_state(BackendState::Stopped);
        }
        *self.state.lock().unwrap()
    }

    fn loaded_model(&self) -> Option<String> {
        self.live().map(|(_, model)| model)
    }

    async fn stop(&self) {
        let running = self.running.lock().unwrap().take();
        if let Some(mut r) = running {
            let pid = r.child.id();
            // kill() waits for the process to exit, so VRAM is free once this returns.
            let _ = r.child.kill().await;
            eprintln!("[engine] sd-server stopped (pid {pid:?})");
        }
        self.set_state(BackendState::Stopped);
    }
}

fn open_log(path: &std::path::Path, port: u16) -> std::io::Result<std::fs::File> {
    // Rotates by size before opening (10 MB, three old logs kept).
    let mut f = crate::logs::open_append(path)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    writeln!(f, "\n===== sd-server start, unix {now}, port {port} =====")?;
    Ok(f)
}
