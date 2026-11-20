//! Custom models, Ollama style: `hf.co/<user>/<repo>[:<file>]` from a public Hugging Face repo,
//! or a local file (`import`). Nothing is marked ready before a compatibility check passes, in
//! this order, stopping at the first failure:
//!
//! 1. source: a public, ungated repo; one weight file picked.
//! 2. format: GGUF or safetensors only (pickle formats can run code: refused).
//! 3. architecture: read from the header alone (HTTP Range or a disk read), matched to a family the
//!    pinned sd.cpp loads and Dawnroom's library carries (arch.rs).
//! 4. companions: text encoders and VAE from that curated family's manifest; files already on disk
//!    are shared, not downloaded again.
//! 5. size: new bytes vs free disk; memory fit as an estimate (files + compute reserve).
//! 6. licence: from the model card, shown as is ("No licence stated" when there's none).
//!
//! then (on the user's yes) the download through the normal downloader and one small real test
//! image (`queue::Engine::test_model`), which decides "ready". Custom manifests live in
//! `<data>/custom/*.toml`, never in `registry/`; they're never used by presets or Auto.
//!
//! Ids: `hf.co/<user>/<repo>:<file>` (the repo id in Hugging Face's own spelling) and
//! `local/<name>` for imports. In URLs they're percent-encoded (`hf.co%2Fuser%2Frepo%3Afile`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{Value, json};

use crate::arch;
use crate::backend::BackendError;
use crate::download::{err, now_ms};
use crate::header::{self, Header, MAX_HEADER_BYTES, Parse};
use crate::models::{FileState, Models};
use crate::registry::{CUSTOM_LABEL, Custom, Defaults, Licence, Manifest, ModelFile, Registry, Role, TestResult};

/// `repo` of an imported file (it has no download URL).
pub const LOCAL_REPO: &str = "local/import";
const LOCAL_REVISION: &str = "0000000000000000000000000000000000000000";

/// Serialises registry swaps (add, test result, forget) so two can't lose each other's change.
static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

const WEIGHT_EXTS: &[&str] = &["gguf", "safetensors", "sft"];
const PICKLE_EXTS: &[&str] = &["ckpt", "bin", "pt", "pth", "pkl", "pickle"];

// ---------- sources ----------

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Hf { repo: String, file: Option<String> },
    Local(PathBuf),
}

/// A Hugging Face model in any form people copy: `user/repo` (the copy button), `hf.co/user/repo`,
/// `huggingface.co/user/repo` (± `https://`, `www.`, `/tree/<rev>`), a file link
/// `…/blob/<rev>/<file>` or `…/resolve/<rev>/<file>` (± `?download=true`), and `…:<file>`.
/// Returns (repo, file). `None` for anything else (a `local/…` id, a path, a curated id).
/// The revision in a file link is ignored: the check pins the repo's current commit.
pub fn parse_hf(s: &str) -> Option<(String, Option<String>)> {
    let s = s.trim();
    let s = s.split(['?', '#']).next().unwrap_or(s);
    let rest = ["https://", "http://"].iter().find_map(|p| s.strip_prefix(p)).unwrap_or(s);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let (rest, bare) = match ["hf.co/", "huggingface.co/"].iter().find_map(|p| rest.strip_prefix(p)) {
        Some(r) => (r, false),
        None => (rest, true),
    };
    // A bare form is only `user/repo[:file]`: never a path, a URL of another site or a local id.
    if bare && (rest.starts_with("local/") || rest.contains("://") || rest.contains('\\') || rest.starts_with('.') || rest.starts_with('/')) {
        return None;
    }
    let (head, mut file) = match rest.split_once(':') {
        Some((r, f)) => (r, Some(f.trim_matches('/').to_string()).filter(|f| !f.is_empty())),
        None => (rest, None),
    };
    let parts: Vec<&str> = head.trim_end_matches('/').split('/').collect();
    if parts.len() < 2 {
        return None;
    }
    match parts.get(2).copied() {
        None => {}
        // /tree/<rev>[/<folder>]: the repo page.
        Some("tree") if !bare && parts.len() >= 3 => {}
        Some("blob" | "resolve") if !bare && file.is_none() && parts.len() >= 5 => file = Some(parts[4..].join("/")),
        _ => return None,
    }
    let ok = |p: &str| !p.is_empty() && p != "." && p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if !ok(parts[0]) || !ok(parts[1]) || file.as_deref().is_some_and(|f| f.contains("..") || f.contains('\\')) {
        return None;
    }
    // `user/model.gguf` typed bare is a relative path, not a repo.
    if bare && WEIGHT_EXTS.iter().chain(PICKLE_EXTS).any(|e| parts[1].to_ascii_lowercase().ends_with(&format!(".{e}"))) {
        return None;
    }
    Some((format!("{}/{}", parts[0], parts[1]), file))
}

/// What a typed or pasted source is: an existing file on this computer first (so a relative
/// `models/x.gguf` stays a path), else a Hugging Face model in any form `parse_hf` takes.
pub fn parse_source(s: &str) -> Result<Source, String> {
    let t = s.trim().trim_matches('"');
    if t.is_empty() {
        return Err("Paste a Hugging Face link or user/repo, or the path of a .gguf or .safetensors file.".into());
    }
    let p = PathBuf::from(t);
    if p.is_file() || p.is_dir() {
        return Ok(Source::Local(p));
    }
    if let Some((repo, file)) = parse_hf(t) {
        return Ok(Source::Hf { repo, file });
    }
    // A path that isn't there: the source step says so ("There's no file at …").
    let low = t.to_ascii_lowercase();
    let looks_local = t.contains('\\') || t.starts_with('/') || t.starts_with('.') || t.get(1..3) == Some(":\\") || t.get(1..3) == Some(":/") || WEIGHT_EXTS.iter().chain(PICKLE_EXTS).any(|e| low.ends_with(&format!(".{e}")));
    if looks_local {
        return Ok(Source::Local(p));
    }
    Err(format!("{t} isn't a Hugging Face model or a file on this computer. Paste the model's page link (huggingface.co/user/repo), user/repo, or a file link."))
}

/// An id or source that names a custom model (`hf.co/…`, `huggingface.co/…`, `local/…`).
pub fn is_custom_id(s: &str) -> bool {
    parse_hf(s).is_some() || s.starts_with("local/")
}

/// The canonical id: `hf.co/<repo>:<file>`.
pub fn hf_id(repo: &str, file: &str) -> String {
    format!("hf.co/{repo}:{file}")
}

// ---------- the report ----------

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    /// source | format | architecture | companions | size | licence
    pub step: &'static str,
    /// ok | failed | skipped
    pub status: &'static str,
    pub message: String,
    /// Set on a failed step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub detail: Value,
}

const STEPS: [&str; 6] = ["source", "format", "architecture", "companions", "size", "licence"];

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub ok: bool,
    pub source: String,
    /// The id the model gets (known once the source step passed).
    pub id: Option<String>,
    pub name: Option<String>,
    pub label: &'static str,
    pub steps: Vec<Step>,
    /// The failing step's code and message (None when ok).
    pub code: Option<&'static str>,
    pub message: Option<String>,
    /// New bytes a pull would put on disk (model, missing companions, runtime; for an import, the copy).
    pub download_bytes: u64,
    /// All the model's files.
    pub size_bytes: u64,
    pub fit: Option<crate::fit::Fit>,
    pub licence: Option<Licence>,
    /// The manifest it would get (not sent: clients read the fields above).
    #[serde(skip)]
    pub plan: Option<Manifest>,
    /// The local file, for an import.
    #[serde(skip)]
    pub local: Option<PathBuf>,
    /// The repo as Hugging Face describes it (name, author, base model, licence, downloads).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoMeta>,
    /// The file this report checked (a repo path, or a local file's name).
    pub file: Option<String>,
    /// Every model file of a repo with more than one (clients group them by `bits`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileOption>,
    /// Read from the header ("Qwen-Image", "SDXL"…), once that step passed.
    pub architecture: Option<String>,
    /// The text encoders and VAE it takes from the library: file, role, size, on_disk.
    pub companions: Vec<Value>,
    /// The manifest it would get, kept even when the size step fails (sizes the other files).
    #[serde(skip)]
    pub sizing: Option<Manifest>,
}

impl Report {
    fn new(source: &str) -> Self {
        Self { ok: false, source: source.into(), id: None, name: None, label: CUSTOM_LABEL, steps: vec![], code: None, message: None, download_bytes: 0, size_bytes: 0, fit: None, licence: None, plan: None, local: None, repo: None, file: None, files: vec![], architecture: None, companions: vec![], sizing: None }
    }
    fn pass(&mut self, step: &'static str, message: impl Into<String>, detail: Value) {
        self.steps.push(Step { step, status: "ok", message: message.into(), code: None, detail });
    }
    /// Records the failure and marks the steps after it skipped.
    fn fail(mut self, step: &'static str, code: &'static str, message: impl Into<String>, detail: Value) -> Self {
        let message = message.into();
        self.steps.push(Step { step, status: "failed", message: message.clone(), code: Some(code), detail });
        for s in STEPS.iter().skip_while(|s| **s != step).skip(1) {
            self.steps.push(Step { step: s, status: "skipped", message: "Not checked: an earlier step failed.".into(), code: None, detail: Value::Null });
        }
        self.code = Some(code);
        self.message = Some(message);
        self
    }
    /// A refusal after the six steps passed (a duplicate found by hashing, a disk error).
    fn refused(mut self, code: &'static str, message: impl Into<String>) -> Self {
        self.ok = false;
        self.code = Some(code);
        self.message = Some(message.into());
        self
    }

    /// The failure as an engine error (for a pull or import that was refused).
    pub fn error(&self) -> BackendError {
        err(self.code.unwrap_or("check_failed"), self.message.clone().unwrap_or_else(|| "The compatibility check failed.".into()))
    }
}

// ---------- reading headers ----------

/// Reads `[start, end)` of a URL (HTTP Range; a server that ignores Range is read only that far).
async fn http_range(http: &reqwest::Client, url: &str, start: u64, end: u64) -> Result<Vec<u8>, String> {
    let mut resp = http
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={start}-{}", end - 1))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach Hugging Face ({e}). Check the connection and try again."))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Hugging Face answered HTTP {} for the file. Try again later.", status.as_u16()));
    }
    if resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("text/html")) {
        return Err("Something between you and Hugging Face answered with a web page instead of the file (a captive portal, proxy or firewall). Sign in to the network or check the proxy, then try again.".into());
    }
    // 206: the body starts at `start`. 200: the whole file from byte 0, so skip to `start`.
    let skip = if status.as_u16() == 206 { 0 } else { start };
    let want = (end - start) as usize;
    let mut out = Vec::with_capacity(want);
    let mut seen = 0u64;
    while out.len() < want {
        let Some(chunk) = resp.chunk().await.map_err(|e| format!("The connection broke while reading the file's header ({e}). Try again."))? else { break };
        let from = skip.saturating_sub(seen).min(chunk.len() as u64) as usize;
        seen += chunk.len() as u64;
        out.extend_from_slice(&chunk[from..]);
    }
    out.truncate(want);
    Ok(out) // dropping `resp` closes the connection: nothing past the header is downloaded
}

