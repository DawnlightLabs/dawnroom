//! Model manifests (`registry/*.toml`) and presets. A manifest says which files a model needs,
//! where to get them (pinned HF revision + sha256), how to launch sd-server, and what was
//! measured on real hardware. Presets map a task ("fast", "quality") to a model, in one place.
//!
//! Schema 2 (written by engine/scripts/gen_models.py): one manifest per model with size tags,
//! `[[variant]]` blocks (q4, q8, bf16…) that override only the files that differ, usually the
//! diffusion weights, and carry their own measurements. At load every variant becomes one
//! `Manifest` with the id `<model>:<tag>`, so downloads, jobs and history work per variant.
//! A bare model id is resolved by `Models::resolve` (the default tag for this machine).
//! Schema 1 (one flat manifest, no tags) is still read, as a model with a single untagged entry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SCHEMA: u32 = 2;

/// One runnable model: a schema-2 variant, a schema-1 manifest, or a custom model (written by
/// custom.rs to `<data>/custom/*.toml` as this struct, never to `registry/`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    /// `<family>:<tag>`, or the family alone for a schema-1 manifest.
    pub id: String,
    /// The model id without a tag (`z-image-turbo`).
    pub family: String,
    pub tag: Option<String>,
    pub name: String,
    pub tagline: String,
    pub tags: Vec<String>,
    /// The runtime it needs, e.g. `sd-cpp@master-1086-465adf3` (see `registry/runtimes/`).
    pub backend: String,
    /// The official repo (the mirrors actually downloaded are in `files`).
    pub original: Option<String>,
    /// sd-server's built-in recipe its defaults come from.
    pub recipe: Option<String>,
    pub licence: Licence,
    pub files: Vec<ModelFile>,
    pub defaults: Defaults,
    /// What it can do: generate (text to image) and/or edit (an instruction plus input images).
    /// Custom manifests written before 6b-3 have none: they generate.
    #[serde(default = "generate_only")]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub measured: Vec<Measured>,
    /// Set only for custom models (`hf.co/…` pulls and local imports): never in presets or Auto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<Custom>,
}

/// A kind of job. Generate = text to image; edit = an instruction applied to an input image
/// (plus optional reference images), sent to sd-server as `ref_images`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    Generate,
    Edit,
}

impl Task {
    pub const ALL: [Task; 2] = [Task::Generate, Task::Edit];
    pub fn as_str(self) -> &'static str {
        match self {
            Task::Generate => "generate",
            Task::Edit => "edit",
        }
    }
    pub fn parse(s: &str) -> Result<Task, String> {
        match s {
            "generate" => Ok(Task::Generate),
            "edit" => Ok(Task::Edit),
            other => Err(format!("There's no task {other:?}. Use generate or edit.")),
        }
    }
}

fn generate_only() -> Vec<Task> {
    vec![Task::Generate]
}

/// Labelled everywhere a custom model appears.
pub const CUSTOM_LABEL: &str = "Custom, not reviewed by Dawnroom";

/// Where a custom model came from, what the check found, and its test generation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Custom {
    /// `hf.co/<user>/<repo>:<file>`, or the local path it was imported from.
    pub source: String,
    /// The architecture, in words ("Z-Image").
    pub architecture: String,
    /// sd.cpp's name for it (`z_image`).
    pub sd_version: String,
    /// The curated model its companions and defaults come from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub added_ms: i64,
    /// The one small test image that decides "ready". `None` until it has run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<TestResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TestResult {
    pub ok: bool,
    pub at_ms: i64,
    /// Plain words: what happened, and on failure what to do.
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
    /// The test image (kept, but not in the Library).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// The test job's id: its image is served at `/api/images/{job}/file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    pub width: u32,
    pub height: u32,
    pub steps: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Licence {
    /// SPDX id, or `LicenseRef-...` when the licence isn't on the SPDX list.
    pub spdx: String,
    /// The licence's own name, as the model card gives it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub url: String,
    pub commercial: bool,
    /// The licence restricts how outputs may be used (e.g. OpenRAIL's use-based restrictions,
    /// a revenue cap), even where some commercial use is allowed. `note` says what they are.
    #[serde(default)]
    pub use_restrictions: bool,
    /// The original repo is gated: we download from an ungated mirror, so the user accepts the
    /// original licence in Dawnroom before the first pull (`POST /api/models/{id}/licence/accept`).
    #[serde(default)]
    pub accept_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    DiffusionModel,
    UncondDiffusionModel,
    Checkpoint,
    Llm,
    LlmVision,
    ClipL,
    ClipG,
    T5xxl,
    Vae,
    Tokenizer,
}

