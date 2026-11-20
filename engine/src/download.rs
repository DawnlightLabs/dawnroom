//! Resumable downloads, shared by model pulls (Hugging Face) and runtime installs (GitHub).
//!
//! Bytes go to a `.part` file with a small sidecar (`<file>.part.json`) that records how many of
//! them are known to be on disk (`synced_bytes`: the `.part` is `sync_all`ed every 64 MB or 5 s,
//! then the sidecar is updated). A resume trims the `.part` back to that number, so a power cut never leaves
//! garbage inside the file and never forces a restart from zero. Network trouble (no connection,
//! a dropped or stalled body, 5xx, 429) is retried with backoff for up to 15 minutes; a response
//! that isn't the file (a captive portal's HTML, the wrong size, an unknown host) is refused
//! before a byte is written. The caller verifies the sha256 before moving the file into place.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::backend::BackendError;

/// The `.part` is synced to disk (and the sidecar updated) after every this many bytes…
pub const SYNC_EVERY: u64 = 64 << 20;
/// …or this often, whichever comes first, so a small file (a 30 MB runtime zip) or a slow
/// connection doesn't lose everything to a crash before its first 64 MB.
pub const SYNC_SECS: u64 = 5;

pub fn http_client() -> reqwest::Client {
    // 30 s without a byte = the connection is gone (Windows keeps a socket open through a Wi-Fi drop).
    http_client_with(Duration::from_secs(30))
}

/// A client whose body reads give up after `read_timeout` without a byte (tests use a short one).
pub fn http_client_with(read_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("dawnroom-engine/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(read_timeout)
        .build()
        .expect("http client")
}

/// sha256 of a file as lowercase hex. Blocking; run it off the async threads.
pub fn sha256_file(path: &Path, mut on_bytes: impl FnMut(u64)) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    let mut done = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        done += n as u64;
        on_bytes(done);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn err(code: &str, message: impl Into<String>) -> BackendError {
    BackendError::with_message(code, message)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

// ---------- sidecar ----------

/// `<file>.part.json`: which download the `.part` belongs to and how much of it is safely on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sidecar {
    pub url: String,
    pub size: u64,
    pub sha256: String,
    /// Bytes at the start of the `.part` that were `sync_all`ed before this was written.
    pub synced_bytes: u64,
    /// When the sidecar was last written (ms since the epoch): "last written" for paused downloads.
    pub updated_ms: i64,
    /// The model whose pull wrote this `.part` (shared encoders and VAEs belong to the pull that
    /// started them, not to every model listing them). Absent in sidecars from before 6b-1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

pub fn sidecar_path(part: &Path) -> PathBuf {
    let mut s = part.as_os_str().to_owned();
    s.push(".json");
    PathBuf::from(s)
}

/// The model whose pull wrote this `.part`, if its sidecar says.
pub fn part_owner(part: &Path) -> Option<String> {
    read_sidecar(part)?.model
}

pub fn read_sidecar(part: &Path) -> Option<Sidecar> {
    serde_json::from_slice(&std::fs::read(sidecar_path(part)).ok()?).ok()
}

/// Written to a temp file, synced, then renamed over the old one: the sidecar is either the old
/// or the new version after a power cut, never half of one.
fn write_sidecar(part: &Path, s: &Sidecar) -> std::io::Result<()> {
    let path = sidecar_path(part);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut f = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, &serde_json::to_vec(s).unwrap())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path)
}