fn file_range(path: &Path, start: u64, end: u64) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek};
    let mut f = std::fs::File::open(path).map_err(|e| format!("Couldn't open {} ({e}).", path.display()))?;
    f.seek(std::io::SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    f.take(end - start).read_to_end(&mut buf).map_err(|e| format!("Couldn't read {} ({e}).", path.display()))?;
    Ok(buf)
}

enum Reader<'a> {
    Http(&'a reqwest::Client, String),
    File(PathBuf),
}

/// Reads as much of the start of the file as its header needs (256 KB first, more on demand).
async fn read_header(r: &Reader<'_>, size: u64) -> Result<Header, (&'static str, String)> {
    let mut buf: Vec<u8> = Vec::new();
    let mut want = (256u64 << 10).min(size);
    loop {
        if (buf.len() as u64) < want {
            let (a, b) = (buf.len() as u64, want);
            let more = match r {
                Reader::Http(http, url) => http_range(http, url, a, b).await,
                Reader::File(p) => file_range(p, a, b),
            }
            .map_err(|e| ("network", e))?;
            if more.is_empty() {
                return Err(("bad_header", "The file ended inside its header: it's truncated or not a model file.".into()));
            }
            buf.extend(more);
        }
        match header::parse(&buf) {
            Ok(h) => return Ok(h),
            Err(Parse::NeedBytes(n)) if n > size => return Err(("bad_header", "The file ended inside its header: it's truncated or not a model file.".into())),
            Err(Parse::NeedBytes(n)) if n > MAX_HEADER_BYTES => return Err(("bad_header", "The file's header is larger than any real model's, so Dawnroom stopped reading.".into())),
            Err(Parse::NeedBytes(n)) => want = n.max(buf.len() as u64 + (256 << 10)).min(size).min(MAX_HEADER_BYTES),
            Err(Parse::Bad(m)) => return Err(("bad_header", m)),
        }
    }
}

// ---------- the check ----------

/// One weight file of a repo, from the Hugging Face API.
#[derive(Debug, Clone)]
struct RepoFile {
    path: String,
    size: u64,
    sha256: Option<String>,
}

struct RepoInfo {
    id: String,
    sha: String,
    files: Vec<RepoFile>,
    card: Value,
    meta: RepoMeta,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoMeta {
    pub id: String,
    pub author: Option<String>,
    /// The card's `base_model` (the first, when it lists several).
    pub base_model: Option<String>,
    /// The card's licence id as written ("apache-2.0", "other").
    pub licence: Option<String>,
    pub downloads: Option<u64>,
    pub likes: Option<u64>,
    pub url: String,
}

/// One model file of a repo, as the size picker shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FileOption {
    pub file: String,
    /// "Q4_K_M", "BF16", "INT8 ConvRot", "FP8 comfy"; else the part of the name the files don't share.
    pub quant: String,
    /// 1–32; `None` when the name doesn't say.
    pub bits: Option<u8>,
    /// gguf | safetensors
    pub format: &'static str,
    pub size: u64,
    /// The pinned sd.cpp loads this format (judged from the name; the header check has the last word).
    pub loadable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Estimated fit here (files + compute reserve), for files that can load.
    pub fit: Option<crate::fit::Tier>,
    pub recommended: bool,
    /// The file this report checked.
    pub selected: bool,
}

impl FileOption {
    pub fn format_label(&self) -> &'static str {
        if self.format == "gguf" { "GGUF" } else { "safetensors" }
    }
}

/// GGUF and float quant names, longest first so `Q4_K_M` wins over `Q4_K`.
const QUANTS: &[(&str, u8)] = &[
    ("IQ2_XXS", 2), ("IQ3_XXS", 3), ("IQ2_XS", 2), ("IQ3_XS", 3), ("IQ4_XS", 4), ("IQ4_NL", 4), ("Q2_K_S", 2), ("Q3_K_S", 3), ("Q3_K_M", 3), ("Q3_K_L", 3),
    ("Q4_K_S", 4), ("Q4_K_M", 4), ("Q5_K_S", 5), ("Q5_K_M", 5), ("IQ1_S", 1), ("IQ1_M", 1), ("IQ2_S", 2), ("IQ2_M", 2), ("IQ3_S", 3), ("IQ3_M", 3),
    ("TQ1_0", 2), ("TQ2_0", 2), ("Q2_K", 2), ("Q3_K", 3), ("Q4_K", 4), ("Q5_K", 5), ("Q6_K", 6), ("Q4_0", 4), ("Q4_1", 4), ("Q5_0", 5), ("Q5_1", 5),
    ("Q8_0", 8), ("MXFP4", 4), ("NVFP4", 4), ("MXFP8", 8), ("BF16", 16), ("FP16", 16), ("F16", 16), ("FP32", 32), ("F32", 32), ("FP8", 8), ("INT8", 8),
    ("NF4", 4), ("INT4", 4),
];
/// Words after the quant that say how it was made.
const FLAVOURS: &[(&str, &str)] = &[("CONVROT", "ConvRot"), ("COMFY", "comfy"), ("SCALED", "scaled"), ("BNB", "bnb"), ("TORCHAO", "torchao"), ("E4M3FN", "e4m3fn"), ("E5M2", "e5m2")];

fn words(up: &str) -> impl Iterator<Item = &str> {
    up.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty())
}

/// (short quant name, bits) from a file path; `None` when no known quant is in it.
pub fn quant_of(path: &str) -> Option<(String, u8)> {
    let up = path.to_ascii_uppercase();
    let b = up.as_bytes();
    // A quant is delimited by anything but a letter or digit (`_` inside `Q4_K_M` is part of it).
    let at_word = |i: usize, len: usize| (i == 0 || !b[i - 1].is_ascii_alphanumeric()) && (i + len == b.len() || !b[i + len].is_ascii_alphanumeric());
    let (q, bits, at) = QUANTS.iter().find_map(|(q, bits)| up.match_indices(q).map(|(i, _)| i).find(|&i| at_word(i, q.len())).map(|i| (*q, *bits, i)))?;
    let tail = &up[at + q.len()..];
    let flavours: Vec<&str> = FLAVOURS.iter().filter(|(w, _)| words(tail).any(|t| t == *w)).map(|(_, l)| *l).collect();
    let q = match q {
        "FP16" => "F16",
        "FP32" => "F32",
        q => q,
    };
    Some((std::iter::once(q).chain(flavours).collect::<Vec<_>>().join(" "), bits))
}

/// Why the pinned sd.cpp (465adf3) can't load a file, judged from its name. `None` = it can.
fn unloadable(path: &str) -> Option<&'static str> {
    let up = path.to_ascii_uppercase();
    let has = |w: &str| words(&up).any(|t| t == w);
    if has("BNB") || has("NF4") || has("BITSANDBYTES") {
        Some("bitsandbytes quantisation, which only runs in PyTorch.")
    } else if has("NVFP4") {
        Some("NVFP4, a ComfyUI format sd.cpp doesn't load.")
    } else if has("TORCHAO") {
        Some("torchao quantisation, which only runs in PyTorch.")
    } else if has("MXFP8") || (has("FP8") && (has("COMFY") || has("SCALED")) && !has("CONVROT")) {
        Some("FP8 with ComfyUI's scale format, which this sd.cpp build doesn't apply.")
    } else {
        None
    }
}

/// Text encoders, VAEs, LoRAs and the like that sit beside a model in its repo (by name or folder).
fn is_companion_name(path: &str) -> bool {
    let low = path.to_ascii_lowercase();
    ["mmproj", "vae", "text_encoder", "text-encoder", "clip_l", "clip_g", "t5xxl", "umt5", "lora", "controlnet", "tokenizer", "image_encoder"].iter().any(|w| low.contains(w))
}

/// The repo's model files as size options: quant, bits, loadable (by name), sorted by size.
/// Shards and companions (encoders, VAEs, LoRAs) are left out.
fn file_options(files: &[RepoFile]) -> Vec<FileOption> {
    let mut out: Vec<FileOption> = files
        .iter()
        .filter(|f| WEIGHT_EXTS.contains(&ext(&f.path).as_str()) && !is_shard(&f.path) && f.size >= 1 << 20 && !is_companion_name(&f.path))
        .map(|f| {
            let reason = unloadable(&f.path);
            let (quant, bits) = quant_of(&f.path).map(|(q, b)| (q, Some(b))).unwrap_or((String::new(), None));
            let format = if ext(&f.path) == "gguf" { "gguf" } else { "safetensors" };
            FileOption { file: f.path.clone(), quant, bits, format, size: f.size, loadable: reason.is_none(), reason: reason.map(str::to_string), fit: None, recommended: false, selected: false }
        })
        .collect();
    // No quant in the name: the part of the path the files don't share.
    let paths: Vec<&str> = out.iter().map(|o| o.file.as_str()).collect();
    let prefix = common_prefix(&paths);
    for o in out.iter_mut().filter(|o| o.quant.is_empty()) {
        let rest = &o.file[prefix.min(o.file.len())..];
        let rest = rest.rsplit_once('.').map(|r| r.0).unwrap_or(rest).trim_matches(|c: char| "-_./".contains(c));
        o.quant = if rest.is_empty() { o.file.rsplit('/').next().unwrap_or(&o.file).to_string() } else { rest.to_string() };
    }
    out.sort_by(|a, b| a.size.cmp(&b.size).then(a.file.cmp(&b.file)));
    out
}

/// Bytes every path shares, backed up to a separator.
fn common_prefix(paths: &[&str]) -> usize {
    let [first, rest @ ..] = paths else { return 0 };
    if rest.is_empty() {
        return 0;
    }
    let n = rest.iter().map(|p| first.bytes().zip(p.bytes()).take_while(|(a, b)| a == b).count()).min().unwrap_or(0);
    first[..n].rfind(['-', '_', '.', '/']).map(|i| i + 1).unwrap_or(0)
}

/// The recommendation: the largest file that fits on the GPU, else the smallest that runs with RAM offload.
pub fn recommend(options: &mut [FileOption]) {
    use crate::fit::Tier;
    let best = |tier: Tier, largest: bool| {
        let it = options.iter().enumerate().filter(|(_, o)| o.loadable && o.fit == Some(tier));
        if largest { it.max_by_key(|(_, o)| o.size).map(|(i, _)| i) } else { it.min_by_key(|(_, o)| o.size).map(|(i, _)| i) }
    };
    let pick = best(Tier::Fits, true).or_else(|| best(Tier::Offload, false));
    for (i, o) in options.iter_mut().enumerate() {
        o.recommended = Some(i) == pick;
    }
}

