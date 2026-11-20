//! The shared downloader against a local server with fault modes: each test shows the download
//! resumed to a verified file, waited and retried, or failed at once with the right code.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::Response;
use engine::download::{Fetch, Retry, Wait, fetch_to_part, read_sidecar, sidecar_path, trusted_bytes};
use sha2::{Digest, Sha256};

fn body(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("dl-faults-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// What the server saw: (path, Range start) per request.
type Seen = Arc<Mutex<Vec<(String, u64)>>>;

/// Serves `data` at every path, honouring `Range: bytes=N-`, with a fault on the first request(s)
/// chosen by the path:
/// - `/drop-<n>`: the first response breaks off after n bytes;
/// - `/503x2`: 503 twice, then OK;
/// - `/429`: 429 with Retry-After: 3 once;
/// - `/503`: always 503;
/// - `/html`: a 200 with an HTML body (a captive portal);
/// - `/wronglen`: a 200 whose Content-Length is one byte short;
/// - `/stall-<n>`: the first response stops after n bytes and goes quiet for 5 s;
/// - `/416`: 416 on the first ranged request;
/// - `/404`: always 404;
/// - `/redirect`: 302 to the same server under another host name (localhost).
async fn server(data: Vec<u8>) -> (String, Seen) {
    let seen: Seen = Default::default();
    let count = Arc::new(AtomicUsize::new(0));
    let (data, log) = (Arc::new(data), seen.clone());
    let app = axum::Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
        let (data, log, count) = (data.clone(), log.clone(), count.clone());
        async move {
            let path = uri.path().to_string();
            let start: usize = headers
                .get(header::RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("bytes="))
                .and_then(|v| v.trim_end_matches('-').parse().ok())
                .unwrap_or(0);
            log.lock().unwrap().push((path.clone(), start as u64));
            let n = count.fetch_add(1, Ordering::SeqCst);
            let status = |s: StatusCode| Response::builder().status(s).body(axum::body::Body::empty()).unwrap();
            let mut cut: Option<(usize, bool)> = None; // (after bytes, stall instead of drop)
            match path.as_str() {
                "/503x2" if n < 2 => return status(StatusCode::SERVICE_UNAVAILABLE),
                "/503" => return status(StatusCode::SERVICE_UNAVAILABLE),
                "/429" if n == 0 => return Response::builder().status(429).header(header::RETRY_AFTER, "3").body(axum::body::Body::empty()).unwrap(),
                "/404" => return status(StatusCode::NOT_FOUND),
                "/416" if n == 0 && start > 0 => return status(StatusCode::RANGE_NOT_SATISFIABLE),
                "/html" => {
                    return Response::builder().header(header::CONTENT_TYPE, "text/html; charset=utf-8").body(axum::body::Body::from("<html>Sign in to Hotel Wi-Fi</html>")).unwrap();
                }
                "/wronglen" => return Response::builder().body(axum::body::Body::from(data[..data.len() - 1].to_vec())).unwrap(),
                "/redirect" => {
                    let port = headers.get(header::HOST).and_then(|h| h.to_str().ok()).and_then(|h| h.rsplit(':').next()).unwrap_or("0").to_string();
                    return Response::builder().status(302).header(header::LOCATION, format!("http://localhost:{port}/ok")).body(axum::body::Body::empty()).unwrap();
                }
                p if n == 0 && p.starts_with("/drop-") => cut = Some((p[6..].parse().unwrap(), false)),
                p if n == 0 && p.starts_with("/stall-") => cut = Some((p[7..].parse().unwrap(), true)),
                _ => {}
            }
            let len = data.len();
            let stream = async_stream::stream! {
                let mut at = start;
                while at < len {
                    let mut end = (at + (256 << 10)).min(len);
                    if let Some((c, stall)) = cut && end >= c {
                        end = c;
                        if end > at { yield Ok(data[at..end].to_vec()); }
                        if stall { tokio::time::sleep(Duration::from_secs(5)).await; }
                        yield Err(std::io::Error::other("fault: connection dropped"));
                        return;
                    }
                    yield Ok::<_, std::io::Error>(data[at..end].to_vec());
                    at = end;
                }
            };
            let mut b = Response::builder().header(header::CONTENT_LENGTH, len - start).header(header::CONTENT_TYPE, "application/octet-stream");
            if start > 0 {
                b = b.status(206).header(header::CONTENT_RANGE, format!("bytes {start}-{}/{len}", len - 1));
            }
            b.body(axum::body::Body::from_stream(stream)).unwrap()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, seen)
}

const FAST: Retry = Retry { first: Duration::from_millis(50), cap: Duration::from_millis(200), budget: Duration::from_secs(20) };

struct Run {
    result: Result<(), engine::backend::BackendError>,
    waits: Vec<(u64, Wait)>,
    secs: f32,
}

async fn fetch(http: &reqwest::Client, url: &str, part: &Path, data: &[u8], retry: Retry, cancel: &AtomicBool) -> Run {
    let t0 = Instant::now();
    let mut waits = Vec::new();
    let digest = sha(data);
    let job = Fetch { url, part, size: data.len() as u64, sha256: &digest, cancel, source: "Hugging Face", again: "Pull again", retry, owner: None };
    let result = fetch_to_part(http, job, |b, _, w| {
        if let Some(w) = w {
            // Every wait comes after a sync: the sidecar already records the bytes reported.
            if b > 0 {
                assert_eq!(read_sidecar(part).map(|s| s.synced_bytes), Some(b), "sidecar behind at a wait");
            }
            // The engine repeats a wait once a second (ticking countdowns): keep one per attempt.
            if waits.last().map(|(_, l): &(u64, Wait)| l.attempt != w.attempt || l.retry_at_ms != w.retry_at_ms).unwrap_or(true) {
                waits.push((b, w));
            }
        }
    })
    .await;
    Run { result, waits, secs: t0.elapsed().as_secs_f32() }
}

fn assert_verified(part: &Path, data: &[u8]) {
    let got = std::fs::read(part).unwrap();
    assert_eq!(got.len(), data.len());
    assert_eq!(sha(&got), sha(data), "sha256 of the finished .part");
}

fn code(r: &Run) -> &str {
    r.result.as_ref().err().map(|e| e.code.as_str()).unwrap_or("ok")
}

#[tokio::test]
async fn dropped_connection_resumes_from_the_synced_bytes() {
    // 70 MB so one 64 MB sync happens before the drop at 66 MB.
    let data = body(70 << 20);
    let (base, seen) = server(data.clone()).await;
    let part = tmp().join("f.part");
    let r = fetch(&engine::download::http_client(), &format!("{base}/drop-{}", 66 << 20), &part, &data, FAST, &AtomicBool::new(false)).await;
    assert_eq!(code(&r), "ok", "{:?}", r.result);
    assert_eq!(r.waits.len(), 1);
    assert_eq!(r.waits[0].1.kind, "connection");
    // The server drops after sending 66 MB; on a slow machine the last bytes in flight may never arrive,
    // so the wait can come a little earlier. Whatever was received was synced and kept, and the resume starts there.
    let at = r.waits[0].0;
    assert!(at > 64 << 20 && at <= 66 << 20, "dropped near 66 MB, got {at}");
    assert_eq!(seen.lock().unwrap().iter().map(|s| s.1).collect::<Vec<_>>(), [0, at]);
    assert_verified(&part, &data);
    assert_eq!(read_sidecar(&part).unwrap().synced_bytes, data.len() as u64);
}

#[tokio::test]
async fn busy_server_is_retried_with_backoff() {
    let data = body(300_000);
    let (base, seen) = server(data.clone()).await;
    let part = tmp().join("f.part");
    let r = fetch(&engine::download::http_client(), &format!("{base}/503x2"), &part, &data, FAST, &AtomicBool::new(false)).await;
    assert_eq!(code(&r), "ok", "{:?}", r.result);
    assert_eq!(r.waits.len(), 2);
    assert!(r.waits.iter().all(|(_, w)| w.kind == "busy" && w.reason == "Hugging Face is busy"));
    assert_eq!((r.waits[0].1.attempt, r.waits[1].1.attempt), (1, 2));
    assert_eq!(seen.lock().unwrap().len(), 3);
    assert_verified(&part, &data);
}

#[tokio::test]
async fn retry_after_is_honoured() {
    let data = body(100_000);
    let (base, _) = server(data.clone()).await;
    let part = tmp().join("f.part");
    let r = fetch(&engine::download::http_client(), &format!("{base}/429"), &part, &data, FAST, &AtomicBool::new(false)).await;
    assert_eq!(code(&r), "ok", "{:?}", r.result);
    assert_eq!(r.waits.len(), 1);
    assert!(r.secs >= 2.9, "waited only {:.2}s, Retry-After said 3", r.secs);
    assert_verified(&part, &data);
}

#[tokio::test]
async fn stalled_body_times_out_and_resumes() {
    let data = body(2 << 20);
    let (base, seen) = server(data.clone()).await;
    let part = tmp().join("f.part");
    let http = engine::download::http_client_with(Duration::from_secs(1));
    let r = fetch(&http, &format!("{base}/stall-{}", 1 << 20), &part, &data, FAST, &AtomicBool::new(false)).await;
    assert_eq!(code(&r), "ok", "{:?}", r.result);
    assert_eq!(r.waits[0].1.reason, "The connection stalled");
    assert!(r.secs < 4.0, "the stall should time out after 1 s, took {:.1}s", r.secs);
    assert_eq!(seen.lock().unwrap()[1].1, 1 << 20);
    assert_verified(&part, &data);
}

/// Leaves a half-done `.part` (with its sidecar) and returns its bytes and sidecar text.
fn half_part(part: &Path, url: &str, data: &[u8]) -> (Vec<u8>, String) {
    let half = data.len() / 2;
    std::fs::write(part, &data[..half]).unwrap();
    let s = engine::download::Sidecar { url: url.into(), size: data.len() as u64, sha256: sha(data), synced_bytes: half as u64, updated_ms: 1, model: None };
    std::fs::write(sidecar_path(part), serde_json::to_vec(&s).unwrap()).unwrap();
    (std::fs::read(part).unwrap(), std::fs::read_to_string(sidecar_path(part)).unwrap())
}

#[tokio::test]
async fn html_and_wrong_length_are_refused_before_writing() {
    let data = body(200_000);
    let (base, seen) = server(data.clone()).await;
    for (path, fresh) in [("/html", false), ("/wronglen", true), ("/redirect", false)] {
        let part = tmp().join("f.part");
        let url = format!("{base}{path}");
        // /wronglen starts fresh (a 200 is checked by Content-Length); the others hold a .part
        // that must come out untouched.
        let before = (!fresh).then(|| half_part(&part, &url, &data));
        let r = fetch(&engine::download::http_client(), &url, &part, &data, FAST, &AtomicBool::new(false)).await;
        assert_eq!(code(&r), "network_intercepted", "{path}: {:?}", r.result);
        let msg = &r.result.as_ref().unwrap_err().message;
        assert!(msg.contains("captive portal, proxy or firewall") && msg.contains("pull again"), "{msg}");
        assert!(r.waits.is_empty(), "{path} must fail at once");
        match before {
            Some((bytes, side)) => {
                assert_eq!(std::fs::read(&part).unwrap(), bytes, "{path}: .part touched");
                assert_eq!(std::fs::read_to_string(sidecar_path(&part)).unwrap(), side, "{path}: sidecar touched");
            }
            None => assert!(!part.exists(), "{path}: nothing should be written"),
        }
    }
    let n = |p: &str| seen.lock().unwrap().iter().filter(|s| s.0 == p).count();
    assert_eq!((n("/html"), n("/wronglen"), n("/redirect"), n("/ok")), (1, 1, 1, 1));
}

#[tokio::test]
async fn range_not_satisfiable_starts_the_file_again() {
    let data = body(300_000);
    let (base, seen) = server(data.clone()).await;
    let part = tmp().join("f.part");
    let url = format!("{base}/416");
    half_part(&part, &url, &data);
    let r = fetch(&engine::download::http_client(), &url, &part, &data, FAST, &AtomicBool::new(false)).await;
    assert_eq!(code(&r), "ok", "{:?}", r.result);
    assert_eq!(seen.lock().unwrap().iter().map(|s| s.1).collect::<Vec<_>>(), [150_000, 0]);
    assert_verified(&part, &data);
}

#[tokio::test]
async fn not_found_fails_at_once() {
    let data = body(1000);
    let (base, seen) = server(data.clone()).await;
    let part = tmp().join("f.part");
    let r = fetch(&engine::download::http_client(), &format!("{base}/404"), &part, &data, FAST, &AtomicBool::new(false)).await;
    assert_eq!(code(&r), "download_refused");
    assert!(r.waits.is_empty());
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn gives_up_after_the_budget_with_the_resume_message() {
    let data = body(1000);
    let (base, _) = server(data.clone()).await;
    let part = tmp().join("f.part");
    let retry = Retry { first: Duration::from_millis(50), cap: Duration::from_millis(100), budget: Duration::from_millis(600) };
    let r = fetch(&engine::download::http_client(), &format!("{base}/503"), &part, &data, retry, &AtomicBool::new(false)).await;
    assert_eq!(code(&r), "download_failed");
    let msg = &r.result.as_ref().unwrap_err().message;
    assert!(msg.starts_with("Connection interrupted (hugging face is busy") && msg.contains("Pull again to continue the download; 0% of f is saved (0 MB of 1 MB)"), "{msg}");
    assert!(r.waits.len() >= 4, "{} waits", r.waits.len());
}

#[tokio::test]
async fn cancel_interrupts_a_wait_and_keeps_the_part() {
    let data = body(300_000);
    let (base, _) = server(data.clone()).await;
    let part = tmp().join("f.part");
    let url = format!("{base}/503");
    let (bytes, _) = half_part(&part, &url, &data);
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        c2.store(true, Ordering::SeqCst);
    });
    let retry = Retry { first: Duration::from_secs(30), cap: Duration::from_secs(60), budget: Duration::from_secs(900) };
    let r = fetch(&engine::download::http_client(), &url, &part, &data, retry, &cancel).await;
    assert_eq!(code(&r), "cancelled");
    assert_eq!(r.waits.len(), 1);
    assert!(r.secs < 1.0, "cancel took {:.2}s", r.secs);
    assert_eq!(std::fs::read(&part).unwrap(), bytes);
}

#[tokio::test]
async fn garbage_past_the_synced_bytes_is_trimmed_not_restarted() {
    // A power cut can leave bytes after the last sync that never reached the platter intact.
    let data = body(8 << 20);
    let (base, seen) = server(data.clone()).await;
    let part = tmp().join("f.part");
    let url = format!("{base}/ok");
    half_part(&part, &url, &data);
    let mut f = std::fs::OpenOptions::new().append(true).open(&part).unwrap();
    std::io::Write::write_all(&mut f, &vec![0xEEu8; 3 << 20]).unwrap();
    drop(f);
    assert_eq!(trusted_bytes(&part, &url, data.len() as u64, &sha(&data)), 4 << 20);
    let r = fetch(&engine::download::http_client(), &url, &part, &data, FAST, &AtomicBool::new(false)).await;
    assert_eq!(code(&r), "ok", "{:?}", r.result);
    assert_eq!(seen.lock().unwrap().iter().map(|s| s.1).collect::<Vec<_>>(), [4 << 20], "resumed at the sync point, no restart from zero");
    assert_verified(&part, &data);
}

#[tokio::test]
async fn a_sidecar_for_another_file_starts_from_zero() {
    let data = body(500_000);
    let (base, seen) = server(data.clone()).await;
    let part = tmp().join("f.part");
    half_part(&part, &format!("{base}/old-revision"), &data);
    let r = fetch(&engine::download::http_client(), &format!("{base}/ok"), &part, &data, FAST, &AtomicBool::new(false)).await;
    assert_eq!(code(&r), "ok");
    assert_eq!(seen.lock().unwrap()[0].1, 0);
    assert_verified(&part, &data);
}
