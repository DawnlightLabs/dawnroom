//! HTTP API. Everything goes through the one queue, so history is always recorded.
//! Every endpoint is listed with examples in engine/API.md; keep the two in step.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::backend::{BackendError, Progress};
use crate::config::{Config, ENGINE_VERSION, PRODUCT_NAME};
use crate::db::JobRecord;
use crate::models::{Models, PullEvent};
use crate::queue::{Engine, EventKind, ImageParams};
use crate::registry::{self, PRESETS, Task};

/// Uploads (edit inputs): several images of up to `inputs::MAX_BYTES` each, base64 included.
const UPLOAD_LIMIT: usize = 4 * crate::inputs::MAX_BYTES;

pub struct AppState {
    pub engine: Arc<Engine>,
    pub models: Arc<Models>,
    pub port: u16,
    /// Where this engine keeps its data; `GET /api/info` reports it.
    pub cfg: Config,
    /// Detected once at start (RAM availability is re-read per request).
    pub hardware: crate::hardware::Hardware,
    /// The data-folder move (Settings → Storage).
    pub mover: crate::storage_api::Mover,
    /// New thumbnails since the last cap check (the folder is capped every so often).
    pub thumbs_made: std::sync::atomic::AtomicU32,
    /// The OpenAI endpoint logs its "unknown model, using the default" fallback only once.
    pub openai_fallback_logged: AtomicBool,
    /// `POST /api/shutdown` (tray Quit, `dawnroom stop`, the uninstaller): serve returns and the engine exits.
    pub quit: Arc<tokio::sync::Notify>,
}

type S = State<Arc<AppState>>;

pub fn router(state: AppState) -> Router {
    use crate::storage_api as st;
    let port = state.port;
    let state = Arc::new(state);
    let app = Router::new()
        .route("/api/storage", get(st::get_storage))
        .route("/api/storage/clear-cache", post(st::clear_cache))
        .route("/api/storage/partials/delete", post(st::delete_partials))
        .route("/api/storage/move", get(st::move_status).post(st::start_move))
        .route("/api/storage/drives", get(st::drives))
        .route("/v1/images/generations", post(openai_generate))
        .route("/v1/images/edits", post(openai_edit).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)))
        .route("/api/inputs", post(create_input).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)))
        .route("/api/inputs/{sha256}", get(input_file))
        .route("/api/auto", get(get_auto))
        .route("/api/jobs", post(create_job))
        .route("/api/jobs/{id}", get(get_job))
        .route("/api/jobs/{id}/events", get(job_events))
        .route("/api/jobs/{id}/cancel", post(cancel_job))
        .route("/api/images", get(list_images))
        .route("/api/images/{id}", axum::routing::delete(delete_image))
        .route("/api/images/{id}/file", get(image_file))
        .route("/api/images/{id}/favourite", post(favourite_image))
        .route("/api/info", get(info))
        .route("/api/models", get(list_models))
        .route("/api/models/custom/check", post(check_custom))
        .route("/api/models/custom/import", post(import_custom))
        .route("/api/models/custom/local", post(find_local))
        .route("/api/models/{id}", axum::routing::delete(delete_model))
        .route("/api/models/{id}/pull", post(pull_model))
        .route("/api/models/{id}/pull/cancel", post(cancel_pull))
        .route("/api/models/{id}/licence/accept", post(accept_licence))
        .route("/api/presets", get(list_presets))
        .route("/api/health", get(health))
        .route("/api/shutdown", post(shutdown))
        .route("/api/hardware", get(hardware))
        .route("/api/runtimes", get(list_runtimes))
        .route("/api/runtimes/import", post(import_runtimes))
        .layer(middleware::from_fn_with_state(state.clone(), st::block_while_moving))
        .with_state(state);
    with_guard(app, port)
}

// ---------- Local security ----------

/// Browser origins allowed to call the engine: the Studio app (Tauri serves it as
/// `tauri://localhost` on macOS/Linux and `http(s)://tauri.localhost` on Windows) and the Vite
/// dev server. Requests without an Origin (CLI, MCP, curl) are not browser requests and pass.
pub const ALLOWED_ORIGINS: &[&str] = &["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost", "http://localhost:5173"];

/// Wraps every route (and 404s/OPTIONS, since it's a `layer`) in `guard`.
pub fn with_guard(app: Router, port: u16) -> Router {
    app.layer(middleware::from_fn_with_state(port, guard))
}

/// Keeps web pages out of a localhost API with no auth:
/// - Host must be `127.0.0.1:<port>` or `localhost:<port>` (stops DNS rebinding).
/// - A request with an Origin must come from `ALLOWED_ORIGINS`; CORS headers only go to those.
/// - POST needs `Content-Type: application/json`, so a page can't send a preflight-free
///   `text/plain` or form POST (e.g. to start a multi-GB pull).
async fn guard(State(port): State<u16>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    if !host.eq_ignore_ascii_case(&format!("127.0.0.1:{port}")) && !host.eq_ignore_ascii_case(&format!("localhost:{port}")) {
        return api_err(StatusCode::FORBIDDEN, "forbidden_host", format!("This engine only answers requests addressed to 127.0.0.1:{port} or localhost:{port}."));
    }
    let origin = match headers.get(header::ORIGIN) {
        None => None,
        Some(v) => match v.to_str().ok().filter(|o| ALLOWED_ORIGINS.contains(o)) {
            Some(o) => Some(HeaderValue::from_str(o).unwrap()),
            None => return api_err(StatusCode::FORBIDDEN, "forbidden_origin", "Web pages can't call the engine. Use Studio, the CLI or the MCP server."),
        },
    };
    let mut resp = if req.method() == Method::OPTIONS && origin.is_some() && headers.contains_key(header::ACCESS_CONTROL_REQUEST_METHOD) {
        // Preflight from an allowed origin.
        let mut r = StatusCode::NO_CONTENT.into_response();
        let h = r.headers_mut();
        h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, DELETE"));
        h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("content-type"));
        h.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
        r
    } else if req.method() == Method::POST && !is_json(headers.get(header::CONTENT_TYPE)) && !(req.uri().path() == OPENAI_EDITS && is_multipart(headers.get(header::CONTENT_TYPE))) {
        api_err(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type", "POST requests need the header Content-Type: application/json (even with an empty body).")
    } else {
        next.run(req).await
    };
    if let Some(o) = origin {
        resp.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
        resp.headers_mut().insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    resp
}

/// The one endpoint that takes multipart/form-data (OpenAI's edits API uploads files). The Host and
/// Origin checks still apply, so a web page still can't call it.
const OPENAI_EDITS: &str = "/v1/images/edits";

fn is_multipart(ct: Option<&HeaderValue>) -> bool {
    ct.and_then(|v| v.to_str().ok()).and_then(|v| v.split(';').next()).is_some_and(|m| m.trim().eq_ignore_ascii_case("multipart/form-data"))
}

fn is_json(ct: Option<&HeaderValue>) -> bool {
    ct.and_then(|v| v.to_str().ok()).and_then(|v| v.split(';').next()).is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
}

/// The engine's error shape: `{"error": "<plain words, what to do next>", "code": "<machine code>"}`.
pub(crate) fn api_err(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into(), "code": code }))).into_response()
}