/// The estimated fit of a plan with its weights file swapped for one of another size.
fn fit_with(m: &Models, plan: &Manifest, size: u64) -> crate::fit::Fit {
    let mut p = plan.clone();
    p.files[0].size = size;
    m.fit(&p)
}


fn ext(path: &str) -> String {
    path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// `…-00001-of-00003.safetensors`: one part of a split model.
fn is_shard(path: &str) -> bool {
    let stem = path.rsplit('/').next().unwrap_or(path);
    let stem = stem.rsplit_once('.').map(|s| s.0).unwrap_or(stem);
    let parts: Vec<&str> = stem.rsplitn(3, '-').collect();
    parts.len() == 3 && parts[1] == "of" && parts[0].len() >= 4 && parts[0].chars().all(|c| c.is_ascii_digit()) && parts[2].rsplit('-').next().is_some_and(|p| p.chars().all(|c| c.is_ascii_digit()))
}

async fn repo_info(m: &Models, repo: &str) -> Result<RepoInfo, (&'static str, String)> {
    let url = format!("{}/api/models/{repo}?blobs=true", m.endpoint);
    let resp = m.http.get(&url).send().await.map_err(|e| ("network", format!("Couldn't reach Hugging Face ({e}). Check the connection and try again.")))?;
    match resp.status().as_u16() {
        200 => {}
        401 | 403 | 404 => return Err(("not_found", format!("Hugging Face has no public repo called {repo} (it may be private, renamed or misspelled)."))),
        s => return Err(("network", format!("Hugging Face answered HTTP {s} for {repo}. Try again later."))),
    }
    let v: Value = resp.json().await.map_err(|e| ("network", format!("Hugging Face sent an answer Dawnroom couldn't read ({e}). Try again.")))?;
    if v["gated"].as_bool() != Some(false) && !v["gated"].is_null() {
        return Err(("gated", "This repo needs a Hugging Face login, which Dawnroom doesn't support yet.".into()));
    }
    if v["private"] == true || v["disabled"] == true {
        return Err(("not_found", format!("{repo} isn't public on Hugging Face.")));
    }
    let sha = v["sha"].as_str().filter(|s| s.len() == 40).ok_or(("network", "Hugging Face didn't say which commit the repo is at. Try again.".to_string()))?;
    let files = v["siblings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let path = f["rfilename"].as_str()?.to_string();
            let size = f["lfs"]["size"].as_u64().or(f["size"].as_u64()).unwrap_or(0);
            Some(RepoFile { path, size, sha256: f["lfs"]["sha256"].as_str().map(str::to_string) })
        })
        .collect();
    let id = v["id"].as_str().unwrap_or(repo).to_string();
    let card = &v["cardData"];
    let meta = RepoMeta {
        id: id.clone(),
        author: v["author"].as_str().map(str::to_string).or(id.split('/').next().map(str::to_string)),
        base_model: card["base_model"].as_str().or(card["base_model"][0].as_str()).map(str::to_string),
        licence: card["license"].as_str().or(card["license"][0].as_str()).map(|l| if l == "other" { card["license_name"].as_str().unwrap_or(l) } else { l }).map(str::to_string),
        downloads: v["downloads"].as_u64(),
        likes: v["likes"].as_u64(),
        url: format!("https://huggingface.co/{id}"),
    };
    Ok(RepoInfo { id, sha: sha.into(), files, card: card.clone(), meta })
}

/// The licence the model card states, as Dawnroom labels it. Custom models are never in presets,
/// so `commercial` only says whether the stated licence is a known permissive one.
fn card_licence(card: &Value, repo: &str, sha: &str) -> Licence {
    let page = format!("https://huggingface.co/{repo}");
    let id = card["license"].as_str().or(card["license"][0].as_str()).map(str::to_lowercase);
    let Some(id) = id else {
        return Licence { spdx: "NOASSERTION".into(), name: Some("No licence stated".into()), url: page, commercial: false, use_restrictions: false, accept_required: false, note: Some("The model card doesn't state a licence. Ask the author before using its images commercially.".into()) };
    };
    let link = card["license_link"].as_str().map(|l| if l.starts_with("http") { l.to_string() } else { format!("{page}/blob/{sha}/{}", l.trim_start_matches("./")) });
    let permissive = ["apache-2.0", "mit", "bsd-2-clause", "bsd-3-clause", "cc0-1.0", "cc-by-4.0", "unlicense", "openrail", "creativeml-openrail-m", "openrail++"];
    let restricted = id.contains("openrail");
    let nc = id.contains("-nc") || id.contains("noncommercial") || id.contains("non-commercial");
    let name = if id == "other" { card["license_name"].as_str().map(str::to_string).or(Some("Other (see the model card)".into())) } else { Some(id.clone()) };
    let note = if id == "other" || (!permissive.contains(&id.as_str()) && !nc) {
        Some("Read the licence on the model card before using its images commercially.".into())
    } else if restricted {
        Some("OpenRAIL: commercial use allowed, with use-based restrictions.".into())
    } else {
        None
    };
    Licence { spdx: if id == "other" { "LicenseRef-other".into() } else { id.clone() }, name, url: link.unwrap_or(page), commercial: permissive.contains(&id.as_str()) && !nc, use_restrictions: restricted, accept_required: false, note }
}

/// A file name safe on every OS: letters, digits, `.`, `-`, `_`.
fn safe(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || ".-_".contains(c) { c } else { '-' }).collect()
}

fn human(b: u64) -> String {
    crate::disk::human(b)
}

/// The curated manifest a custom model takes companions from: the variant of `base` whose
/// companions are most on disk already, else its smallest.
fn base_variant(m: &Models, base: &str) -> Option<Manifest> {
    let reg = m.registry();
    let vs = reg.variants(base);
    let on_disk = |v: &&Manifest| v.files.iter().filter(|f| !f.role.is_weights() && m.file_state(f) == FileState::Verified).count();
    let best = vs.iter().max_by_key(|v| (on_disk(v), std::cmp::Reverse(v.total_size())));
    best.map(|v| (*v).clone())
}

/// Runs steps 1–6 for a source (any form `parse_source` takes). Never downloads weights. A repo
/// with several model files gets `files` (quant, bits, size, loadable, fit, recommended); without a
/// file named, the recommended one is checked.
pub async fn check(m: &Models, source: &str) -> Report {
    let r = Report::new(source);
    let (repo, want) = match parse_source(source) {
        Ok(Source::Local(p)) => return check_local(m, r, p).await,
        Ok(Source::Hf { repo, file }) => (repo, file),
        Err(msg) => return r.fail("source", "invalid_source", msg, Value::Null),
    };
    let mut r = r;
    let info = match repo_info(m, &repo).await {
        Ok(i) => i,
        Err((code, msg)) => return r.fail("source", code, msg, Value::Null),
    };
    r.repo = Some(info.meta.clone());
    let mut options = file_options(&info.files);
    let pick = |path: &str| info.files.iter().find(|f| f.path == path).cloned();
    let target = match &want {
        Some(w) => match pick(w) {
            Some(f) => Some(f),
            None => {
                r.files = options;
                return r.fail("source", "file_not_found", format!("{} has no file called {w}.", info.id), Value::Null);
            }
        },
        None => None,
    };
    let loadable: Vec<FileOption> = options.iter().filter(|o| o.loadable).cloned().collect();
    let target_ok = target.as_ref().is_none_or(|t| options.iter().find(|o| o.file == t.path).is_none_or(|o| o.loadable));
    // The file the check reads first: the one named (if it can load), else the smallest that can.
    let probe = match (&target, target_ok) {
        (Some(t), true) => t.clone(),
        _ => match loadable.iter().min_by_key(|o| o.size) {
            Some(o) => pick(&o.file).unwrap(),
            None if options.is_empty() => {
                let pickles: Vec<&RepoFile> = info.files.iter().filter(|f| PICKLE_EXTS.contains(&ext(&f.path).as_str()) && f.size > 10 << 20).collect();
                match target.clone().or(pickles.first().map(|p| (*p).clone())) {
                    Some(p) => p, // the format step says why it's refused
                    None => return r.fail("source", "no_model_file", format!("{} has no model files (.gguf or .safetensors).", info.id), Value::Null),
                }
            }
            None => {
                let n = options.len();
                let msg = if n == 1 {
                    format!("{} is {}: {} Nothing in this repo runs here.", options[0].file, options[0].quant, options[0].reason.clone().unwrap_or_default())
                } else {
                    format!("All {n} model files in {} are in formats Dawnroom can't run yet (bitsandbytes, NVFP4, torchao, ComfyUI FP8). Look for a GGUF version of this model.", info.id)
                };
                r.files = options;
                return r.fail("format", "no_loadable_file", msg, Value::Null);
            }
        },
    };
    let mut rep = check_file(m, r.clone(), &info, probe.clone()).await;
    if options.len() > 1 {
        if let Some(plan) = &rep.sizing {
            for o in options.iter_mut().filter(|o| o.loadable) {
                o.fit = Some(fit_with(m, plan, o.size).tier);
            }
        }
        recommend(&mut options);
        let rec = options.iter().find(|o| o.recommended).map(|o| o.file.clone());
        if want.is_none()
            && let Some(rec) = rec.as_deref().filter(|f| *f != probe.path)
        {
            rep = check_file(m, r.clone(), &info, pick(rec).unwrap()).await;
        }
        if !target_ok {
            let t = target.as_ref().unwrap();
            let o = options.iter().find(|o| o.file == t.path).unwrap();
            let instead = options.iter().find(|o| o.recommended).or(loadable.first()).map(|x| format!(" Pick the {} {} in this repo instead.", x.quant, x.format_label())).unwrap_or_default();
            let mut fr = r.clone();
            fr.id = Some(hf_id(&info.id, &t.path));
            rep = fr.fail("format", "unsupported_quant", format!("This file is {}: {}{instead}", o.quant, o.reason.clone().unwrap_or_default()), Value::Null);
        }
    }
    let checked = target.map(|t| t.path).unwrap_or(rep.file.clone().unwrap_or_default());
    for o in options.iter_mut() {
        o.selected = o.file == checked;
    }
    rep.file = Some(checked);
    if options.len() > 1 {
        rep.files = options;
    }
    rep
}

/// A local file: steps 1–6.
async fn check_local(m: &Models, mut r: Report, path: PathBuf) -> Report {
    let meta = match std::fs::metadata(&path) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return r.fail("source", "not_a_file", format!("{} is a folder. Point at the model file itself (.gguf or .safetensors).", path.display()), Value::Null),
        Err(_) => return r.fail("source", "not_found", format!("There's no file at {}.", path.display()), Value::Null),
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Already imported from this very path: point at it (the import would refuse it by hash anyway).
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if let Some(cur) = m.registry().models.iter().find(|x| x.custom.as_ref().is_some_and(|c| canon(Path::new(&c.source)) == canon(&path))) {
        r.file = Some(name);
        return r.fail("source", "already_added", format!("This file is already added as {}.", cur.name), json!({ "model": cur.id }));
    }
    r.pass("source", format!("{} ({}), on this computer.", path.display(), human(meta.len())), json!({ "path": path, "size": meta.len() }));
    r.file = Some(name.clone());
    let file = RepoFile { path: name, size: meta.len(), sha256: None };
    let source = path.display().to_string();
    finish(m, r, None, file, Reader::File(path.clone()), Some(path), source).await
}