/// Removes a `.part` and its sidecar (a paused download the user discarded).
pub fn remove_part(part: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(sidecar_path(part));
    match std::fs::remove_file(part) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// How many bytes of `part` can be trusted for this download, given its sidecar:
/// - sidecar matches (url, size, sha256): its `synced_bytes` (anything past that may be garbage);
/// - sidecar for a different file, or unreadable: 0;
/// - no sidecar (a `.part` from before sidecars existed): its length minus the last 64 MB.
pub fn trusted_bytes(part: &Path, url: &str, size: u64, sha256: &str) -> u64 {
    let len = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if len == 0 || len > size {
        return 0;
    }
    if !sidecar_path(part).exists() {
        return len.saturating_sub(SYNC_EVERY);
    }
    match read_sidecar(part) {
        Some(s) if s.url == url && s.size == size && s.sha256.eq_ignore_ascii_case(sha256) => s.synced_bytes.min(len),
        _ => 0,
    }
}

// ---------- response checks ----------

/// Hosts a download may be redirected to without the manifest naming them: Hugging Face's and
/// GitHub's own domains and CDNs (e.g. `cas-bridge.xethub.hf.co`, `release-assets.githubusercontent.com`).
const KNOWN_HOSTS: [&str; 4] = ["huggingface.co", "hf.co", "github.com", "githubusercontent.com"];

pub fn host_allowed(manifest_url: &str, final_host: &str) -> bool {
    let named = reqwest::Url::parse(manifest_url).ok().and_then(|u| u.host_str().map(str::to_lowercase));
    let h = final_host.to_lowercase();
    named.as_deref() == Some(h.as_str()) || KNOWN_HOSTS.iter().any(|d| h == *d || h.ends_with(&format!(".{d}")))
}

const INTERCEPTED: &str = "answered instead of the file (a captive portal, proxy or firewall). Sign in to the network or check the proxy, then";

/// The parts of a response that say whether it's the file.
#[derive(Debug, Default)]
pub struct Answer<'a> {
    /// Host of the final URL, after redirects.
    pub host: &'a str,
    pub status: u16,
    pub content_type: Option<&'a str>,
    pub content_length: Option<u64>,
    pub content_range: Option<&'a str>,
}

/// Whether this response is the file we asked for. Checked before a byte is written.
pub fn check_response(manifest_url: &str, size: u64, a: &Answer<'_>, source: &str, again: &str) -> Result<(), BackendError> {
    let Answer { host: final_host, status, content_type, content_length, content_range } = *a;
    let reject = |why: String| {
        eprintln!("[engine] refused a download response: {why}");
        Err(err("network_intercepted", format!("Something between you and {source} {INTERCEPTED} {} again.", again.to_lowercase())))
    };
    if content_type.is_some_and(|t| t.trim().to_ascii_lowercase().starts_with("text/html")) {
        return reject(format!("Content-Type {content_type:?}"));
    }
    if !host_allowed(manifest_url, final_host) {
        return reject(format!("redirected to {final_host}"));
    }
    if status == 200 && content_length.is_some_and(|n| n != size) {
        return reject(format!("Content-Length {content_length:?}, expected {size}"));
    }
    if status == 206 {
        let total = content_range.and_then(|v| v.rsplit('/').next()).and_then(|t| t.trim().parse::<u64>().ok());
        if total.is_some_and(|t| t != size) {
            return reject(format!("Content-Range {content_range:?}, expected a total of {size}"));
        }
    }
    Ok(())
}

/// Where a 206 says its body starts.
fn range_start(content_range: Option<&str>) -> Option<u64> {
    content_range?.strip_prefix("bytes ")?.split('-').next()?.trim().parse().ok()
}

/// `Retry-After` in seconds (the HTTP-date form falls back to our own backoff).
pub fn retry_after(v: Option<&str>) -> Option<Duration> {
    v?.trim().parse::<u64>().ok().map(Duration::from_secs)
}

fn is_disk_full(e: &std::io::Error) -> bool {
    // ERROR_HANDLE_DISK_FULL (39) and ERROR_DISK_FULL (112) on Windows; ENOSPC (28) elsewhere.
    let code = e.raw_os_error();
    e.kind() == std::io::ErrorKind::StorageFull || if cfg!(windows) { matches!(code, Some(39 | 112)) } else { code == Some(28) }
}

fn disk_full(part: &Path, have: u64, size: u64, again: &str) -> BackendError {
    let free = crate::disk::space(part).map(|s| s.free_bytes).unwrap_or(0);
    let need = size.saturating_sub(have);
    err(
        "disk_full",
        format!(
            "The disk filled up: this file still needs {} but the drive has {} free. Free up space and {} again; the downloaded part is kept.",
            crate::disk::human(need),
            crate::disk::human(free),
            again.to_lowercase()
        ),
    )
}

// ---------- the download ----------

/// How long to keep retrying. Production: 2 s, 4 s, 8 s… capped at 60 s, for 15 minutes.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    pub first: Duration,
    pub cap: Duration,
    /// The longest run of failures without a byte of progress before giving up.
    pub budget: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self { first: Duration::from_secs(2), cap: Duration::from_secs(60), budget: Duration::from_secs(15 * 60) }
    }
}

