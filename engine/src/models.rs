//! Model manager: which registry models are on disk (verified by sha256), pulling them from
//! Hugging Face with resume, and deleting them. Files live in sd.cpp's category layout under
//! the models dir. Verified hashes are cached in SQLite by (path, size, mtime), so big files
//! are hashed once, not on every start.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::broadcast;

use crate::backend::{BackendError, ModelSpec};
use crate::db::Db;
use crate::download::{Fetch, Retry, Wait, err, fetch_to_part, sha256_file};
use crate::hardware::Gpu;
use crate::fit::Machine;
use crate::registry::{Manifest, ModelFile, Registry, TestResult};
use crate::runtimes::Runtimes;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FileState {
    Missing,
    /// A `.part` download of this many bytes is waiting to resume.
    Partial { bytes: u64 },
    /// Present but not hashed yet.
    Unverified,
    Verifying,
    Verified,
    /// Present with the wrong size or hash. A pull replaces it.
    Corrupt { reason: String },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PullEvent {
    /// `stage` is downloading | waiting | verifying | present | unpacking. Bytes are for the
    /// current file. `wait` is set while `stage` is "waiting" (network trouble, retrying).
    Progress { file: String, file_index: usize, file_count: usize, stage: String, bytes: u64, total: u64, speed_bps: u64, wait: Option<Wait> },
    Done { model: String },
    Error { code: String, message: String },
}

/// A running pull. The daemon owns it: clients attach and detach freely, and only
/// `cancel_pull` (or a failure) stops it.
#[derive(Clone)]
struct Pull {
    tx: broadcast::Sender<PullEvent>,
    cancel: Arc<AtomicBool>,
    /// The latest progress event, for clients that attach later or just poll the model list.
    last: Arc<Mutex<Option<PullEvent>>>,
}

impl Pull {
    fn send(&self, ev: PullEvent) {
        *self.last.lock().unwrap() = Some(ev.clone());
        let _ = self.tx.send(ev); // no receivers is fine: nobody is watching right now
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// Runs the test image of a custom model (set by the queue at start; see `queue::Engine::test_model`).
/// Gets the model id and a progress callback (stage, step, total).
pub type Tester = Arc<dyn Fn(String, TestProgress) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> + Send + Sync>;
/// Progress of a test image: (job stage, step, total).
pub type TestProgress = Box<dyn Fn(String, Option<u32>, Option<u32>) + Send + Sync>;

pub struct Models {
    /// Curated manifests plus custom ones; swapped whole when a custom model is added or removed.
    registry: std::sync::RwLock<Arc<Registry>>,
    /// Detected once at start; drives the "fits this GPU" answer.
    pub gpu: Option<Gpu>,
    /// The memory the fit is decided against (the chosen GPU and system RAM).
    pub machine: Option<Machine>,
    /// The image runtime this machine uses; a pull installs it with the model.
    pub runtimes: Arc<Runtimes>,
    pub(crate) dir: PathBuf,
    pub(crate) db: Arc<Db>,
    pub(crate) http: reqwest::Client,
    /// Download base, normally `https://huggingface.co` (also the Hugging Face API base).
    pub(crate) endpoint: String,
    tester: std::sync::OnceLock<Tester>,
    states: Mutex<HashMap<String, FileState>>,
    /// A background `verify_all` is running.
    verifying: AtomicBool,
    pulls: Mutex<HashMap<String, Pull>>,
    /// How the last pull of each model ended badly (including a cancel). Cleared by the next pull.
    pull_errors: Mutex<HashMap<String, PullEvent>>,
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// A paused download: bytes kept (what a resume starts from), of `total`, last written when.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Paused {
    pub bytes: u64,
    pub total: u64,
    pub updated_ms: Option<i64>,
}

/// When a `.part` was last written: its sidecar's time, else the file's mtime.
fn last_written(part: &std::path::Path) -> Option<i64> {
    crate::download::read_sidecar(part).map(|s| s.updated_ms).or_else(|| std::fs::metadata(part).ok().map(|m| mtime_ms(&m)))
}

fn chrono_ms() -> i64 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn cancelled() -> BackendError {
    err("cancelled", "The pull was cancelled. Pull again to resume; the downloaded part is kept.")
}

impl Models {
    pub fn new(registry: Registry, gpu: Option<Gpu>, machine: Option<Machine>, dir: PathBuf, db: Arc<Db>, endpoint: &str, runtimes: Arc<Runtimes>) -> Arc<Self> {
        let http = crate::download::http_client();
        let m = Arc::new(Self {
            registry: std::sync::RwLock::new(Arc::new(registry)),
            gpu,
            machine,
            runtimes,
            dir,
            db,
            http,
            endpoint: endpoint.trim_end_matches('/').to_string(),
            states: Mutex::new(HashMap::new()),
            verifying: Default::default(),
            pulls: Mutex::new(HashMap::new()),
            pull_errors: Mutex::new(HashMap::new()),
            tester: Default::default(),
        });
        for f in m.registry().all_files() {
            let s = m.scan(f);
            m.set(f, s);
        }
        m
    }

    /// The current registry (curated + custom). Cheap: an `Arc` clone.
    pub fn registry(&self) -> Arc<Registry> {
        self.registry.read().unwrap().clone()
    }

    /// Replaces the registry (custom.rs, after adding or removing a custom model) and picks up the
    /// state of any files it hasn't seen.
    pub(crate) fn set_registry(&self, r: Registry) {
        for f in r.all_files() {
            let known = self.states.lock().unwrap().contains_key(&f.rel_path());
            if !known {
                let s = self.scan(f);
                self.set(f, s);
            }
        }
        *self.registry.write().unwrap() = Arc::new(r);
    }

    /// The custom-model test runner (the queue sets it once at start).
    pub fn set_tester(&self, t: Tester) {
        let _ = self.tester.set(t);
    }

    /// Where custom manifests live: `<data>/custom/` (the models dir is `<data>/models`).
    pub fn custom_dir(&self) -> PathBuf {
        self.dir.parent().unwrap_or(&self.dir).join("custom")
    }

    /// What the backend needs to launch this model.
    pub fn spec(&self, m: &Manifest) -> ModelSpec {
        // Offload is sd.cpp's auto-fit, chosen on purpose and passed explicitly (measured in Slice 6a
        // Part 2): it places weights on the GPU, then RAM, then disk by the memory that's free, so
        // the driver never spills silently. `--offload-to-cpu` pins every weight in RAM and ran out
        // of memory; it's never used.
        let mut args = m.server_args(&self.dir);
        args.extend(["--auto-fit".to_string(), "on".to_string()]);
        ModelSpec { id: m.id.clone(), exe: Default::default(), files: m.files.iter().map(|f| self.path_of(f)).collect(), args }
    }

    #[cfg(test)]
    pub fn pretend_all_verified(&self) {
        for f in self.registry().all_files() {
            self.set(f, FileState::Verified);
        }
    }

    pub fn path_of(&self, f: &ModelFile) -> PathBuf {
        self.dir.join(&f.category).join(&f.filename)
    }

    pub(crate) fn part_path(&self, f: &ModelFile) -> PathBuf {
        self.dir.join(&f.category).join(format!("{}.part", f.filename))
    }

    pub(crate) fn set(&self, f: &ModelFile, s: FileState) {
        self.states.lock().unwrap().insert(f.rel_path(), s);
    }

    pub fn file_state(&self, f: &ModelFile) -> FileState {
        self.states.lock().unwrap().get(&f.rel_path()).cloned().unwrap_or(FileState::Missing)
    }

    /// Looks at the disk and the hash cache; never hashes.
    fn scan(&self, f: &ModelFile) -> FileState {
        let path = self.path_of(f);
        let Ok(meta) = std::fs::metadata(&path) else {
            let part = self.part_path(f);
            return match part.exists() {
                // What a resume keeps (see download::trusted_bytes), not the raw length.
                true => FileState::Partial { bytes: crate::download::trusted_bytes(&part, &f.url(&self.endpoint), f.size, &f.sha256) },
                false => FileState::Missing,
            };
        };
        if meta.len() != f.size {
            return FileState::Corrupt { reason: format!("size is {} bytes, expected {}", meta.len(), f.size) };
        }
        match self.db.cached_hash(&path.to_string_lossy(), meta.len(), mtime_ms(&meta)) {
            Some(h) if h == f.sha256 => FileState::Verified,
            Some(_) => FileState::Corrupt { reason: "sha256 doesn't match".into() },
            None => FileState::Unverified,
        }
    }

    /// Hashes `path`, caches the result, and says whether it matches the manifest.
    async fn verify(&self, f: &ModelFile, path: PathBuf, on_bytes: impl FnMut(u64) + Send + 'static) -> Result<bool, String> {
        let p = path.clone();
        let hash = tokio::task::spawn_blocking(move || sha256_file(&p, on_bytes))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("Couldn't read {} ({e}).", path.display()))?;
        if hash == f.sha256
            && let Ok(meta) = std::fs::metadata(&path) {
                let _ = self.db.save_hash(&path.to_string_lossy(), meta.len(), mtime_ms(&meta), &hash);
            }
        Ok(hash == f.sha256)
    }

    /// Picks up files that appeared or changed on disk while running (stat only, no hashing),
    /// and starts hashing any new ones in the background. Cheap; called when models are listed.
    pub fn refresh(self: &Arc<Self>) {
        let mut unverified = false;
        for f in self.registry().all_files() {
            let old = self.file_state(f);
            if self.is_pulling_file(f) || matches!(old, FileState::Verifying | FileState::Unverified) {
                unverified |= old == FileState::Unverified;
                continue;
            }
            let now = self.scan(f);
            unverified |= now == FileState::Unverified;
            if now != old {
                self.set(f, now);
            }
        }
        if unverified {
            tokio::spawn(self.clone().verify_all()); // no-op if one is already running
        }
    }

    /// This model's own pull wrote `f`'s `.part`, and no pull is writing it right now.
    fn owns_part(&self, m: &Manifest, f: &ModelFile) -> bool {
        !self.is_pulling_file(f) && crate::download::part_owner(&self.part_path(f)).as_deref() == Some(m.id.as_str())
    }

    /// True if an active pull includes the file this `.part` path belongs to: Discard must never
    /// delete a `.part` that's being written.
    pub fn part_in_use(&self, part: &Path) -> bool {
        let pulls = self.pulls.lock().unwrap();
        self.registry().models.iter().filter(|m| pulls.contains_key(&m.id)).flat_map(|m| &m.files).any(|f| part.ends_with(Path::new(&f.category).join(format!("{}.part", f.filename))))
    }

    fn is_pulling_file(&self, f: &ModelFile) -> bool {
        let pulls = self.pulls.lock().unwrap();
        self.registry().models.iter().any(|m| pulls.contains_key(&m.id) && m.files.iter().any(|g| g.rel_path() == f.rel_path()))
    }

    /// Hashes every present-but-unverified file once. Run in the background at start.
    pub async fn verify_all(self: Arc<Self>) {
        if self.verifying.swap(true, Ordering::SeqCst) {
            return; // already running; it will pick these files up
        }
        self.verify_unverified().await;
        self.verifying.store(false, Ordering::SeqCst);
    }

    async fn verify_unverified(&self) {
        for f in self.registry().all_files() {
            if self.file_state(f) != FileState::Unverified {
                continue;
            }
            self.set(f, FileState::Verifying);
            let t0 = Instant::now();
            let state = match self.verify(f, self.path_of(f), |_| {}).await {
                Ok(true) => FileState::Verified,
                Ok(false) => FileState::Corrupt { reason: "sha256 doesn't match".into() },
                Err(e) => FileState::Corrupt { reason: e },
            };
            eprintln!("[engine] verified {} in {:.1}s: {:?}", f.rel_path(), t0.elapsed().as_secs_f32(), state);
            self.set(f, state);
        }
    }

    pub fn is_installed(&self, m: &Manifest) -> bool {
        m.files.iter().all(|f| self.file_state(f) == FileState::Verified)
    }

    /// True while any model is downloading.
    pub fn any_pulling(&self) -> bool {
        !self.pulls.lock().unwrap().is_empty()
    }

    pub fn is_pulling(&self, id: &str) -> bool {
        self.pulls.lock().unwrap().contains_key(id)
    }

    /// installed | pulling | verifying | partial | not_installed, and for custom models whose files
    /// are all here: needs_test (the test image hasn't run) | test_failed
    pub fn model_state(&self, m: &Manifest) -> &'static str {
        if self.is_pulling(&m.id) {
            return "pulling";
        }
        let states: Vec<FileState> = m.files.iter().map(|f| self.file_state(f)).collect();
        // Size tags and models share text encoders and VAEs: a shared file that's here, or a
        // shared `.part` another model's pull wrote (or is writing), doesn't make this one partly
        // downloaded. It's started once its own weights are on disk, or a `.part` its own pull
        // wrote is waiting.
        let started = m.files.iter().zip(&states).any(|(f, s)| (f.role.is_weights() && *s != FileState::Missing) || (matches!(s, FileState::Partial { .. }) && self.owns_part(m, f)));
        if states.iter().all(|s| *s == FileState::Verified) {
            // A custom model is ready only once its test image passed.
            match m.custom.as_ref().map(|c| c.test.as_ref().map(|t| t.ok)) {
                None | Some(Some(true)) => "installed",
                Some(Some(false)) => "test_failed",
                Some(None) => "needs_test",
            }
        } else if !started {
            "not_installed"
        } else if states.iter().any(|s| matches!(s, FileState::Verifying | FileState::Unverified)) {
            "verifying"
        } else if states.iter().any(|s| *s != FileState::Missing) {
            "partial"
        } else {
            "not_installed"
        }
    }

    /// Bytes still to download for this model (verified files count as done).
    pub fn download_size(&self, m: &Manifest) -> u64 {
        m.files
            .iter()
            .map(|f| match self.file_state(f) {
                FileState::Verified | FileState::Verifying | FileState::Unverified => 0,
                FileState::Partial { bytes } => f.size.saturating_sub(bytes),
                _ => f.size,
            })
            .sum()
    }

    /// A download that stopped (cancel, crash, power cut, reboot) and waits for the user: never
    /// resumed automatically, since the connection may be metered now. `None` while pulling,
    /// when installed, or when nothing of it is on disk. Counts the runtime while that isn't ready.
    pub fn paused(&self, m: &Manifest) -> Option<Paused> {
        if self.is_pulling(&m.id) || self.model_state(m) != "partial" {
            return None;
        }
        let mut p = Paused { bytes: 0, total: 0, updated_ms: None };
        for f in &m.files {
            p.total += f.size;
            match self.file_state(f) {
                FileState::Verified | FileState::Verifying | FileState::Unverified => p.bytes += f.size,
                FileState::Partial { bytes } => {
                    p.bytes += bytes;
                    p.updated_ms = p.updated_ms.max(last_written(&self.part_path(f)));
                }
                _ => {}
            }
        }
        if let Some(r) = self.runtime_paused() {
            p.bytes += r.bytes;
            p.total += r.total;
            p.updated_ms = p.updated_ms.max(r.updated_ms);
        }
        Some(p)
    }

    /// The image runtime's paused download, if it isn't installed and a `.part` of it is on disk.
    pub fn runtime_paused(&self) -> Option<Paused> {
        if self.any_pulling() || self.runtimes.ready() {
            return None;
        }
        let rt = self.runtimes.active()?;
        let parts = self.runtimes.partial_downloads();
        if parts.is_empty() {
            return None;
        }
        let total: u64 = rt.files.iter().map(|f| f.size).sum();
        let need: u64 = rt.files.iter().map(|f| f.size).sum::<u64>() + rt.files.iter().map(|f| f.unpacked_size).sum::<u64>();
        let bytes = (need - self.runtimes.needed_bytes(rt).min(need)).min(total);
        let updated_ms = parts.iter().filter_map(|(n, _)| last_written(&self.runtimes.download_dir().join(n))).max();
        Some(Paused { bytes, total, updated_ms })
    }

    /// fits | offload | needs_more_memory | no_gpu, with the numbers (see fit.rs).
    pub fn fit(&self, m: &Manifest) -> crate::fit::Fit {
        crate::fit::fit(m, self.machine.as_ref())
    }

    /// The fit for one task (generate or edit): only that task's measurements count.
    pub fn fit_for(&self, m: &Manifest, task: crate::registry::Task) -> crate::fit::Fit {
        crate::fit::fit_for(m, task, self.machine.as_ref())
    }

    /// The measured time for this task to show here ("on your GPU" or "on <gpu>"), or none.
    pub fn timing(&self, m: &Manifest, task: crate::registry::Task) -> Option<crate::auto::Timing> {
        crate::auto::timing(m, task, self.gpu.as_ref().map(|g| g.name.as_str()), self.machine.is_some_and(|mc| mc.integrated))
    }

    /// A full id (`z-image-turbo:q4`) or a bare model id (`z-image-turbo`) to the variant to use.
    pub fn resolve(&self, id: &str) -> Result<Manifest, BackendError> {
        let reg = self.registry();
        if let Some(m) = reg.get(id) {
            return Ok(m.clone());
        }
        self.default_variant(id).ok_or_else(|| {
            let hint = match id.split_once(':') {
                Some((fam, _)) if !reg.variants(fam).is_empty() => {
                    format!(" {fam} comes in: {}.", reg.variants(fam).iter().filter_map(|m| m.tag.as_deref()).collect::<Vec<_>>().join(", "))
                }
                _ => String::new(),
            };
            err("unknown_model", format!("There's no model called {id:?}.{hint} Run `list --all` to see the models."))
        })
    }

    /// The tag a bare model id means here: the largest tag already installed that runs on this
    /// machine (so a bare id never starts a surprise download), else the largest tag that fits
    /// (measured or estimated) by the fit formula, else the smallest.
    pub fn default_variant(&self, family: &str) -> Option<Manifest> {
        use crate::fit::Tier;
        let reg = self.registry();
        let vs = reg.variants(family); // smallest first
        let runs = |m: &&Manifest| matches!(self.fit(m).tier, Tier::Fits | Tier::Offload);
        vs.iter().rev().find(|m| self.is_installed(m) && runs(m)).or_else(|| vs.iter().rev().find(|m| self.fit(m).tier == Tier::Fits)).or(vs.first()).map(|m| (*m).clone())
    }

    /// When the user accepted this model's licence; `None` if it was never asked for or not yet given.
    pub fn licence_accepted_ms(&self, m: &Manifest) -> Option<i64> {
        if !m.licence.accept_required {
            return None;
        }
        self.db.licence_accepted(&m.family, &m.licence.url)
    }

    /// The original repo is gated and the user hasn't accepted its licence in Dawnroom yet.
    pub fn needs_licence(&self, m: &Manifest) -> bool {
        m.licence.accept_required && self.licence_accepted_ms(m).is_none()
    }

    /// Records the user's acceptance (covers every tag of the model). Only the user does this,
    /// through Studio or the CLI: MCP has no way to.
    pub fn accept_licence(&self, m: &Manifest) -> Result<i64, BackendError> {
        let now = chrono_ms();
        self.db.accept_licence(&m.family, &m.licence.url, now).map_err(|e| err("db_error", format!("Couldn't save the licence acceptance ({e}).")))?;
        Ok(now)
    }

    /// The plain-words refusal for a pull that needs the licence first.
    pub fn licence_message(m: &Manifest) -> String {
        let name = m.licence.name.as_deref().unwrap_or(&m.licence.spdx);
        format!(
            "{} is under the {name} ({}). Its official repo asks people to accept that licence, so Dawnroom asks too: accept it in Dawnroom Studio, or run `dawnroom pull {}` and answer yes. Nothing was downloaded.",
            m.name, m.licence.url, m.family
        )
    }

    /// Starts pulling in the background (or joins a pull already running) and returns its event
    /// stream. Dropping the receiver doesn't stop the pull; `cancel_pull` does.
    pub fn pull(self: &Arc<Self>, id: &str) -> Result<broadcast::Receiver<PullEvent>, BackendError> {
        let m = self.resolve(id)?;
        let id = m.id.as_str();
        if self.needs_licence(&m) {
            return Err(err("licence_not_accepted", Self::licence_message(&m)));
        }
        let mut pulls = self.pulls.lock().unwrap();
        if let Some(p) = pulls.get(id) {
            return Ok(p.tx.subscribe());
        }
        let (tx, rx) = broadcast::channel(256);
        let pull = Pull { tx, cancel: Default::default(), last: Default::default() };
        pulls.insert(id.to_string(), pull.clone());
        drop(pulls);
        self.pull_errors.lock().unwrap().remove(id);
        let me = self.clone();
        tokio::spawn(async move {
            let mut result = me.pull_model(&m, &pull).await;
            // A custom model is ready after one small real image; a passed test isn't run again.
            if result.is_ok() && m.custom.is_some() && !me.resolve(&m.id).is_ok_and(|x| x.custom_ready()) {
                result = me.run_test(&m.id, &pull).await;
            }
            let ev = match result {
                Ok(()) => PullEvent::Done { model: m.id.clone() },
                Err(e) => {
                    eprintln!("[engine] pull {} stopped: {} ({})", m.id, e.message, e.code);
                    let ev = PullEvent::Error { code: e.code, message: e.message };
                    me.pull_errors.lock().unwrap().insert(m.id.clone(), ev.clone());
                    ev
                }
            };
            me.pulls.lock().unwrap().remove(&m.id);
            pull.send(ev);
        });
        Ok(rx)
    }

    /// The test image of a custom model, as the last stage of its pull. The result is saved in its
    /// manifest either way; a failure keeps the files.
    async fn run_test(&self, id: &str, p: &Pull) -> Result<(), BackendError> {
        let Some(tester) = self.tester.get().cloned() else {
            return Err(err("test_failed", "The engine can't run the test image right now. Pull again to retry."));
        };
        let p2 = p.clone();
        let on = Box::new(move |stage: String, step: Option<u32>, total: Option<u32>| {
            let (bytes, total) = (step.unwrap_or(0) as u64, total.unwrap_or(0) as u64);
            p2.send(PullEvent::Progress { file: format!("test:{stage}"), file_index: 0, file_count: 1, stage: "testing".into(), bytes, total, speed_bps: 0, wait: None });
        });
        p.send(PullEvent::Progress { file: "test:starting".into(), file_index: 0, file_count: 1, stage: "testing".into(), bytes: 0, total: 0, speed_bps: 0, wait: None });
        let result = tester(id.to_string(), on).await;
        crate::custom::save_test(self, id, result.clone()).map_err(|e| err("disk_error", e))?;
        if result.ok {
            Ok(())
        } else {
            Err(err("test_failed", format!("{} The files are kept: pull again to retry the test, or delete the model with `dawnroom rm {id}`.", result.message)))
        }
    }

    /// Asks a running pull to stop; the `.part` stays for resume. False if nothing is pulling.
    pub fn cancel_pull(&self, id: &str) -> bool {
        match self.pulls.lock().unwrap().get(id) {
            Some(p) => {
                p.cancel.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// The latest progress event of a running pull (`None` if not pulling or nothing sent yet).
    pub fn pull_progress(&self, id: &str) -> Option<PullEvent> {
        self.pulls.lock().unwrap().get(id).and_then(|p| p.last.lock().unwrap().clone())
    }

    /// How the last pull of this model failed or was cancelled, until the next pull starts.
    pub fn pull_error(&self, id: &str) -> Option<PullEvent> {
        self.pull_errors.lock().unwrap().get(id).cloned()
    }

    /// Everything a pull of this model still has to put on disk: its files plus the runtime.
    pub fn needed_bytes(&self, m: &Manifest) -> u64 {
        self.download_size(m) + self.runtimes.active_needed_bytes()
    }

    async fn pull_model(&self, m: &Manifest, p: &Pull) -> Result<(), BackendError> {
        let t0 = Instant::now();
        self.runtimes.check_glibc()?;
        crate::disk::check(&self.dir, self.needed_bytes(m), &m.name)?;
        // The runtime first: it's small next to the weights, and a model is useless without it.
        let rt_count = if self.runtimes.ready() || self.runtimes.active().is_none() { 0 } else { self.runtimes.active().map(|r| r.files.len()).unwrap_or(0) };
        let count = m.files.len() + rt_count;
        if rt_count > 0 {
            let p2 = p.clone();
            let on_step = move |s: crate::runtimes::Step| {
                p2.send(PullEvent::Progress { file: s.file, file_index: s.file_index, file_count: count, stage: s.stage.into(), bytes: s.bytes, total: s.total, speed_bps: s.speed_bps, wait: s.wait });
            };
            self.runtimes.ensure(&p.cancel, &on_step).await.map_err(|e| if e.code == "cancelled" { cancelled() } else { e })?;
        }
        for (i, f) in m.files.iter().enumerate() {
            let i = i + rt_count;
            // Wait out a background verification of this file rather than racing it.
            while self.file_state(f) == FileState::Verifying {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            if p.cancelled() {
                return Err(cancelled());
            }
            match self.file_state(f) {
                FileState::Verified => {
                    p.send(PullEvent::Progress { file: f.rel_path(), file_index: i, file_count: count, stage: "present".into(), bytes: f.size, total: f.size, speed_bps: 0, wait: None });
                    continue;
                }
                FileState::Unverified
                    // Present with the right size: hash it before deciding to download.
                    if self.verify_with_progress(f, self.path_of(f), i, count, p).await? => {
                        self.set(f, FileState::Verified);
                        continue;
                    }
                _ => {}
            }
            self.download_file(f, &m.id, i, count, p).await?;
        }
        eprintln!("[engine] pulled {} in {:.1}s", m.id, t0.elapsed().as_secs_f32());
        Ok(())
    }

    async fn verify_with_progress(&self, f: &ModelFile, path: PathBuf, i: usize, count: usize, p: &Pull) -> Result<bool, BackendError> {
        self.set(f, FileState::Verifying);
        let (p2, file, total) = (p.clone(), f.rel_path(), f.size);
        let mut last = Instant::now();
        let ok = self
            .verify(f, path, move |bytes| {
                if last.elapsed() >= Duration::from_millis(250) || bytes == total {
                    last = Instant::now();
                    p2.send(PullEvent::Progress { file: file.clone(), file_index: i, file_count: count, stage: "verifying".into(), bytes, total, speed_bps: 0, wait: None });
                }
            })
            .await
            .map_err(|e| err("verify_failed", e))?;
        Ok(ok)
    }

    async fn download_file(&self, f: &ModelFile, owner: &str, i: usize, count: usize, p: &Pull) -> Result<(), BackendError> {
        let final_path = self.path_of(f);
        if f.repo == crate::custom::LOCAL_REPO {
            return Err(err("file_missing", format!("{} was imported from this computer and isn't in the models folder any more (or it changed). Import it again.", f.filename)));
        }
        let part = self.part_path(f);
        std::fs::create_dir_all(final_path.parent().unwrap()).map_err(|e| err("disk_error", format!("Couldn't create {} ({e}).", self.dir.display())))?;
        // A wrong file in the final place gets replaced. (For a dev hardlink this only removes the link.)
        if final_path.exists() {
            std::fs::remove_file(&final_path).map_err(|e| err("disk_error", format!("Couldn't replace {} ({e}). Is the engine still using it?", final_path.display())))?;
        }
        let file = f.rel_path();
        let url = f.url(&self.endpoint);
        let job = Fetch { url: &url, part: &part, size: f.size, sha256: &f.sha256, cancel: &p.cancel, source: "Hugging Face", again: "Pull again", retry: Retry::default(), owner: Some(owner) };
        let result = fetch_to_part(&self.http, job, |bytes, speed, wait| {
            self.set(f, FileState::Partial { bytes });
            let stage = if wait.is_some() { "waiting" } else { "downloading" };
            p.send(PullEvent::Progress { file: file.clone(), file_index: i, file_count: count, stage: stage.into(), bytes, total: f.size, speed_bps: speed, wait });
        })
        .await;
        if let Err(e) = result {
            return Err(if e.code == "cancelled" { cancelled() } else { e });
        }

        if !self.verify_with_progress(f, part.clone(), i, count, p).await? {
            let _ = crate::download::remove_part(&part);
            self.set(f, FileState::Missing);
            return Err(err("checksum_mismatch", format!("{} downloaded but its sha256 doesn't match the registry, so it was deleted. Pull again.", f.filename)));
        }
        std::fs::rename(&part, &final_path).map_err(|e| err("disk_error", format!("Couldn't move the download into place ({e}).")))?;
        let _ = std::fs::remove_file(crate::download::sidecar_path(&part));
        // The rename keeps size and mtime, but the cache is keyed by path: store it for the final name.
        if let Ok(meta) = std::fs::metadata(&final_path) {
            let _ = self.db.save_hash(&final_path.to_string_lossy(), meta.len(), mtime_ms(&meta), &f.sha256);
        }
        self.set(f, FileState::Verified);
        Ok(())
    }

    /// Deletes a model's files, keeping any that another installed model also uses.
    /// Returns (deleted, kept) relative paths.
    pub fn delete(&self, id: &str) -> Result<(Vec<String>, Vec<String>), BackendError> {
        let m = self.resolve(id)?;
        if self.is_pulling(&m.id) {
            return Err(err("model_busy", "This model is downloading. Wait for the pull to finish, then delete it."));
        }
        let in_use: HashSet<String> = self
            .registry()
            .models
            .iter()
            .filter(|o| o.id != m.id && self.is_installed(o))
            .flat_map(|o| o.files.iter().map(|f| f.rel_path()))
            .collect();
        let (mut deleted, mut kept) = (Vec::new(), Vec::new());
        for f in &m.files {
            if in_use.contains(&f.rel_path()) {
                kept.push(f.rel_path());
                continue;
            }
            let mut removed = false;
            let _ = std::fs::remove_file(crate::download::sidecar_path(&self.part_path(f)));
            for p in [self.path_of(f), self.part_path(f)] {
                match std::fs::remove_file(&p) {
                    Ok(()) => removed = true,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(err("disk_error", format!("Couldn't delete {} ({e}). Is it in use?", p.display()))),
                }
            }
            self.set(f, FileState::Missing);
            if removed {
                deleted.push(f.rel_path());
            }
        }
        // A custom model leaves the registry too (its manifest file is deleted).
        if m.custom.is_some() {
            crate::custom::forget(self, &m.id).map_err(|e| err("disk_error", e))?;
        }
        Ok((deleted, kept))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::path::Path;

    fn manifest(id: &str, files: &[(&str, &[u8])]) -> Manifest {
        let hex = |b: &[u8]| {
            let mut h = Sha256::new();
            h.update(b);
            h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        let files = files
            .iter()
            .enumerate()
            .map(|(i, (name, body))| ModelFile {
                role: if i == 0 { crate::registry::Role::DiffusionModel } else { crate::registry::Role::Vae },
                category: if i == 0 { "diffusion_models".into() } else { "vae".into() },
                filename: name.to_string(),
                repo: "a/b".into(),
                revision: "0".repeat(40),
                path: name.to_string(),
                size: body.len() as u64,
                sha256: hex(body),
                provenance: None,
            })
            .collect();
        let toml = format!(
            "schema = 1\nid = \"{id}\"\nname = \"n\"\ntagline = \"t\"\nbackend = \"sd-cpp@x\"\n[licence]\nspdx = \"MIT\"\nurl = \"https://x\"\ncommercial = true\n[defaults]\nsteps = 1\ncfg = 1.0\nwidth = 512\nheight = 512\n[[files]]\nrole = \"diffusion_model\"\ncategory = \"diffusion_models\"\nfilename = \"x\"\nrepo = \"a/b\"\nrevision = \"{}\"\npath = \"x\"\nsize = 1\nsha256 = \"{}\"",
            "0".repeat(40),
            "0".repeat(64)
        );
        let mut m = crate::registry::parse(&toml).unwrap().remove(0);
        m.files = files;
        m
    }

    fn fixed_rt(dir: &Path) -> Arc<Runtimes> {
        Arc::new(Runtimes::fixed(dir, "sd-server.exe".into()))
    }

    fn setup(models: Vec<Manifest>) -> (Registry, PathBuf, Arc<Db>) {
        let dir = std::env::temp_dir().join(format!("models-test-{}", uuid::Uuid::new_v4()));
        (Registry::new(models).unwrap(), dir, Arc::new(Db::open_in_memory().unwrap()))
    }

    fn write(dir: &Path, rel: &str, body: &[u8]) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// Schnell's pull writing a shared VAE `.part` made FLUX.1 dev, PixArt and SD 3.5 Large
    /// look paused, and Discard on one of them could delete the `.part` being written.
    #[tokio::test]
    async fn a_shared_part_belongs_to_the_pull_that_wrote_it() {
        let (reg, data, db) = setup(vec![manifest("a", &[("a.gguf", b"aaaa"), ("v.st", b"shared vae")]), manifest("b", &[("b.gguf", b"bbbb"), ("v.st", b"shared vae")])]);
        let cfg = crate::config::Config { data_dir: data.clone(), ..crate::config::Config::default() };
        let dir = cfg.models_dir();
        let part = dir.join("vae/v.st.part");
        write(&dir, "vae/v.st.part", b"shared");
        let side = |owner: Option<&str>| {
            let s = crate::download::Sidecar { url: "u".into(), size: 10, sha256: "0".into(), synced_bytes: 6, updated_ms: 1, model: owner.map(str::to_string) };
            std::fs::write(crate::download::sidecar_path(&part), serde_json::to_vec(&s).unwrap()).unwrap();
        };
        side(Some("a"));
        let m = Models::new(reg, None, None, dir.clone(), db, "http://unused", fixed_rt(&dir));
        let reg = m.registry();
        let (a, b) = (reg.get("a").unwrap(), reg.get("b").unwrap());
        assert_eq!(m.model_state(a), "partial", "a's own pull wrote it");
        assert!(m.paused(a).is_some());
        assert_eq!(m.model_state(b), "not_installed", "b only shares the file");
        assert!(m.paused(b).is_none());
        let owners: Vec<_> = crate::storage::partials(&cfg, &m.registry()).into_iter().map(|p| (p.file, p.owner)).collect();
        assert_eq!(owners, vec![("models/vae/v.st.part".to_string(), "a".to_string())], "listed once, under a: b's Discard deletes nothing");

        // While a pull (here b's, resuming the shared file) is writing it: nobody is paused, and
        // the `.part` counts as in use, so Discard refuses it.
        let (tx, _) = broadcast::channel(4);
        m.pulls.lock().unwrap().insert("b".into(), Pull { tx, cancel: Arc::new(AtomicBool::new(false)), last: Arc::new(Mutex::new(None)) });
        assert!(m.part_in_use(&part) && m.part_in_use(&data.join("models").join("vae").join("v.st.part")));
        assert!(!m.part_in_use(&dir.join("diffusion_models/a.gguf.part")));
        assert_eq!(m.model_state(a), "not_installed", "a's .part is being written by b's pull");
        assert!(m.paused(a).is_none());
        m.pulls.lock().unwrap().clear();

        // A sidecar from before owners were recorded: the shared file makes nobody paused.
        side(None);
        assert_eq!((m.model_state(a), m.model_state(b)), ("not_installed", "not_installed"));
        // ...but a weights `.part` still makes its own model paused.
        write(&dir, "diffusion_models/b.gguf.part", b"bb");
        m.refresh();
        assert_eq!((m.model_state(a), m.model_state(b)), ("not_installed", "partial"));
        let owners: Vec<_> = crate::storage::partials(&cfg, &m.registry()).into_iter().map(|p| p.owner).collect();
        assert_eq!(owners, vec!["a".to_string(), "b".to_string()], "the old shared one under the first model, b's weights under b");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn hash_mismatch_is_rejected_and_cached_hash_is_reused() {
        let (reg, dir, db) = setup(vec![manifest("a", &[("a.gguf", b"good weights"), ("v.st", b"vae")])]);
        write(&dir, "diffusion_models/a.gguf", b"evil weights"); // same size, wrong bytes
        write(&dir, "vae/v.st", b"vae");
        let m = Models::new(reg, None, None, dir.clone(), db, "http://unused", fixed_rt(&dir));
        m.clone().verify_all().await;
        let reg = m.registry();
        let a = reg.get("a").unwrap();
        assert!(matches!(m.file_state(&a.files[0]), FileState::Corrupt { .. }));
        assert_eq!(m.file_state(&a.files[1]), FileState::Verified);
        assert!(!m.is_installed(a));
        // A fresh manager trusts the cache for the unchanged file without re-hashing it.
        let again = Models::new((*m.registry()).clone(), None, None, dir.clone(), m.db.clone(), "http://unused", fixed_rt(&dir));
        assert_eq!(again.file_state(&a.files[1]), FileState::Verified);
    }

    /// Serves `body` for any path, `chunk` bytes every `delay`, honouring `Range: bytes=N-`.
    /// Records the start offset of every request.
    async fn slow_server(body: Vec<u8>, chunk: usize, delay: Duration) -> (String, Arc<Mutex<Vec<u64>>>) {
        use axum::http::{HeaderMap, StatusCode, header};
        let starts = Arc::new(Mutex::new(Vec::new()));
        let (body, seen) = (Arc::new(body), starts.clone());
        let app = axum::Router::new().fallback(move |headers: HeaderMap| {
            let (body, seen) = (body.clone(), seen.clone());
            async move {
                let start: usize = headers
                    .get(header::RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("bytes="))
                    .and_then(|v| v.trim_end_matches('-').parse().ok())
                    .unwrap_or(0);
                seen.lock().unwrap().push(start as u64);
                let len = body.len();
                let stream = async_stream::stream! {
                    let mut at = start;
                    while at < len {
                        tokio::time::sleep(delay).await;
                        let end = (at + chunk).min(len);
                        yield Ok::<_, std::io::Error>(body[at..end].to_vec());
                        at = end;
                    }
                };
                let mut resp = axum::response::Response::new(axum::body::Body::from_stream(stream));
                if start > 0 {
                    *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
                    resp.headers_mut().insert(header::CONTENT_RANGE, format!("bytes {start}-{}/{len}", len - 1).parse().unwrap());
                }
                resp
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, starts)
    }

    async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
        for _ in 0..500 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test]
    async fn pull_survives_detach_and_cancel_keeps_the_part_for_resume() {
        let body: Vec<u8> = (0..64 * 1024u32).map(|i| (i % 251) as u8).collect();
        let (reg, dir, db) = setup(vec![manifest("tiny", &[("tiny.gguf", &body)])]);
        let (url, starts) = slow_server(body.clone(), 1024, Duration::from_millis(10)).await; // ~0.7 s
        let m = Models::new(reg, None, None, dir.clone(), db, &url, fixed_rt(&dir));
        let f = m.registry().get("tiny").unwrap().files[0].clone();

        // Start, then detach at once (a Studio reload): the pull carries on.
        drop(m.pull("tiny").unwrap());
        wait_until("some bytes", || matches!(m.file_state(&f), FileState::Partial { bytes } if bytes > 0)).await;
        assert!(matches!(m.pull_progress("tiny"), Some(PullEvent::Progress { .. })));

        // Cancel explicitly: it stops, says why, and keeps the .part.
        assert!(m.cancel_pull("tiny"));
        wait_until("the pull to stop", || !m.is_pulling("tiny")).await;
        assert!(matches!(m.pull_error("tiny"), Some(PullEvent::Error { ref code, .. }) if code == "cancelled"));
        let part = std::fs::metadata(m.part_path(&f)).unwrap().len();
        assert!(part > 0 && part < body.len() as u64, "part is {part} bytes");
        assert!(!m.cancel_pull("tiny"), "nothing left to cancel");

        // Pull again, detached again: it resumes from the .part and verifies.
        drop(m.pull("tiny").unwrap());
        assert!(m.pull_error("tiny").is_none(), "a new pull clears the old error");
        wait_until("install", || m.is_installed(m.registry().get("tiny").unwrap())).await;
        let starts = starts.lock().unwrap().clone();
        assert_eq!(starts.len(), 2);
        assert_eq!(starts[0], 0);
        assert!(starts[1] > 0, "second request should resume with a Range");
        assert_eq!(std::fs::read(m.path_of(&f)).unwrap(), body);
    }

    fn real(machine: Option<Machine>) -> Arc<Models> {
        let reg = Registry::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("registry")).unwrap();
        let dir = std::env::temp_dir().join(format!("models-test-{}", uuid::Uuid::new_v4()));
        Models::new(reg, None, machine, dir.clone(), Arc::new(Db::open_in_memory().unwrap()), "http://unused", fixed_rt(&dir))
    }

    #[tokio::test]
    async fn bare_ids_pick_the_largest_tag_that_fits_or_one_already_installed() {
        let laptop = Machine { gpu_mib: 8151, integrated: false, ram_total_mib: 24 * 1024 };
        let m = real(Some(laptop));
        // 8 GB: q4 is measured to fit; q8's estimate (files + 2 GB) needs offload.
        assert_eq!(m.resolve("z-image-turbo").unwrap().id, "z-image-turbo:q4");
        assert_eq!(m.resolve("z-image-turbo:q8").unwrap().id, "z-image-turbo:q8", "a full id is taken as is");
        // Nothing fits: the smallest tag.
        assert_eq!(m.resolve("flux2-dev").unwrap().id, "flux2-dev:q4");
        // 24 GB card: q8 fits by its estimate.
        let big = real(Some(Machine { gpu_mib: 24 * 1024, integrated: false, ram_total_mib: 64 * 1024 }));
        assert_eq!(big.resolve("z-image-turbo").unwrap().id, "z-image-turbo:q8");
        // ...unless a tag that runs here is already installed: no surprise download.
        for f in &big.registry().get("z-image-turbo:q4").unwrap().files {
            big.set(f, FileState::Verified);
        }
        assert_eq!(big.resolve("z-image-turbo").unwrap().id, "z-image-turbo:q4");
        let e = m.resolve("z-image-turbo:q2").unwrap_err();
        assert!(e.message.contains("comes in: q4, q8"), "{}", e.message);
        assert_eq!(real(None).resolve("qwen-image").unwrap().id, "qwen-image:q4", "no GPU: the smallest");
    }

    #[tokio::test]
    async fn gated_originals_need_the_licence_before_a_pull() {
        let m = real(None);
        let err = m.pull("flux1-schnell:q4").unwrap_err();
        assert_eq!(err.code, "licence_not_accepted");
        assert!(err.message.contains("https://huggingface.co/black-forest-labs/FLUX.1-schnell") && err.message.contains("Nothing was downloaded"), "{}", err.message);
        assert!(!m.is_pulling("flux1-schnell:q4"));
        // One acceptance covers every tag of the model; ungated models never ask.
        m.accept_licence(m.registry().get("flux1-schnell:q4").unwrap()).unwrap();
        assert!(!m.needs_licence(m.registry().get("flux1-schnell:q8").unwrap()));
        assert!(m.licence_accepted_ms(m.registry().get("flux1-schnell:q8").unwrap()).is_some());
        assert!(!m.needs_licence(m.registry().get("z-image-turbo:q4").unwrap()));
        assert!(m.needs_licence(m.registry().get("flux1-dev:q4").unwrap()));
    }

    #[tokio::test]
    async fn shared_files_are_counted_once_and_kept_on_delete() {
        let shared: (&str, &[u8]) = ("shared.st", b"shared vae");
        let (reg, dir, db) = setup(vec![manifest("a", &[("a.gguf", b"aaa"), shared]), manifest("b", &[("b.gguf", b"bbb"), shared])]);
        for (rel, body) in [("diffusion_models/a.gguf", &b"aaa"[..]), ("diffusion_models/b.gguf", b"bbb"), ("vae/shared.st", b"shared vae")] {
            write(&dir, rel, body);
        }
        let m = Models::new(reg, None, None, dir.clone(), db, "http://unused", fixed_rt(&dir));
        m.clone().verify_all().await;
        assert_eq!(m.registry().all_files().len(), 3);
        let (deleted, kept) = m.delete("a").unwrap();
        assert_eq!(deleted, ["diffusion_models/a.gguf"]);
        assert_eq!(kept, ["vae/shared.st"]);
        assert!(m.is_installed(m.registry().get("b").unwrap()));
        let (deleted, _) = m.delete("b").unwrap();
        assert_eq!(deleted.len(), 2);
    }
}