/// One file of a Hugging Face repo: steps 1–6.
async fn check_file(m: &Models, mut r: Report, info: &RepoInfo, file: RepoFile) -> Report {
    let id = hf_id(&info.id, &file.path);
    r.id = Some(id.clone());
    r.file = Some(file.path.clone());
    // The same bytes as a file in the curated library: pull that model instead.
    if let Some(sha) = &file.sha256
        && let Some(cur) = m.registry().models.iter().find(|x| x.custom.is_none() && x.files.iter().any(|f| f.role.is_weights() && f.sha256 == *sha))
    {
        return r.fail("source", "in_library", format!("This is the same file as {} ({}) in Dawnroom's library. Pull that instead: dawnroom pull {}", cur.name, cur.id, cur.id), json!({ "model": cur.id }));
    }
    r.pass("source", format!("{}, public. File {} ({}).", info.id, file.path, human(file.size)), json!({ "repo": info.id, "revision": info.sha, "file": file.path, "size": file.size }));
    let url = format!("{}/{}/resolve/{}/{}", m.endpoint, info.id, info.sha, file.path);
    finish(m, r, Some(info), file, Reader::Http(&m.http, url), None, id).await
}

/// Steps 2–6, after the source step passed.
async fn finish(m: &Models, mut r: Report, info: Option<&RepoInfo>, file: RepoFile, reader: Reader<'_>, local: Option<PathBuf>, source: String) -> Report {
    // 2. Format, by extension; the header read below checks the bytes too.
    let e = ext(&file.path);
    if PICKLE_EXTS.contains(&e.as_str()) {
        return r.fail(
            "format",
            "pickle",
            format!("{} is a .{e} file, which uses Python's pickle format: loading it can run code hidden in it, so Dawnroom doesn't open it. Look for a .safetensors or .gguf version of the model.", file.path),
            Value::Null,
        );
    }
    if !WEIGHT_EXTS.contains(&e.as_str()) {
        return r.fail("format", "unsupported_format", format!("{} isn't a model file Dawnroom reads: it reads .gguf and .safetensors only.", file.path), Value::Null);
    }
    if is_shard(&file.path) {
        return r.fail("format", "split_model", format!("{} is one part of a model split into several files. Dawnroom needs a single-file model: look for a GGUF or a single .safetensors version.", file.path), Value::Null);
    }
    if file.size < 1 << 20 {
        return r.fail("format", "too_small", format!("{} is only {}: too small to be an image model.", file.path, human(file.size)), Value::Null);
    }
    if info.is_some() && file.sha256.is_none() {
        return r.fail("format", "no_checksum", format!("Hugging Face gives no sha256 for {} (it isn't stored with Git LFS), so Dawnroom can't verify the download.", file.path), Value::Null);
    }

    // 3. Architecture, from the header only.
    let h = match read_header(&reader, file.size).await {
        Ok(h) => h,
        Err(("network", msg)) => return r.fail("architecture", "network", msg, Value::Null),
        Err((_, msg)) => {
            let fmt = if e == "gguf" { "GGUF" } else { "safetensors" };
            return r.fail("format", "bad_header", format!("{} doesn't read as a {fmt} file: {msg}", file.path), Value::Null);
        }
    };
    let count = if h.tensors.is_empty() { String::new() } else { format!(" ({} tensors)", h.tensors.len()) };
    r.pass("format", format!("{}: its header reads as one{count}.", h.format.label()), json!({ "format": h.format }));
    let read = format!("read from the first {} of the file", human(h.header_bytes));
    let family = match arch::classify(&h) {
        Ok(f) => f,
        Err(no) => return r.fail("architecture", no.code, no.message, json!({ "what": no.what, "header_bytes": h.header_bytes, "general_architecture": h.meta.get("general.architecture") })),
    };
    let role = arch::weights_role(&h);
    r.pass("architecture", format!("{} (sd.cpp: {}), {read}.", family.label, family.version), json!({ "architecture": family.label, "sd_version": family.version, "base": family.base, "role": role, "header_bytes": h.header_bytes }));

    // 4. Companions, from the curated family.
    let base = family.base.and_then(|b| base_variant(m, b));
    if family.base.is_some() && base.is_none() {
        return r.fail("companions", "no_base", format!("Dawnroom's library is missing {}, which this model takes its text encoder and VAE from.", family.base.unwrap_or("?")), Value::Null);
    }
    let companions: Vec<ModelFile> = match &base {
        Some(b) => b
            .files
            .iter()
            .filter(|f| !f.role.is_weights())
            // SDXL keeps the library's fp16-fixed VAE even when the checkpoint has its own (the stock one overflows in fp16).
            .filter(|f| !arch::carries(&h, f.role) || (f.role == Role::Vae && b.family == "sdxl"))
            .cloned()
            .collect(),
        None => vec![],
    };
    if base.is_none() && !(arch::carries(&h, Role::Vae) && (arch::carries(&h, Role::ClipL) || arch::carries(&h, Role::ClipG))) {
        return r.fail("companions", "no_base", format!("This {} model doesn't carry its own text encoder and VAE, and Dawnroom's library has no {} model to take them from.", family.label, family.label), Value::Null);
    }
    let missing: u64 = companions.iter().filter(|f| m.file_state(f) != FileState::Verified).map(|f| f.size).sum();
    let comp_detail: Vec<Value> = companions.iter().map(|f| json!({ "file": f.rel_path(), "role": f.role, "size": f.size, "on_disk": m.file_state(f) == FileState::Verified })).collect();
    let comp_msg = match (&base, companions.is_empty()) {
        (_, true) => "None needed: the file carries its own text encoder and VAE.".to_string(),
        (Some(b), false) => {
            let names: Vec<String> = companions.iter().map(|f| format!("{} ({})", f.filename, human(f.size))).collect();
            let tail = if missing == 0 { "all already on disk, shared".to_string() } else { format!("{} still to download", human(missing)) };
            format!("From {}: {}; {tail}.", b.name, names.join(", "))
        }
        (None, false) => unreachable!(),
    };
    r.pass("companions", comp_msg, json!({ "files": comp_detail, "missing_bytes": missing }));

    // The manifest it would get.
    let reg = m.registry();
    let backend = base.as_ref().map(|b| b.backend.clone()).or_else(|| reg.models.first().map(|x| x.backend.clone())).unwrap_or_default();
    let defaults = base.as_ref().map(|b| b.defaults.clone()).unwrap_or_else(|| Defaults {
        steps: 20,
        cfg: 7.0,
        width: 768,
        height: 768,
        sampler: None,
        scheduler: None,
        distilled_guidance: None,
        flow_shift: None,
        sigmas: None,
        negative_prompt: None,
        server_flags: vec!["--flash-attn".into(), "on".into(), "--vae-tiling".into()],
    });
    let licence = match &info {
        Some(i) => card_licence(&i.card, &i.id, &i.sha),
        None => match h.meta.get("modelspec.license").or(h.meta.get("license")) {
            Some(l) => Licence { spdx: l.clone(), name: Some(format!("{l} (from the file's metadata)")), url: String::new(), commercial: false, use_restrictions: false, accept_required: false, note: Some("Read the licence before using its images commercially.".into()) },
            None => Licence { spdx: "NOASSERTION".into(), name: Some("No licence stated".into()), url: String::new(), commercial: false, use_restrictions: false, accept_required: false, note: Some("A local file: check where it came from and its licence before using its images commercially.".into()) },
        },
    };
    let (id, name, weights) = match &info {
        Some(i) => {
            let stem = file.path.rsplit('/').next().unwrap_or(&file.path);
            let stem = stem.rsplit_once('.').map(|s| s.0).unwrap_or(stem);
            let repo_name = i.id.split('/').nth(1).unwrap_or(&i.id);
            let wf = ModelFile {
                role,
                category: if role == Role::Checkpoint { "checkpoints".into() } else { "diffusion_models".into() },
                filename: safe(&format!("custom--{}--{}", i.id.replace('/', "--"), file.path.replace('/', "--"))),
                repo: i.id.clone(),
                revision: i.sha.clone(),
                path: file.path.clone(),
                size: file.size,
                sha256: file.sha256.clone().unwrap_or_default(),
                provenance: Some(format!("custom: {CUSTOM_LABEL}")),
            };
            (hf_id(&i.id, &file.path), format!("{repo_name} · {}", quant_of(&file.path).map(|q| q.0).unwrap_or_else(|| stem.to_string())), wf)
        }
        None => {
            let p = local.as_ref().unwrap();
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let slug: String = safe(&stem.to_lowercase()).trim_matches('-').to_string();
            let wf = ModelFile {
                role,
                category: if role == Role::Checkpoint { "checkpoints".into() } else { "diffusion_models".into() },
                filename: safe(&format!("custom--local--{}", file.path)),
                repo: LOCAL_REPO.into(),
                revision: LOCAL_REVISION.into(),
                path: safe(&file.path),
                size: file.size,
                sha256: "0".repeat(64), // filled in by the import, which hashes the file
                provenance: Some(format!("custom: imported from {} ({CUSTOM_LABEL})", p.display())),
            };
            (format!("local/{slug}"), stem, wf)
        }
    };
    r.id = Some(id.clone());
    r.name = Some(name.clone());
    let mut files = vec![weights];
    files.extend(companions);
    let manifest = Manifest {
        family: id.split_once(':').map(|(f, _)| f.to_string()).unwrap_or_else(|| id.clone()),
        id,
        tag: None,
        name,
        tagline: format!("{CUSTOM_LABEL}. {} architecture.", family.label),
        tags: vec!["custom".into()],
        backend,
        original: info.as_ref().map(|i| i.id.clone()),
        recipe: base.as_ref().and_then(|b| b.recipe.clone()),
        licence: licence.clone(),
        files,
        defaults,
        // Custom models generate; editing needs a curated, tested model (6b-3).
        tasks: vec![crate::registry::Task::Generate],
        measured: vec![],
        custom: Some(Custom { source, architecture: family.label.into(), sd_version: family.version.into(), base: base.as_ref().map(|b| b.family.clone()), added_ms: now_ms(), test: None }),
    };

    r.sizing = Some(manifest.clone());
    r.architecture = Some(family.label.to_string());
    r.companions = comp_detail.clone();

    // 5. Size and memory.
    let weights_new = match &local {
        Some(_) => file.size, // the copy (a hard link on the same drive costs nothing; counted anyway)
        None if m.file_state(&manifest.files[0]) == FileState::Verified => 0,
        None => file.size,
    };
    let need = weights_new + missing + m.runtimes.active_needed_bytes();
    r.download_bytes = need;
    r.size_bytes = manifest.total_size();
    let free = crate::disk::space(&m.dir).map(|s| s.free_bytes);
    if let Some(free) = free
        && free < need + crate::disk::RESERVE_BYTES
    {
        return r.fail("size", "not_enough_space", format!("It needs {} on disk and the drive has {} free. Free up space, then check again.", human(need), human(free)), json!({ "needed_bytes": need, "free_bytes": free }));
    }
    let fit = m.fit(&manifest);
    r.fit = Some(fit.clone());
    let gb = |mib: u64| format!("{:.1} GB", mib as f64 / 1024.0);
    let need_s = gb(fit.need_mib.unwrap_or(0));
    let memory = match fit.tier {
        crate::fit::Tier::Fits => format!("Should fit (estimate): about {need_s} of {} VRAM.", gb(fit.gpu_mib.unwrap_or(0))),
        crate::fit::Tier::Offload => format!("Runs with RAM offload (estimate, slower): about {need_s} against {} VRAM + {} spare RAM.", gb(fit.gpu_mib.unwrap_or(0)), gb(fit.usable_ram_mib.unwrap_or(0))),
        crate::fit::Tier::NeedsMoreMemory => {
            let integrated = m.machine.is_some_and(|mc| mc.integrated);
            return r.fail("size", "needs_more_memory", format!("{} (estimate from its file sizes)", crate::fit::needs_more_message(&manifest.name, &fit, integrated)), json!({ "fit": fit }));
        }
        crate::fit::Tier::NoGpu => {
            let why = m.runtimes.usable().err().map(|e| e.message).unwrap_or_else(|| "No usable GPU was found, so the test image can't run.".into());
            return r.fail("size", "no_gpu", why, json!({ "fit": fit }));
        }
    };
    let disk = if local.is_some() {
        format!("Adds {} to the models folder (a hard link on the same drive, else a copy)", human(weights_new))
    } else {
        format!("{} to download", human(need))
    };
    let free_s = free.map(|f| format!(" ({} free)", human(f))).unwrap_or_default();
    r.pass("size", format!("{disk}{free_s}. {memory}"), json!({ "download_bytes": need, "free_bytes": free, "fit": fit }));

    // 6. Licence: shown, never a reason to refuse.
    let lname = licence.name.clone().unwrap_or(licence.spdx.clone());
    let from = if info.is_some() && licence.spdx != "NOASSERTION" { " (from the model card)" } else { "" };
    r.pass("licence", format!("{lname}{from}.{}", licence.note.as_deref().map(|n| format!(" {n}")).unwrap_or_default()), json!({ "licence": licence }));
    r.licence = Some(licence);
    r.plan = Some(manifest);
    r.local = local;
    r.ok = true;
    r
}