impl Role {
    /// The sd-server flag that takes this file. The one place roles map to flags.
    pub fn flag(self) -> &'static str {
        match self {
            Role::DiffusionModel => "--diffusion-model",
            Role::UncondDiffusionModel => "--uncond-diffusion-model",
            Role::Checkpoint => "--model",
            Role::Llm => "--llm",
            Role::LlmVision => "--llm_vision",
            Role::ClipL => "--clip_l",
            Role::ClipG => "--clip_g",
            Role::T5xxl => "--t5xxl",
            Role::Vae => "--vae",
            Role::Tokenizer => "--tokenizer",
        }
    }

    /// The weights that make the model what it is (what a size tag changes).
    pub fn is_weights(self) -> bool {
        matches!(self, Role::DiffusionModel | Role::Checkpoint)
    }
}

const CATEGORIES: &[&str] = &["diffusion_models", "text_encoders", "vae", "checkpoints", "tokenizers"];

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelFile {
    pub role: Role,
    /// sd.cpp category folder under the models dir.
    pub category: String,
    /// Local file name inside `category`.
    pub filename: String,
    /// Hugging Face repo id (canonical, not a redirecting alias).
    pub repo: String,
    /// Pinned commit.
    pub revision: String,
    /// Path inside the repo.
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// Where the bytes come from: "official file", "identical to <official repo>/<path> (sha256
    /// checked …)", "quantised by <uploader>" or "repacked by <uploader>" (gen_models.py).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
}

impl ModelFile {
    /// Identity of a file across manifests, and its path relative to the models dir.
    pub fn rel_path(&self) -> String {
        format!("{}/{}", self.category, self.filename)
    }
    /// Download URL under `endpoint` (normally `https://huggingface.co`).
    pub fn url(&self, endpoint: &str) -> String {
        format!("{endpoint}/{}/resolve/{}/{}", self.repo, self.revision, self.path)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    pub steps: u32,
    pub cfg: f32,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampler: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<String>,
    /// Guidance embedding for guidance-distilled models (FLUX dev).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distilled_guidance: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_shift: Option<f32>,
    /// A fixed sigma schedule (Qwen-Image 2.1 Turbo); its length sets the step count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigmas: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
    #[serde(default)]
    pub server_flags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Measured {
    pub gpu: String,
    pub vram_total_mib: u64,
    pub peak_vram_mib: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cold_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The runtime build it ran on (e.g. `windows-vulkan`); missing means `windows-cuda12`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// Measured on integrated graphics: `vram_total_mib` is its shared budget and `peak_vram_mib`
    /// the adapter's dedicated + shared use during the job, minus what was in use before.
    /// Never used for a discrete card's fit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub integrated: bool,
    /// The memory it needs to run with nothing offloaded, when that's more than this GPU had
    /// (measured with offload). The fit formula uses this, else `peak_vram_mib` (see fit.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub need_mib: Option<u64>,
    /// How far free system RAM dropped during the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_ram_mib: Option<u64>,
    /// How sd.cpp placed the weights: `auto-fit` (the engine's choice) or `off`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offload: Option<String>,
    /// Who measured it: missing = us (makes the model "tested"); `community` = a contributed
    /// `dawnroom bench` result (Slice 6d), which makes it "community" tested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// What was measured: missing = generate. An edit measurement makes the model tested for edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<Task>,
    /// The output size it was measured at, `<w>x<h>` (timings only compare at the same size).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
}

impl Measured {
    pub fn task(&self) -> Task {
        self.task.unwrap_or(Task::Generate)
    }
}

/// How much a variant has been tried. Derived from its measurements, never written by hand.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Testing {
    /// We measured it.
    Tested,
    /// Someone else measured it and contributed the result (6d).
    Community,
    Untested,
}

// ---------- On-disk shapes ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    schema: u32,
    id: String,
    name: String,
    tagline: String,
    #[serde(default)]
    tags: Vec<String>,
    backend: String,
    #[serde(default)]
    original: Option<String>,
    #[serde(default)]
    recipe: Option<String>,
    /// Schema 1 only (the old offload-measurement fixture); ignored.
    #[serde(default)]
    #[allow(dead_code)]
    fixture: bool,
    licence: Licence,
    #[serde(default)]
    files: Vec<ModelFile>,
    defaults: Defaults,
    /// Missing = generate only.
    #[serde(default)]
    tasks: Vec<Task>,
    /// Schema 1 only; schema 2 measures per variant.
    #[serde(default)]
    measured: Vec<Measured>,
    #[serde(default, rename = "variant")]
    variants: Vec<RawVariant>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVariant {
    tag: String,
    #[serde(default)]
    files: Vec<ModelFile>,
    #[serde(default)]
    measured: Vec<Measured>,
}

fn valid_name(s: &str, extra: char) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == extra)
}