/// Sent while a download waits to retry.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Wait {
    /// connection | busy | server
    pub kind: &'static str,
    /// Plain words: "No connection", "Hugging Face is busy".
    pub reason: String,
    /// When the next attempt starts (ms since the epoch).
    pub retry_at_ms: i64,
    /// How many attempts have failed in a row.
    pub attempt: u32,
}

/// One resumable download: `url` into `part` until it holds `size` bytes.
pub struct Fetch<'a> {
    pub url: &'a str,
    pub part: &'a Path,
    pub size: u64,
    /// Recorded in the sidecar, so a `.part` is only resumed for the same file.
    pub sha256: &'a str,
    pub cancel: &'a AtomicBool,
    /// Names the server in messages ("Hugging Face", "GitHub").
    pub source: &'a str,
    /// How to retry, in messages ("Pull again").
    pub again: &'a str,
    pub retry: Retry,
    /// The model this download is for, recorded in the sidecar (`None` for runtimes).
    pub owner: Option<&'a str>,
}

/// What an attempt ended with, short of success.
enum Stop {
    /// Give up now with this error (cancel, refused, disk, 4xx).
    Fail(BackendError),
    /// Throw the `.part` away and start the file again (416, or a 206 at the wrong offset).
    Restart(&'static str),
    /// Network trouble: wait and resume.
    Retry { kind: &'static str, reason: String, after: Option<Duration> },
}

/// Downloads `job.url` into `job.part`, resuming what's safely there and retrying network trouble.
/// `on(bytes_on_disk, speed_bps, wait)` reports progress about four times a second, and once per
/// wait with `Some(wait)`. On cancel (also during a wait) the `.part` is synced and kept, and the
/// error code is `cancelled`.
pub async fn fetch_to_part(http: &reqwest::Client, job: Fetch<'_>, mut on: impl FnMut(u64, u64, Option<Wait>)) -> Result<(), BackendError> {
    let _awake = crate::awake::hold();
    if let Some(dir) = job.part.parent() {
        std::fs::create_dir_all(dir).map_err(|e| err("disk_error", format!("Couldn't create {} ({e}).", dir.display())))?;
    }
    let mut start = trusted_bytes(job.part, job.url, job.size, job.sha256);
    if start == job.size && std::fs::metadata(job.part).map(|m| m.len()).ok() == Some(job.size) {
        return Ok(());
    }
    let mut backoff = job.retry.first;
    let mut failing_since: Option<Instant> = None;
    let mut attempt = 0u32;
    let mut restarted = false;
    loop {
        if job.cancel.load(Ordering::SeqCst) {
            return Err(cancelled(job.again));
        }
        let before = start;
        let stop = match try_once(http, &job, &mut start, &mut on).await {
            Ok(()) => return Ok(()),
            Err(s) => s,
        };
        match stop {
            Stop::Fail(e) => return Err(e),
            Stop::Restart(why) => {
                if restarted {
                    return Err(err("download_failed", format!("{} keeps refusing the download ({why}). {} in a minute.", job.source, job.again)));
                }
                eprintln!("[engine] {why}: starting {} again from zero", job.part.display());
                restarted = true;
                start = 0;
                reset_part(&job).map_err(|e| err("disk_error", format!("Couldn't reset {} ({e}).", job.part.display())))?;
            }
            Stop::Retry { kind, reason, after } => {
                if start > before {
                    // Progress since the last failure: this is a new run of trouble.
                    failing_since = None;
                    backoff = job.retry.first;
                    attempt = 0;
                }
                let since = *failing_since.get_or_insert_with(Instant::now);
                let left = job.retry.budget.saturating_sub(since.elapsed());
                if left.is_zero() {
                    return Err(err("download_failed", interrupted(&job, start, &reason)));
                }
                let delay = after.unwrap_or(backoff).min(left);
                backoff = (backoff * 2).min(job.retry.cap);
                attempt += 1;
                eprintln!("[engine] {}: {reason}; retry {attempt} in {:.1}s from byte {start}", job.part.display(), delay.as_secs_f32());
                // Re-sent once a second, so every client's countdown ticks without a clock of its own.
                let wait = Wait { kind, reason, retry_at_ms: now_ms() + delay.as_millis() as i64, attempt };
                let until = Instant::now() + delay;
                loop {
                    on(start, 0, Some(wait.clone()));
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    if sleep_or_cancel(left.min(Duration::from_secs(1)), job.cancel).await {
                        return Err(cancelled(job.again));
                    }
                }
            }
        }
    }
}

/// "Connection interrupted (no connection for 15 minutes). Pull again to continue the download;
/// 76% of ae.safetensors is saved (255 MB of 335 MB)."
fn interrupted(job: &Fetch<'_>, have: u64, reason: &str) -> String {
    let name = job.part.file_name().map(|n| n.to_string_lossy().trim_end_matches(".part").to_string()).unwrap_or_default();
    let pct = (have * 100).checked_div(job.size).unwrap_or(0);
    format!(
        "Connection interrupted ({} for {} minutes). {} to continue the download; {pct}% of {name} is saved ({} of {}).",
        reason.to_lowercase(),
        job.retry.budget.as_secs().div_ceil(60),
        job.again,
        crate::disk::human(have),
        crate::disk::human(job.size)
    )
}

fn cancelled(again: &str) -> BackendError {
    err("cancelled", format!("The download was cancelled. {again} to resume; the downloaded part is kept."))
}

/// Sleeps `d`, waking early (true) if `cancel` is set.
async fn sleep_or_cancel(d: Duration, cancel: &AtomicBool) -> bool {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if cancel.load(Ordering::SeqCst) {
            return true;
        }
        tokio::time::sleep((end - Instant::now()).min(Duration::from_millis(100))).await;
    }
    cancel.load(Ordering::SeqCst)
}

/// Resolves when `cancel` is set.
async fn until_cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn sidecar(job: &Fetch<'_>, synced: u64) -> Sidecar {
    Sidecar { url: job.url.to_string(), size: job.size, sha256: job.sha256.to_lowercase(), synced_bytes: synced, updated_ms: now_ms(), model: job.owner.map(str::to_string) }
}

fn reset_part(job: &Fetch<'_>) -> std::io::Result<()> {
    write_sidecar(job.part, &sidecar(job, 0))?;
    std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(job.part).map(|_| ())
}

/// One request: send, check, write. `start` is the trusted length going in and the synced length
/// coming out (also after a failure).
async fn try_once(http: &reqwest::Client, job: &Fetch<'_>, start: &mut u64, on: &mut impl FnMut(u64, u64, Option<Wait>)) -> Result<(), Stop> {
    let (source, again) = (job.source, job.again);
    let mut req = http.get(job.url);
    if *start > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={start}-"));
    }
    let sent = tokio::select! {
        r = req.send() => r,
        _ = until_cancelled(job.cancel) => return Err(Stop::Fail(cancelled(again))),
    };
    let mut resp = match sent {
        Ok(r) => r,
        Err(e) if e.is_builder() || e.is_redirect() => return Err(Stop::Fail(err("download_failed", format!("Couldn't start the download from {source} ({e}).")))),
        Err(e) => {
            eprintln!("[engine] request failed: {e}");
            return Err(Stop::Retry { kind: "connection", reason: "No connection".into(), after: None });
        }
    };
    let status = resp.status().as_u16();
    let header = |name| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(String::from);
    let content_range = header(reqwest::header::CONTENT_RANGE);
    match status {
        200 | 206 => {}
        416 => return Err(Stop::Restart("HTTP 416: the partial file is longer than the file")),
        429 | 503 => return Err(Stop::Retry { kind: "busy", reason: format!("{source} is busy"), after: retry_after(header(reqwest::header::RETRY_AFTER).as_deref()) }),
        500..=599 => return Err(Stop::Retry { kind: "server", reason: format!("{source} had a problem (HTTP {status})"), after: None }),
        401 | 403 | 404 => {
            let what = if status == 404 { "doesn't have this file any more" } else { "refused the download" };
            return Err(Stop::Fail(err("download_refused", format!("{source} {what} (HTTP {status}). Check for a Dawnroom update; the registry may have moved."))));
        }
        other => return Err(Stop::Fail(err("download_refused", format!("{source} answered HTTP {other}. {again} in a minute.")))),
    }
    let host = resp.url().host_str().unwrap_or("").to_string();
    let content_type = header(reqwest::header::CONTENT_TYPE);
    let answer = Answer { host: &host, status, content_type: content_type.as_deref(), content_length: resp.content_length(), content_range: content_range.as_deref() };
    check_response(job.url, job.size, &answer, source, again).map_err(Stop::Fail)?;
    let mut have = if status == 200 { 0 } else { *start };
    if status == 206 && range_start(content_range.as_deref()) != Some(*start) {
        return Err(Stop::Restart("a range answer at the wrong offset"));
    }