fn status_for(code: &str) -> StatusCode {
    match code {
        "invalid_request" | "invalid_json" | "unknown_model" | "task_not_supported" | "invalid_image" | "mask_not_supported" => StatusCode::BAD_REQUEST,
        "input_not_found" => StatusCode::NOT_FOUND,
        "no_model_for_task" => StatusCode::CONFLICT,
        "not_found" => StatusCode::NOT_FOUND,
        "model_not_installed" | "model_not_ready" | "model_busy" | "already_finished" | "not_pulling" | "needs_more_memory" | "moving" | "busy" | "licence_not_accepted" => StatusCode::CONFLICT,
        "no_gpu" | "shutting_down" | "driver_too_old" | "glibc_too_old" | "no_driver" | "unsupported_os" | "no_runtime" => StatusCode::SERVICE_UNAVAILABLE,
        "not_enough_space" => StatusCode::INSUFFICIENT_STORAGE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn from_engine(e: BackendError) -> Response {
    api_err(status_for(&e.code), &e.code, e.message)
}

fn bad_json(r: JsonRejection) -> Response {
    api_err(StatusCode::BAD_REQUEST, "invalid_json", format!("The request body isn't valid JSON for this endpoint: {}", r.body_text()))
}

fn openai_err(status: StatusCode, message: impl Into<String>) -> Response {
    let kind = if status.is_client_error() { "invalid_request_error" } else { "server_error" };
    (status, Json(json!({ "error": { "message": message.into(), "type": kind, "code": null } }))).into_response()
}

// ---------- OpenAI-compatible, synchronous ----------

#[derive(Deserialize)]
struct OpenAiRequest {
    prompt: String,
    /// A registry model id, "auto" or a preset id ("fast", "quality"). Anything else (e.g.
    /// "gpt-image-1") uses Auto, so stock OpenAI clients work unchanged.
    model: Option<String>,
    n: Option<u32>,
    size: Option<String>,
    response_format: Option<String>,
    // Extensions beyond OpenAI's schema.
    seed: Option<i64>,
    negative_prompt: Option<String>,
}

async fn openai_generate(State(s): S, body: Result<Json<OpenAiRequest>, JsonRejection>) -> Response {
    let r = match body {
        Ok(Json(r)) => r,
        Err(e) => return openai_err(StatusCode::BAD_REQUEST, format!("Invalid JSON body: {}", e.body_text())),
    };
    if r.n.unwrap_or(1) != 1 {
        return openai_err(StatusCode::BAD_REQUEST, "Only n=1 is supported for now. Send one request per image.");
    }
    let (width, height) = match r.size.as_deref() {
        None | Some("auto") => (None, None),
        Some(size) => match size.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))) {
            Some((w, h)) => (Some(w), Some(h)),
            None => return openai_err(StatusCode::BAD_REQUEST, format!("size must look like 1024x1024 (got {size}).")),
        },
    };
    let as_url = match r.response_format.as_deref() {
        None | Some("b64_json") => false,
        Some("url") => true,
        Some(other) => return openai_err(StatusCode::BAD_REQUEST, format!("response_format must be b64_json or url (got {other}).")),
    };
    let (preset, model) = openai_model(&s, r.model);
    let params = ImageParams { prompt: r.prompt, negative_prompt: r.negative_prompt.unwrap_or_default(), width, height, seed: r.seed, preset, model, ..Default::default() };
    openai_run(&s, params, as_url).await
}

/// OpenAI's `model`: a model here, "auto", a preset, or anything else (stock clients) = Auto.
fn openai_model(s: &AppState, model: Option<String>) -> (Option<String>, Option<String>) {
    match model {
        Some(m) if m == "auto" || s.models.resolve(&m).is_ok() => (None, Some(m)),
        Some(m) if PRESETS.iter().any(|p| p.id == m) => (Some(m), None),
        Some(m) => {
            if !s.openai_fallback_logged.swap(true, Ordering::SeqCst) {
                let shown: String = m.chars().take(64).collect();
                eprintln!("[engine] info: OpenAI request for model {shown:?}, which isn't a model or preset here; using Auto. (Logged once.)");
            }
            (None, None)
        }
        None => (None, None),
    }
}

/// Runs one job to the end and answers in OpenAI's shape.
async fn openai_run(s: &AppState, params: ImageParams, as_url: bool) -> Response {
    let rx = match s.engine.submit(params) {
        Ok((_, rx)) => rx,
        Err(e) => return openai_err(status_for(&e.code), e.message),
    };
    let job = match rx.await {
        Ok(job) => job,
        Err(_) => return openai_err(StatusCode::SERVICE_UNAVAILABLE, "The engine stopped before the image was ready. Start it and try again."),
    };
    if job.status != "completed" {
        return openai_err(StatusCode::INTERNAL_SERVER_ERROR, job.error.unwrap_or_else(|| "Generation failed. Try again.".into()));
    }
    let created = job.finished_ms.unwrap_or(job.created_ms) / 1000;
    let item = if as_url {
        json!({ "url": format!("http://127.0.0.1:{}/api/images/{}/file", s.port, job.id) })
    } else {
        match tokio::fs::read(job.output_path.as_deref().unwrap_or_default()).await {
            Ok(bytes) => json!({ "b64_json": base64::engine::general_purpose::STANDARD.encode(bytes) }),
            Err(e) => return openai_err(StatusCode::INTERNAL_SERVER_ERROR, format!("The image was made but couldn't be read back ({e}).")),
        }
    };
    Json(json!({ "created": created, "data": [item] })).into_response()
}