/// Parses one manifest file into its runnable entries (one per variant).
pub fn parse(text: &str) -> Result<Vec<Manifest>, String> {
    let raw: RawManifest = toml::from_str(text).map_err(|e| e.to_string())?;
    let id = raw.id.clone();
    if !valid_name(&id, '-') {
        return Err(format!("{id:?}: id must be lowercase letters, digits and dashes"));
    }
    let base = |tag: Option<String>, files: Vec<ModelFile>, measured: Vec<Measured>| Manifest {
        id: match &tag {
            Some(t) => format!("{id}:{t}"),
            None => id.clone(),
        },
        family: id.clone(),
        tag,
        name: raw.name.clone(),
        tagline: raw.tagline.clone(),
        tags: raw.tags.clone(),
        backend: raw.backend.clone(),
        original: raw.original.clone(),
        recipe: raw.recipe.clone(),
        licence: raw.licence.clone(),
        files,
        defaults: raw.defaults.clone(),
        tasks: if raw.tasks.is_empty() { generate_only() } else { raw.tasks.clone() },
        measured,
        custom: None,
    };
    let out = match raw.schema {
        1 => {
            if !raw.variants.is_empty() {
                return Err(format!("{id}: schema 1 has no [[variant]] blocks"));
            }
            vec![base(None, raw.files.clone(), raw.measured.clone())]
        }
        2 => {
            if !raw.measured.is_empty() {
                return Err(format!("{id}: schema 2 measures per variant ([[variant.measured]])"));
            }
            if raw.variants.is_empty() {
                return Err(format!("{id}: needs at least one [[variant]]"));
            }
            let mut out = Vec::new();
            for v in &raw.variants {
                if !valid_name(&v.tag, '_') {
                    return Err(format!("{id}: tag {:?} must be lowercase letters, digits and underscores", v.tag));
                }
                if out.iter().any(|m: &Manifest| m.tag.as_deref() == Some(v.tag.as_str())) {
                    return Err(format!("{id}: duplicate tag {}", v.tag));
                }
                // The variant's files replace the shared files of the same role.
                let mut files: Vec<ModelFile> = raw.files.iter().filter(|f| !v.files.iter().any(|g| g.role == f.role)).cloned().collect();
                files.extend(v.files.iter().cloned());
                out.push(base(Some(v.tag.clone()), files, v.measured.clone()));
            }
            out
        }
        n => return Err(format!("{id}: schema {n} isn't supported (expected 1 or {SCHEMA})")),
    };
    for m in &out {
        m.validate()?;
    }
    Ok(out)
}

impl Manifest {
    pub fn is_custom(&self) -> bool {
        self.custom.is_some()
    }

    /// A custom model whose test image passed (curated models are always "tested enough" to use).
    pub fn custom_ready(&self) -> bool {
        self.custom.as_ref().is_none_or(|c| c.test.as_ref().is_some_and(|t| t.ok))
    }

    /// Download size, counting each file once.
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Tested / community / untested, from who measured it (any task).
    pub fn testing(&self) -> Testing {
        Self::testing_of(self.measured.iter())
    }

    /// Tested / community / untested for one task: only that task's measurements count, and a
    /// model that can't do the task is untested for it.
    pub fn testing_for(&self, task: Task) -> Testing {
        if !self.supports(task) {
            return Testing::Untested;
        }
        Self::testing_of(self.measured.iter().filter(|m| m.task() == task))
    }

