//! What the data folder holds and the operations on it: the usage report (each hardlinked file
//! counted once), "Clear cache" (an allowlist of rebuildable things), paused downloads, the
//! thumbnail cap, moving the folder to another drive, and relinking dev model files. Plain
//! functions: the engine's endpoints and the `dawnroom storage` CLI call the same ones, and the CLI
//! works with the engine stopped (the installer will use it).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::config::{Config, UserConfig};
use crate::db::Db;
use crate::disk::Space;
use crate::registry::Registry;

/// Thumbnails are capped at about this much; the oldest go first (must-fix 10).
pub const THUMBS_CAP_BYTES: u64 = 500_000_000;
/// Written in the destination while a move copies; a folder with it is an unfinished move.
const MOVING_MARKER: &str = ".dawnroom-moving";

// ---------- Walking the folder ----------

/// Identity of a file on its volume, so hardlinks are counted (and copied) once.
#[cfg(windows)]
pub fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    let f = std::fs::File::open(path).ok()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle()), &mut info) }.ok()?;
    Some((info.dwVolumeSerialNumber as u64, ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64))
}

#[cfg(unix)]
pub fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some((m.dev(), m.ino()))
}

/// Every file under `dir` (not following links), as (path, size).
fn files(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                out.push((e.path(), e.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    out
}

/// Bytes under `dir`, each hardlinked file once.
pub fn dir_bytes(dir: &Path) -> u64 {
    let mut seen = HashSet::new();
    files(dir).into_iter().filter(|(p, _)| file_id(p).is_none_or(|id| seen.insert(id))).map(|(_, n)| n).sum()
}

fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

// ---------- Report ----------

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Usage {
    pub models: u64,
    pub images: u64,
    pub runtimes: u64,
    /// Everything "Clear cache" may remove (see `cache_items`).
    pub cache: u64,
    /// Paused downloads (`.part`).
    pub partial: u64,
    /// The history database, current logs and anything else.
    pub other: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModelUsage {
    pub id: String,
    pub name: String,
    /// Every file is on disk at its full size (hashes are the engine's job).
    pub complete: bool,
    /// Bytes of its files on disk; a file shared with another model is counted for each.
    pub bytes: u64,
    /// Files another model with files on disk also uses: deleting this model keeps them.
    pub shared: Vec<String>,
    pub last_used_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Partial {
    /// Path inside the data folder, e.g. `models/diffusion_models/x.gguf.part`.
    pub file: String,
    /// The model it belongs to, or `runtime`.
    pub owner: String,
    pub bytes: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CacheItem {
    /// thumbnails | rotated_log | old_runtime | unpack_leftover | runtime_zip
    pub kind: &'static str,
    /// Path inside the data folder (a folder for old runtimes and thumbnails).
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Report {
    pub data_dir: String,
    pub drive: Option<Space>,
    pub usage: Usage,
    pub models: Vec<ModelUsage>,
    pub partials: Vec<Partial>,
    pub cache: Vec<CacheItem>,
}

/// Installed runtimes to keep: the active one plus the most recently used other one. Without an
/// active runtime (CLI with the engine stopped), the two most recently used.
pub fn runtimes_to_keep(installed: &[(String, i64)], active: Option<&str>) -> HashSet<String> {
    let mut by_recent = installed.to_vec();
    by_recent.sort_by_key(|x| std::cmp::Reverse(x.1));
    let mut keep: HashSet<String> = active.filter(|a| installed.iter().any(|(id, _)| id == a)).map(|a| a.to_string()).into_iter().collect();
    for (id, _) in by_recent {
        if keep.len() >= 2 {
            break;
        }
        keep.insert(id);
    }
    keep
}

/// Runtime folders with a `runtime.json`, as (folder name, last used ms).
pub fn installed_runtimes(runtimes_dir: &Path) -> Vec<(String, i64)> {
    let Ok(rd) = std::fs::read_dir(runtimes_dir) else { return vec![] };
    rd.flatten()
        .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(e.path().join("runtime.json")).ok()?).ok()?;
            Some((e.file_name().to_string_lossy().into_owned(), v["last_used_ms"].as_i64().unwrap_or(0)))
        })
        .collect()
}

/// The rebuildable things in the data folder. Never models, images, the database, the current
/// and previous runtime, or paused downloads.
pub fn cache_items(cfg: &Config, active_runtime: Option<&str>) -> Vec<CacheItem> {
    let data = &cfg.data_dir;
    let mut out = Vec::new();
    let thumbs = cfg.thumbs_dir();
    let tb = dir_bytes(&thumbs);
    if tb > 0 {
        out.push(CacheItem { kind: "thumbnails", path: rel(data, &thumbs), bytes: tb });
    }
    for p in crate::logs::rotated(&cfg.logs_dir()) {
        let bytes = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        out.push(CacheItem { kind: "rotated_log", path: rel(data, &p), bytes });
    }
    let rt = data.join("runtimes");
    let installed = installed_runtimes(&rt);
    let keep = runtimes_to_keep(&installed, active_runtime);
    for (id, _) in &installed {
        if !keep.contains(id) {
            let p = rt.join(id);
            out.push(CacheItem { kind: "old_runtime", bytes: dir_bytes(&p), path: rel(data, &p) });
        }
    }
    if let Ok(rd) = std::fs::read_dir(&rt) {
        for e in rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(".unpack-")) {
            out.push(CacheItem { kind: "unpack_leftover", bytes: dir_bytes(&e.path()), path: rel(data, &e.path()) });
        }
    }
    // A finished zip left by an interrupted install: downloaded again if needed.
    if let Ok(rd) = std::fs::read_dir(rt.join(".downloads")) {
        for e in rd.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".zip")) {
            out.push(CacheItem { kind: "runtime_zip", bytes: e.metadata().map(|m| m.len()).unwrap_or(0), path: rel(data, &e.path()) });
        }
    }
    out
}