/// `POST /v1/images/edits`: OpenAI's multipart edits API. `image` (or `image[]`, repeated): the
/// first is the image to edit, the others references. `mask` is refused: none of the edit models
/// here uses one. Waits for the result like `/v1/images/generations`.
async fn openai_edit(State(s): S, mut form: Multipart) -> Response {
    let mut images = Vec::new();
    let (mut prompt, mut model, mut size, mut format, mut seed, mut n, mut negative) = (None, None, None, None, None, None, None);
    loop {
        let field = match form.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return openai_err(StatusCode::BAD_REQUEST, format!("Couldn't read the form ({e}).")),
        };
        let name = field.name().unwrap_or("").to_string();
        let bytes = match field.bytes().await {
            Ok(b) => b,
            Err(e) => return openai_err(StatusCode::BAD_REQUEST, format!("Couldn't read the {name} field ({e}).")),
        };
        let text = || String::from_utf8_lossy(&bytes).trim().to_string();
        match name.as_str() {
            "image" | "image[]" => {
                let dir = s.engine.inputs_dir.clone();
                match tokio::task::spawn_blocking(move || crate::inputs::store(&dir, &bytes)).await {
                    Ok(Ok(i)) => images.push(i.sha256),
                    Ok(Err(e)) => return openai_err(StatusCode::BAD_REQUEST, e),
                    Err(e) => return openai_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
                }
            }
            "mask" => return openai_err(StatusCode::BAD_REQUEST, "Masks aren't supported: the edit models here change the image from the instruction alone. Leave out mask and say in the prompt what to change."),
            "prompt" => prompt = Some(text()),
            "model" => model = Some(text()),
            "size" => size = Some(text()),
            "response_format" => format = Some(text()),
            "seed" => seed = Some(text()),
            "n" => n = Some(text()),
            "negative_prompt" => negative = Some(text()),
            _ => {} // OpenAI fields we don't use (quality, background, user…)
        }
    }
    let Some(prompt) = prompt else { return openai_err(StatusCode::BAD_REQUEST, "prompt is required: say what to change.") };
    if images.is_empty() {
        return openai_err(StatusCode::BAD_REQUEST, "image is required: upload the image to edit.");
    }
    if n.as_deref().is_some_and(|n| n != "1") {
        return openai_err(StatusCode::BAD_REQUEST, "Only n=1 is supported for now. Send one request per image.");
    }
    let (width, height) = match size.as_deref() {
        None | Some("auto") => (None, None),
        Some(sz) => match sz.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))) {
            Some((w, h)) => (Some(w), Some(h)),
            None => return openai_err(StatusCode::BAD_REQUEST, format!("size must look like 1024x1024 (got {sz}).")),
        },
    };
    let as_url = match format.as_deref() {
        None | Some("b64_json") => false,
        Some("url") => true,
        Some(other) => return openai_err(StatusCode::BAD_REQUEST, format!("response_format must be b64_json or url (got {other}).")),
    };
    let seed = match seed.map(|x| x.parse::<i64>()) {
        None => None,
        Some(Ok(v)) => Some(v),
        Some(Err(_)) => return openai_err(StatusCode::BAD_REQUEST, "seed must be a whole number."),
    };
    let (preset, model) = openai_model(&s, model);
    let image = images.remove(0);
    let params = ImageParams { prompt, negative_prompt: negative.unwrap_or_default(), width, height, seed, preset, model, task: Some("edit".into()), image: Some(image), refs: images, ..Default::default() };
    openai_run(&s, params, as_url).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputBody {
    /// A file on this computer (CLI, MCP).
    path: Option<String>,
    /// The image itself, base64 (a data URL prefix is allowed).
    b64: Option<String>,
}

/// `POST /api/inputs`: stores an edit input by sha256 and returns its id, size and format.
/// `GET /api/inputs/{sha256}`: a stored input image as it was sent (Studio's thumbnail for a dropped file).
async fn input_file(State(s): S, Path(sha256): Path<String>) -> Response {
    let dir = s.engine.inputs_dir.clone();
    let found = tokio::task::spawn_blocking(move || crate::inputs::get(&dir, &sha256)).await;
    match found {
        Ok(Ok(input)) => match tokio::fs::read(&input.path).await {
            Ok(bytes) => ([(header::CONTENT_TYPE, format!("image/{}", input.format)), (header::CACHE_CONTROL, "private, max-age=31536000, immutable".to_string())], bytes).into_response(),
            Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string()),
        },
        Ok(Err(msg)) => api_err(StatusCode::NOT_FOUND, "input_not_found", msg),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string()),
    }
}

async fn create_input(State(s): S, body: Result<Json<InputBody>, JsonRejection>) -> Response {
    let b = match body {
        Ok(Json(b)) => b,
        Err(e) => return bad_json(e),
    };
    let bytes = match (b.path, b.b64) {
        (Some(path), None) => {
            match tokio::fs::metadata(&path).await {
                Ok(m) if m.len() as usize > crate::inputs::MAX_BYTES => return api_err(StatusCode::BAD_REQUEST, "invalid_image", format!("{path} is bigger than {} MB.", crate::inputs::MAX_BYTES / 1024 / 1024)),
                Ok(m) if m.is_file() => {}
                _ => return api_err(StatusCode::NOT_FOUND, "not_found", format!("There's no file at {path}.")),
            }
            match tokio::fs::read(&path).await {
                Ok(b) => b,
                Err(e) => return api_err(StatusCode::BAD_REQUEST, "invalid_image", format!("Couldn't read {path} ({e}).")),
            }
        }
        (None, Some(b64)) => {
            let data = b64.split_once(";base64,").map(|(_, d)| d).unwrap_or(&b64);
            match base64::engine::general_purpose::STANDARD.decode(data.trim()) {
                Ok(b) => b,
                Err(_) => return api_err(StatusCode::BAD_REQUEST, "invalid_image", "b64 isn't valid base64."),
            }
        }
        _ => return api_err(StatusCode::BAD_REQUEST, "invalid_request", "Send exactly one of path or b64."),
    };
    let dir = s.engine.inputs_dir.clone();
    match tokio::task::spawn_blocking(move || crate::inputs::store(&dir, &bytes)).await {
        Ok(Ok(i)) => {
            let parent = s.engine.db.image_by_sha256(&i.sha256).ok().flatten();
            // `image_id`: the Library image with exactly these bytes, if it is one (an edit of it links back).
            (StatusCode::CREATED, Json(json!({ "sha256": i.sha256, "format": i.format, "width": i.width, "height": i.height, "bytes": i.bytes, "image_id": parent }))).into_response()
        }
        Ok(Err(e)) => api_err(StatusCode::BAD_REQUEST, "invalid_image", e),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()),
    }
}