    fn testing_of<'a>(ms: impl Iterator<Item = &'a Measured>) -> Testing {
        let ms: Vec<_> = ms.collect();
        if ms.iter().any(|m| m.source.is_none()) {
            Testing::Tested
        } else if !ms.is_empty() {
            Testing::Community
        } else {
            Testing::Untested
        }
    }

    pub fn supports(&self, task: Task) -> bool {
        self.tasks.contains(&task)
    }

    /// sd-server arguments: one flag per file, then the model's own flags.
    pub fn server_args(&self, models_dir: &Path) -> Vec<String> {
        let mut args = Vec::new();
        for f in &self.files {
            args.push(f.role.flag().to_string());
            args.push(models_dir.join(&f.category).join(&f.filename).to_string_lossy().into_owned());
        }
        args.extend(self.defaults.server_flags.iter().cloned());
        args
    }

    pub fn validate(&self) -> Result<(), String> {
        let id = &self.id;
        if self.files.is_empty() {
            return Err(format!("{id}: no files"));
        }
        if !self.files.iter().any(|f| f.role.is_weights()) {
            return Err(format!("{id}: needs a diffusion_model or checkpoint file"));
        }
        for (i, f) in self.files.iter().enumerate() {
            let name = &f.filename;
            if self.files[..i].iter().any(|g| g.role == f.role) {
                return Err(format!("{id}: two {:?} files", f.role));
            }
            if !CATEGORIES.contains(&f.category.as_str()) {
                return Err(format!("{id}: {name}: unknown category {:?}", f.category));
            }
            if name.is_empty() || name.contains(['/', '\\', ':']) || name.starts_with('.') {
                return Err(format!("{id}: {name:?}: filename must be a plain file name"));
            }
            if f.path.contains("..") || f.repo.contains("..") || f.repo.split('/').count() != 2 {
                return Err(format!("{id}: {name}: bad repo or path"));
            }
            if f.revision.len() != 40 || !f.revision.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!("{id}: {name}: revision must be a full 40-character commit hash"));
            }
            if f.sha256.len() != 64 || !f.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
                return Err(format!("{id}: {name}: sha256 must be 64 lowercase hex characters"));
            }
            if f.size == 0 {
                return Err(format!("{id}: {name}: size is 0"));
            }
        }
        if let Some(s) = &self.defaults.sigmas
            && s.len() < 2 {
                return Err(format!("{id}: sigmas needs at least two values"));
            }
        if self.tasks.is_empty() || (1..self.tasks.len()).any(|i| self.tasks[..i].contains(&self.tasks[i])) {
            return Err(format!("{id}: tasks must list generate and/or edit, each once"));
        }
        for m in &self.measured {
            if !self.supports(m.task()) {
                return Err(format!("{id}: measured {} but tasks doesn't include it", m.task().as_str()));
            }
            if let Some(sz) = &m.size
                && sz.split_once('x').and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))).is_none() {
                    return Err(format!("{id}: measured size {sz:?} must look like 1024x1024"));
                }
        }
        Ok(())
    }
}

/// All manifests, validated. Sorted by model id, then by size (smallest tag first).
#[derive(Debug, Clone)]
pub struct Registry {
    pub models: Vec<Manifest>,
}

impl Registry {
    pub fn load_dir(dir: &Path) -> Result<Self, String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("Couldn't read the model registry at {} ({e}).", dir.display()))?;
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
        paths.sort();
        let mut models = Vec::new();
        for p in paths {
            let text = std::fs::read_to_string(&p).map_err(|e| format!("Couldn't read {} ({e}).", p.display()))?;
            models.extend(parse(&text).map_err(|e| format!("{}: {e}", p.display()))?);
        }
        let reg = Self::new(models)?;
        for p in PRESETS {
            let Some(m) = reg.get(p.model) else {
                return Err(format!("Preset {} points at {}, which isn't in the registry at {}.", p.id, p.model, dir.display()));
            };
            // Presets are what people get without choosing: tested and commercially clean only.
            if m.testing() != Testing::Tested || !m.licence.commercial || m.licence.accept_required {
                return Err(format!("Preset {} points at {}, which isn't tested and commercially clean.", p.id, p.model));
            }
        }
        Ok(reg)
    }

    pub fn new(mut models: Vec<Manifest>) -> Result<Self, String> {
        models.sort_by(|a, b| a.family.cmp(&b.family).then(a.total_size().cmp(&b.total_size())).then(a.id.cmp(&b.id)));
        let mut by_path: HashMap<String, (&str, &ModelFile)> = HashMap::new();
        for (i, m) in models.iter().enumerate() {
            m.validate()?;
            if models[..i].iter().any(|o| o.id == m.id) {
                return Err(format!("{}: duplicate model id", m.id));
            }
            if models[..i].iter().any(|o| o.family == m.family && o.tag.is_none() != m.tag.is_none()) {
                return Err(format!("{}: defined twice (a schema 1 and a schema 2 manifest?)", m.family));
            }
            for f in &m.files {
                if let Some((other, g)) = by_path.insert(f.rel_path(), (&m.id, f))
                    && (g.sha256 != f.sha256 || g.size != f.size) {
                        return Err(format!("{} and {other} both use {} but with different contents", m.id, f.rel_path()));
                    }
            }
        }
        Ok(Self { models })
    }

    /// This registry plus custom models, each checked on its own: one that's invalid or clashes
    /// with a file another model uses is left out (and named in the returned list) rather than
    /// stopping the engine.
    pub fn with_custom(self, extra: Vec<Manifest>) -> (Self, Vec<String>) {
        let mut reg = self;
        let mut skipped = Vec::new();
        for m in extra {
            if m.custom.is_none() {
                skipped.push(format!("{}: not a custom manifest", m.id));
                continue;
            }
            let mut models = reg.models.clone();
            models.retain(|o| o.id != m.id);
            models.push(m.clone());
            match Self::new(models) {
                Ok(r) => reg = r,
                Err(e) => skipped.push(e),
            }
        }
        (reg, skipped)
    }

    /// Exactly this id (`z-image-turbo:q4`, or a schema-1 id). Bare model ids: `variants`.
    pub fn get(&self, id: &str) -> Option<&Manifest> {
        self.models.iter().find(|m| m.id == id)
    }

    /// Every size tag of a model, smallest first (empty if there's no such model).
    pub fn variants(&self, family: &str) -> Vec<&Manifest> {
        self.models.iter().filter(|m| m.family == family).collect()
    }

    /// The model ids, each once, in registry order.
    pub fn families(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for m in &self.models {
            if out.last() != Some(&m.family.as_str()) {
                out.push(&m.family);
            }
        }
        out
    }

    /// Every distinct file across all manifests (shared files appear once).
    pub fn all_files(&self) -> Vec<&ModelFile> {
        let mut seen = std::collections::HashSet::new();
        self.models.iter().flat_map(|m| &m.files).filter(|f| seen.insert(f.rel_path())).collect()
    }
}