// ---------- models already on this computer ----------

/// A folder Dawnroom looks in for models the user already downloaded.
#[derive(Debug, Clone, Serialize)]
pub struct LocalRoot {
    pub path: PathBuf,
    /// "Hugging Face cache", "ComfyUI", "Downloads", "This folder"…
    pub label: String,
    pub exists: bool,
}

/// An image model found on disk, with the check's answer for it.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub path: PathBuf,
    pub file: String,
    /// Where it was found: "ComfyUI", "Hugging Face cache · user/repo"…
    pub place: String,
    pub size: u64,
    pub architecture: Option<String>,
    pub ok: bool,
    /// Why it can't be added (the check's sentence), when it can't.
    pub message: Option<String>,
    pub fit: Option<crate::fit::Tier>,
    /// New bytes adding it would download (missing text encoder / VAE).
    pub download_bytes: u64,
    /// Already added as this custom model.
    pub added: Option<String>,
    /// The same file (name and size) as this model in Dawnroom's library.
    pub library: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalScan {
    pub roots: Vec<LocalRoot>,
    pub found: Vec<Found>,
    /// Model-sized files looked at that aren't image models (LLMs, encoders, VAEs, LoRAs…).
    pub skipped: usize,
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// The usual places other tools keep image models (they're only read; nothing is moved).
pub fn local_roots() -> Vec<LocalRoot> {
    let mut out: Vec<(PathBuf, &str)> = vec![];
    let hf = std::env::var_os("HF_HUB_CACHE").map(PathBuf::from).or_else(|| std::env::var_os("HF_HOME").map(|h| PathBuf::from(h).join("hub")));
    if let Some(h) = home() {
        out.push((hf.unwrap_or_else(|| h.join(".cache").join("huggingface").join("hub")), "Hugging Face cache"));
        for d in [h.join("ComfyUI").join("models"), h.join("Documents").join("ComfyUI").join("models")] {
            out.push((d, "ComfyUI"));
        }
        for d in ["stable-diffusion-webui", "stable-diffusion-webui-forge", "sd-webui-forge"] {
            out.push((h.join(d).join("models").join("Stable-diffusion"), "A1111 / Forge"));
        }
        if cfg!(windows) {
            if let Some(a) = std::env::var_os("APPDATA") {
                out.push((PathBuf::from(a).join("StabilityMatrix").join("Models"), "Stability Matrix"));
            }
        } else {
            out.push((h.join(".local").join("share").join("StabilityMatrix").join("Models"), "Stability Matrix"));
        }
        out.push((h.join("Downloads"), "Downloads"));
    }
    if cfg!(windows) {
        for drive in ["C:", "D:"] {
            out.push((PathBuf::from(format!("{drive}\\ComfyUI\\models")), "ComfyUI"));
        }
    }
    out.into_iter().map(|(path, label)| LocalRoot { exists: path.is_dir(), path, label: label.into() }).collect()
}

/// Model-sized weight files under `dir` (at most `depth` folders down). Skips `skip` (Dawnroom's own
/// models folder) and folders no model lives in.
fn weight_files(dir: &Path, depth: usize, skip: &Path, out: &mut Vec<PathBuf>) {
    const MIN: u64 = 300 << 20; // image models are hundreds of MB and up; smaller files are LoRAs, configs…
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if out.len() >= 400 || p.starts_with(skip) {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_lowercase();
        // metadata() follows symlinks (Hugging Face snapshots link into blobs/).
        let Ok(meta) = std::fs::metadata(&p) else { continue };
        if meta.is_dir() {
            if depth > 0 && !name.starts_with('.') && !["blobs", "node_modules", "loras", "lora", "controlnet", "upscale_models", "embeddings", "custom_nodes", "output", "input"].contains(&name.as_str()) {
                weight_files(&p, depth - 1, skip, out);
            }
        } else if meta.len() >= MIN && WEIGHT_EXTS.contains(&ext(&name).as_str()) && !is_shard(&name) {
            out.push(p);
        }
    }
}

/// "Hugging Face cache · user/repo" from a path inside the cache (`models--user--repo/snapshots/…`).
fn place_of(path: &Path, root: &LocalRoot) -> String {
    let repo = path.components().find_map(|c| c.as_os_str().to_str().and_then(|s| s.strip_prefix("models--")).map(|r| r.replacen("--", "/", 1)));
    // The subfolder it sits in under the root ("checkpoints", "sdxl"), when it isn't at the top.
    let sub = path.parent().and_then(|p| p.strip_prefix(&root.path).ok()).map(|p| p.display().to_string()).filter(|s| !s.is_empty());
    match (repo, sub) {
        (Some(r), _) if root.label == "Hugging Face cache" => format!("{} · {r}", root.label),
        (_, Some(s)) if root.label == "This folder" => s,
        (_, Some(s)) => format!("{} · {s}", root.label),
        _ => root.label.clone(),
    }
}

/// Image models already on this computer: the usual places, or only `dir` when given. Each is run
/// through the same check as an import (header reads only, nothing copied).
pub async fn find_local(m: &Models, dir: Option<&Path>) -> LocalScan {
    let roots = match dir {
        Some(d) => vec![LocalRoot { path: d.to_path_buf(), label: "This folder".into(), exists: d.is_dir() }],
        None => local_roots(),
    };
    let mut files: Vec<(PathBuf, usize)> = vec![];
    for (i, r) in roots.iter().enumerate().filter(|(_, r)| r.exists) {
        let mut fs = vec![];
        // Downloads is a big, mixed folder: only its top two levels.
        weight_files(&r.path, if r.label == "Downloads" { 1 } else { 6 }, &m.dir, &mut fs);
        files.extend(fs.into_iter().map(|f| (f, i)));
    }
    // The same file reached twice (a root inside another, a snapshot link) is listed once.
    let mut seen = std::collections::HashSet::new();
    files.retain(|(f, _)| seen.insert(std::fs::canonicalize(f).unwrap_or_else(|_| f.clone())));
    let reg = m.registry();
    let mut found = vec![];
    let mut skipped = 0;
    for (path, i) in files.into_iter().take(80) {
        let r = check(m, &path.to_string_lossy()).await;
        // Not an image model (LLM, encoder, VAE, LoRA, video…): not listed.
        if r.architecture.is_none() {
            skipped += 1;
            continue;
        }
        let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let size = std::fs::metadata(&path).map(|x| x.len()).unwrap_or(0);
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let added = reg.models.iter().find(|x| x.custom.as_ref().is_some_and(|c| canon(Path::new(&c.source)) == canon(&path))).map(|x| x.id.clone());
        let library = reg.models.iter().find(|x| x.custom.is_none() && x.files.iter().any(|f| f.role.is_weights() && f.size == size && f.path.rsplit('/').next() == Some(file.as_str()))).map(|x| x.id.clone());
        let missing = r.companions.iter().filter(|c| c["on_disk"] != true).map(|c| c["size"].as_u64().unwrap_or(0)).sum();
        found.push(Found { place: place_of(&path, &roots[i]), path, file, size, architecture: r.architecture.clone(), ok: r.ok, message: r.message.clone(), fit: r.fit.as_ref().map(|f| f.tier), download_bytes: missing, added, library });
    }
    // Ready to add first, then by size.
    found.sort_by_key(|f| (f.added.is_some() || f.library.is_some(), !f.ok, std::cmp::Reverse(f.size)));
    LocalScan { roots, found, skipped }
}

// ---------- adding, importing, forgetting ----------

/// The manifest file for an id: a readable slug plus a short hash (two ids never share a file).
fn manifest_path(dir: &Path, id: &str) -> PathBuf {
    let mut slug: String = safe(&id.to_lowercase()).chars().take(80).collect();
    slug = slug.trim_matches('-').to_string();
    dir.join(format!("{slug}-{:08x}.toml", crc32fast::hash(id.as_bytes())))
}

fn write_manifest(dir: &Path, m: &Manifest) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {} ({e}).", dir.display()))?;
    let text = format!("# A custom model ({CUSTOM_LABEL}), written by the Dawnroom engine. Delete it with `dawnroom rm {}`.\n{}", m.id, toml::to_string(m).map_err(|e| e.to_string())?);
    let path = manifest_path(dir, &m.id);
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("Couldn't write {} ({e}).", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("Couldn't write {} ({e}).", path.display()))
}

