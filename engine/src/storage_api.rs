//! Storage endpoints (Studio Settings → Storage): the usage report, Clear cache, paused downloads
//! and moving the data folder. The work is in `storage.rs`; this is the HTTP side plus the move's
//! choreography (block writes, stop sd-server, copy, switch, restart).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::api::{AppState, api_err};
use crate::storage::{self, MoveProgress};

type S = State<Arc<AppState>>;

/// The data-folder move in progress (or how the last one ended).
#[derive(Default)]
pub struct Mover {
    pub progress: Mutex<MoveProgress>,
    /// True from the moment a move starts until it fails (a successful move ends in a restart).
    pub active: AtomicBool,
}

impl Mover {
    fn set(&self, f: impl FnOnce(&mut MoveProgress)) {
        f(&mut self.progress.lock().unwrap());
    }
}

/// While a move copies the folder, nothing may write to it: every POST and DELETE (jobs, pulls,
/// favourites, deletes) and new thumbnails are refused. Reads keep working.
pub async fn block_while_moving(State(s): S, req: Request, next: Next) -> Response {
    if s.mover.active.load(Ordering::SeqCst) {
        let path = req.uri().path();
        let writes = req.method() == Method::POST || req.method() == Method::DELETE;
        let thumbnail = path.ends_with("/file") && req.uri().query().is_some_and(|q| q.contains("w="));
        if (writes || thumbnail) && !path.starts_with("/api/storage/move") {
            return api_err(StatusCode::CONFLICT, "moving", "The data folder is moving to another drive. Wait for it to finish, then try again.");
        }
    }
    next.run(req).await
}

fn active_runtime(s: &AppState) -> Option<String> {
    s.models.runtimes.choice.runtime.clone()
}

pub async fn get_storage(State(s): S) -> Response {
    let s2 = s.clone();
    let report = tokio::task::spawn_blocking(move || storage::report(&s2.cfg, &s2.models.registry(), Some(&s2.engine.db), active_runtime(&s2).as_deref())).await;
    match report {
        Ok(r) => {
            let mv = s.mover.progress.lock().unwrap().clone();
            let mut v = serde_json::to_value(r).unwrap();
            v["move"] = serde_json::to_value(mv).unwrap();
            Json(v).into_response()
        }
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", format!("Couldn't read the data folder ({e}).")),
    }
}

pub async fn clear_cache(State(s): S) -> Response {
    let s2 = s.clone();
    let r = tokio::task::spawn_blocking(move || {
        let items = storage::cache_items(&s2.cfg, active_runtime(&s2).as_deref());
        storage::clear_cache(&s2.cfg, &items)
    })
    .await;
    match r {
        Ok((freed, removed)) => Json(json!({ "freed_bytes": freed, "removed": removed })).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", e.to_string()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialBody {
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    all: bool,
}

/// Deletes paused downloads: one (`file`, as listed in `GET /api/storage`) or `all`. Studio and
/// the CLI ask first. Refused for anything still downloading.
pub async fn delete_partials(State(s): S, body: Result<Json<PartialBody>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else { return api_err(StatusCode::BAD_REQUEST, "invalid_json", "Send {\"file\": \"…\"} or {\"all\": true}.") };
    let list = storage::partials(&s.cfg, &s.models.registry());
    let targets: Vec<_> = match (&b.file, b.all) {
        (Some(f), false) => list.into_iter().filter(|p| &p.file == f).collect(),
        (None, true) => list,
        _ => return api_err(StatusCode::BAD_REQUEST, "invalid_request", "Send either {\"file\": \"…\"} or {\"all\": true}."),
    };
    if targets.is_empty() {
        return api_err(StatusCode::NOT_FOUND, "not_found", "There's no such paused download.");
    }
    let mut deleted = Vec::new();
    let mut freed = 0;
    for p in targets {
        // A `.part` any active pull is writing (shared encoders and VAEs included) is never deleted.
        let busy = if p.owner == "runtime" { s.models.any_pulling() } else { s.models.part_in_use(&s.cfg.data_dir.join(&p.file)) };
        if busy && b.all {
            continue; // "delete all" leaves what's downloading right now
        }
        if busy {
            return api_err(StatusCode::CONFLICT, "model_busy", format!("{} is being downloaded right now. Cancel that download first.", p.file));
        }
        match storage::delete_partial(&s.cfg, &s.models.registry(), &p.file) {
            Ok(n) => {
                freed += n;
                deleted.push(p.file);
            }
            Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, "disk_error", e),
        }
    }
    s.models.refresh();
    Json(json!({ "deleted": deleted, "freed_bytes": freed })).into_response()
}