// ---------- Presets ----------

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    /// A pinned variant: the one that was measured.
    pub model: &'static str,
}

/// The one place presets are defined.
pub const PRESETS: &[Preset] = &[
    Preset { id: "fast", label: "Fast", model: "flux2-klein-4b:q8" },
    Preset { id: "quality", label: "Quality", model: "z-image-turbo:q4" },
];
pub const DEFAULT_PRESET: &str = "quality";

/// The preset a variant is behind, if any.
pub fn presets_for(id: &str) -> Vec<&'static str> {
    PRESETS.iter().filter(|p| p.model == id).map(|p| p.id).collect()
}

/// Aspect ratios Studio and MCP offer, as sizes near 1 MP (what the models were measured at),
/// multiples of 16. The one place they are defined.
pub const ASPECTS: &[(&str, u32, u32)] = &[
    ("1:1", 1024, 1024),
    ("4:3", 1152, 864),
    ("3:4", 864, 1152),
    ("3:2", 1248, 832),
    ("2:3", 832, 1248),
    ("16:9", 1360, 768),
    ("9:16", 768, 1360),
];

pub fn aspect_size(aspect: &str) -> Result<(u32, u32), String> {
    ASPECTS
        .iter()
        .find(|(a, ..)| *a == aspect)
        .map(|(_, w, h)| (*w, *h))
        .ok_or_else(|| format!("There's no aspect {aspect:?}. Use one of: {}.", ASPECTS.iter().map(|a| a.0).collect::<Vec<_>>().join(", ")))
}

