//! Runtimes on demand: the sd.cpp builds in `registry/runtimes/*.toml`, which one this machine
//! should use (`pick`, a pure function of the detected hardware), and installing it into
//! `<data>/runtimes/<id>/`: download the zip(s) to `.part` with resume, verify sha256, unpack
//! into a temp folder, rename it into place, delete the zips. Only the runtime in use and the
//! one before it are kept.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::backend::BackendError;
use crate::download::{Fetch, Retry, Wait, err, fetch_to_part, sha256_file};
use crate::hardware::{GpuInfo, Hardware, Vendor};

pub const SCHEMA: u32 = 1;
const MARKER: &str = "runtime.json";

pub const NVIDIA_DRIVERS: &str = "https://www.nvidia.com/en-us/drivers/";
pub const AMD_DRIVERS: &str = "https://www.amd.com/en/support/download/drivers.html";
pub const INTEL_DRIVERS: &str = "https://www.intel.com/content/www/us/en/support/detect.html";
/// Oldest CUDA architecture the cuda12 build ships kernels for (build.yml at 465adf3: 61;70;…;120).
const MIN_COMPUTE_CAP: (u32, u32) = (6, 1);

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeManifest {
    pub schema: u32,
    /// Folder name under `<data>/runtimes/`, e.g. `sd-cpp-master-1086-465adf3-windows-cuda12`.
    pub id: String,
    /// What model manifests name in `backend`, e.g. `sd-cpp@master-1086-465adf3`.
    pub runtime: String,
    /// `windows-cuda12`, `windows-vulkan`, `linux-vulkan`.
    pub build: String,
    pub os: String,
    /// `cuda` | `vulkan`.
    pub gpu: String,
    pub experimental: bool,
    pub server: String,
    pub cli: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_driver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_driver_source: Option<String>,
    /// Linux: the C library version the build was linked against (its binaries refuse older ones).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_glibc: Option<String>,
    pub files: Vec<RuntimeFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFile {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub unpacked_size: u64,
}

impl RuntimeManifest {
    pub fn download_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
    pub fn unpacked_size(&self) -> u64 {
        self.files.iter().map(|f| f.unpacked_size).sum()
    }
    fn validate(&self) -> Result<(), String> {
        let id = &self.id;
        if self.schema != SCHEMA {
            return Err(format!("runtime {id}: schema {} isn't supported", self.schema));
        }
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!("runtime {id:?}: id must be letters, digits and dashes"));
        }
        for n in [&self.server, &self.cli] {
            if n.contains(['/', '\\']) || n.contains("..") {
                return Err(format!("runtime {id}: {n:?} must be a plain file name"));
            }
        }
        for f in &self.files {
            if f.sha256.len() != 64 || !f.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
                return Err(format!("runtime {id}: {}: bad sha256", f.name));
            }
            if !f.url.starts_with("https://") || f.name.contains(['/', '\\']) || f.size == 0 {
                return Err(format!("runtime {id}: {}: bad url, name or size", f.name));
            }
        }
        if self.files.is_empty() {
            return Err(format!("runtime {id}: no files"));
        }
        Ok(())
    }
}