/// `GET /api/storage/drives`: fixed drives with their space and where the data folder would go
/// on each (first run's "Where models live"; moving there is `POST /api/storage/move`).
pub async fn drives(State(s): S) -> Response {
    let (data, default) = (s.cfg.data_dir.clone(), crate::config::default_data_dir());
    let list = tokio::task::spawn_blocking(move || crate::disk::drives(&data, &default)).await.unwrap_or_default();
    Json(serde_json::json!({ "data_dir": s.cfg.data_dir.display().to_string(), "drives": list })).into_response()
}

pub async fn move_status(State(s): S) -> Response {
    Json(s.mover.progress.lock().unwrap().clone()).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveBody {
    to: PathBuf,
}

/// Starts moving the data folder to `to` (202). Poll `GET /api/storage/move`; when it says
/// `restarting`, the engine comes back on the same port using the new folder.
pub async fn start_move(State(s): S, body: Result<Json<MoveBody>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else { return api_err(StatusCode::BAD_REQUEST, "invalid_json", "Send {\"to\": \"D:\\\\Dawnroom\"}.") };
    if s.engine.is_busy() || s.models.any_pulling() {
        return api_err(StatusCode::CONFLICT, "busy", "Images are developing or a model is downloading. Wait for them to finish (or cancel them), then move.");
    }
    if s.models.runtimes.is_installing() {
        return api_err(StatusCode::CONFLICT, "busy", "The image runtime is installing. Wait for it to finish, then move.");
    }
    if s.mover.active.swap(true, Ordering::SeqCst) {
        return api_err(StatusCode::CONFLICT, "moving", "A move is already running.");
    }
    let from = s.cfg.data_dir.clone();
    let to = b.to.clone();
    let check = {
        let (f, t) = (from.clone(), to.clone());
        tokio::task::spawn_blocking(move || storage::check_move(&f, &t)).await.unwrap_or_else(|e| Err(("storage_error".into(), e.to_string())))
    };
    let total = match check {
        Ok(n) => n,
        Err((code, msg)) => {
            s.mover.active.store(false, Ordering::SeqCst);
            let status = if code == "not_enough_space" { StatusCode::INSUFFICIENT_STORAGE } else { StatusCode::BAD_REQUEST };
            return api_err(status, &code, msg);
        }
    };
    s.mover.set(|p| *p = MoveProgress { state: "copying".into(), bytes: 0, total, from: Some(from.display().to_string()), to: Some(to.display().to_string()), error: None });
    let s2 = s.clone();
    tokio::spawn(async move { run_move(s2, from, to).await });
    (StatusCode::ACCEPTED, Json(s.mover.progress.lock().unwrap().clone())).into_response()
}

async fn run_move(s: Arc<AppState>, from: PathBuf, to: PathBuf) {
    eprintln!("[engine] moving the data folder {} -> {}", from.display(), to.display());
    // Nothing may hold model files open, and the database must be one whole file.
    s.engine.shutdown_backend().await;
    if let Err(e) = s.engine.db.checkpoint() {
        eprintln!("[engine] db checkpoint before the move: {e}");
    }
    let s2 = s.clone();
    let (f, t) = (from.clone(), to.clone());
    let copied = tokio::task::spawn_blocking(move || {
        let never = AtomicBool::new(false);
        let r = storage::copy_tree(&f, &t, &never, &|b| s2.mover.set(|p| p.bytes = b));
        s2.mover.set(|p| p.state = "verifying".into());
        r
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    let switched = copied.and_then(|()| {
        s.mover.set(|p| p.state = "switching".into());
        storage::switch_to(&from, &to)
    });
    match switched {
        Ok(()) => {
            eprintln!("[engine] data folder moved; restarting to use {}", to.display());
            s.mover.set(|p| p.state = "restarting".into());
            // Give pollers a moment to see "restarting", then hand over to a fresh engine.
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            crate::restart_self();
        }
        Err(e) => {
            eprintln!("[engine] the move failed: {e}");
            // Our unfinished copy goes; the old folder was never touched.
            let t = to.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if t.join(".dawnroom-moving").exists() {
                    let _ = std::fs::remove_dir_all(&t);
                }
            })
            .await;
            s.mover.set(|p| {
                p.state = "failed".into();
                p.error = Some(e);
            });
            s.mover.active.store(false, Ordering::SeqCst);
        }
    }
}