/// The preset's model id, or a plain message if there's no such preset.
pub fn preset_model(preset: Option<&str>) -> Result<&'static str, String> {
    let pid = preset.unwrap_or(DEFAULT_PRESET);
    PRESETS
        .iter()
        .find(|p| p.id == pid)
        .map(|p| p.model)
        .ok_or_else(|| format!("There's no preset called {pid:?}. Use one of: {}.", PRESETS.iter().map(|p| p.id).collect::<Vec<_>>().join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn repo_registry() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("registry")
    }

    #[test]
    fn every_manifest_in_registry_is_valid() {
        let reg = Registry::load_dir(&repo_registry()).unwrap();
        assert_eq!(reg.families().len(), 34);
        // Edit models edit; FLUX.2/3 klein do both; the rest generate.
        for f in ["qwen-image-edit-2511", "flux1-kontext-dev", "fibo2-edit-turbo"] {
            assert_eq!(reg.variants(f)[0].tasks, [Task::Edit], "{f}");
        }
        assert_eq!(reg.get("flux2-klein-4b:q8").unwrap().tasks, [Task::Generate, Task::Edit]);
        assert_eq!(reg.get("z-image-turbo:q4").unwrap().tasks, [Task::Generate]);
        assert!(reg.get("qwen-image-edit-2511:q4").unwrap().defaults.server_flags.contains(&"qwen_image_zero_cond_t=true".to_string()));
        for m in &reg.models {
            assert!(m.licence.url.starts_with("https://"), "{}", m.id);
            assert!(m.tag.is_some(), "{}: the repo registry is all schema 2", m.id);
            assert!(m.original.is_some() && m.recipe.is_some(), "{}", m.id);
        }
        // Tested = measured by us; nothing hand-typed.
        for id in ["z-image-turbo:q4", "flux2-klein-4b:q8", "realvisxl-v5:fp16", "qwen-image:q4"] {
            assert_eq!(reg.get(id).unwrap().testing(), Testing::Tested, "{id}");
        }
        assert_eq!(reg.get("z-image-turbo:q8").unwrap().testing(), Testing::Untested);
    }

    #[test]
    fn variants_share_files_and_override_the_weights() {
        let reg = Registry::load_dir(&repo_registry()).unwrap();
        let tags: Vec<_> = reg.variants("z-image-turbo").iter().map(|m| m.tag.clone().unwrap()).collect();
        assert_eq!(tags, ["q4", "q8"], "smallest first");
        let (q4, q8) = (reg.get("z-image-turbo:q4").unwrap(), reg.get("z-image-turbo:q8").unwrap());
        // The installed Slice 1 files keep their names, so they map to q4 without a download.
        let names: Vec<_> = q4.files.iter().map(|f| f.filename.as_str()).collect();
        assert_eq!(names, ["Qwen3-4B-Instruct-2507-Q4_K_M.gguf", "ae.safetensors", "z_image_turbo-Q4_K.gguf"]);
        assert_eq!(q4.files[..2], q8.files[..2]);
        assert_ne!(q4.files[2], q8.files[2]);
        assert!(reg.get("z-image-turbo").is_none(), "bare ids go through Models::resolve");
    }

    #[test]
    fn schema_1_still_reads() {
        let reg = Registry::load_dir(&repo_registry()).unwrap();
        let mut m = reg.get("flux2-klein-4b:q8").unwrap().clone();
        let text = format!(
            "schema = 1\nid = \"old\"\nname = \"Old\"\ntagline = \"t\"\nbackend = \"{}\"\n[licence]\nspdx = \"MIT\"\nurl = \"https://x\"\ncommercial = true\n[defaults]\nsteps = 4\ncfg = 1.0\nwidth = 1024\nheight = 1024\n{}",
            m.backend,
            m.files.iter().map(|f| format!("[[files]]\n{}", toml::to_string(f).unwrap())).collect::<String>()
        );
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!((parsed[0].id.as_str(), parsed[0].tag.as_deref()), ("old", None));
        m.id = "old".into();
        assert_eq!(parsed[0].files, m.files);
    }

    #[test]
    fn presets_are_pinned_tested_variants() {
        let reg = Registry::load_dir(&repo_registry()).unwrap();
        assert_eq!(preset_model(Some("fast")).unwrap(), "flux2-klein-4b:q8");
        assert_eq!(preset_model(None).unwrap(), "z-image-turbo:q4");
        assert!(preset_model(Some("turbo")).is_err());
        for p in PRESETS {
            let m = reg.get(p.model).unwrap();
            assert_eq!(m.testing(), Testing::Tested);
            assert!(m.licence.commercial && !m.licence.accept_required);
        }
    }

    #[test]
    fn bad_manifests_are_rejected() {
        let reg = Registry::load_dir(&repo_registry()).unwrap();
        let good = reg.get("z-image-turbo:q4").unwrap().clone();

        let mut m = good.clone();
        m.files[0].filename = "../evil.gguf".into();
        assert!(m.validate().is_err());

        let mut m = good.clone();
        m.files[0].sha256 = "abc".into();
        assert!(m.validate().is_err());

        // Same local path, different contents in two manifests.
        let mut other = good.clone();
        other.id = "other:q4".into();
        other.family = "other".into();
        other.files[1].sha256 = "0".repeat(64);
        assert!(Registry::new(vec![good.clone(), other]).unwrap_err().contains("different contents"));

        assert!(parse("schema = 2\nid = \"x\"\nsurprise = 1").is_err());
        assert!(parse("schema = 3\nid = \"x\"").is_err());
    }
}