/// Every runtime manifest in `dir`, validated, sorted by id.
pub fn load_dir(dir: &Path) -> Result<Vec<RuntimeManifest>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("Couldn't read the runtime registry at {} ({e}).", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        let text = std::fs::read_to_string(&p).map_err(|e| format!("Couldn't read {} ({e}).", p.display()))?;
        let m: RuntimeManifest = toml::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
        m.validate()?;
        if out.iter().any(|o: &RuntimeManifest| o.id == m.id) {
            return Err(format!("runtime {}: duplicate id", m.id));
        }
        out.push(m);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

// ---------- Which runtime ----------

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Ready to use (an experimental runtime still says `ok`; see `experimental`).
    Ok,
    DriverTooOld,
    /// An NVIDIA card is there but nvidia-smi doesn't answer.
    NoDriver,
    NoGpu,
    UnsupportedOs,
    /// Linux: the C library is older than this runtime build needs.
    GlibcTooOld,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Choice {
    /// The GPU Dawnroom will use (index into `Hardware::gpus`).
    pub gpu_index: Option<usize>,
    /// Runtime manifest id, even when the driver needs updating first (so pulls still work).
    pub runtime: Option<String>,
    pub build: Option<String>,
    pub experimental: bool,
    pub status: Status,
    /// Why this GPU and runtime, in plain words.
    pub reason: String,
    /// What to do, when `status` isn't `ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// The user forced the GPU or runtime (`--gpu`, `--runtime`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub forced: bool,
}

/// "572.61" < "596.36"; compares numbers part by part, never as floats.
pub fn version_lt(a: &str, b: &str) -> bool {
    let parse = |s: &str| s.split('.').map(|p| p.trim().parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parse(a) < parse(b)
}

fn driver_link(v: Vendor) -> &'static str {
    match v {
        Vendor::Nvidia => NVIDIA_DRIVERS,
        Vendor::Amd => AMD_DRIVERS,
        _ => INTEL_DRIVERS,
    }
}

/// Preference: discrete before integrated, then NVIDIA (the tested CUDA path), then most memory.
fn rank(g: &GpuInfo) -> (bool, bool, std::cmp::Reverse<u64>) {
    (g.integrated, g.vendor != Vendor::Nvidia, std::cmp::Reverse(g.memory_mib()))
}

/// Picks the GPU and runtime build for this machine. `force_gpu` is a name fragment,
/// `force_build` a build like `windows-vulkan`; both are for testing and odd machines.
pub fn pick(hw: &Hardware, builds: &[RuntimeManifest], force_gpu: Option<&str>, force_build: Option<&str>) -> Choice {
    let mine: Vec<&RuntimeManifest> = builds.iter().filter(|b| b.os == hw.os).collect();
    let mut choice = Choice { gpu_index: None, runtime: None, build: None, experimental: false, status: Status::Ok, reason: String::new(), message: None, link: None, forced: force_gpu.is_some() || force_build.is_some() };
    if mine.is_empty() {
        choice.status = Status::UnsupportedOs;
        choice.reason = format!("There's no Dawnroom runtime for {} in this version.", hw.os);
        choice.message = Some("v0.1 runs on Windows and Linux (x86_64). Mac support comes after v0.1.".into());
        return choice;
    }

    let mut order: Vec<usize> = (0..hw.gpus.len()).collect();
    order.sort_by_key(|&i| rank(&hw.gpus[i]));
    let gi = match force_gpu {
        Some(frag) => order.iter().copied().find(|&i| hw.gpus[i].name.to_lowercase().contains(&frag.to_lowercase())),
        None => order.first().copied(),
    };
    let Some(gi) = gi else {
        choice.status = Status::NoGpu;
        choice.reason = match force_gpu {
            Some(f) => format!("No GPU matches {f:?}."),
            None => "No GPU found.".into(),
        };
        choice.message = Some("Dawnroom needs a GPU: NVIDIA, AMD or Intel, including integrated graphics (experimental). If this computer has one, install or update its graphics driver and restart Dawnroom.".into());
        return choice;
    };
    let g = &hw.gpus[gi];
    choice.gpu_index = Some(gi);

    let find = |build_suffix: &str| mine.iter().find(|b| b.build.ends_with(build_suffix)).copied();
    let cap_ok = g.compute_cap.as_deref().map(|c| {
        let mut p = c.split('.').map(|x| x.parse::<u32>().unwrap_or(0));
        (p.next().unwrap_or(0), p.next().unwrap_or(0)) >= MIN_COMPUTE_CAP
    });
    let (build, reason) = match force_build {
        Some(fb) => (mine.iter().find(|b| b.build == fb).copied(), format!("Runtime forced to {fb}.")),
        None if g.vendor == Vendor::Nvidia && cap_ok != Some(false) && find("cuda12").is_some() => {
            (find("cuda12"), format!("{} is an NVIDIA GPU: CUDA, the tested path.", g.name))
        }
        None if g.vendor == Vendor::Nvidia && cap_ok == Some(false) => (
            find("vulkan"),
            format!("{} (compute {}) is older than this CUDA build supports (6.1+), so it uses Vulkan (experimental).", g.name, g.compute_cap.as_deref().unwrap_or("?")),
        ),
        None if g.integrated => (find("vulkan"), format!("{} is integrated graphics: Vulkan (experimental), working from shared system memory, so it's slow.", g.name)),
        None if g.vendor == Vendor::Nvidia => (find("vulkan"), format!("{} is an NVIDIA GPU; CUDA on {} comes after v0.1, so it uses Vulkan (experimental).", g.name, hw.os)),
        None => (find("vulkan"), format!("{} is {} {} GPU: Vulkan (experimental).", g.name, article(g.vendor.label()), g.vendor.label())),
    };
    let Some(build) = build else {
        choice.status = Status::UnsupportedOs;
        choice.reason = format!("{reason} But there's no such runtime for {}.", hw.os);
        return choice;
    };
    choice.runtime = Some(build.id.clone());
    choice.build = Some(build.build.clone());
    choice.experimental = build.experimental;
    choice.reason = reason;

    // The Linux build is linked against a new glibc; on an older one it can't even start.
    if let (Some(min), Some(have)) = (&build.min_glibc, &hw.glibc)
        && version_lt(have, min)
    {
        choice.status = Status::GlibcTooOld;
        choice.message = Some(format!("This Linux runtime needs glibc {min} or newer (Ubuntu 26.04, current Fedora or Arch); you have {have}."));
        return choice;
    }

    // Driver checks. Only CUDA has a known floor; Vulkan problems show up as the runtime not
    // listing the GPU (see `Runtimes::sees_gpu`).
    if build.gpu == "cuda" {
        match (&g.driver, &build.min_driver) {
            (None, _) => {
                choice.status = Status::NoDriver;
                choice.message = Some(format!("Found {} but its driver isn't answering (nvidia-smi failed). Install the latest NVIDIA driver, then restart Dawnroom.", g.name));
                choice.link = Some(NVIDIA_DRIVERS.into());
            }
            (Some(have), Some(min)) if version_lt(have, min) => {
                choice.status = Status::DriverTooOld;
                choice.message = Some(format!("Your NVIDIA driver is {have}; Dawnroom's CUDA runtime needs {min} or newer. Update it (the NVIDIA app works too), then restart Dawnroom."));
                choice.link = Some(NVIDIA_DRIVERS.into());
            }
            _ => {}
        }
    } else if g.driver.is_none() && hw.os == "windows" {
        choice.link = Some(driver_link(g.vendor).into());
    }
    choice
}

// ---------- Installing ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Marker {
    id: String,
    sha256: Vec<String>,
    installed_ms: i64,
    last_used_ms: i64,
}

