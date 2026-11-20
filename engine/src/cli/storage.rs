//! `dawnroom storage`: what the data folder holds, Clear cache, paused downloads, moving the
//! folder and relinking dev model files. With the engine running it goes through the API (so the
//! numbers are Studio's); with it stopped it calls the same library functions itself, which is
//! what the installer will use.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use clap::Subcommand;
use serde_json::{Value, json};

use engine::config::Config;
use engine::disk::human;
use engine::storage;

use super::{Client, Fail, R, pad};

#[derive(Subcommand)]
pub enum StorageCmd {
    /// Usage by kind, models, paused downloads and what Clear cache would remove (the default).
    Show,
    /// Remove rebuildable things only: thumbnails, rotated logs, old runtimes. Asks first.
    ClearCache {
        #[arg(short, long)]
        yes: bool,
    },
    /// Delete paused downloads (.part): one by its file name, or --all. Asks first.
    DeletePartial {
        file: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(short, long)]
        yes: bool,
    },
    /// Move the data folder to another folder or drive (copies, checks, switches, deletes the old one).
    Move { to: PathBuf },
    /// Hardlink model files found under SOURCE into the models folder (checked by sha256; same
    /// drive only; engine stopped). The CLI form of link-models.ps1.
    Link { source: PathBuf },
}

async fn engine_up(c: &Client) -> bool {
    c.get("/api/health").await.is_ok()
}

fn local_report(cfg: &Config) -> R<Value> {
    let reg = engine::custom::load_registry(&cfg.registry_dir, &cfg.custom_dir()).map_err(|e| Fail::new("registry_error", e))?;
    let db = if cfg.db_path().is_file() { engine::db::Db::open(&cfg.db_path()).ok() } else { None };
    let mut v = serde_json::to_value(storage::report(cfg, &reg, db.as_ref(), None)).unwrap();
    v["move"] = json!({ "state": "idle", "bytes": 0, "total": 0 });
    Ok(v)
}

async fn report(c: &Client, cfg: &Config) -> R<Value> {
    if engine_up(c).await { c.get("/api/storage").await } else { local_report(cfg) }
}

fn confirm(c: &Client, yes: bool, question: &str) -> R<()> {
    if yes {
        return Ok(());
    }
    if c.json {
        return Err(Fail::new("confirm_needed", "Add --yes to confirm (needed with --json)."));
    }
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    if line.trim().eq_ignore_ascii_case("y") { Ok(()) } else { Err(Fail::new("cancelled", "Nothing was changed.")) }
}

pub async fn run(c: &Client, cmd: Option<&StorageCmd>) -> R<Value> {
    let cfg = Config::resolved();
    match cmd.unwrap_or(&StorageCmd::Show) {
        StorageCmd::Show => {
            let v = report(c, &cfg).await?;
            if !c.json {
                print_report(c, &v);
            }
            Ok(v)
        }
        StorageCmd::ClearCache { yes } => {
            let v = report(c, &cfg).await?;
            let items = v["cache"].as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                if !c.json {
                    println!("Nothing to clear.");
                }
                return Ok(json!({ "freed_bytes": 0, "removed": [] }));
            }
            if !c.json {
                for it in &items {
                    println!("  {}  {}  {}", pad(it["kind"].as_str().unwrap_or(""), 16), pad(&human(it["bytes"].as_u64().unwrap_or(0)), 9), it["path"].as_str().unwrap_or(""));
                }
            }
            confirm(c, *yes, &format!("Remove these ({})? Models, images and paused downloads stay.", human(v["usage"]["cache"].as_u64().unwrap_or(0))))?;
            let r = if engine_up(c).await {
                c.send(c.post("/api/storage/clear-cache", &json!({}))).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?
            } else {
                let items = storage::cache_items(&cfg, None);
                let (freed, removed) = storage::clear_cache(&cfg, &items);
                json!({ "freed_bytes": freed, "removed": removed })
            };
            if !c.json {
                println!("{} {}", c.st.accent("Freed"), human(r["freed_bytes"].as_u64().unwrap_or(0)));
            }
            Ok(r)
        }
        StorageCmd::DeletePartial { file, all, yes } => {
            if file.is_some() == *all {
                return Err(Fail::new("invalid_request", "Name one paused download, or pass --all."));
            }
            let what = file.clone().unwrap_or_else(|| "every paused download".into());
            confirm(c, *yes, &format!("Delete {what}? Pulling the model again starts it from zero."))?;
            let body = match file {
                Some(f) => json!({ "file": f }),
                None => json!({ "all": true }),
            };
            let r = if engine_up(c).await {
                c.send(c.post("/api/storage/partials/delete", &body)).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?
            } else {
                let reg = engine::custom::load_registry(&cfg.registry_dir, &cfg.custom_dir()).map_err(|e| Fail::new("registry_error", e))?;
                let targets: Vec<String> = storage::partials(&cfg, &reg).into_iter().map(|p| p.file).filter(|p| file.as_ref().is_none_or(|f| f == p)).collect();
                if targets.is_empty() {
                    return Err(Fail::new("not_found", "There's no such paused download."));
                }
                let mut freed = 0;
                for t in &targets {
                    freed += storage::delete_partial(&cfg, &reg, t).map_err(|e| Fail::new("disk_error", e))?;
                }
                json!({ "deleted": targets, "freed_bytes": freed })
            };
            if !c.json {
                println!("{} {}", c.st.accent("Deleted"), human(r["freed_bytes"].as_u64().unwrap_or(0)));
            }
            Ok(r)
        }
        StorageCmd::Move { to } => {
            let to = std::path::absolute(to).unwrap_or(to.clone());
            if engine_up(c).await { move_with_engine(c, &to).await } else { move_offline(c, &cfg, &to) }
        }
        StorageCmd::Link { source } => {
            if engine_up(c).await {
                return Err(Fail::new("engine_running", "Stop the engine first: linking replaces model files it may have open."));
            }
            let reg = engine::custom::load_registry(&cfg.registry_dir, &cfg.custom_dir()).map_err(|e| Fail::new("registry_error", e))?;
            if !c.json {
                eprintln!("{}", c.st.muted("Checking sha256 of each match before linking…"));
            }
            let results = storage::link_models(&cfg.models_dir(), &reg, source);
            if !c.json {
                for r in &results {
                    println!("  {}  {}{}", pad(&r.result, 15), r.file, r.detail.as_ref().map(|d| format!("  ({d})")).unwrap_or_default());
                }
            }
            Ok(json!({ "results": results }))
        }
    }
}