/// `GET /api/auto`: what Auto would pick now for each task, and why (or what to pull).
async fn get_auto(State(s): S) -> Response {
    let mut out = json!({ "loaded": s.engine.expected_loaded() });
    for t in Task::ALL {
        out[t.as_str()] = match s.engine.auto(t) {
            Ok(p) => json!({ "ok": true, "model": p.model, "name": p.name, "reason": p.reason, "loaded": p.loaded }),
            Err(no) => json!({ "ok": false, "code": "no_model_for_task", "error": no.message, "suggest": no.suggest }),
        };
    }
    Json(out).into_response()
}

// ---------- Async jobs ----------

#[derive(Serialize)]
struct JobView {
    #[serde(flatten)]
    job: JobRecord,
    progress: Option<Progress>,
}

async fn create_job(State(s): S, body: Result<Json<ImageParams>, JsonRejection>) -> Response {
    let p = match body {
        Ok(Json(p)) => p,
        Err(e) => return bad_json(e),
    };
    match s.engine.submit(p) {
        Ok((job, _)) => (StatusCode::ACCEPTED, Json(job)).into_response(),
        Err(e) => from_engine(e),
    }
}

/// The job plus its live progress; the error is a ready-made response (boxed: it is large).
fn job_view(s: &AppState, id: &str) -> Result<JobView, Box<Response>> {
    match s.engine.db.get(id) {
        Ok(Some(job)) => Ok(JobView { progress: s.engine.progress(id), job }),
        Ok(None) => Err(Box::new(api_err(StatusCode::NOT_FOUND, "not_found", "No job with that id."))),
        Err(e) => Err(Box::new(api_err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", format!("Couldn't read history ({e}).")))),
    }
}

async fn get_job(State(s): S, Path(id): Path<String>) -> Response {
    match job_view(&s, &id) {
        Ok(v) => Json(v).into_response(),
        Err(r) => *r,
    }
}

async fn cancel_job(State(s): S, Path(id): Path<String>) -> Response {
    let view = match job_view(&s, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if !s.engine.cancel(&id) {
        return api_err(StatusCode::CONFLICT, "already_finished", format!("That job already finished ({}).", view.job.status));
    }
    (StatusCode::ACCEPTED, Json(json!({ "id": id, "cancel_requested": true }))).into_response()
}

fn is_done(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

/// SSE: a `snapshot` event first, then `started`, `progress` (stage + step), `preview`
/// (base64 PNG frame) and a final `finished` event, after which the stream closes.
async fn job_events(State(s): S, Path(id): Path<String>) -> Response {
    // Subscribe before reading the snapshot so nothing between the two is missed.
    let mut rx = s.engine.events.subscribe();
    let snapshot = match job_view(&s, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let stream = async_stream::stream! {
        let done = is_done(&snapshot.job.status);
        yield Ok::<_, Infallible>(SseEvent::default().event("snapshot").json_data(&snapshot).unwrap());
        if done { return; }
        loop {
            match rx.recv().await {
                Ok(ev) if ev.job_id == id => {
                    let name = match &ev.kind {
                        EventKind::Started => "started",
                        EventKind::Progress { .. } => "progress",
                        EventKind::Preview { .. } => "preview",
                        EventKind::Finished { .. } => "finished",
                    };
                    yield Ok(SseEvent::default().event(name).json_data(&ev.kind).unwrap());
                    if name == "finished" { break; }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => continue, // a slow client skips frames, not the stream
                Err(RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

// ---------- Models and presets ----------

fn model_view(s: &AppState, m: &registry::Manifest) -> serde_json::Value {
    let files: Vec<_> = m
        .files
        .iter()
        .map(|f| json!({ "path": f.rel_path(), "role": f.role, "size": f.size, "status": s.models.file_state(f) }))
        .collect();
    let fit = s.models.fit(m);
    json!({
        "id": m.id,
        // The model without its size tag, and the tag (null for an old untagged manifest).
        "family": m.family,
        "tag": m.tag,
        "name": m.name,
        "tagline": m.tagline,
        "tags": m.tags,
        "original": m.original,
        "recipe": m.recipe,
        "licence": m.licence,
        // accept_required models: when the user accepted the licence here (null = not yet).
        "licence_accepted_ms": s.models.licence_accepted_ms(m),
        // tested (we measured it) | community | untested. Derived from `measured`.
        "tier": m.testing(),
        "state": s.models.model_state(m),
        "fit": fit.tier,
        // The numbers behind it: need, VRAM (or an iGPU's shared budget), usable RAM, and
        // `estimate: true` when the need isn't measured (file sizes + compute reserve).
        "fit_detail": fit,
        // generate and/or edit, and how tested each is (only that task's measurements count).
        "tasks": m.tasks,
        "tiers": m.tasks.iter().map(|t| (t.as_str().to_string(), json!(m.testing_for(*t)))).collect::<serde_json::Map<_, _>>(),
        // Measured time per task: {cold_s, warm_s, gpu, size, here, label: "on your GPU" | "on <gpu>"},
        // or null ("no timing yet"). Never estimated.
        "timing": m.tasks.iter().map(|t| (t.as_str().to_string(), json!(s.models.timing(m, *t)))).collect::<serde_json::Map<_, _>>(),
        // The fit per task (an edit can peak higher than generating); `fit` above is the main task's.
        "task_fit": m.tasks.iter().map(|t| (t.as_str().to_string(), json!(s.models.fit_for(m, *t)))).collect::<serde_json::Map<_, _>>(),
        "size_bytes": m.total_size(),
        "download_bytes": s.models.download_size(m),
        // What a pull still puts on disk: the model plus the runtime, if that isn't installed yet.
        "needed_bytes": s.models.needed_bytes(m),
        "presets": registry::presets_for(&m.id),
        "loaded": s.engine.loaded_model().as_deref() == Some(m.id.as_str()),
        "defaults": { "steps": m.defaults.steps, "cfg": m.defaults.cfg, "width": m.defaults.width, "height": m.defaults.height },
        // Only variants with a fixed sigma schedule: the steps are set by it.
        "fixed_steps": m.defaults.sigmas.is_some(),
        "measured": m.measured,
        "files": files,
        // While pulling: the latest progress event (null until the first one).
        "pull": s.models.pull_progress(&m.id),
        // How the last pull failed or was cancelled (null after a success or a new pull).
        "pull_error": s.models.pull_error(&m.id),
        // A stopped download waiting for the user (never resumed on its own): bytes, total, updated_ms.
        "paused": s.models.paused(m),
        // Custom models only (null otherwise): where it came from, its architecture, the curated
        // model its companions come from, and its test image. Always labelled.
        "custom": m.custom,
        "label": m.custom.as_ref().map(|_| registry::CUSTOM_LABEL),
    })
}

async fn list_models(State(s): S) -> Response {
    s.models.refresh();
    let reg = s.models.registry();
    let models: Vec<_> = reg.models.iter().map(|m| model_view(&s, m)).collect();
    // One row per model: its tags (smallest first) and the one a bare id means on this machine.
    let families: Vec<_> = reg
        .families()
        .into_iter()
        .map(|f| {
            let variants: Vec<_> = reg.variants(f).iter().map(|m| m.id.clone()).collect();
            let custom = reg.variants(f).iter().any(|m| m.is_custom());
            json!({ "id": f, "default": s.models.default_variant(f).map(|m| m.id.clone()), "variants": variants, "custom": custom })
        })
        .collect();
    Json(json!({ "families": families, "gpu": s.models.gpu, "disk": crate::disk::space(&s.cfg.data_dir), "runtime_needed_bytes": s.models.runtimes.active_needed_bytes(), "runtime_paused": s.models.runtime_paused(), "models": models })).into_response()
}

/// SSE: `progress` events (file, file_index, file_count, stage, bytes, total, speed_bps), then
/// one `done` or `error` event, after which the stream closes. The daemon owns the pull: closing
/// the stream detaches without stopping it, and a POST during a pull joins (re-attaches to) it.
/// Only `POST /api/models/{id}/pull/cancel` stops it; the `.part` stays for resume.
async fn pull_model(State(s): S, Path(id): Path<String>) -> Response {
    // `hf.co/<user>/<repo>[:<file>]` not added yet: the compatibility check first, then the pull.
    // A failed check downloads nothing and answers 409 with the whole report.
    let id = match crate::custom::parse_hf(&id) {
        Some((repo, file)) if s.models.resolve(&id).is_err() && file.as_deref().is_none_or(|f| s.models.resolve(&crate::custom::hf_id(&repo, f)).is_err()) => {
            match crate::custom::add(&s.models, &id).await {
                Ok(m) => m.id,
                Err(r) => return check_refused(&r),
            }
        }
        // Another spelling of an added one (huggingface.co/…, lower case): the stored id.
        Some((repo, Some(file))) => s.models.resolve(&id).map(|m| m.id).unwrap_or_else(|_| crate::custom::hf_id(&repo, &file)),
        _ => id,
    };
    // A gated original: the user accepts its licence first (Studio's sheet or the CLI's question).
    if let Ok(m) = s.models.resolve(&id)
        && s.models.needs_licence(&m) {
            let body = json!({
                "error": crate::models::Models::licence_message(&m),
                "code": "licence_not_accepted",
                "model": m.id,
                "licence": { "name": m.licence.name.as_deref().unwrap_or(&m.licence.spdx), "spdx": m.licence.spdx, "url": m.licence.url, "commercial": m.licence.commercial, "note": m.licence.note },
            });
            return (StatusCode::CONFLICT, Json(body)).into_response();
        }
    let mut rx = match s.models.pull(&id) {
        Ok(rx) => rx,
        Err(e) => return from_engine(e),
    };
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let (name, last) = match &ev {
                        PullEvent::Progress { .. } => ("progress", false),
                        PullEvent::Done { .. } => ("done", true),
                        PullEvent::Error { .. } => ("error", true),
                    };
                    yield Ok::<_, Infallible>(SseEvent::default().event(name).json_data(&ev).unwrap());
                    if last { break; }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// The user accepts a gated model's original licence. Studio and the CLI call this only after
/// showing the licence and getting an explicit yes; MCP never calls it.
async fn accept_licence(State(s): S, Path(id): Path<String>) -> Response {
    let m = match s.models.resolve(&id) {
        Ok(m) => m,
        Err(_) => return api_err(StatusCode::NOT_FOUND, "not_found", format!("There's no model called {id:?}.")),
    };
    if !m.licence.accept_required {
        return api_err(StatusCode::BAD_REQUEST, "invalid_request", format!("{} doesn't need a licence acceptance.", m.name));
    }
    match s.models.accept_licence(&m) {
        Ok(ms) => Json(json!({ "model": m.family, "licence_url": m.licence.url, "accepted_ms": ms })).into_response(),
        Err(e) => from_engine(e),
    }
}

async fn cancel_pull(State(s): S, Path(id): Path<String>) -> Response {
    let id = match s.models.resolve(&id) {
        Ok(m) => m.id.clone(),
        Err(_) => return api_err(StatusCode::NOT_FOUND, "not_found", format!("There's no model called {id:?}.")),
    };
    if !s.models.cancel_pull(&id) {
        return api_err(StatusCode::CONFLICT, "not_pulling", format!("{id} isn't downloading, so there's nothing to cancel."));
    }
    (StatusCode::ACCEPTED, Json(json!({ "id": id, "cancel_requested": true }))).into_response()
}

async fn delete_model(State(s): S, Path(id): Path<String>) -> Response {
    let id = match s.models.resolve(&id) {
        Ok(m) => m.id.clone(),
        Err(_) => return api_err(StatusCode::NOT_FOUND, "not_found", format!("There's no model called {id:?}.")),
    };
    if s.engine.is_busy() {
        return api_err(StatusCode::CONFLICT, "model_busy", "Images are still developing. Wait for them to finish (or cancel them), then delete.");
    }
    s.engine.unload(&id).await;
    match s.models.delete(&id) {
        Ok((deleted, kept)) => Json(json!({ "id": id, "deleted": deleted, "kept": kept })).into_response(),
        Err(e) => from_engine(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomBody {
    /// `hf.co/<user>/<repo>[:<file>]` (check only), or a local file path.
    source: Option<String>,
    path: Option<String>,
}

fn check_refused(r: &crate::custom::Report) -> Response {
    let e = r.error();
    (StatusCode::CONFLICT, Json(json!({ "error": e.message, "code": e.code, "check": r }))).into_response()
}

/// The compatibility check for a custom model (steps 1–6), without downloading weights: only the
/// file's header is read, over HTTP Range or from disk. 200 with the report whether it passed or
/// not (`ok`, `steps`, and on failure `code` + `message`).
async fn check_custom(State(s): S, body: Result<Json<CustomBody>, JsonRejection>) -> Response {
    let b = match body {
        Ok(Json(b)) => b,
        Err(r) => return bad_json(r),
    };
    // `source` takes every form (user/repo, a Hugging Face page or file link, hf.co/…, a path).
    let source = match (b.source, b.path) {
        (Some(src), None) | (None, Some(src)) => src,
        _ => return api_err(StatusCode::BAD_REQUEST, "invalid_request", "Send \"source\": a Hugging Face link, user/repo, or the path of a model file."),
    };
    if let Err(msg) = crate::custom::parse_source(&source) {
        return api_err(StatusCode::BAD_REQUEST, "invalid_source", msg);
    }
    Json(crate::custom::check(&s.models, &source).await).into_response()
}

/// Imports a local file as a custom model: the check, then the file is hashed and hard-linked
/// (or copied) into the models folder. A pull of the returned id then fetches any companions it
/// needs and runs the test image.
async fn import_custom(State(s): S, body: Result<Json<CustomBody>, JsonRejection>) -> Response {
    let path = match body {
        Ok(Json(CustomBody { path: Some(p), source: None })) if !p.trim().is_empty() => p,
        Ok(_) => return api_err(StatusCode::BAD_REQUEST, "invalid_request", "Send \"path\": the model file to import."),
        Err(r) => return bad_json(r),
    };
    match crate::custom::import(&s.models, &path).await {
        Ok(m) => Json(model_view(&s, &m)).into_response(),
        Err(r) => check_refused(&r),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct LocalBody {
    /// Look only in this folder (and below); without it, the usual places other tools keep models.
    dir: Option<String>,
}

/// Image models already on this computer (Hugging Face cache, ComfyUI, A1111/Forge, Stability Matrix,
/// Downloads, or one folder), each with the check's answer. Reads headers only; nothing is copied.
async fn find_local(State(s): S, body: Result<Json<LocalBody>, JsonRejection>) -> Response {
    let b = match body {
        Ok(Json(b)) => b,
        Err(r) => return bad_json(r),
    };
    let dir = b.dir.as_deref().map(|d| std::path::PathBuf::from(d.trim().trim_matches('"'))).filter(|d| !d.as_os_str().is_empty());
    if let Some(d) = &dir
        && !d.is_dir()
    {
        return api_err(StatusCode::BAD_REQUEST, "not_a_folder", format!("There's no folder at {}.", d.display()));
    }
    Json(crate::custom::find_local(&s.models, dir.as_deref()).await).into_response()
}

async fn list_presets(State(s): S) -> Response {
    let presets: Vec<_> = PRESETS
        .iter()
        .map(|p| {
            let reg = s.models.registry();
            let m = reg.get(p.model);
            json!({
                "id": p.id,
                "label": p.label,
                "model": p.model,
                "default": p.id == registry::DEFAULT_PRESET,
                "state": m.map(|m| s.models.model_state(m)),
                "fit": m.map(|m| s.models.fit(m).tier),
            })
        })
        .collect();
    let aspects: Vec<_> = registry::ASPECTS.iter().map(|(id, w, h)| json!({ "id": id, "width": w, "height": h })).collect();
    Json(json!({ "presets": presets, "aspects": aspects })).into_response()
}

// ---------- History ----------

#[derive(Deserialize)]
struct Page {
    limit: Option<u32>,
    offset: Option<u32>,
    /// Text to find in the prompt.
    q: Option<String>,
    /// `true`: only favourites.
    favourite: Option<bool>,
}

#[derive(Serialize)]
struct ImageView {
    #[serde(flatten)]
    job: JobRecord,
    url: String,
}

async fn list_images(State(s): S, Query(page): Query<Page>) -> Response {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0);
    match s.engine.db.list_images(limit, offset, page.q.as_deref(), page.favourite.unwrap_or(false)) {
        Ok((items, total)) => {
            let items: Vec<_> = items.into_iter().map(|job| ImageView { url: format!("/api/images/{}/file", job.id), job }).collect();
            Json(json!({ "items": items, "total": total, "limit": limit, "offset": offset })).into_response()
        }
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", format!("Couldn't read history ({e}).")),
    }
}

#[derive(Deserialize)]
struct FileQuery {
    /// Longest side of a JPEG thumbnail, 64 to 1024. Without it: the original PNG.
    w: Option<u32>,
}

/// A JPEG no larger than `w` on its longest side.
fn make_thumb(png: &[u8], w: u32) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(png).map_err(|e| e.to_string())?.thumbnail(w, w).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img).map_err(|e| e.to_string())?;
    Ok(out)
}

async fn image_file(State(s): S, Path(id): Path<String>, Query(q): Query<FileQuery>) -> Response {
    let path = match s.engine.db.get(&id) {
        Ok(Some(JobRecord { output_path: Some(p), .. })) => p,
        Ok(_) => return api_err(StatusCode::NOT_FOUND, "not_found", "No image with that id."),
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", format!("Couldn't read history ({e}).")),
    };
    // An image never changes once made, so clients may keep it.
    let cache = (header::CACHE_CONTROL, "private, max-age=31536000, immutable");
    let missing = || api_err(StatusCode::GONE, "file_missing", "The image file is missing from the outputs folder. It may have been moved or deleted.");
    let Some(w) = q.w else {
        return match tokio::fs::read(&path).await {
            Ok(bytes) => ([(header::CONTENT_TYPE, "image/png"), cache], bytes).into_response(),
            Err(_) => missing(),
        };
    };
    // Thumbnails are made on first request and kept next to the data.
    let w = w.clamp(64, 1024);
    let thumb = s.cfg.thumbs_dir().join(format!("{id}-{w}.jpg"));
    if let Ok(bytes) = tokio::fs::read(&thumb).await {
        return ([(header::CONTENT_TYPE, "image/jpeg"), cache], bytes).into_response();
    }
    let Ok(png) = tokio::fs::read(&path).await else { return missing() };
    match tokio::task::spawn_blocking(move || make_thumb(&png, w)).await {
        Ok(Ok(jpeg)) => {
            // A thumbnail that can't be saved is still served; it's just made again next time.
            let _ = tokio::fs::create_dir_all(s.cfg.thumbs_dir()).await;
            let _ = tokio::fs::write(&thumb, &jpeg).await;
            // Keep the folder near its cap (~500 MB, oldest first); checked every 25 new thumbnails.
            if s.thumbs_made.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 25 == 24 {
                let dir = s.cfg.thumbs_dir();
                tokio::task::spawn_blocking(move || crate::storage::cap_thumbs(&dir, crate::storage::THUMBS_CAP_BYTES));
            }
            ([(header::CONTENT_TYPE, "image/jpeg"), cache], jpeg).into_response()
        }
        _ => api_err(StatusCode::INTERNAL_SERVER_ERROR, "thumbnail_failed", "Couldn't make a thumbnail of that image. Open the full image instead."),
    }
}

fn db_err(e: rusqlite::Error) -> Response {
    api_err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", format!("Couldn't update history ({e})."))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Favourite {
    favourite: bool,
}

async fn favourite_image(State(s): S, Path(id): Path<String>, body: Result<Json<Favourite>, JsonRejection>) -> Response {
    let f = match body {
        Ok(Json(f)) => f,
        Err(e) => return bad_json(e),
    };
    match s.engine.db.set_favourite(&id, f.favourite) {
        Ok(true) => Json(json!({ "id": id, "favourite": f.favourite })).into_response(),
        Ok(false) => api_err(StatusCode::NOT_FOUND, "not_found", "No image with that id."),
        Err(e) => db_err(e),
    }
}

/// Removes the image from the library and its PNG from the outputs folder.
async fn delete_image(State(s): S, Path(id): Path<String>) -> Response {
    let path = match s.engine.db.get(&id) {
        Ok(Some(JobRecord { output_path: Some(p), status, .. })) if status == "completed" => p,
        Ok(_) => return api_err(StatusCode::NOT_FOUND, "not_found", "No image with that id."),
        Err(e) => return db_err(e),
    };
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return api_err(StatusCode::INTERNAL_SERVER_ERROR, "disk_error", format!("Couldn't delete the image file ({e}). Close any program that has it open, then try again."));
        }
    }
    // Its thumbnails go too.
    if let Ok(mut dir) = tokio::fs::read_dir(s.cfg.thumbs_dir()).await {
        while let Ok(Some(entry)) = dir.next_entry().await {
            if entry.file_name().to_string_lossy().starts_with(&format!("{id}-")) {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }
    match s.engine.db.mark_deleted(&id) {
        Ok(_) => Json(json!({ "id": id, "deleted": true })).into_response(),
        Err(e) => db_err(e),
    }
}

// ---------- Info and health ----------

/// Bytes of every file under `dir` (0 if it doesn't exist).
fn dir_size(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// An absolute path in the form people paste into configs (no Windows `\\?\` prefix).
pub fn plain(p: &std::path::Path) -> String {
    let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    p.to_string_lossy().trim_start_matches(r"\\?\").to_string()
}

/// Where things live on disk, for Settings: the folders, their sizes and this executable
/// (the path an agent's MCP config should point at).
async fn info(State(s): S) -> Response {
    let cfg = s.cfg.clone();
    let (models_dir, outputs_dir) = (cfg.models_dir(), cfg.outputs_dir());
    let sizes = tokio::task::spawn_blocking({
        let (m, o) = (models_dir.clone(), outputs_dir.clone());
        move || (dir_size(&m), dir_size(&o))
    });
    let (models_bytes, outputs_bytes) = sizes.await.unwrap_or((0, 0));
    Json(json!({
        "name": PRODUCT_NAME,
        "version": ENGINE_VERSION,
        "port": s.port,
        "exe": std::env::current_exe().ok().map(|p| plain(&p)),
        "data_dir": plain(&cfg.data_dir),
        "models_dir": plain(&models_dir),
        "outputs_dir": plain(&outputs_dir),
        "logs_dir": plain(&cfg.logs_dir()),
        "models_bytes": models_bytes,
        "outputs_bytes": outputs_bytes,
    }))
    .into_response()
}

/// Stops the engine: running jobs and pulls end (a pull keeps its `.part`), sd-server is stopped,
/// and the process exits. Answers first, so the caller sees 202 before the port closes.
async fn shutdown(State(s): S) -> Response {
    eprintln!("[engine] shutdown requested");
    s.quit.notify_one();
    (StatusCode::ACCEPTED, Json(json!({ "ok": true }))).into_response()
}

async fn health(State(s): S) -> Response {
    Json(json!({
        "name": PRODUCT_NAME,
        "version": ENGINE_VERSION,
        "backend": s.engine.backend_state(),
        "model": s.engine.loaded_model(),
        "busy": s.engine.is_busy(),
        "gpu": crate::hardware::nvidia_live().await,
        "runtime": s.models.runtimes.choice.build,
        "runtime_ready": s.models.runtimes.ready(),
    }))
    .into_response()
}

/// This machine: GPUs (any vendor), RAM, the chosen GPU and runtime and why, driver advice,
/// what the runtime itself can see, and free space on the data drive. `dawnroom doctor` prints
/// the same report when the engine isn't running.
pub async fn hardware_report(hw: &crate::hardware::Hardware, rt: &crate::runtimes::Runtimes, data_dir: &std::path::Path) -> serde_json::Value {
    let mut hw = hw.clone();
    hw.ram = crate::hardware::ram();
    let runtime = rt.active().map(|m| {
        json!({
            "id": m.id, "build": m.build, "gpu": m.gpu, "experimental": m.experimental,
            "installed": rt.is_installed(m),
            "path": rt.is_installed(m).then(|| plain(&rt.dir_of(m))),
            "download_bytes": m.download_size(), "unpacked_bytes": m.unpacked_size(),
            "needed_bytes": rt.needed_bytes(m),
            "min_driver": m.min_driver,
        })
    });
    json!({
        "os": hw.os, "os_version": hw.os_version, "cpu": hw.cpu,
        "gpus": hw.gpus, "ram": hw.ram,
        "choice": rt.choice,
        "runtime": runtime,
        "sd_server_override": rt.is_override(),
        "devices": rt.devices().await,
        "runtime_sees_gpu": rt.sees_gpu().await,
        "disk": { "path": plain(data_dir), "space": crate::disk::space(data_dir) },
        "simulated": hw.simulated,
    })
}

async fn hardware(State(s): S) -> Response {
    Json(hardware_report(&s.hardware, &s.models.runtimes, &s.cfg.data_dir).await).into_response()
}

async fn list_runtimes(State(s): S) -> Response {
    let rt = &s.models.runtimes;
    let all: Vec<_> = rt
        .all
        .iter()
        .map(|m| json!({ "id": m.id, "build": m.build, "os": m.os, "gpu": m.gpu, "experimental": m.experimental, "download_bytes": m.download_size(), "unpacked_bytes": m.unpacked_size(), "installed": rt.is_installed(m) }))
        .collect();
    let installed: Vec<_> = rt.installed().into_iter().map(|(id, t)| json!({ "id": id, "last_used_ms": t })).collect();
    let partial: Vec<_> = rt.partial_downloads().into_iter().map(|(n, b)| json!({ "file": n, "bytes": b })).collect();
    Json(json!({ "active": rt.choice.runtime, "runtimes": all, "installed": installed, "partial": partial })).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportBody {
    paths: Vec<std::path::PathBuf>,
}

/// Imports runtime zips that are already on this machine (matched by sha256, not name).
async fn import_runtimes(State(s): S, body: Result<Json<ImportBody>, JsonRejection>) -> Response {
    let Json(b) = match body {
        Ok(b) => b,
        Err(r) => return bad_json(r),
    };
    match s.models.runtimes.import(&b.paths).await {
        Ok(files) => {
            let files: Vec<_> = files.into_iter().map(|(p, m)| json!({ "path": p, "matched": m })).collect();
            let rt = &s.models.runtimes;
            Json(json!({ "files": files, "active_installed": rt.active().is_some_and(|m| rt.is_installed(m)) })).into_response()
        }
        Err(e) => from_engine(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt as _;

    const PORT: u16 = 8420;

    /// The real guard around a stand-in route, so the checks are tested without a GPU or models.
    fn app() -> Router {
        with_guard(Router::new().route("/api/x", get(|| async { "ok" }).post(|| async { "ok" }).delete(|| async { "ok" })), PORT)
    }

    async fn send(method: &str, host: &str, extra: &[(&str, &str)]) -> Response {
        let mut req = axum::http::Request::builder().method(method).uri("/api/x").header(header::HOST, host);
        for (k, v) in extra {
            req = req.header(*k, *v);
        }
        app().oneshot(req.body(Body::empty()).unwrap()).await.unwrap()
    }

    async fn code(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["code"].as_str().unwrap_or("").to_string()
    }

    const LOCAL: &str = "127.0.0.1:8420";

    #[test]
    fn thumbnails_are_small_jpegs() {
        let big = image::RgbImage::from_pixel(400, 200, image::Rgb([200, 80, 40]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(big).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let jpeg = make_thumb(&png, 100).unwrap();
        let thumb = image::load_from_memory(&jpeg).unwrap();
        assert_eq!(image::guess_format(&jpeg).unwrap(), image::ImageFormat::Jpeg);
        assert_eq!((thumb.width(), thumb.height()), (100, 50), "fits the box, keeps the shape");
        assert!(make_thumb(b"not an image", 100).is_err());
    }

    #[tokio::test]
    async fn host_must_be_loopback_on_our_port() {
        assert_eq!(send("GET", "127.0.0.1:8420", &[]).await.status(), StatusCode::OK);
        assert_eq!(send("GET", "localhost:8420", &[]).await.status(), StatusCode::OK);
        assert_eq!(send("GET", "LOCALHOST:8420", &[]).await.status(), StatusCode::OK);
        for bad in ["evil.example:8420", "127.0.0.1:9999", "127.0.0.1", "localhost", "", "127.0.0.1.evil.example:8420"] {
            let r = send("GET", bad, &[]).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "host {bad:?}");
            assert_eq!(code(r).await, "forbidden_host");
        }
        // Unknown routes are guarded too (the guard is a layer, not a route_layer).
        let req = axum::http::Request::builder().uri("/nope").header(header::HOST, "evil.example:8420").body(Body::empty()).unwrap();
        assert_eq!(app().oneshot(req).await.unwrap().status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn cors_only_for_studio_and_vite() {
        for o in ALLOWED_ORIGINS {
            let r = send("GET", LOCAL, &[("origin", o)]).await;
            assert_eq!(r.status(), StatusCode::OK);
            assert_eq!(r.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], *o);
        }
        for bad in ["https://evil.example", "http://localhost:5174", "null", "http://tauri.localhost.evil.example", "tauri://localhost.evil"] {
            let r = send("GET", LOCAL, &[("origin", bad)]).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "origin {bad:?}");
            assert!(r.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
            assert_eq!(code(r).await, "forbidden_origin");
        }
        // No Origin (CLI, MCP, curl): allowed, and no CORS headers.
        let r = send("GET", LOCAL, &[]).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(r.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());

        // Preflight: answered for allowed origins, refused for others.
        let pre = [("origin", "http://localhost:5173"), ("access-control-request-method", "DELETE"), ("access-control-request-headers", "content-type")];
        let r = send("OPTIONS", LOCAL, &pre).await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert_eq!(r.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:5173");
        assert!(r.headers()[header::ACCESS_CONTROL_ALLOW_METHODS].to_str().unwrap().contains("DELETE"));
        let r = send("OPTIONS", LOCAL, &[("origin", "https://evil.example"), ("access-control-request-method", "POST")]).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn post_needs_json_content_type() {
        for bad in [None, Some("text/plain"), Some("application/x-www-form-urlencoded"), Some("multipart/form-data; boundary=x"), Some("application/jsonx")] {
            let extra: Vec<(&str, &str)> = bad.map(|ct| ("content-type", ct)).into_iter().collect();
            let r = send("POST", LOCAL, &extra).await;
            assert_eq!(r.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE, "content-type {bad:?}");
            assert_eq!(code(r).await, "unsupported_media_type");
        }
        for ok in ["application/json", "application/json; charset=utf-8", "Application/JSON"] {
            assert_eq!(send("POST", LOCAL, &[("content-type", ok)]).await.status(), StatusCode::OK, "{ok}");
        }
        // Studio sees the 415 too (CORS headers on errors), but a page from another origin doesn't get that far.
        let r = send("POST", LOCAL, &[("origin", "http://localhost:5173"), ("content-type", "text/plain")]).await;
        assert_eq!(r.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(r.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:5173");
        assert_eq!(send("POST", LOCAL, &[("origin", "https://evil.example"), ("content-type", "text/plain")]).await.status(), StatusCode::FORBIDDEN);
        // GET and DELETE don't need a body type.
        assert_eq!(send("DELETE", LOCAL, &[]).await.status(), StatusCode::OK);
    }
}