/// Reads every custom manifest in `dir`; one that can't be read is named and left out.
pub fn load_dir(dir: &Path) -> (Vec<Manifest>, Vec<String>) {
    let (mut out, mut bad) = (Vec::new(), Vec::new());
    let Ok(entries) = std::fs::read_dir(dir) else { return (out, bad) };
    for p in entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "toml")) {
        match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| toml::from_str::<Manifest>(&t).map_err(|e| e.to_string())) {
            Ok(m) => match m.validate() {
                Ok(()) if m.custom.is_some() => out.push(m),
                Ok(()) => bad.push(format!("{}: not a custom manifest", p.display())),
                Err(e) => bad.push(format!("{}: {e}", p.display())),
            },
            Err(e) => bad.push(format!("{}: {e}", p.display())),
        }
    }
    (out, bad)
}

/// The registry plus the custom models in `<data>/custom/` (what serve and the offline CLI use).
pub fn load_registry(registry_dir: &Path, custom_dir: &Path) -> Result<Registry, String> {
    let reg = Registry::load_dir(registry_dir)?;
    let (customs, bad) = load_dir(custom_dir);
    let (reg, skipped) = reg.with_custom(customs);
    for e in bad.iter().chain(&skipped) {
        eprintln!("[engine] custom model left out: {e}");
    }
    Ok(reg)
}

/// Puts `m` into the registry (replacing an entry with its id) and writes its manifest.
fn put(models: &Models, m: Manifest) -> Result<(), String> {
    let _g = REGISTRY_LOCK.lock().unwrap();
    let (reg, skipped) = (*models.registry()).clone().with_custom(vec![m.clone()]);
    if let Some(e) = skipped.first() {
        return Err(e.clone());
    }
    write_manifest(&models.custom_dir(), &m)?;
    models.set_registry(reg);
    Ok(())
}

/// Checks a Hugging Face source and, if it passes, adds it (manifest only: the pull downloads).
/// Already added: returns the existing entry.
pub async fn add(models: &Models, source: &str) -> Result<Manifest, Box<Report>> {
    let r = check(models, source).await;
    let Some(plan) = r.plan.clone().filter(|_| r.ok) else { return Err(Box::new(r)) };
    if let Some(existing) = models.registry().get(&plan.id) {
        return Ok(existing.clone());
    }
    put(models, plan.clone()).map_err(|e| Box::new(r.clone().refused("disk_error", e)))?;
    eprintln!("[engine] custom model added: {}", plan.id);
    Ok(plan)
}

/// Checks a local file and, if it passes, hashes it and puts it in the models folder (a hard link
/// on the same drive, else a copy). The companions and the test come with the pull that follows.
pub async fn import(models: &Models, path: &str) -> Result<Manifest, Box<Report>> {
    let r = check(models, path).await;
    let (Some(mut plan), Some(src)) = (r.plan.clone().filter(|_| r.ok), r.local.clone()) else { return Err(Box::new(r)) };
    let refuse = |code: &'static str, msg: String| Box::new(r.clone().refused(code, msg));
    // A Hugging Face cache snapshot is a link into blobs/: link the real file.
    let src = std::fs::canonicalize(&src).unwrap_or(src);
    let src2 = src.clone();
    let sha = tokio::task::spawn_blocking(move || crate::download::sha256_file(&src2, |_| {}))
        .await
        .map_err(|e| refuse("disk_error", e.to_string()))?
        .map_err(|e| refuse("disk_error", format!("Couldn't read {} ({e}).", src.display())))?;
    let reg = models.registry();
    if let Some(cur) = reg.models.iter().find(|x| x.files.iter().any(|f| f.role.is_weights() && f.sha256 == sha)) {
        let msg = if cur.custom.is_some() {
            format!("This file is already imported as {}.", cur.id)
        } else {
            format!("This is the same file as {} ({}) in Dawnroom's library. Pull that instead: dawnroom pull {}", cur.name, cur.id, cur.id)
        };
        return Err(refuse("in_library", msg));
    }
    // Another import with the same name: number this one.
    let mut id = plan.id.clone();
    let mut n = 2;
    while reg.get(&id).is_some() {
        id = format!("{}-{n}", plan.id);
        n += 1;
    }
    // The file name follows the (possibly numbered) id, so two imports never share a file.
    let ext = ext(&plan.files[0].filename);
    plan.files[0].filename = safe(&format!("custom--{}.{ext}", id.replace('/', "--")));
    plan.family = id.clone();
    plan.id = id;
    plan.files[0].sha256 = sha.clone();
    let dest = models.path_of(&plan.files[0]);
    let (src2, dest2) = (src.clone(), dest.clone());
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        std::fs::create_dir_all(dest2.parent().unwrap()).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&dest2);
        if std::fs::hard_link(&src2, &dest2).is_err() {
            std::fs::copy(&src2, &dest2).map_err(|e| format!("Couldn't copy it into the models folder ({e}).")).map(|_| ())?;
        }
        Ok(())
    })
    .await
    .map_err(|e| refuse("disk_error", e.to_string()))?
    .map_err(|e| refuse("disk_error", e))?;
    if let Ok(meta) = std::fs::metadata(&dest) {
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
        let _ = models.db.save_hash(&dest.to_string_lossy(), meta.len(), mtime, &sha);
    }
    models.set(&plan.files[0], FileState::Verified);
    put(models, plan.clone()).map_err(|e| refuse("disk_error", e))?;
    eprintln!("[engine] custom model imported: {} from {}", plan.id, src.display());
    Ok(plan)
}

/// Saves a custom model's test result in its manifest.
pub fn save_test(models: &Models, id: &str, t: TestResult) -> Result<(), String> {
    let Some(mut m) = models.registry().get(id).cloned() else { return Err(format!("{id} isn't in the registry any more.")) };
    if let Some(c) = m.custom.as_mut() {
        c.test = Some(t);
    }
    put(models, m)
}

