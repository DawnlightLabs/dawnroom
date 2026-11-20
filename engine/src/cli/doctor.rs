//! `dawnroom doctor`: everything a bug report needs, as Markdown ready to paste into a GitHub
//! issue (or one JSON document with `--json`). Works without a running engine (hardware and
//! runtime are detected here); with one it adds models and the engine's version. `--test` runs
//! one Fast image and reports its time and peak GPU memory.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use engine::config::{Config, ENGINE_VERSION, PRODUCT_NAME};
use engine::disk::human;

use super::{Client, Fail, R};

const TEST_PROMPT: &str = "a lighthouse on a cliff at dawn, film photograph";

pub async fn doctor(c: &Client, test: bool, cfg: &Config) -> R<Value> {
    let engine = c.get("/api/info").await.ok();
    let hardware = match &engine {
        Some(_) => c.get("/api/hardware").await?,
        None => local_hardware(cfg).await?,
    };
    let models = match &engine {
        Some(_) => c.get("/api/models").await.ok(),
        None => None,
    };
    let installed: Vec<String> = match &models {
        Some(m) => m["models"].as_array().cloned().unwrap_or_default().iter().filter(|m| m["state"] == "installed").filter_map(|m| m["id"].as_str().map(String::from)).collect(),
        None => local_models(cfg),
    };
    let test_result = if test {
        if engine.is_none() {
            return Err(Fail::new("engine_not_running", format!("`doctor --test` needs the engine. Start it with: {} serve", env!("CARGO_BIN_NAME"))));
        }
        Some(run_test(c, &hardware).await?)
    } else {
        None
    };
    let report = json!({
        "cli_version": ENGINE_VERSION,
        "engine": engine,
        "hardware": hardware,
        "installed_models": installed,
        "test": test_result,
    });
    if !c.json {
        println!("{}", markdown(&report));
    }
    Ok(if c.json { report } else { Value::Null })
}

/// Installed models read straight from the data folder, for when the engine isn't running.
/// Uses the engine's hash cache (never hashes): files present but not yet checked say so.
fn local_models(cfg: &Config) -> Vec<String> {
    let Ok(reg) = engine::custom::load_registry(&cfg.registry_dir, &cfg.custom_dir()) else { return vec![] };
    let db_path = cfg.db_path();
    let db = if db_path.is_file() { engine::db::Db::open(&db_path).ok() } else { None };
    let Some(db) = db.or_else(|| engine::db::Db::open_in_memory().ok()) else { return vec![] };
    let rt = std::sync::Arc::new(engine::runtimes::Runtimes::fixed(&cfg.data_dir, "unused".into()));
    let models = engine::models::Models::new(reg, None, None, cfg.models_dir(), std::sync::Arc::new(db), "http://unused", rt);
    models
        .registry()
        .models
        .iter()
        .filter_map(|m| match models.model_state(m) {
            "installed" => Some(m.id.clone()),
            "verifying" => Some(format!("{} (files present, not checked yet)", m.id)),
            _ => None,
        })
        .collect()
}

/// The same report the engine's `GET /api/hardware` gives, detected in this process.
async fn local_hardware(cfg: &Config) -> R<Value> {
    let hw = engine::hardware::detect().await;
    let all = engine::runtimes::load_dir(&cfg.runtimes_registry()).map_err(|e| Fail::new("registry_error", e))?;
    let rt = engine::runtimes::Runtimes::new(&cfg.data_dir, all, &hw, cfg.gpu.as_deref(), cfg.runtime.as_deref(), None);
    Ok(engine::api::hardware_report(&hw, &rt, &cfg.data_dir).await)
}

fn gib(mib: u64) -> String {
    format!("{:.1} GB", mib as f64 / 1024.0)
}