/// A device the runtime itself can use (`sd-cli --list-devices`): `CUDA0`, `Vulkan1`, `CPU`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Device {
    pub name: String,
    pub description: String,
}

/// What to start: the server exe and runtime-level arguments (e.g. `--backend Vulkan1`).
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub exe: PathBuf,
    pub args: Vec<String>,
}

/// One step of an install, for progress: `stage` is downloading | verifying | unpacking.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub file: String,
    pub file_index: usize,
    pub file_count: usize,
    pub stage: &'static str,
    pub bytes: u64,
    pub total: u64,
    pub speed_bps: u64,
    /// Set while the download waits to retry (stage "waiting").
    pub wait: Option<Wait>,
}

pub struct Runtimes {
    dir: PathBuf,
    pub all: Vec<RuntimeManifest>,
    pub choice: Choice,
    /// The GPU the choice points at, for pinning the device.
    gpu: Option<GpuInfo>,
    /// `--sd-server`: use this exe as is; nothing is installed or pruned.
    override_exe: Option<PathBuf>,
    http: reqwest::Client,
    install: tokio::sync::Mutex<()>,
    devices: Mutex<Option<Vec<Device>>>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

/// Lowercase letters and digits only: "Intel(R) Arc(TM) 140T GPU" and "Intel Arc 140T GPU (16GB)" compare.
fn squash(s: &str) -> String {
    s.to_lowercase().replace("(r)", "").replace("(tm)", "").chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// `sd-cli --list-devices` prints `name<TAB>description` lines on stdout (logs go to stderr).
pub fn parse_devices(stdout: &str) -> Vec<Device> {
    stdout
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(n, d)| Device { name: n.trim().to_string(), description: d.trim().to_string() })
        .filter(|d| !d.name.is_empty())
        .collect()
}

/// `--backend <device>` when the runtime sees more than one GPU (a hybrid laptop on Vulkan sees
/// the iGPU too, and sd.cpp's auto-fit may otherwise spread weights onto it). One GPU: nothing.
pub fn pin_args(devices: &[Device], gpu: &GpuInfo) -> Vec<String> {
    let gpus: Vec<&Device> = devices.iter().filter(|d| d.name != "CPU" && !d.name.starts_with("RPC")).collect();
    if gpus.len() < 2 {
        return vec![];
    }
    let want = squash(&gpu.name);
    match gpus.iter().find(|d| squash(&d.description).contains(&want) || want.contains(&squash(&d.description))) {
        Some(d) => vec!["--backend".into(), d.name.clone()],
        None => vec![],
    }
}

impl Runtimes {
    pub fn new(data_dir: &Path, all: Vec<RuntimeManifest>, hw: &Hardware, force_gpu: Option<&str>, force_build: Option<&str>, override_exe: Option<PathBuf>) -> Self {
        let choice = pick(hw, &all, force_gpu, force_build);
        let gpu = choice.gpu_index.map(|i| hw.gpus[i].clone());
        Self {
            dir: data_dir.join("runtimes"),
            all,
            choice,
            gpu,
            override_exe,
            http: crate::download::http_client(),
            install: Default::default(),
            devices: Mutex::new(None),
        }
    }

    /// A fixed sd-server exe (`--sd-server`, and tests): no detection, nothing to install.
    pub fn fixed(data_dir: &Path, exe: PathBuf) -> Self {
        let choice = Choice { gpu_index: None, runtime: None, build: None, experimental: false, status: Status::Ok, reason: format!("Using {} (--sd-server).", exe.display()), message: None, link: None, forced: true };
        Self { dir: data_dir.join("runtimes"), all: vec![], choice, gpu: None, override_exe: Some(exe), http: crate::download::http_client(), install: Default::default(), devices: Mutex::new(None) }
    }