/// Removes a custom model from the registry and deletes its manifest (its files are deleted by
/// `Models::delete`, which keeps any another installed model uses).
pub fn forget(models: &Models, id: &str) -> Result<(), String> {
    let _g = REGISTRY_LOCK.lock().unwrap();
    let path = manifest_path(&models.custom_dir(), id);
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("Couldn't delete {} ({e}).", path.display())),
        _ => {}
    }
    let reg = models.registry();
    let rest: Vec<Manifest> = reg.models.iter().filter(|m| m.id != id).cloned().collect();
    models.set_registry(Registry::new(rest)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::tests::{gguf, safetensors};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn sources_parse_in_every_spelling_and_odd_ones_are_refused() {
        let want = Some(("unsloth/Z-Image-Turbo-GGUF".to_string(), Some("z-image-turbo-Q3_K_M.gguf".to_string())));
        for s in ["hf.co/unsloth/Z-Image-Turbo-GGUF:z-image-turbo-Q3_K_M.gguf", "https://huggingface.co/unsloth/Z-Image-Turbo-GGUF:z-image-turbo-Q3_K_M.gguf", " huggingface.co/unsloth/Z-Image-Turbo-GGUF/:z-image-turbo-Q3_K_M.gguf "] {
            assert_eq!(parse_hf(s), want, "{s}");
        }
        assert_eq!(parse_hf("hf.co/a/b"), Some(("a/b".into(), None)));
        assert_eq!(parse_hf("hf.co/a/b:sub/dir/x.safetensors").unwrap().1.as_deref(), Some("sub/dir/x.safetensors"));
        for bad in ["hf.co/a", "hf.co/a/b/c", "hf.co/../b", "z-image-turbo", "hf.co/a/b:../x", "hf.co/a b/c"] {
            assert_eq!(parse_hf(bad), None, "{bad}");
        }
        assert!(is_custom_id("local/my-model") && !is_custom_id("z-image-turbo:q4"));

        // What people copy: the copy button, the page (± /tree/main), file links, hf.co.
        let repo = Some(("chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized".to_string(), None));
        for s in [
            "chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized",
            "https://huggingface.co/chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized",
            "https://huggingface.co/chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized/",
            "https://huggingface.co/chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized/tree/main",
            "https://www.huggingface.co/chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized?library=diffusers",
            "hf.co/chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized",
        ] {
            assert_eq!(parse_hf(s), repo, "{s}");
        }
        let file = |f: &str| Some(("chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized".to_string(), Some(f.to_string())));
        assert_eq!(parse_hf("https://huggingface.co/chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized/blob/main/qwen-image-2.1-step90000-Q8_0.gguf"), file("qwen-image-2.1-step90000-Q8_0.gguf"));
        assert_eq!(parse_hf("https://huggingface.co/chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized/resolve/3f2a1b0/qwen-image-2.1-step90000-Q4_0.gguf?download=true"), file("qwen-image-2.1-step90000-Q4_0.gguf"));
        assert_eq!(parse_hf("huggingface.co/chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized/blob/main/qwen-image-2.1-step90000-BF16/diffusion_pytorch_model.safetensors"), file("qwen-image-2.1-step90000-BF16/diffusion_pytorch_model.safetensors"));
        assert_eq!(parse_hf("chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized:qwen-image-2.1-step90000-Q6_K.gguf"), file("qwen-image-2.1-step90000-Q6_K.gguf"));
        // Not repos: paths, local ids, other sites, a bare file name, a blob link without a file.
        for bad in ["local/my-model", "models/x.gguf", "./a/b", "C:\\models\\x.gguf", "C:/models/x", "https://github.com/a/b", "a/b/blob/main/x.gguf", "hf.co/a/b/blob/main", "hf.co/a/b/discussions/3"] {
            assert_eq!(parse_hf(bad), None, "{bad}");
        }
        assert!(matches!(parse_source("chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized"), Ok(Source::Hf { .. })));
        let err = parse_source("not a model").unwrap_err();
        assert!(!err.contains('<') && err.contains("user/repo"), "no stripped placeholders: {err}");
        assert!(matches!(parse_source("D:\\nowhere\\x.gguf"), Ok(Source::Local(_))), "a missing path reaches the source step");
        let here = std::env::current_exe().unwrap();
        assert_eq!(parse_source(&format!("\"{}\"", here.display())), Ok(Source::Local(here)), "an existing file, quoted as Explorer copies it");
        assert!(is_shard("transformer/diffusion_pytorch_model-00001-of-00003.safetensors") && !is_shard("z-image-turbo-Q3_K_M.gguf"));
    }

    #[test]
    fn found_models_say_where_they_are() {
        let hf = LocalRoot { path: PathBuf::from("/h/.cache/huggingface/hub"), label: "Hugging Face cache".into(), exists: true };
        assert_eq!(place_of(Path::new("/h/.cache/huggingface/hub/models--unsloth--Z-Image-Turbo-GGUF/snapshots/abc/z.gguf"), &hf), "Hugging Face cache · unsloth/Z-Image-Turbo-GGUF");
        let comfy = LocalRoot { path: PathBuf::from("/h/ComfyUI/models"), label: "ComfyUI".into(), exists: true };
        assert_eq!(place_of(Path::new("/h/ComfyUI/models/checkpoints/m.safetensors"), &comfy), "ComfyUI · checkpoints");
        let dir = LocalRoot { path: PathBuf::from("/d/models"), label: "This folder".into(), exists: true };
        assert_eq!(place_of(Path::new("/d/models/sdxl/m.safetensors"), &dir), "sdxl");
        assert_eq!(place_of(Path::new("/d/models/m.safetensors"), &dir), "This folder");
        assert!(local_roots().iter().any(|r| r.label == "Hugging Face cache"));
    }

    #[test]
    fn quants_are_named_short_and_grouped_by_bits() {
        for (path, want) in [
            ("qwen-image-2.1-step90000-Q4_K_M.gguf", ("Q4_K_M", 4)),
            ("qwen-image-2.1-step90000-Q4_0.gguf", ("Q4_0", 4)),
            ("z_image_turbo-Q3_K.gguf", ("Q3_K", 3)),
            ("flux1-schnell-Q8_0.gguf", ("Q8_0", 8)),
            ("model-IQ4_XS.gguf", ("IQ4_XS", 4)),
            ("qwen-image-2.1-step90000-BF16.gguf", ("BF16", 16)),
            ("qwen-image-2.1-step90000-BF16/diffusion_pytorch_model.safetensors", ("BF16", 16)),
            ("qwen-image-2.1-step90000-INT8-ConvRot-comfy.safetensors", ("INT8 ConvRot comfy", 8)),
            ("qwen-image-2.1-step90000-FP8-comfy.safetensors", ("FP8 comfy", 8)),
            ("t5xxl_fp8_e4m3fn.safetensors", ("FP8 e4m3fn", 8)),
            ("sd_xl_base_1.0_fp16.safetensors", ("F16", 16)),
            ("NVFP4-comfy.safetensors", ("NVFP4 comfy", 4)),
        ] {
            assert_eq!(quant_of(path), Some((want.0.to_string(), want.1)), "{path}");
        }
        assert_eq!(quant_of("RealVisXL_V5.0.safetensors"), None);
    }

    #[test]
    fn a_repo_lists_its_sizes_with_what_can_load_and_a_recommendation() {
        use crate::fit::Tier;
        // chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized as the HF API listed it (2026-11-19).
        let gb = |g: f64| (g * 1e9) as usize;
        let listed = [
            ("README.md", 14715),
            ("qwen-image-2.1-step90000-BF16.gguf", gb(14.23)),
            ("qwen-image-2.1-step90000-BF16/config.json", 370),
            ("qwen-image-2.1-step90000-BF16/diffusion_pytorch_model.safetensors", gb(14.23)),
            ("qwen-image-2.1-step90000-FP8-comfy.safetensors", gb(7.12)),
            ("qwen-image-2.1-step90000-FP8-torchao/diffusion_pytorch_model.safetensors", gb(7.12)),
            ("qwen-image-2.1-step90000-INT8-ConvRot-comfy.safetensors", gb(7.26)),
            ("qwen-image-2.1-step90000-INT8-bnb/diffusion_pytorch_model.safetensors", gb(7.12)),
            ("qwen-image-2.1-step90000-NF4-bnb/diffusion_pytorch_model.safetensors", gb(3.67)),
            ("qwen-image-2.1-step90000-NVFP4-comfy.safetensors", gb(4.2)),
            ("qwen-image-2.1-step90000-Q4_0.gguf", gb(4.19)),
            ("qwen-image-2.1-step90000-Q4_K_M.gguf", gb(4.47)),
            ("qwen-image-2.1-step90000-Q5_K_M.gguf", gb(5.21)),
            ("qwen-image-2.1-step90000-Q6_K.gguf", gb(6.0)),
            ("qwen-image-2.1-step90000-Q8_0.gguf", gb(7.69)),
            ("text_encoder/model-00001-of-00002.safetensors", gb(4.0)),
        ];
        let files: Vec<RepoFile> = listed.iter().map(|(p, n)| RepoFile { path: p.to_string(), size: *n as u64, sha256: None }).collect();
        let mut o = file_options(&files);
        assert_eq!(o.len(), 13, "the 13 model files; README, config, a shard of the encoder left out");
        let no: Vec<&str> = o.iter().filter(|f| !f.loadable).map(|f| f.quant.as_str()).collect();
        assert_eq!(no, ["NF4 bnb", "NVFP4 comfy", "FP8 comfy", "FP8 torchao", "INT8 bnb"], "the five sd.cpp can't load, smallest first");
        assert!(o.iter().find(|f| f.quant == "INT8 ConvRot comfy").unwrap().loadable, "int8 ConvRot loads at 465adf3");
        assert_eq!(o.iter().filter(|f| f.bits == Some(4) && f.loadable).count(), 2);

        // Fits beats offload (the largest that fits), else the smallest that offloads.
        let set = |o: &mut Vec<FileOption>, f: &dyn Fn(u64) -> Tier| {
            for x in o.iter_mut().filter(|x| x.loadable) {
                x.fit = Some(f(x.size));
            }
            recommend(o);
            o.iter().find(|x| x.recommended).map(|x| x.quant.clone())
        };
        assert_eq!(set(&mut o, &|n| if n < gb(6.5) as u64 { Tier::Fits } else { Tier::Offload }).as_deref(), Some("Q6_K"));
        assert_eq!(set(&mut o, &|n| if n < gb(10.0) as u64 { Tier::Offload } else { Tier::NeedsMoreMemory }).as_deref(), Some("Q4_0"));
        assert_eq!(set(&mut o, &|_| Tier::NeedsMoreMemory), None);
        assert!(o.iter().filter(|x| !x.loadable).all(|x| !x.recommended));
    }

    #[test]
    fn licences_come_from_the_card() {
        let l = card_licence(&json!({ "license": "apache-2.0" }), "a/b", "s");
        assert!(l.commercial && l.note.is_none());
        let l = card_licence(&json!({}), "a/b", "s");
        assert_eq!(l.name.as_deref(), Some("No licence stated"));
        assert!(!l.commercial);
        let l = card_licence(&json!({ "license": "other", "license_name": "flux-1-dev-non-commercial-license", "license_link": "LICENSE.md" }), "a/b", "abc");
        assert_eq!((l.name.as_deref(), l.url.as_str(), l.commercial), (Some("flux-1-dev-non-commercial-license"), "https://huggingface.co/a/b/blob/abc/LICENSE.md", false));
        assert!(card_licence(&json!({ "license": "creativeml-openrail-m" }), "a/b", "s").use_restrictions);
        assert!(!card_licence(&json!({ "license": "cc-by-nc-4.0" }), "a/b", "s").commercial);
    }

    /// A fake Hugging Face: `/api/models/<repo>` from `repos`, `/<repo>/resolve/<rev>/<path>` from
    /// `files` with Range. Counts the body bytes it sends.
    async fn fake_hf(repos: HashMap<String, Value>, files: HashMap<String, Vec<u8>>) -> (String, Arc<AtomicU64>) {
        use axum::http::{HeaderMap, StatusCode, Uri, header};
        let sent = Arc::new(AtomicU64::new(0));
        let (repos, files, s2) = (Arc::new(repos), Arc::new(files), sent.clone());
        let app = axum::Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
            let (repos, files, sent) = (repos.clone(), files.clone(), s2.clone());
            async move {
                let path = uri.path().trim_start_matches('/').to_string();
                if let Some(repo) = path.strip_prefix("api/models/") {
                    return match repos.get(repo) {
                        Some(v) => axum::Json(v.clone()).into_response(),
                        None => StatusCode::NOT_FOUND.into_response(),
                    };
                }
                // <user>/<repo>/resolve/<rev>/<path>
                let parts: Vec<&str> = path.splitn(5, '/').collect();
                let Some(body) = (parts.len() == 5 && parts[2] == "resolve").then(|| files.get(&format!("{}/{}/{}", parts[0], parts[1], parts[4]))).flatten() else {
                    return StatusCode::NOT_FOUND.into_response();
                };
                let (a, b) = headers
                    .get(header::RANGE)
                    .and_then(|v| v.to_str().ok()?.strip_prefix("bytes=")?.split_once('-').map(|(a, b)| (a.parse::<usize>().ok(), b.parse::<usize>().ok())))
                    .map(|(a, b)| (a.unwrap_or(0), b.unwrap_or(body.len() - 1).min(body.len() - 1)))
                    .unwrap_or((0, body.len() - 1));
                sent.fetch_add((b + 1 - a) as u64, Ordering::SeqCst);
                let mut resp = body[a..=b].to_vec().into_response();
                *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
                resp.headers_mut().insert(header::CONTENT_RANGE, format!("bytes {a}-{b}/{}", body.len()).parse().unwrap());
                resp
            }
        });
        use axum::response::IntoResponse;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, sent)
    }

    /// The repo API answer for these files (sha256 = fake but well-formed).
    fn repo(id: &str, files: &[(&str, usize)], gated: Value, card: Value) -> Value {
        let siblings: Vec<Value> = files.iter().enumerate().map(|(i, (p, n))| json!({ "rfilename": p, "size": n, "lfs": { "sha256": format!("{:064x}", i + 1), "size": n } })).collect();
        json!({ "id": id, "sha": "1".repeat(40), "gated": gated, "private": false, "siblings": siblings, "cardData": card })
    }

    /// A Models over the real registry, on an 8 GB laptop, talking to `endpoint`.
    fn models(endpoint: &str) -> (Arc<Models>, PathBuf) {
        let reg = Registry::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("registry")).unwrap();
        let data = std::env::temp_dir().join(format!("custom-test-{}", uuid::Uuid::new_v4()));
        let dir = data.join("models");
        let laptop = crate::fit::Machine { gpu_mib: 8151, integrated: false, ram_total_mib: 24 * 1024 };
        let rt = Arc::new(crate::runtimes::Runtimes::fixed(&dir, "sd-server.exe".into()));
        (Models::new(reg, None, Some(laptop), dir, Arc::new(crate::db::Db::open_in_memory().unwrap()), endpoint, rt), data)
    }

    const MB: usize = 1 << 20;

    #[tokio::test]
    async fn a_zimage_gguf_passes_reading_only_its_header_and_shares_the_library_companions() {
        let mut body = gguf(&[("general.architecture", "z_image")], &[("cap_embedder.0.weight", &[2560, 3840]), ("layers.0.attention.qkv.weight", &[3840, 11520])]);
        body.resize(40 * MB, 0);
        let (url, sent) = fake_hf(
            HashMap::from([("someone/zimage-gguf".into(), repo("someone/Zimage-GGUF", &[("zimage-Q3.gguf", body.len()), ("README.md", 100)], json!(false), json!({ "license": "apache-2.0" })))]),
            HashMap::from([("someone/Zimage-GGUF/zimage-Q3.gguf".into(), body.clone())]),
        )
        .await;
        let (m, data) = models(&url);
        // The library's Z-Image text encoder is on disk already: shared, not downloaded again.
        let base = m.registry().get("z-image-turbo:q4").unwrap().clone();
        let enc = base.files.iter().find(|f| f.role == Role::Llm).unwrap();
        m.set(enc, FileState::Verified);

        let r = check(&m, "hf.co/someone/zimage-gguf").await;
        assert!(r.ok, "{:#?}", r.steps);
        assert_eq!(r.id.as_deref(), Some("hf.co/someone/Zimage-GGUF:zimage-Q3.gguf"), "the canonical spelling, and the one weight file picked");
        assert!(sent.load(Ordering::SeqCst) <= 256 * 1024, "only the header was read ({} bytes)", sent.load(Ordering::SeqCst));
        let plan = r.plan.as_ref().unwrap();
        assert_eq!(plan.custom.as_ref().unwrap().base.as_deref(), Some("z-image-turbo"));
        assert_eq!(plan.files[0].role, Role::DiffusionModel);
        assert_eq!(plan.files.len(), base.files.len(), "weights + the same companions");
        let vae = plan.files.iter().find(|f| f.role == Role::Vae).unwrap();
        assert_eq!(r.download_bytes, body.len() as u64 + vae.size, "the encoder on disk isn't counted");
        assert!(r.fit.as_ref().unwrap().estimate);
        assert_eq!(plan.defaults, base.defaults);
        assert_eq!(r.licence.as_ref().unwrap().spdx, "apache-2.0");
        assert!(r.steps.iter().all(|s| s.status == "ok") && r.steps.len() == 6);

        // Added: manifest in <data>/custom, in the registry, labelled, untested, not ready.
        let added = add(&m, "hf.co/someone/zimage-gguf").await.unwrap();
        let got = m.resolve("hf.co/someone/Zimage-GGUF").unwrap();
        assert_eq!(got.id, added.id, "a bare repo id finds its one custom file");
        assert_eq!(got.testing(), crate::registry::Testing::Untested);
        assert!(!got.custom_ready());
        assert_eq!(load_dir(&data.join("custom")).0, vec![added.clone()], "the manifest round-trips");
        assert!(crate::registry::PRESETS.iter().all(|p| p.model != added.id));
        // A test result is saved; forgetting removes manifest and entry.
        save_test(&m, &added.id, TestResult { ok: true, at_ms: 1, message: "ok".into(), seconds: Some(1.0), image: None, job: None, width: 512, height: 512, steps: 4 }).unwrap();
        assert!(m.resolve(&added.id).unwrap().custom_ready());
        forget(&m, &added.id).unwrap();
        assert!(m.resolve(&added.id).is_err() && load_dir(&data.join("custom")).0.is_empty());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn refusals_say_why_and_download_nothing_past_the_header() {
        let mut llm = gguf(&[("general.architecture", "qwen3"), ("tokenizer.ggml.model", "gpt2")], &[("token_embd.weight", &[1024, 151936])]);
        llm.resize(30 * MB, 0);
        let mut lora = safetensors(&[("diffusion_model.double_blocks.0.img_attn.qkv.lora_A.weight", &[16, 3072])], &[], 0);
        lora.resize(5 * MB, 0);
        let mut sd1 = safetensors(
            &[
                ("model.diffusion_model.input_blocks.0.0.weight", &[320, 4, 3, 3]),
                ("model.diffusion_model.middle_block.1.norm.weight", &[320]),
                ("cond_stage_model.transformer.text_model.embeddings.token_embedding.weight", &[49408, 768]),
                ("first_stage_model.decoder.conv_in.weight", &[512, 4, 3, 3]),
            ],
            &[],
            0,
        );
        sd1.resize(20 * MB, 0);
        let (url, sent) = fake_hf(
            HashMap::from([
                ("q/llm".into(), repo("q/llm", &[("Qwen3-0.6B-Q4_K_M.gguf", llm.len())], json!(false), json!({ "license": "apache-2.0" }))),
                ("q/lora".into(), repo("q/lora", &[("style.safetensors", lora.len())], json!(false), json!({}))),
                ("q/pickle".into(), repo("q/pickle", &[("model.ckpt", 50 * MB)], json!(false), json!({}))),
                ("q/gated".into(), repo("q/gated", &[("m.safetensors", 50 * MB)], json!("manual"), json!({}))),
                ("q/many".into(), repo("q/many", &[("a-Q4.gguf", 30 * MB), ("a-Q8.gguf", 50 * MB), ("README.md", 1)], json!(false), json!({}))),
                ("q/sd1".into(), repo("q/sd1", &[("fine.safetensors", sd1.len())], json!(false), json!({ "license": "creativeml-openrail-m" }))),
            ]),
            HashMap::from([("q/llm/Qwen3-0.6B-Q4_K_M.gguf".into(), llm), ("q/lora/style.safetensors".into(), lora), ("q/sd1/fine.safetensors".into(), sd1)]),
        )
        .await;
        let (m, data) = models(&url);
        let failed = |r: &Report| (r.code, r.steps.iter().find(|s| s.status == "failed").map(|s| s.step));

        let r = check(&m, "hf.co/q/llm").await;
        assert_eq!(failed(&r), (Some("not_image_model"), Some("architecture")));
        assert!(r.message.as_ref().unwrap().contains("language model (LLM, architecture qwen3)"), "{:?}", r.message);
        assert_eq!(r.steps.len(), 6, "later steps listed as skipped");
        assert!(sent.load(Ordering::SeqCst) <= 256 * 1024);

        let r = check(&m, "hf.co/q/lora").await;
        assert_eq!(failed(&r), (Some("lora"), Some("architecture")));

        let before = sent.load(Ordering::SeqCst);
        let r = check(&m, "hf.co/q/pickle").await;
        assert_eq!(failed(&r), (Some("pickle"), Some("format")));
        assert!(r.message.as_ref().unwrap().contains("can run code"));
        assert_eq!(sent.load(Ordering::SeqCst), before, "a pickle isn't even opened");

        let r = check(&m, "hf.co/q/gated").await;
        assert_eq!(failed(&r), (Some("gated"), Some("source")));
        assert_eq!(r.message.as_deref(), Some("This repo needs a Hugging Face login, which Dawnroom doesn't support yet."));

        // Several files: listed as sizes (smallest first, the README left out) and one is checked.
        let r = check(&m, "q/many").await;
        let listed: Vec<(&str, &str)> = r.files.iter().map(|f| (f.file.as_str(), f.quant.as_str())).collect();
        assert_eq!(listed, [("a-Q4.gguf", "Q4"), ("a-Q8.gguf", "Q8")], "no known quant: the part the names don't share");
        assert_eq!(r.files.iter().filter(|f| f.selected).count(), 1);
        let r = check(&m, "https://huggingface.co/q/many/blob/main/a-Q8.gguf").await;
        assert_eq!(r.file.as_deref(), Some("a-Q8.gguf"));
        assert!(r.files.iter().any(|f| f.selected && f.file == "a-Q8.gguf"));

        assert_eq!(check(&m, "hf.co/q/missing").await.code, Some("not_found"));

        // An SD 1.x checkpoint carries its own text encoder and VAE: no companions.
        let r = check(&m, "hf.co/q/sd1").await;
        assert!(r.ok, "{:#?}", r.steps);
        let plan = r.plan.unwrap();
        assert_eq!((plan.files.len(), plan.files[0].role, plan.custom.unwrap().base.as_deref()), (1, Role::Checkpoint, Some("sd15")));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn a_local_safetensors_is_imported_hashed_and_numbered() {
        let (m, data) = models("http://unused");
        let src_dir = data.join("downloads");
        std::fs::create_dir_all(&src_dir).unwrap();
        let mut body = safetensors(&[("model.diffusion_model.input_blocks.0.0.weight", &[320, 4, 3, 3]), ("model.diffusion_model.middle_block.1.norm.weight", &[320]), ("cond_stage_model.transformer.text_model.embeddings.token_embedding.weight", &[49408, 768]), ("first_stage_model.decoder.conv_in.weight", &[512, 4, 3, 3])], &[("modelspec.license", "creativeml-openrail-m")], 0);
        body.resize(3 * MB, 7);
        let src = src_dir.join("My Fine-Tune v2.safetensors");
        std::fs::write(&src, &body).unwrap();

        let r = check(&m, src.to_str().unwrap()).await;
        assert!(r.ok, "{:#?}", r.steps);
        assert!(r.licence.unwrap().name.unwrap().contains("creativeml-openrail-m"));
        let a = import(&m, src.to_str().unwrap()).await.unwrap();
        assert_eq!(a.id, "local/my-fine-tune-v2");
        assert_eq!(std::fs::read(m.path_of(&a.files[0])).unwrap(), body, "placed in the models folder");
        assert_eq!(m.model_state(&m.resolve(&a.id).unwrap()), "needs_test", "files here, test not run yet");
        // The same path again: already added (said by the check). A copy elsewhere: a duplicate by hash.
        // A different file of the same name: numbered.
        assert_eq!(check(&m, src.to_str().unwrap()).await.code, Some("already_added"));
        let other = data.join("elsewhere");
        std::fs::create_dir_all(&other).unwrap();
        let copy = other.join("My Fine-Tune v2.safetensors");
        std::fs::write(&copy, &body).unwrap();
        assert_eq!(import(&m, copy.to_str().unwrap()).await.unwrap_err().code, Some("in_library"));
        body[2 * MB] = 9;
        std::fs::write(&copy, &body).unwrap();
        assert_eq!(import(&m, copy.to_str().unwrap()).await.unwrap().id, "local/my-fine-tune-v2-2");
        // Refusals for local files.
        assert_eq!(check(&m, data.join("nope.safetensors").to_str().unwrap()).await.code, Some("not_found"));
        let ckpt = src_dir.join("old.ckpt");
        std::fs::write(&ckpt, vec![0x80u8; 2 * MB]).unwrap();
        assert_eq!(check(&m, ckpt.to_str().unwrap()).await.code, Some("pickle"));
        // A .safetensors that is really a pickle: the header read catches it.
        let fake = src_dir.join("fake.safetensors");
        std::fs::write(&fake, [b"PK\x03\x04".to_vec(), vec![0u8; 2 * MB]].concat()).unwrap();
        let r = check(&m, fake.to_str().unwrap()).await;
        assert_eq!(r.code, Some("bad_header"));
        assert!(r.message.unwrap().contains("PyTorch zip"));
        let _ = std::fs::remove_dir_all(&data);
    }
}