fn markdown(r: &Value) -> String {
    let h = &r["hardware"];
    let s = |v: &Value| v.as_str().unwrap_or("?").to_string();
    let mut out = vec![format!("### {PRODUCT_NAME} doctor"), String::new()];
    let engine = match r["engine"].as_object() {
        Some(e) => format!("engine {} running on port {}", e["version"].as_str().unwrap_or("?"), e["port"]),
        None => "engine not running (hardware detected by the CLI)".into(),
    };
    out.push(format!("- **{PRODUCT_NAME}**: CLI {}, {engine}", s(&r["cli_version"])));
    out.push(format!("- **OS**: {} ({})", s(&h["os_version"]), s(&h["os"])));
    out.push(format!("- **CPU**: {}", s(&h["cpu"])));
    let ram = &h["ram"];
    out.push(format!("- **RAM**: {} total, {} available", human(ram["total_bytes"].as_u64().unwrap_or(0)), human(ram["available_bytes"].as_u64().unwrap_or(0))));
    if let Some(sim) = h["simulated"].as_str() {
        out.push(format!("- **Simulated**: `DAWNROOM_SIMULATE={sim}` (not the real hardware)"));
    }
    out.push(String::new());
    out.push("| GPU | Vendor | Memory | Driver | Kind |".into());
    out.push("|---|---|---|---|---|".into());
    let chosen = h["choice"]["gpu_index"].as_u64();
    for (i, g) in h["gpus"].as_array().into_iter().flatten().enumerate() {
        let integrated = g["integrated"] == true;
        let mem = if integrated {
            format!("{} shared{}", gib(g["vram_mib"].as_u64().unwrap_or(0) + g["shared_mib"].as_u64().unwrap_or(0)), if g["memory_estimated"] == true { " (estimated)" } else { "" })
        } else {
            gib(g["vram_mib"].as_u64().unwrap_or(0))
        };
        let mark = if chosen == Some(i as u64) { " **(used)**" } else { "" };
        out.push(format!("| {}{mark} | {} | {mem} | {} | {} |", s(&g["name"]), s(&g["vendor"]), g["driver"].as_str().unwrap_or("not found"), if integrated { "integrated" } else { "discrete" }));
    }
    if h["gpus"].as_array().is_none_or(|a| a.is_empty()) {
        out.push("| none found | | | | |".into());
    }
    out.push(String::new());
    let ch = &h["choice"];
    let build = ch["build"].as_str().unwrap_or("none");
    let exp = if ch["experimental"] == true { ", experimental" } else { "" };
    out.push(format!("- **Runtime**: `{build}` ({}{exp}). {}", s(&ch["status"]), s(&ch["reason"])));
    if let Some(m) = ch["message"].as_str() {
        out.push(format!("- **To do**: {m}{}", ch["link"].as_str().map(|l| format!(" {l}")).unwrap_or_default()));
    }
    let rt = &h["runtime"];
    if h["sd_server_override"] == true {
        out.push("- **Runtime files**: `--sd-server` override".into());
    } else if rt.is_object() {
        if rt["installed"] == true {
            out.push(format!("- **Runtime files**: installed at `{}`", s(&rt["path"])));
        } else {
            out.push(format!("- **Runtime files**: not installed yet ({} to download, {} on disk)", human(rt["download_bytes"].as_u64().unwrap_or(0)), human(rt["unpacked_bytes"].as_u64().unwrap_or(0))));
        }
    }
    let devices: Vec<String> = h["devices"].as_array().into_iter().flatten().map(|d| format!("`{}` {}", s(&d["name"]), s(&d["description"]))).collect();
    if !devices.is_empty() {
        out.push(format!("- **Runtime sees**: {}", devices.join(", ")));
    }
    if h["runtime_sees_gpu"] == false {
        out.push("- **Warning**: the runtime doesn't list the chosen GPU. Update its graphics driver.".into());
    }
    let disk = &h["disk"];
    out.push(format!(
        "- **Data folder**: `{}`, {} free of {}",
        s(&disk["path"]),
        human(disk["space"]["free_bytes"].as_u64().unwrap_or(0)),
        human(disk["space"]["total_bytes"].as_u64().unwrap_or(0))
    ));
    let models: Vec<&str> = r["installed_models"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect();
    out.push(format!("- **Installed models**: {}", if models.is_empty() { "none".into() } else { models.join(", ") }));
    if let Some(t) = r["test"].as_object() {
        out.push(String::new());
        out.push(format!(
            "**Test image** (Fast, {}, 1024x1024, seed 42): {}",
            t["model"].as_str().unwrap_or("?"),
            match t["status"].as_str() {
                Some("completed") => format!("{:.1} s{}", t["seconds"].as_f64().unwrap_or(0.0), t["peak_note"].as_str().map(|n| format!(", {n}")).unwrap_or_default()),
                Some(other) => format!("{other}: {}", t["error"].as_str().unwrap_or("")),
                None => "?".into(),
            }
        ));
    }
    out.join("\n")
}

// ---------- --test ----------

async fn run_test(c: &Client, hw: &Value) -> R<Value> {
    let chosen = hw["choice"]["gpu_index"].as_u64().and_then(|i| hw["gpus"].get(i as usize)).cloned().unwrap_or(Value::Null);
    let sampler = Sampler::start(&chosen).await;
    tokio::time::sleep(Duration::from_millis(1500)).await; // a baseline sample before the job
    let job: Value = c.send(c.post("/api/jobs", &json!({ "prompt": TEST_PROMPT, "preset": "fast", "seed": 42 }))).await?.json().await.map_err(|e| Fail::new("bad_response", e.to_string()))?;
    let id = job["id"].as_str().unwrap_or("").to_string();
    let t0 = Instant::now();
    if !c.json {
        eprintln!("{}", c.st.muted("Developing a Fast test image…"));
    }
    let done = loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let j = c.get(&format!("/api/jobs/{id}")).await?;
        if matches!(j["status"].as_str(), Some("completed" | "failed" | "cancelled")) {
            break j;
        }
        if t0.elapsed() > Duration::from_secs(600) {
            return Err(Fail::new("timeout", "The test image took more than 10 minutes. Check the engine log."));
        }
    };
    let peak = sampler.stop().await;
    let seconds = match (done["started_ms"].as_i64(), done["finished_ms"].as_i64()) {
        (Some(a), Some(b)) => (b - a) as f64 / 1000.0,
        _ => t0.elapsed().as_secs_f64(),
    };
    let peak_note = peak.as_ref().map(|p| format!("GPU memory in use peaked at {} ({} was in use before the job, including any model already loaded; {})", gib(p.peak_mib), gib(p.baseline_mib), p.source));
    Ok(json!({
        "status": done["status"], "error": done["error"], "model": done["model"], "seconds": seconds,
        "path": done["output_path"],
        "peak_mib": peak.as_ref().map(|p| p.peak_mib), "baseline_mib": peak.as_ref().map(|p| p.baseline_mib),
        "peak_source": peak.as_ref().map(|p| p.source.clone()),
        "peak_note": peak_note.unwrap_or_else(|| "peak memory not measured on this platform".into()),
    }))
}