/// Whose paused download a model `.part` is: the model whose pull wrote it (its sidecar), else
/// the model it's the weights of. A shared encoder or VAE `.part` from before sidecars named
/// their model falls back to the first model listing it.
fn part_owner(reg: &Registry, f: &crate::registry::ModelFile, part: &Path) -> Option<String> {
    let lists = |m: &&crate::registry::Manifest| m.files.iter().any(|g| g.rel_path() == f.rel_path());
    crate::download::part_owner(part)
        .filter(|o| reg.models.iter().filter(lists).any(|m| &m.id == o))
        .or_else(|| reg.models.iter().filter(lists).find(|m| m.files.iter().any(|g| g.rel_path() == f.rel_path() && g.role.is_weights())).map(|m| m.id.clone()))
}

/// Paused downloads: model `.part` files (with the model they belong to) and runtime `.part` zips.
pub fn partials(cfg: &Config, reg: &Registry) -> Vec<Partial> {
    let mut out = Vec::new();
    for m in &reg.models {
        for f in &m.files {
            let p = cfg.models_dir().join(&f.category).join(format!("{}.part", f.filename));
            if out.iter().any(|x: &Partial| x.file == rel(&cfg.data_dir, &p)) {
                continue; // shared file, listed once
            }
            if let Ok(meta) = std::fs::metadata(&p) {
                out.push(Partial { file: rel(&cfg.data_dir, &p), owner: part_owner(reg, f, &p).unwrap_or_else(|| m.id.clone()), bytes: meta.len(), total: Some(f.size) });
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(cfg.data_dir.join("runtimes").join(".downloads")) {
        for e in rd.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")) {
            out.push(Partial { file: rel(&cfg.data_dir, &e.path()), owner: "runtime".into(), bytes: e.metadata().map(|m| m.len()).unwrap_or(0), total: None });
        }
    }
    out
}

pub fn report(cfg: &Config, reg: &Registry, db: Option<&Db>, active_runtime: Option<&str>) -> Report {
    let data = &cfg.data_dir;
    let cache = cache_items(cfg, active_runtime);
    let partials = partials(cfg, reg);
    let cache_paths: Vec<PathBuf> = cache.iter().map(|c| data.join(&c.path)).collect();
    let part_paths: HashSet<PathBuf> = partials.iter().map(|p| data.join(&p.file)).collect();

    // One pass over the folder: every file lands in exactly one category, hardlinks once.
    let mut usage = Usage::default();
    let mut seen = HashSet::new();
    for (p, n) in files(data) {
        if file_id(&p).is_some_and(|id| !seen.insert(id)) {
            continue;
        }
        let slot = if part_paths.contains(&p) {
            &mut usage.partial
        } else if cache_paths.iter().any(|c| p.starts_with(c)) {
            &mut usage.cache
        } else if p.starts_with(cfg.models_dir()) {
            &mut usage.models
        } else if p.starts_with(cfg.outputs_dir()) {
            &mut usage.images
        } else if p.starts_with(data.join("runtimes")) {
            &mut usage.runtimes
        } else {
            &mut usage.other
        };
        *slot += n;
        usage.total += n;
    }

    let last = db.and_then(|d| d.last_used().ok()).unwrap_or_default();
    let on_disk = |f: &crate::registry::ModelFile| std::fs::metadata(cfg.models_dir().join(&f.category).join(&f.filename)).ok().map(|m| m.len());
    // A variant is on disk when its own weights are: tags share text encoders and VAEs, and a
    // shared file alone doesn't make every tag "on disk".
    let has_files = |m: &crate::registry::Manifest| m.files.iter().filter(|f| f.role.is_weights()).any(|f| on_disk(f).is_some());
    let models = reg
        .models
        .iter()
        .filter(|m| has_files(m))
        .map(|m| ModelUsage {
            id: m.id.clone(),
            name: m.name.clone(),
            complete: m.files.iter().all(|f| on_disk(f) == Some(f.size)),
            bytes: m.files.iter().filter_map(on_disk).sum(),
            shared: m
                .files
                .iter()
                .filter(|f| reg.models.iter().any(|o| o.id != m.id && has_files(o) && o.files.iter().any(|g| g.rel_path() == f.rel_path())))
                .map(|f| f.rel_path())
                .collect(),
            // Jobs before size tags recorded the bare model id.
            last_used_ms: last.get(&m.id).copied().max(last.get(&m.family).copied()),
        })
        .collect();
    Report { data_dir: data.to_string_lossy().trim_start_matches(r"\\?\").to_string(), drive: crate::disk::space(data), usage, models, partials, cache }
}

// ---------- Clear cache, partials, thumbnails ----------

/// Removes exactly the listed cache items (paths inside the data folder that `cache_items` gave).
/// Returns (bytes freed, paths removed).
pub fn clear_cache(cfg: &Config, items: &[CacheItem]) -> (u64, Vec<String>) {
    let (mut freed, mut removed) = (0, Vec::new());
    for it in items {
        let p = cfg.data_dir.join(&it.path);
        // Allowlist: only these places, and never anything outside the data folder.
        let allowed = p.starts_with(cfg.thumbs_dir()) || p.starts_with(cfg.logs_dir()) || p.starts_with(cfg.data_dir.join("runtimes"));
        if !allowed || it.path.contains("..") {
            continue;
        }
        let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        match r {
            Ok(()) => {
                freed += it.bytes;
                removed.push(it.path.clone());
            }
            Err(e) => eprintln!("[engine] couldn't remove {}: {e}", p.display()),
        }
    }
    if removed.iter().any(|r| r == &rel(&cfg.data_dir, &cfg.thumbs_dir())) {
        let _ = std::fs::create_dir_all(cfg.thumbs_dir());
    }
    (freed, removed)
}

/// Deletes one paused download. `file` must be one `partials` lists (nothing else can be named).
pub fn delete_partial(cfg: &Config, reg: &Registry, file: &str) -> Result<u64, String> {
    let p = partials(cfg, reg).into_iter().find(|p| p.file == file).ok_or_else(|| format!("There's no paused download called {file:?}."))?;
    crate::download::remove_part(&cfg.data_dir.join(&p.file)).map_err(|e| format!("Couldn't delete {file} ({e}). Is it still downloading?"))?;
    Ok(p.bytes)
}

/// Deletes the oldest thumbnails until the folder is under `cap`. Returns bytes freed.
pub fn cap_thumbs(dir: &Path, cap: u64) -> u64 {
    let mut list: Vec<(PathBuf, u64, std::time::SystemTime)> = files(dir)
        .into_iter()
        .map(|(p, n)| {
            let t = std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            (p, n, t)
        })
        .collect();
    let mut total: u64 = list.iter().map(|x| x.1).sum();
    if total <= cap {
        return 0;
    }
    list.sort_by_key(|x| x.2);
    let mut freed = 0;
    for (p, n, _) in list {
        if total <= cap {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= n;
            freed += n;
        }
    }
    freed
}

// ---------- Moving the data folder ----------

/// The two paths are the same folder or one is inside the other. Case-insensitive on Windows,
/// where `D:\Data` and `d:\data\inner` are the same place.
pub fn nested(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = std::path::absolute(p).unwrap_or(p.to_path_buf()).to_string_lossy().replace('\\', "/").trim_end_matches('/').to_string();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    let (a, b) = (norm(a), norm(b));
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MoveProgress {
    /// idle | checking | copying | verifying | switching | restarting | failed
    pub state: String,
    pub bytes: u64,
    pub total: u64,
    pub from: Option<String>,
    pub to: Option<String>,
    pub error: Option<String>,
}

impl Default for MoveProgress {
    fn default() -> Self {
        Self { state: "idle".into(), bytes: 0, total: 0, from: None, to: None, error: None }
    }
}

/// Checks a destination before anything is copied: absolute, not inside (or around) the current
/// folder, empty or an unfinished move, and with room for everything. Returns bytes to copy.
pub fn check_move(from: &Path, to: &Path) -> Result<u64, (String, String)> {
    let bad = |c: &str, m: String| Err((c.to_string(), m));
    if !to.is_absolute() {
        return bad("invalid_request", format!("{} isn't a full path. Pick a folder like D:\\Dawnroom.", to.display()));
    }
    let (f, t) = (std::path::absolute(from).unwrap_or(from.to_path_buf()), std::path::absolute(to).unwrap_or(to.to_path_buf()));
    if nested(&f, &t) {
        return bad("invalid_request", "The new folder can't be the current data folder, inside it, or around it. Pick a separate folder.".into());
    }
    let has_files = std::fs::read_dir(&t).is_ok_and(|mut rd| rd.next().is_some());
    if has_files && !t.join(MOVING_MARKER).exists() {
        return bad("folder_not_empty", format!("{} isn't empty. Pick an empty folder (or a new one).", t.display()));
    }
    let need = dir_bytes(&f);
    crate::disk::check(&t, need, "Moving the data folder").map_err(|e| (e.code, e.message))?;
    Ok(need)
}

/// Copies `from` into `to` (hardlink groups stay hardlinked), then checks every file's size.
/// An unfinished earlier attempt in `to` is cleared first. The marker file stays until the
/// caller has switched the config, so a crash leaves a folder that is obviously unfinished.
pub fn copy_tree(from: &Path, to: &Path, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> Result<(), String> {
    if to.join(MOVING_MARKER).exists() {
        std::fs::remove_dir_all(to).map_err(|e| format!("Couldn't clear the unfinished move in {} ({e}).", to.display()))?;
    }
    std::fs::create_dir_all(to).map_err(|e| format!("Couldn't create {} ({e}).", to.display()))?;
    std::fs::write(to.join(MOVING_MARKER), b"An unfinished Dawnroom data folder move. Safe to delete.\n").map_err(|e| e.to_string())?;
    let list = files(from);
    let mut copied: HashMap<(u64, u64), PathBuf> = HashMap::new();
    let mut done = 0u64;
    for (src, n) in &list {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let dst = to.join(src.strip_prefix(from).map_err(|e| e.to_string())?);
        if let Some(d) = dst.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("Couldn't create {} ({e}).", d.display()))?;
        }
        let id = file_id(src);
        match id.and_then(|i| copied.get(&i)) {
            Some(first) => std::fs::hard_link(first, &dst).map_err(|e| format!("Couldn't link {} ({e}).", dst.display()))?,
            None => {
                copy_with_progress(src, &dst, &mut |b| progress(done + b)).map_err(|e| format!("Couldn't copy {} ({e}). Check free space on the new drive.", src.display()))?;
                done += n;
                progress(done);
                if let Some(i) = id {
                    copied.insert(i, dst.clone());
                }
            }
        }
    }
    for (src, n) in &list {
        let dst = to.join(src.strip_prefix(from).unwrap());
        let got = std::fs::metadata(&dst).map(|m| m.len()).map_err(|e| format!("{} is missing after the copy ({e}).", dst.display()))?;
        if got != *n {
            return Err(format!("{} is {got} bytes after the copy, expected {n}.", dst.display()));
        }
    }
    Ok(())
}

fn copy_with_progress(src: &Path, dst: &Path, progress: &mut dyn FnMut(u64)) -> std::io::Result<()> {
    use std::io::{Read, Write};
    let mut r = std::fs::File::open(src)?;
    let mut w = std::fs::File::create(dst)?;
    let mut buf = vec![0u8; 8 << 20];
    let mut n = 0u64;
    loop {
        let k = r.read(&mut buf)?;
        if k == 0 {
            break;
        }
        w.write_all(&buf[..k])?;
        n += k as u64;
        progress(n);
    }
    w.flush()?;
    // Keep the modification time: the engine's hash cache is keyed by (path, size, mtime).
    if let Ok(t) = std::fs::metadata(src).and_then(|m| m.modified()) {
        let _ = w.set_modified(t);
    }
    Ok(())
}

/// The move is copied and checked: point the config at `to`, name `from` for deletion, and
/// drop the marker. After this returns, `to` is the data folder.
pub fn switch_to(from: &Path, to: &Path) -> Result<(), String> {
    let mut uc = UserConfig::load();
    uc.data_dir = Some(to.to_path_buf());
    uc.delete_after_move = Some(from.to_path_buf());
    uc.save()?;
    let _ = std::fs::remove_file(to.join(MOVING_MARKER));
    Ok(())
}

/// Deletes the old folder a finished move left behind (call it before anything opens files in
/// the data folder). Refuses anything that isn't clearly a separate, old data folder.
pub fn finish_pending_delete(current: &Path) -> Option<String> {
    let mut uc = UserConfig::load();
    let old = uc.delete_after_move.take()?;
    let cur = std::path::absolute(current).unwrap_or(current.to_path_buf());
    // Only when the current folder really holds the data (a whole move, not an empty folder that
    // reappeared on another disk), and the old one is clearly separate and a data folder.
    let current_ok = cur.join("history.sqlite").exists() && !cur.join(MOVING_MARKER).exists();
    if !current_ok {
        // Keep it named; try again once the current folder is whole.
        uc.delete_after_move = Some(old.clone());
        let _ = uc.save();
        return Some(format!("kept {} for now: the current folder {} doesn't hold the data yet", old.display(), cur.display()));
    }
    let safe = !nested(&old, &cur) && old.join("history.sqlite").exists();
    let msg = if !old.exists() {
        "already gone".to_string()
    } else if !safe {
        format!("left in place (it doesn't look like an old data folder): {}", old.display())
    } else {
        match std::fs::remove_dir_all(&old) {
            Ok(()) => format!("deleted {}", old.display()),
            Err(e) => {
                // Keep it named so the next start tries again.
                uc.delete_after_move = Some(old.clone());
                format!("couldn't delete {} yet ({e}); trying again next start", old.display())
            }
        }
    };
    let _ = uc.save();
    Some(msg)
}

// ---------- Relinking dev model files (the CLI form of link-models.ps1) ----------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LinkResult {
    pub file: String,
    /// linked | already_linked | not_found | hash_mismatch | failed
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// For every registry file, finds a file with the same name and size under `source`, checks its
/// sha256 against the manifest, and hardlinks it into the models folder in place of any copy
/// there (saves the space a cross-drive move costs). Same drive only, engine stopped.
pub fn link_models(models_dir: &Path, reg: &Registry, source: &Path) -> Vec<LinkResult> {
    let found = files(source);
    let mut out = Vec::new();
    for f in reg.all_files() {
        let r = |result: &str, detail: Option<String>| LinkResult { file: f.rel_path(), result: result.into(), detail };
        let Some((src, _)) = found.iter().find(|(p, n)| *n == f.size && p.file_name().is_some_and(|x| x.to_string_lossy() == f.filename)) else {
            out.push(r("not_found", None));
            continue;
        };
        let dst = models_dir.join(&f.category).join(&f.filename);
        if dst.exists() && file_id(&dst).is_some() && file_id(&dst) == file_id(src) {
            out.push(r("already_linked", None));
            continue;
        }
        match crate::download::sha256_file(src, |_| {}) {
            Ok(h) if h == f.sha256 => {}
            Ok(_) => {
                out.push(r("hash_mismatch", Some(src.display().to_string())));
                continue;
            }
            Err(e) => {
                out.push(r("failed", Some(e.to_string())));
                continue;
            }
        }
        let _ = std::fs::create_dir_all(dst.parent().unwrap());
        let tmp = dst.with_extension("linking");
        let _ = std::fs::remove_file(&tmp);
        match std::fs::hard_link(src, &tmp).and_then(|_| std::fs::rename(&tmp, &dst)) {
            Ok(()) => out.push(r("linked", Some(src.display().to_string()))),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                out.push(r("failed", Some(format!("{e} (hardlinks only work on the same drive)"))));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("storage-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(p: &Path, n: usize) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![7u8; n]).unwrap();
    }

    fn cfg(data: &Path) -> Config {
        Config { data_dir: data.to_path_buf(), ..Config::default() }
    }

    fn runtime(dir: &Path, id: &str, last: i64) {
        put(&dir.join("runtimes").join(id).join("sd-server.exe"), 10);
        std::fs::write(dir.join("runtimes").join(id).join("runtime.json"), format!("{{\"id\":\"{id}\",\"sha256\":[],\"installed_ms\":0,\"last_used_ms\":{last}}}")).unwrap();
    }

    #[test]
    fn clear_cache_only_removes_rebuildable_things() {
        let d = tmp();
        let c = cfg(&d);
        // Sentinels that must survive.
        put(&c.models_dir().join("diffusion_models/m.gguf"), 100);
        put(&c.models_dir().join("vae/v.safetensors.part"), 50);
        put(&c.outputs_dir().join("a.png"), 30);
        put(&d.join("history.sqlite"), 20);
        put(&c.logs_dir().join("sd-server.log"), 5);
        runtime(&d, "rt-current", 300);
        runtime(&d, "rt-previous", 200);
        // Cache.
        put(&c.thumbs_dir().join("a_512.jpg"), 40);
        put(&c.logs_dir().join("sd-server.log.1"), 7);
        runtime(&d, "rt-old", 100);
        put(&d.join("runtimes/.unpack-x/f"), 3);

        let items = cache_items(&c, Some("rt-current"));
        let kinds: HashSet<&str> = items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, HashSet::from(["thumbnails", "rotated_log", "old_runtime", "unpack_leftover"]));
        let (freed, removed) = clear_cache(&c, &items);
        assert_eq!(removed.len(), 4);
        assert_eq!(freed, items.iter().map(|i| i.bytes).sum::<u64>());
        assert!(freed > 40 + 7 + 10 + 3, "thumbs, the rotated log, the old runtime and the leftover");
        for keep in ["models/diffusion_models/m.gguf", "models/vae/v.safetensors.part", "outputs/a.png", "history.sqlite", "logs/sd-server.log", "runtimes/rt-current/sd-server.exe", "runtimes/rt-previous/sd-server.exe"] {
            assert!(d.join(keep).exists(), "{keep} must survive clear cache");
        }
        assert!(!d.join("runtimes/rt-old").exists() && c.thumbs_dir().exists());
        // An item pointing anywhere else is ignored.
        let (f, r) = clear_cache(&c, &[CacheItem { kind: "thumbnails", path: "models".into(), bytes: 1 }]);
        assert_eq!((f, r.len()), (0, 0));
        assert!(d.join("models/diffusion_models/m.gguf").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn report_counts_hardlinks_once_and_sorts_categories() {
        let d = tmp();
        let c = cfg(&d);
        put(&c.outputs_dir().join("a.png"), 30);
        put(&c.models_dir().join("vae/x.bin"), 100);
        std::fs::hard_link(c.models_dir().join("vae/x.bin"), c.models_dir().join("vae/y.bin")).unwrap();
        put(&c.thumbs_dir().join("t.jpg"), 9);
        let reg = Registry::new(vec![]).unwrap();
        let r = report(&c, &reg, None, None);
        assert_eq!((r.usage.models, r.usage.images, r.usage.cache), (100, 30, 9));
        assert_eq!(r.usage.total, 139, "the hardlink counts once");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn copy_keeps_hardlinks_and_checks_sizes() {
        let (a, b) = (tmp(), tmp().join("new"));
        put(&a.join("models/x.bin"), 1000);
        std::fs::hard_link(a.join("models/x.bin"), a.join("models/y.bin")).unwrap();
        put(&a.join("outputs/p.png"), 10);
        assert!(check_move(&a, &a.join("inside")).is_err(), "not inside the current folder");
        #[cfg(windows)]
        {
            let upper = PathBuf::from(a.to_string_lossy().to_uppercase()).join("inner");
            assert!(check_move(&a, &upper).is_err(), "not inside it in another letter case either");
        }
        assert!(nested(Path::new("/x/data"), Path::new("/x/data/inner")) && !nested(Path::new("/x/data"), Path::new("/x/data2")));
        assert_eq!(check_move(&a, &b).unwrap(), 1010);
        copy_tree(&a, &b, &AtomicBool::new(false), &|_| {}).unwrap();
        assert_eq!(file_id(&b.join("models/x.bin")), file_id(&b.join("models/y.bin")), "still one file");
        assert!(b.join(MOVING_MARKER).exists(), "unfinished until the switch");
        // A second attempt over the unfinished one is allowed and starts clean.
        assert!(check_move(&a, &b).is_ok());
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(b.parent().unwrap());
    }

    #[test]
    fn thumbnails_are_capped_oldest_first() {
        let d = tmp();
        for i in 0..5 {
            put(&d.join(format!("{i}.jpg")), 100);
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(100 - i);
            std::fs::File::options().write(true).open(d.join(format!("{i}.jpg"))).unwrap().set_modified(t).unwrap();
        }
        assert_eq!(cap_thumbs(&d, 250), 300);
        assert!(!d.join("0.jpg").exists() && d.join("4.jpg").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn keeps_active_and_previous_runtime() {
        let inst = vec![("a".to_string(), 1), ("b".to_string(), 3), ("c".to_string(), 2)];
        assert_eq!(runtimes_to_keep(&inst, Some("a")), HashSet::from(["a".to_string(), "b".to_string()]));
        assert_eq!(runtimes_to_keep(&inst, None), HashSet::from(["b".to_string(), "c".to_string()]));
    }
}