    // Accepted: record the download, then trim the .part to what we trust and append from there.
    let disk = |e: std::io::Error| err("disk_error", format!("Couldn't write {} ({e}).", job.part.display()));
    write_sidecar(job.part, &sidecar(job, have)).map_err(|e| Stop::Fail(disk(e)))?;
    let mut file = tokio::fs::OpenOptions::new().create(true).write(true).truncate(false).open(job.part).await.map_err(|e| Stop::Fail(disk(e)))?;
    file.set_len(have).await.map_err(|e| Stop::Fail(disk(e)))?;
    file.seek(std::io::SeekFrom::Start(have)).await.map_err(|e| Stop::Fail(disk(e)))?;
    *start = have;
    eprintln!("[engine] downloading {} from byte {have} (HTTP {status})", job.part.display());
    let mut out = tokio::io::BufWriter::with_capacity(1 << 20, file);

    // Flush + sync the .part, then record it in the sidecar.
    async fn sync(out: &mut tokio::io::BufWriter<tokio::fs::File>, job: &Fetch<'_>, have: u64) -> std::io::Result<()> {
        out.flush().await?;
        out.get_mut().sync_all().await?;
        write_sidecar(job.part, &sidecar(job, have))
    }

    let (mut last, mut last_bytes, mut speed, mut since_sync, mut synced_at) = (Instant::now(), have, 0u64, 0u64, Instant::now());
    let ended: Option<Stop> = loop {
        let next = tokio::select! {
            c = resp.chunk() => c,
            _ = until_cancelled(job.cancel) => break Some(Stop::Fail(cancelled(again))),
        };
        let chunk = match next {
            Ok(Some(c)) => c,
            Ok(None) => break None,
            Err(e) => {
                eprintln!("[engine] body broke off at byte {have}: {e}");
                let reason = if e.is_timeout() { "The connection stalled" } else { "The connection dropped" };
                break Some(Stop::Retry { kind: "connection", reason: reason.into(), after: None });
            }
        };
        if have + chunk.len() as u64 > job.size {
            break Some(Stop::Restart("the server sent more than the file's size"));
        }
        if let Err(e) = out.write_all(&chunk).await {
            break Some(Stop::Fail(if is_disk_full(&e) { disk_full(job.part, have, job.size, again) } else { err("disk_error", format!("Couldn't write the download ({e}).")) }));
        }
        have += chunk.len() as u64;
        since_sync += chunk.len() as u64;
        if since_sync >= SYNC_EVERY || synced_at.elapsed() >= Duration::from_secs(SYNC_SECS) {
            if let Err(e) = sync(&mut out, job, have).await {
                break Some(Stop::Fail(if is_disk_full(&e) { disk_full(job.part, have, job.size, again) } else { disk(e) }));
            }
            *start = have;
            since_sync = 0;
            synced_at = Instant::now();
        }
        let dt = last.elapsed();
        if dt >= Duration::from_millis(250) {
            speed = ((have - last_bytes) as f64 / dt.as_secs_f64()) as u64;
            (last, last_bytes) = (Instant::now(), have);
            on(have, speed, None);
        }
    };