struct Peak {
    peak_mib: u64,
    baseline_mib: u64,
    source: String,
}

/// Samples the chosen GPU's memory in use while the test runs. Windows: the "GPU Adapter Memory"
/// performance counters (what Task Manager shows; dedicated + shared, any vendor), matched by
/// adapter LUID. Elsewhere: nvidia-smi for NVIDIA, nothing otherwise.
struct Sampler {
    child: Option<tokio::process::Child>,
    task: Option<tokio::task::JoinHandle<Vec<u64>>>,
    source: String,
}

impl Sampler {
    async fn start(gpu: &Value) -> Self {
        use tokio::io::AsyncBufReadExt;
        let luid = gpu["luid"].as_str().map(String::from);
        let nvidia = gpu["vendor"] == "nvidia";
        let (mut cmd, source) = if let (true, Some(l)) = (cfg!(windows), &luid) {
            let script = format!(
                "Get-Counter -Counter '\\GPU Adapter Memory(luid_{l}_phys_0)\\Dedicated Usage','\\GPU Adapter Memory(luid_{l}_phys_0)\\Shared Usage' -SampleInterval 1 -Continuous | ForEach-Object {{ [Console]::WriteLine([long](($_.CounterSamples | Measure-Object -Property CookedValue -Sum).Sum / 1MB)) }}"
            );
            let mut c = tokio::process::Command::new("powershell");
            c.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
            (c, "Windows GPU memory counters, dedicated + shared".to_string())
        } else if nvidia {
            let mut c = tokio::process::Command::new("nvidia-smi");
            c.args(["--query-gpu=memory.used", "--format=csv,noheader,nounits", "-lms", "250"]);
            (c, "nvidia-smi".to_string())
        } else {
            return Self { child: None, task: None, source: String::new() };
        };
        cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).stdin(std::process::Stdio::null()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000 /* CREATE_NO_WINDOW */);
        let Ok(mut child) = cmd.spawn() else { return Self { child: None, task: None, source } };
        let stdout = child.stdout.take().unwrap();
        let task = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            let mut v = Vec::new();
            while let Ok(Some(l)) = lines.next_line().await {
                if let Ok(n) = l.trim().parse::<u64>() {
                    v.push(n);
                }
            }
            v
        });
        Self { child: Some(child), task: Some(task), source }
    }

    async fn stop(mut self) -> Option<Peak> {
        let mut child = self.child.take()?;
        tokio::time::sleep(Duration::from_millis(1200)).await; // one more sample after the job
        let _ = child.kill().await;
        let samples = self.task.take()?.await.ok()?;
        Some(Peak { baseline_mib: *samples.first()?, peak_mib: *samples.iter().max()?, source: self.source })
    }
}