    /// Refuses jobs when this machine can't run them yet, with what to do (`no_gpu`, `driver_too_old`, …).
    pub fn usable(&self) -> Result<(), BackendError> {
        let code = match self.choice.status {
            Status::Ok => return Ok(()),
            Status::DriverTooOld => "driver_too_old",
            Status::NoDriver => "no_driver",
            Status::NoGpu => "no_gpu",
            Status::UnsupportedOs => "unsupported_os",
            Status::GlibcTooOld => "glibc_too_old",
        };
        let mut msg = self.choice.message.clone().unwrap_or_else(|| self.choice.reason.clone());
        if let Some(l) = &self.choice.link {
            msg = format!("{msg} {l}");
        }
        Err(err(code, msg))
    }

    pub fn active(&self) -> Option<&RuntimeManifest> {
        let id = self.choice.runtime.as_deref()?;
        self.all.iter().find(|m| m.id == id)
    }

    /// True while a runtime is downloading or unpacking.
    pub fn is_installing(&self) -> bool {
        self.install.try_lock().is_err()
    }

    pub fn is_override(&self) -> bool {
        self.override_exe.is_some()
    }

    pub fn dir_of(&self, m: &RuntimeManifest) -> PathBuf {
        self.dir.join(&m.id)
    }

    pub fn download_dir(&self) -> PathBuf {
        self.downloads()
    }

    fn downloads(&self) -> PathBuf {
        self.dir.join(".downloads")
    }

    fn marker(&self, m: &RuntimeManifest) -> Option<Marker> {
        let text = std::fs::read_to_string(self.dir_of(m).join(MARKER)).ok()?;
        serde_json::from_str::<Marker>(&text).ok().filter(|k| k.id == m.id && self.dir_of(m).join(&m.server).is_file())
    }

    pub fn is_installed(&self, m: &RuntimeManifest) -> bool {
        self.marker(m).is_some()
    }

    /// True when the runtime Dawnroom will use is ready (or `--sd-server` points at an exe).
    pub fn ready(&self) -> bool {
        match (&self.override_exe, self.active()) {
            (Some(p), _) => p.is_file(),
            (None, Some(m)) => self.is_installed(m),
            _ => false,
        }
    }

    /// Bytes on disk an install still needs at its peak: zips not yet downloaded + everything unpacked.
    pub fn needed_bytes(&self, m: &RuntimeManifest) -> u64 {
        if self.is_installed(m) {
            return 0;
        }
        let have = |f: &RuntimeFile| {
            let done = self.downloads().join(&f.name);
            let part = self.downloads().join(format!("{}.part", f.name));
            // A `.part` counts for what a resume keeps (download::trusted_bytes), not its raw length.
            std::fs::metadata(&done).map(|m| m.len()).unwrap_or_else(|_| crate::download::trusted_bytes(&part, &f.url, f.size, &f.sha256)).min(f.size)
        };
        m.files.iter().map(|f| f.size - have(f) + f.unpacked_size).sum()
    }

    /// What the active runtime still needs (0 when installed or overridden).
    pub fn active_needed_bytes(&self) -> u64 {
        if self.override_exe.is_some() {
            return 0;
        }
        self.active().map(|m| self.needed_bytes(m)).unwrap_or(0)
    }