    // Every way out syncs what was written, so the next attempt (or a restart) resumes from it.
    // A failed sync (disk full while flushing) keeps the older, smaller `start`.
    let synced = sync(&mut out, job, have).await;
    if let Err(e) = &synced {
        eprintln!("[engine] couldn't sync {} ({e})", job.part.display());
    } else {
        *start = have;
    }
    on(*start, if ended.is_none() { speed } else { 0 }, None);
    match ended {
        Some(s) => Err(s),
        None => match synced {
            Err(e) if is_disk_full(&e) => Err(Stop::Fail(disk_full(job.part, have, job.size, again))),
            Err(e) => Err(Stop::Fail(disk(e))),
            Ok(()) if have == job.size => Ok(()),
            Ok(()) => Err(Stop::Retry { kind: "connection", reason: "The download ended early".into(), after: None }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("dl-unit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn trusts_only_synced_bytes_of_a_matching_sidecar() {
        let d = tmp();
        let part = d.join("x.part");
        std::fs::write(&part, vec![1u8; 1000]).unwrap();
        // No sidecar: an old .part keeps its bytes minus the last 64 MB (here: all of it goes).
        assert_eq!(trusted_bytes(&part, "u", 2000, "ab"), 0);
        let s = Sidecar { url: "u".into(), size: 2000, sha256: "ab".into(), synced_bytes: 600, updated_ms: 1, model: None };
        write_sidecar(&part, &s).unwrap();
        assert_eq!(trusted_bytes(&part, "u", 2000, "AB"), 600);
        assert_eq!(trusted_bytes(&part, "other", 2000, "ab"), 0);
        assert_eq!(trusted_bytes(&part, "u", 3000, "ab"), 0);
        assert_eq!(trusted_bytes(&part, "u", 2000, "cd"), 0);
        // Longer than the file: start again.
        assert_eq!(trusted_bytes(&part, "u", 999, "ab"), 0);
        std::fs::write(sidecar_path(&part), b"{not json").unwrap();
        assert_eq!(trusted_bytes(&part, "u", 2000, "ab"), 0);
        remove_part(&part).unwrap();
        assert!(!part.exists() && !sidecar_path(&part).exists());
    }

    #[test]
    fn legacy_part_loses_its_last_64_mb() {
        let d = tmp();
        let part = d.join("old.part");
        let f = std::fs::File::create(&part).unwrap();
        f.set_len(SYNC_EVERY + 5000).unwrap();
        assert_eq!(trusted_bytes(&part, "u", SYNC_EVERY * 3, "ab"), 5000);
    }

    #[test]
    fn responses_that_are_not_the_file_are_refused() {
        let ok = |host, status, content_type, content_length, content_range| {
            let a = Answer { host, status, content_type, content_length, content_range };
            check_response("https://huggingface.co/a/b/resolve/x/f", 100, &a, "Hugging Face", "Pull again")
        };
        assert!(ok("huggingface.co", 200, Some("application/octet-stream"), Some(100), None).is_ok());
        assert!(ok("cas-bridge.xethub.hf.co", 206, None, Some(50), Some("bytes 50-99/100")).is_ok());
        assert!(ok("cdn-lfs-us-1.hf.co", 206, None, None, Some("bytes 50-99/*")).is_ok());
        let e = ok("huggingface.co", 200, Some("text/html; charset=utf-8"), Some(100), None).unwrap_err();
        assert_eq!(e.code, "network_intercepted");
        assert!(e.message.contains("captive portal") && e.message.contains("pull again"), "{}", e.message);
        assert!(ok("huggingface.co", 200, None, Some(99), None).is_err());
        assert!(ok("huggingface.co", 206, None, None, Some("bytes 50-99/101")).is_err());
        assert!(ok("login.hotel-wifi.example", 200, None, Some(100), None).is_err());
        assert!(ok("evilhuggingface.co", 200, None, Some(100), None).is_err());
        assert!(host_allowed("https://github.com/x/y/releases/download/t/a.zip", "release-assets.githubusercontent.com"));
        assert!(host_allowed("http://127.0.0.1:9/x", "127.0.0.1"));
    }

    #[test]
    fn retry_after_and_range_parsing() {
        assert_eq!(retry_after(Some("3")), Some(Duration::from_secs(3)));
        assert_eq!(retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")), None);
        assert_eq!(range_start(Some("bytes 1000-1999/2000")), Some(1000));
        assert_eq!(range_start(None), None);
    }

    #[test]
    fn disk_full_is_recognised() {
        let code = if cfg!(windows) { 112 } else { 28 };
        assert!(is_disk_full(&std::io::Error::from_raw_os_error(code)));
        assert!(!is_disk_full(&std::io::Error::other("x")));
        let e = disk_full(&std::env::temp_dir().join("x.part"), 1_000_000_000, 3_000_000_000, "Pull again");
        assert_eq!(e.code, "disk_full");
        assert!(e.message.contains("2.0 GB") && e.message.contains("free"), "{}", e.message);
    }
}