async fn move_with_engine(c: &Client, to: &Path) -> R<Value> {
    let start: Value = c.send(c.post("/api/storage/move", &json!({ "to": to }))).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
    let total = start["total"].as_u64().unwrap_or(0);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let Ok(p) = c.get("/api/storage/move").await else {
            break; // the engine is restarting into the new folder
        };
        if !c.json {
            let b = p["bytes"].as_u64().unwrap_or(0);
            c.st.line(&format!("  {} {}  {} / {}", c.st.bar(b as f64 / total.max(1) as f64, 24), p["state"].as_str().unwrap_or(""), human(b), human(total)));
        }
        match p["state"].as_str() {
            Some("failed") => return Err(Fail::new("move_failed", p["error"].as_str().unwrap_or("The move failed.").to_string())),
            Some("restarting") => break,
            _ => {}
        }
    }
    if !c.json {
        c.st.end_line();
        eprintln!("{}", c.st.muted("Waiting for the engine to come back…"));
    }
    // The old engine answers for a moment longer; wait for the one on the new folder.
    let from = start["from"].as_str().unwrap_or("").to_string();
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let info = c.get("/api/info").await.ok().filter(|i| i["data_dir"].as_str() != Some(from.as_str()));
        if let Some(info) = info {
            if !c.json {
                println!("{} the data folder is now {}", c.st.accent("Moved:"), info["data_dir"].as_str().unwrap_or(""));
            }
            return Ok(json!({ "moved": true, "data_dir": info["data_dir"] }));
        }
    }
    Err(Fail::new("engine_not_back", format!("The data was moved to {}, but the engine didn't come back. Start it again.", to.display())))
}

fn move_offline(c: &Client, cfg: &Config, to: &Path) -> R<Value> {
    let total = storage::check_move(&cfg.data_dir, to).map_err(|(code, msg)| Fail::new(&code, msg))?;
    let st = &c.st;
    let json = c.json;
    storage::copy_tree(&cfg.data_dir, to, &AtomicBool::new(false), &|b| {
        if !json {
            st.line(&format!("  {} copying  {} / {}", st.bar(b as f64 / total.max(1) as f64, 24), human(b), human(total)));
        }
    })
    .map_err(|e| Fail::new("move_failed", e))?;
    if !json {
        st.end_line();
    }
    storage::switch_to(&cfg.data_dir, to).map_err(|e| Fail::new("move_failed", e))?;
    let deleted = storage::finish_pending_delete(to);
    if !json {
        println!("{} the data folder is now {} (old folder: {})", st.accent("Moved:"), to.display(), deleted.as_deref().unwrap_or("kept"));
    }
    Ok(json!({ "moved": true, "data_dir": to, "old": deleted }))
}

fn print_report(c: &Client, v: &Value) {
    let u = &v["usage"];
    let g = |k: &str| human(u[k].as_u64().unwrap_or(0));
    println!("{} {}", c.st.bold("Data folder"), v["data_dir"].as_str().unwrap_or(""));
    if let Some(free) = v["drive"]["free_bytes"].as_u64() {
        println!("  {} free of {} on this drive", human(free), human(v["drive"]["total_bytes"].as_u64().unwrap_or(0)));
    }
    println!("  models {}   images {}   runtimes {}   cache {}   paused downloads {}   other {}", g("models"), g("images"), g("runtimes"), g("cache"), g("partial"), g("other"));
    let w = v["models"].as_array().into_iter().flatten().map(|m| m["id"].as_str().unwrap_or("").len() + 2).max().unwrap_or(0).max(17);
    println!("{}", c.st.muted(&format!("{}{}{}", pad("MODEL", w), pad("ON DISK", 10), "LAST USED")));
    for m in v["models"].as_array().into_iter().flatten() {
        let last = m["last_used_ms"].as_i64().map(ago).unwrap_or_else(|| "never".into());
        let shared = if m["shared"].as_array().is_some_and(|a| !a.is_empty()) { "  (shares files)" } else { "" };
        println!("{}{}{}{shared}", pad(m["id"].as_str().unwrap_or(""), w), pad(&human(m["bytes"].as_u64().unwrap_or(0)), 10), last);
    }
    for p in v["partials"].as_array().into_iter().flatten() {
        println!("  {} {} ({}, {})", c.st.muted("paused"), p["file"].as_str().unwrap_or(""), p["owner"].as_str().unwrap_or(""), human(p["bytes"].as_u64().unwrap_or(0)));
    }
}

fn ago(ms: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
    let s = (now - ms).max(0) / 1000;
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}