    /// `.part` downloads of runtime zips, as (name, bytes).
    pub fn partial_downloads(&self) -> Vec<(String, u64)> {
        let Ok(rd) = std::fs::read_dir(self.downloads()) else { return vec![] };
        rd.filter_map(|e| e.ok())
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.ends_with(".part").then(|| (n, e.metadata().map(|m| m.len()).unwrap_or(0)))
            })
            .collect()
    }

    /// Refuses, in plain words, when the C library is too old for the Linux runtime build (nothing is downloaded).
    pub fn check_glibc(&self) -> Result<(), BackendError> {
        match self.choice.status {
            Status::GlibcTooOld => Err(err("glibc_too_old", self.choice.message.clone().unwrap_or_default())),
            _ => Ok(()),
        }
    }

    /// Installs the active runtime if needed and says how to start it. `on_step` reports progress.
    /// Safe to call from a pull and a job at once: the second waits for the first.
    pub async fn ensure(&self, cancel: &AtomicBool, on_step: &(dyn Fn(Step) + Send + Sync)) -> Result<Launch, BackendError> {
        if let Some(exe) = &self.override_exe {
            return Ok(Launch { exe: exe.clone(), args: vec![] });
        }
        self.check_glibc()?;
        let m = self.active().ok_or_else(|| {
            err("no_runtime", self.choice.message.clone().unwrap_or_else(|| format!("{} There's no runtime for this machine.", self.choice.reason)))
        })?;
        {
            let _guard = self.install.lock().await;
            if !self.is_installed(m) {
                crate::disk::check(&self.dir, self.needed_bytes(m), "The image runtime")?;
                self.install(m, cancel, on_step).await?;
            }
            self.touch(m);
        }
        let args = match &self.gpu {
            Some(g) => pin_args(&self.devices().await, g),
            None => vec![],
        };
        Ok(Launch { exe: self.dir_of(m).join(&m.server), args })
    }

    async fn install(&self, m: &RuntimeManifest, cancel: &AtomicBool, on_step: &(dyn Fn(Step) + Send + Sync)) -> Result<(), BackendError> {
        let count = m.files.len();
        let dl = self.downloads();
        for (i, f) in m.files.iter().enumerate() {
            let zip = dl.join(&f.name);
            let step = |stage, bytes, speed_bps, wait| on_step(Step { file: format!("runtime/{}", f.name), file_index: i, file_count: count, stage, bytes, total: f.size, speed_bps, wait });
            // A finished zip from an interrupted install (or an import) is re-checked, not re-downloaded.
            if zip.is_file() && std::fs::metadata(&zip).map(|x| x.len()).unwrap_or(0) == f.size {
                step("verifying", 0, 0, None);
                if hash(&zip).await? == f.sha256 {
                    continue;
                }
                let _ = std::fs::remove_file(&zip);
            }
            let part = dl.join(format!("{}.part", f.name));
            let job = Fetch { url: &f.url, part: &part, size: f.size, sha256: &f.sha256, cancel, source: "GitHub", again: "Try again", retry: Retry::default(), owner: None };
            fetch_to_part(&self.http, job, |b, s, w| step(if w.is_some() { "waiting" } else { "downloading" }, b, s, w)).await?;
            step("verifying", f.size, 0, None);
            if hash(&part).await? != f.sha256 {
                let _ = crate::download::remove_part(&part);
                return Err(err("checksum_mismatch", format!("{} downloaded but its sha256 doesn't match the registry, so it was deleted. Try again.", f.name)));
            }
            std::fs::rename(&part, &zip).map_err(|e| err("disk_error", format!("Couldn't move {} into place ({e}).", f.name)))?;
            let _ = std::fs::remove_file(crate::download::sidecar_path(&part));
        }

        // Unpack everything into a temp folder, then rename: a killed unpack never looks installed.
        let tmp = self.dir.join(format!(".unpack-{}", m.id));
        let _ = std::fs::remove_dir_all(&tmp);
        for (i, f) in m.files.iter().enumerate() {
            on_step(Step { file: format!("runtime/{}", f.name), file_index: i, file_count: count, stage: "unpacking", bytes: 0, total: f.unpacked_size, speed_bps: 0, wait: None });
            let (zip, to) = (dl.join(&f.name), tmp.clone());
            tokio::task::spawn_blocking(move || unzip(&zip, &to)).await.map_err(|e| err("disk_error", e.to_string()))?.map_err(|e| err("disk_error", format!("Couldn't unpack {} ({e}). Check free disk space and try again.", f.name)))?;
        }
        let marker = Marker { id: m.id.clone(), sha256: m.files.iter().map(|f| f.sha256.clone()).collect(), installed_ms: now_ms(), last_used_ms: now_ms() };
        std::fs::write(tmp.join(MARKER), serde_json::to_vec_pretty(&marker).unwrap()).map_err(|e| err("disk_error", e.to_string()))?;
        if !tmp.join(&m.server).is_file() {
            return Err(err("runtime_broken", format!("The runtime {} unpacked without {}. Report this; the release may have changed.", m.build, m.server)));
        }
        let dest = self.dir_of(m);
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&tmp, &dest).map_err(|e| err("disk_error", format!("Couldn't move the runtime into place ({e}).")))?;
        for f in &m.files {
            let _ = std::fs::remove_file(dl.join(&f.name));
        }
        *self.devices.lock().unwrap() = None;
        eprintln!("[engine] runtime {} installed in {}", m.id, dest.display());
        self.prune(m);
        Ok(())
    }

    /// Records the runtime as just used (for "keep current + previous").
    fn touch(&self, m: &RuntimeManifest) {
        if let Some(mut k) = self.marker(m) {
            k.last_used_ms = now_ms();
            let _ = std::fs::write(self.dir_of(m).join(MARKER), serde_json::to_vec_pretty(&k).unwrap());
        }
    }

    /// Installed runtimes as (folder name, last used ms), newest first. Includes ones no
    /// manifest names any more (an old pinned release).
    pub fn installed(&self) -> Vec<(String, i64)> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return vec![] };
        let mut v: Vec<(String, i64)> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
            .filter_map(|e| {
                let k: Marker = serde_json::from_str(&std::fs::read_to_string(e.path().join(MARKER)).ok()?).ok()?;
                Some((e.file_name().to_string_lossy().into_owned(), k.last_used_ms))
            })
            .collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        v
    }

    /// Keeps `current` plus the most recently used other runtime; deletes the rest.
    fn prune(&self, current: &RuntimeManifest) {
        let others: Vec<String> = self.installed().into_iter().map(|(id, _)| id).filter(|id| *id != current.id).collect();
        for id in others.iter().skip(1) {
            match std::fs::remove_dir_all(self.dir.join(id)) {
                Ok(()) => eprintln!("[engine] removed old runtime {id}"),
                Err(e) => eprintln!("[engine] couldn't remove old runtime {id}: {e}"),
            }
        }
    }

    /// Imports runtime zips already on disk (e.g. a downloads folder): each file is hashed and matched
    /// to a manifest file by size and sha256, whatever its name, then copied in. Any runtime whose
    /// zips are now all present is installed. Returns (file, matched name or None).
    pub async fn import(&self, paths: &[PathBuf]) -> Result<Vec<(String, Option<String>)>, BackendError> {
        let _guard = self.install.lock().await;
        let dl = self.downloads();
        std::fs::create_dir_all(&dl).map_err(|e| err("disk_error", e.to_string()))?;
        let mut out = Vec::new();
        for p in paths {
            let size = std::fs::metadata(p).map_err(|e| err("not_found", format!("Couldn't read {} ({e}).", p.display())))?.len();
            let candidates: Vec<&RuntimeFile> = self.all.iter().flat_map(|m| &m.files).filter(|f| f.size == size).collect();
            if candidates.is_empty() {
                out.push((p.display().to_string(), None));
                continue;
            }
            let h = hash(p).await?;
            match candidates.into_iter().find(|f| f.sha256 == h) {
                Some(f) => {
                    crate::disk::check(&dl, size, "Importing the runtime")?;
                    std::fs::copy(p, dl.join(&f.name)).map_err(|e| err("disk_error", format!("Couldn't copy {} ({e}).", p.display())))?;
                    out.push((p.display().to_string(), Some(f.name.clone())));
                }
                None => out.push((p.display().to_string(), None)),
            }
        }
        drop(_guard);
        let never = AtomicBool::new(false);
        for m in &self.all {
            if !self.is_installed(m) && m.files.iter().all(|f| dl.join(&f.name).is_file()) {
                let _g = self.install.lock().await;
                self.install(m, &never, &|_| {}).await?;
            }
        }
        Ok(out)
    }

    /// The devices the installed active runtime can use (cached; empty when not installed).
    pub async fn devices(&self) -> Vec<Device> {
        if let Some(d) = self.devices.lock().unwrap().clone() {
            return d;
        }
        let Some(m) = self.active().filter(|m| self.is_installed(m)) else { return vec![] };
        let mut cmd = tokio::process::Command::new(self.dir_of(m).join(&m.cli));
        cmd.arg("--list-devices").stdin(std::process::Stdio::null()).current_dir(self.dir_of(m));
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000 /* CREATE_NO_WINDOW */);
        let list = match tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output()).await {
            Ok(Ok(o)) => parse_devices(&String::from_utf8_lossy(&o.stdout)),
            _ => vec![],
        };
        *self.devices.lock().unwrap() = Some(list.clone());
        list
    }

    /// Does the runtime list the chosen GPU? `None` until it's installed. `false` on Vulkan usually
    /// means the graphics driver is too old or lacks Vulkan.
    pub async fn sees_gpu(&self) -> Option<bool> {
        let g = self.gpu.as_ref()?;
        let d = self.devices().await;
        if d.is_empty() {
            return None;
        }
        let want = squash(&g.name);
        Some(d.iter().any(|x| x.name != "CPU" && (squash(&x.description).contains(&want) || want.contains(&squash(&x.description)))))
    }
}

