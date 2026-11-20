pub mod api;
pub mod arch;
pub mod auto;
pub mod awake;
pub mod backend;
pub mod config;
pub mod custom;
pub mod db;
pub mod disk;
pub mod download;
pub mod fit;
pub mod hardware;
pub mod header;
pub mod inputs;
#[cfg(windows)]
pub mod job_object;
pub mod logs;
pub mod models;
pub mod png_meta;
pub mod queue;
pub mod registry;
pub mod runtimes;
pub mod sd_server;
pub mod storage;
pub mod storage_api;

use std::future::IntoFuture;
use std::sync::Arc;

use config::{Config, ENGINE_VERSION, PRODUCT_NAME};

/// Runs the engine until Ctrl-C, then stops the sd-server child and waits for it to exit.
/// Starts with zero models installed: `pull` works on a fresh install.
pub async fn serve(mut cfg: Config) -> Result<(), String> {
    // Absolute: sd-server runs in its runtime's folder, so relative model paths would break.
    cfg.data_dir = std::path::absolute(&cfg.data_dir).map_err(|e| format!("Couldn't resolve {} ({e}).", cfg.data_dir.display()))?;
    for dir in [cfg.data_dir.clone(), cfg.models_dir(), cfg.outputs_dir(), cfg.inputs_dir(), cfg.logs_dir(), cfg.custom_dir()] {
        std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {} ({e}).", dir.display()))?;
    }
    // A finished move left the old folder behind: delete it before anything opens files.
    if let Some(msg) = storage::finish_pending_delete(&cfg.data_dir) {
        eprintln!("[engine] old data folder after a move: {msg}");
    }
    // Curated manifests plus the custom models in <data>/custom (a broken one is left out, not fatal).
    let reg = custom::load_registry(&cfg.registry_dir, &cfg.custom_dir())?;
    let hw = hardware::detect().await;
    let runtimes = Arc::new(match &cfg.sd_server {
        Some(exe) => {
            if !exe.is_file() {
                return Err(format!("--sd-server {} doesn't exist.", exe.display()));
            }
            runtimes::Runtimes::fixed(&cfg.data_dir, exe.clone())
        }
        None => {
            let all = runtimes::load_dir(&cfg.runtimes_registry())?;
            if let Some(m) = reg.models.iter().find(|m| !all.iter().any(|r| r.runtime == m.backend)) {
                return Err(format!("{} needs runtime {}, which isn't in {}.", m.id, m.backend, cfg.runtimes_registry().display()));
            }
            runtimes::Runtimes::new(&cfg.data_dir, all, &hw, cfg.gpu.as_deref(), cfg.runtime.as_deref(), None)
        }
    });
    for g in &hw.gpus {
        eprintln!("[engine] GPU: {} ({}, {} MiB{}, driver {})", g.name, g.vendor.label(), g.memory_mib(), if g.integrated { " shared" } else { "" }, g.driver.as_deref().unwrap_or("?"));
    }
    let c = &runtimes.choice;
    eprintln!("[engine] runtime: {} ({:?}). {}{}", c.build.as_deref().unwrap_or("none"), c.status, c.reason, c.message.as_deref().map(|m| format!(" {m}")).unwrap_or_default());
    // The GPU the fit answer uses: the chosen one (nvidia-smi numbers for NVIDIA), else the first NVIDIA.
    let gpu = match c.gpu_index {
        Some(i) => Some(hw.gpus[i].as_gpu()),
        None if runtimes.is_override() => hardware::nvidia_live().await.map(|g| hardware::Gpu { vram_used_mib: 0, ..g }),
        None => None,
    };
    let integrated = c.gpu_index.is_some_and(|i| hw.gpus[i].integrated);
    let machine = gpu.as_ref().map(|g| fit::Machine { gpu_mib: g.vram_total_mib, integrated, ram_total_mib: hw.ram.total_bytes >> 20 });

    let db = Arc::new(db::Db::open(&cfg.db_path()).map_err(|e| format!("Couldn't open history at {} ({e}).", cfg.db_path().display()))?);
    match db.rebase_outputs(&cfg.outputs_dir()) {
        Ok(0) => {}
        Ok(n) => eprintln!("[engine] {n} image path(s) now point into {}", cfg.outputs_dir().display()),
        Err(e) => eprintln!("[engine] db: {e}"),
    }
    match db.fail_stale(queue::now_ms()) {
        Ok(0) => {}
        Ok(n) => eprintln!("[engine] marked {n} unfinished job(s) from a previous run as failed"),
        Err(e) => eprintln!("[engine] db: {e}"),
    }

    let models = models::Models::new(reg, gpu, machine, cfg.models_dir(), db.clone(), &cfg.hf_endpoint, runtimes);
    // Hash files the cache doesn't know yet, without blocking start-up or the API.
    tokio::spawn(models.clone().verify_all());

    let backend = Arc::new(sd_server::SdServerBackend::new(&cfg));
    let engine = queue::Engine::start(backend, db, models.clone(), cfg.outputs_dir(), cfg.idle_timeout);

    // The thumbnail cache stays near its cap even if nobody opens Settings.
    let thumbs = cfg.thumbs_dir();
    tokio::task::spawn_blocking(move || storage::cap_thumbs(&thumbs, storage::THUMBS_CAP_BYTES));

    let listener = bind(cfg.port)
        .await
        .map_err(|e| format!("Couldn't listen on 127.0.0.1:{} ({e}). Is the engine already running? Otherwise stop the other program using it or pass --port.", cfg.port))?;
    println!("{PRODUCT_NAME} engine {ENGINE_VERSION} on http://127.0.0.1:{}  (data: {}, idle stop after {}s)", cfg.port, cfg.data_dir.display(), cfg.idle_timeout.as_secs());

    if cfg.hf_endpoint != "https://huggingface.co" {
        eprintln!("[engine] model downloads come from {}", cfg.hf_endpoint);
    }
    let quit = Arc::new(tokio::sync::Notify::new());
    let app = api::router(api::AppState {
        engine: engine.clone(),
        models,
        port: cfg.port,
        cfg: cfg.clone(),
        hardware: hw,
        mover: Default::default(),
        thumbs_made: Default::default(),
        openai_fallback_logged: Default::default(),
        quit: quit.clone(),
    });
    let result = tokio::select! {
        r = axum::serve(listener, app).into_future() => r.map_err(|e| e.to_string()),
        _ = tokio::signal::ctrl_c() => { eprintln!("[engine] Ctrl-C received"); Ok(()) }
        _ = quit.notified() => Ok(()),
    };
    // Explicit kill + wait; kill_on_drop and the Job Object are only the backstops.
    engine.shutdown().await;
    eprintln!("[engine] shut down");
    result
}

/// Set on the engine a move restarts, so it waits for the old one to let go of the port.
const RESTARTED_ENV: &str = "DAWNROOM_RESTARTED";

async fn bind(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    let retry = std::env::var_os(RESTARTED_ENV).is_some();
    let mut tries = 0;
    loop {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Err(_) if retry && tries < 60 => {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            r => return r,
        }
    }
}

/// Starts a fresh engine with the same arguments (it reads the new data folder from the config
/// file), then exits this one. Used after a data-folder move: the database and logs stay open
/// otherwise. Output goes where this engine's went (Studio's engine.log, a terminal).
pub fn restart_self() -> ! {
    let exe = std::env::current_exe().expect("current exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.args(std::env::args_os().skip(1)).env(RESTARTED_ENV, "1").stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Detached and windowless, like Studio starts it; it outlives this process.
        cmd.creation_flags(0x0000_0200 /* CREATE_NEW_PROCESS_GROUP */ | 0x0800_0000 /* CREATE_NO_WINDOW */);
    }
    match cmd.spawn() {
        Ok(c) => eprintln!("[engine] restarted as pid {}", c.id()),
        Err(e) => eprintln!("[engine] couldn't restart ({e}); start the engine again"),
    }
    std::process::exit(0)
}