async fn hash(p: &Path) -> Result<String, BackendError> {
    let p2 = p.to_path_buf();
    tokio::task::spawn_blocking(move || sha256_file(&p2, |_| {}))
        .await
        .map_err(|e| err("verify_failed", e.to_string()))?
        .map_err(|e| err("verify_failed", format!("Couldn't read {} ({e}).", p.display())))
}

/// Unpacks `zip` into `to`, refusing entries that would land outside it.
fn unzip(zip: &Path, to: &Path) -> std::io::Result<()> {
    let mut a = zip::ZipArchive::new(std::fs::File::open(zip)?).map_err(std::io::Error::other)?;
    for i in 0..a.len() {
        let mut e = a.by_index(i).map_err(std::io::Error::other)?;
        let Some(rel) = e.enclosed_name() else {
            return Err(std::io::Error::other(format!("unsafe path in zip: {:?}", e.name())));
        };
        let out = to.join(rel);
        if e.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut f = std::fs::File::create(&out)?;
        std::io::copy(&mut e, &mut f)?;
        #[cfg(unix)]
        if let Some(mode) = e.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

/// "an AMD", "an Intel", "a Qualcomm": the article for a vendor name.
fn article(word: &str) -> &'static str {
    if word.starts_with(['A', 'E', 'I', 'O', 'U']) { "an" } else { "a" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{Hardware, Ram};

    fn repo_runtimes() -> Vec<RuntimeManifest> {
        load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("registry").join("runtimes")).unwrap()
    }

    fn gpu(name: &str, vendor: Vendor, integrated: bool, vram: u64, driver: Option<&str>) -> GpuInfo {
        GpuInfo { name: name.into(), vendor, integrated, vram_mib: vram, shared_mib: if integrated { 16000 } else { 0 }, driver: driver.map(String::from), compute_cap: None, source: "test", memory_estimated: false, luid: None }
    }

    fn hw(os: &'static str, gpus: Vec<GpuInfo>) -> Hardware {
        Hardware { os, os_version: String::new(), cpu: None, glibc: None, gpus, ram: Ram { total_bytes: 32 << 30, available_bytes: 16 << 30 }, simulated: None }
    }

    fn laptop() -> Vec<GpuInfo> {
        vec![gpu("Intel(R) Arc(TM) 140T GPU", Vendor::Intel, true, 128, Some("32.0.101.6881")), gpu("NVIDIA GeForce RTX 5060 Laptop GPU", Vendor::Nvidia, false, 8151, Some("596.36"))]
    }

    #[test]
    fn manifests_load_and_cover_v01_platforms() {
        let all = repo_runtimes();
        let builds: Vec<_> = all.iter().map(|m| m.build.as_str()).collect();
        assert_eq!(builds, ["linux-vulkan", "windows-cuda12", "windows-vulkan"]);
        assert!(all.iter().all(|m| m.runtime == "sd-cpp@master-1086-465adf3"));
        let cuda = all.iter().find(|m| m.build == "windows-cuda12").unwrap();
        assert_eq!(cuda.files.len(), 2);
        assert_eq!(cuda.min_driver.as_deref(), Some("572.61"));
    }

    #[test]
    fn an_old_glibc_refuses_the_linux_runtime_in_plain_words() {
        let mut h = hw("linux", vec![laptop()[1].clone()]);
        h.glibc = Some("2.35".into());
        let c = pick(&h, &repo_runtimes(), None, None);
        assert_eq!(c.status, Status::GlibcTooOld);
        assert_eq!(c.message.as_deref(), Some("This Linux runtime needs glibc 2.43 or newer (Ubuntu 26.04, current Fedora or Arch); you have 2.35."));
        h.glibc = Some("2.43".into());
        assert_eq!(pick(&h, &repo_runtimes(), None, None).status, Status::Ok);
        h.glibc = Some("2.9".into());
        assert_eq!(pick(&h, &repo_runtimes(), None, None).status, Status::GlibcTooOld);
    }

    #[test]
    fn hybrid_laptop_picks_the_nvidia_card_and_cuda() {
        let c = pick(&hw("windows", laptop()), &repo_runtimes(), None, None);
        assert_eq!(c.gpu_index, Some(1));
        assert_eq!(c.build.as_deref(), Some("windows-cuda12"));
        assert_eq!((c.status, c.experimental), (Status::Ok, false));
    }

    #[test]
    fn old_or_missing_nvidia_driver_says_what_to_update() {
        let mut g = laptop();
        g[1].driver = Some("531.18".into());
        let c = pick(&hw("windows", g.clone()), &repo_runtimes(), None, None);
        assert_eq!(c.status, Status::DriverTooOld);
        assert!(c.message.as_deref().unwrap().contains("531.18") && c.message.as_deref().unwrap().contains("572.61"));
        assert_eq!(c.link.as_deref(), Some(NVIDIA_DRIVERS));
        g[1].driver = None;
        assert_eq!(pick(&hw("windows", g), &repo_runtimes(), None, None).status, Status::NoDriver);
        assert!(version_lt("572.6", "572.61") && !version_lt("596.36", "572.61") && !version_lt("572.61", "572.61"));
    }

    #[test]
    fn amd_intel_and_igpu_only_use_vulkan() {
        let amd = pick(&hw("windows", vec![gpu("AMD Radeon RX 9060 XT", Vendor::Amd, false, 16304, None)]), &repo_runtimes(), None, None);
        assert_eq!((amd.build.as_deref(), amd.experimental, amd.status), (Some("windows-vulkan"), true, Status::Ok));
        let igpu = pick(&hw("windows", vec![laptop()[0].clone()]), &repo_runtimes(), None, None);
        assert_eq!(igpu.build.as_deref(), Some("windows-vulkan"));
        assert!(igpu.reason.contains("integrated"));
        let linux_nv = pick(&hw("linux", vec![laptop()[1].clone()]), &repo_runtimes(), None, None);
        assert_eq!(linux_nv.build.as_deref(), Some("linux-vulkan"));
    }

    #[test]
    fn no_gpu_unsupported_os_and_forcing() {
        let none = pick(&hw("windows", vec![]), &repo_runtimes(), None, None);
        assert_eq!((none.status, none.runtime.clone()), (Status::NoGpu, None));
        assert!(none.message.unwrap().contains("driver"));
        assert_eq!(pick(&hw("macos", laptop()), &repo_runtimes(), None, None).status, Status::UnsupportedOs);
        let f = pick(&hw("windows", laptop()), &repo_runtimes(), Some("arc"), None);
        assert_eq!((f.gpu_index, f.build.as_deref(), f.forced), (Some(0), Some("windows-vulkan"), true));
        let f = pick(&hw("windows", laptop()), &repo_runtimes(), None, Some("windows-vulkan"));
        assert_eq!((f.gpu_index, f.build.as_deref()), (Some(1), Some("windows-vulkan")));
    }

    #[test]
    fn device_pinning_only_when_the_runtime_sees_several_gpus() {
        let cuda = parse_devices("CUDA0\tNVIDIA GeForce RTX 5060 Laptop GPU\nCPU\tIntel(R) Core(TM) Ultra 7 255H\n");
        assert!(pin_args(&cuda, &laptop()[1]).is_empty());
        let vk = parse_devices("Vulkan0\tIntel(R) Arc(TM) 140T GPU (16GB)\nVulkan1\tNVIDIA GeForce RTX 5060 Laptop GPU\nCPU\tx\n");
        assert_eq!(pin_args(&vk, &laptop()[1]), ["--backend", "Vulkan1"]);
        assert_eq!(pin_args(&vk, &laptop()[0]), ["--backend", "Vulkan0"]);
        assert!(parse_devices("ggml_cuda_init: found 1 CUDA devices\n").is_empty());
    }

    #[tokio::test]
    async fn install_from_imported_zip_then_prune_keeps_two() {
        use std::io::Write;
        let tmp = std::env::temp_dir().join(format!("rt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let mk = |build: &str| {
            let zpath = tmp.join(format!("{build}.zip"));
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zpath).unwrap());
            w.start_file("sd-server.exe", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(build.as_bytes()).unwrap();
            w.finish().unwrap();
            let size = std::fs::metadata(&zpath).unwrap().len();
            let sha = sha256_file(&zpath, |_| {}).unwrap();
            let m = RuntimeManifest { schema: 1, id: format!("rt-{build}"), runtime: "sd-cpp@x".into(), build: build.into(), os: "windows".into(), gpu: "vulkan".into(), experimental: true, server: "sd-server.exe".into(), cli: "sd-cli.exe".into(), min_driver: None, min_driver_source: None, min_glibc: None, files: vec![RuntimeFile { name: format!("{build}-real-name.zip"), url: "https://unused".into(), size, sha256: sha, unpacked_size: 10 }] };
            (zpath, m)
        };
        let (za, a) = mk("windows-vulkan");
        let (zb, b) = mk("windows-b");
        let (zc, c) = mk("windows-c");
        let data = tmp.join("data");
        let h = hw("windows", vec![laptop()[0].clone()]);
        for (z, m) in [(&zb, &b), (&zc, &c), (&za, &a)] {
            let rt = Runtimes::new(&data, vec![a.clone(), b.clone(), c.clone()], &h, None, Some(&m.build), None);
            let r = rt.import(&[z.clone(), tmp.join("nope.bin")]).await;
            assert!(r.is_err(), "a missing file is an error");
            let r = rt.import(std::slice::from_ref(z)).await.unwrap();
            assert_eq!(r[0].1.as_deref(), Some(m.files[0].name.as_str()), "matched by hash, not name");
            assert!(rt.is_installed(m));
            assert!(!data.join("runtimes/.downloads").join(&m.files[0].name).exists(), "zip deleted after unpacking");
            let launch = rt.ensure(&AtomicBool::new(false), &|_| {}).await.unwrap();
            assert_eq!(std::fs::read_to_string(launch.exe).unwrap(), m.build);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Installed b, then c, then a: b is the oldest and goes.
        let rt = Runtimes::new(&data, vec![a.clone(), b.clone(), c.clone()], &h, None, None, None);
        let kept: Vec<String> = rt.installed().into_iter().map(|(id, _)| id).collect();
        assert_eq!(kept, ["rt-windows-vulkan", "rt-windows-c"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
