use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The product name lives here and only here (Dawnroom, by Dawnlight Labs).
pub const PRODUCT_NAME: &str = "Dawnroom";
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_PORT: u16 = 8420;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub data_dir: PathBuf,
    /// The engine always binds 127.0.0.1; only the port is configurable.
    pub port: u16,
    /// `--sd-server`: use this sd-server exe as is instead of the managed runtime (dev, odd setups).
    pub sd_server: Option<PathBuf>,
    /// `--runtime`: force a runtime build, e.g. `windows-vulkan` (testing the AMD/Intel path on NVIDIA).
    pub runtime: Option<String>,
    /// `--gpu`: use the GPU whose name contains this (e.g. `arc` for the iGPU of a hybrid laptop).
    pub gpu: Option<String>,
    /// Folder of model manifests (`*.toml`); runtime manifests are in its `runtimes/`. Dev default: the repo's `registry/`.
    pub registry_dir: PathBuf,
    /// Stop the sd-server child after this long with no jobs.
    pub idle_timeout: Duration,
    /// Where model files download from. `HF_ENDPOINT` (the Hugging Face convention, e.g. a
    /// mirror) or `--hf-endpoint` overrides it; tests point it at a local fixture server.
    pub hf_endpoint: String,
}

impl Default for Config {
    /// Dev defaults: everything lives in the repo (`.data/` is gitignored).
    fn default() -> Self {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        Self {
            data_dir: default_data_dir(),
            port: DEFAULT_PORT,
            sd_server: None,
            runtime: None,
            gpu: None,
            registry_dir: default_registry_dir(&repo),
            idle_timeout: Duration::from_secs(5 * 60),
            hf_endpoint: std::env::var("HF_ENDPOINT").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| "https://huggingface.co".into()),
        }
    }
}

impl Config {
    /// The defaults plus the per-user config file: where the data folder is after a move.
    /// Every real entry point (serve, CLI, MCP, doctor) starts from this.
    pub fn resolved() -> Self {
        let mut cfg = Self::default();
        if let Some(d) = UserConfig::load().data_dir {
            cfg.data_dir = d;
        }
        cfg
    }

    /// Parses `--data-dir`, `--port`, `--sd-server`, `--runtime`, `--gpu`, `--registry-dir`, `--idle-secs`, `--hf-endpoint` on top of the defaults.
    pub fn from_args(args: &[String]) -> Result<Self, String> {
        let mut cfg = Self::resolved();
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--data-dir" => cfg.data_dir = PathBuf::from(value()?),
                "--sd-server" => cfg.sd_server = Some(PathBuf::from(value()?)),
                "--runtime" => cfg.runtime = Some(value()?.to_string()),
                "--gpu" => cfg.gpu = Some(value()?.to_string()),
                "--registry-dir" => cfg.registry_dir = PathBuf::from(value()?),
                "--hf-endpoint" => cfg.hf_endpoint = value()?.trim_end_matches('/').to_string(),
                "--port" => cfg.port = value()?.parse().map_err(|_| "--port must be a number from 1 to 65535".to_string())?,
                "--idle-secs" => {
                    let secs: u64 = value()?.parse().map_err(|_| "--idle-secs must be a whole number of seconds".to_string())?;
                    cfg.idle_timeout = Duration::from_secs(secs);
                }
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(cfg)
    }

    pub fn models_dir(&self) -> PathBuf { self.data_dir.join("models") }
    pub fn outputs_dir(&self) -> PathBuf { self.data_dir.join("outputs") }
    /// Edit inputs, by sha256 (inputs.rs). Images the user gave; never sent anywhere.
    pub fn inputs_dir(&self) -> PathBuf { self.data_dir.join("inputs") }
    pub fn logs_dir(&self) -> PathBuf { self.data_dir.join("logs") }
    /// JPEG thumbnails of outputs, made on demand; safe to delete.
    pub fn thumbs_dir(&self) -> PathBuf { self.data_dir.join("thumbs") }
    pub fn db_path(&self) -> PathBuf { self.data_dir.join("history.sqlite") }
    /// Custom model manifests (hf.co pulls and imports); never the repo's `registry/`.
    pub fn custom_dir(&self) -> PathBuf { self.data_dir.join("custom") }
    pub fn runtimes_registry(&self) -> PathBuf { self.registry_dir.join("runtimes") }
}

/// The model and runtime manifests. Debug builds read the repo's `registry/`. An installed engine
/// reads the copy the installer put next to it: `registry/` beside the exe (Windows), or
/// `../lib/Dawnroom/registry` (Linux .deb and AppImage, where Tauri puts resources). The repo path
/// stays the last resort, for a release build run from `target/`.
fn default_registry_dir(repo: &Path) -> PathBuf {
    if cfg!(debug_assertions) {
        return repo.join("registry");
    }
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    exe_dir
        .into_iter()
        .flat_map(|d| [d.join("registry"), d.join("..").join("lib").join(PRODUCT_NAME).join("registry")])
        .find(|d| d.join("runtimes").is_dir())
        .unwrap_or_else(|| repo.join("registry"))
}

/// Where the data folder lives when nobody moved it. Debug builds keep everything in the repo
/// (`.data/`, gitignored); a release build uses the per-user data folder.
pub fn default_data_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        return Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join(".data");
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
    };
    base.unwrap_or_else(std::env::temp_dir).join(if cfg!(windows) { PRODUCT_NAME } else { "dawnroom" })
}

/// The engine's own per-user config file: `%APPDATA%\Dawnroom\config.toml` on Windows,
/// `~/.config/dawnroom/config.toml` on Linux. `DAWNROOM_CONFIG` replaces the path (tests use it so
/// the real file is never touched).
pub fn user_config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DAWNROOM_CONFIG").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let dir = if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(PRODUCT_NAME))
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).map(|d| d.join("dawnroom"))
    };
    dir.map(|d| d.join("config.toml"))
}

/// What the config file holds. Studio's shell passes nothing; the engine reads this itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserConfig {
    /// The data folder, once it has been moved away from the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PathBuf>,
    /// The old data folder after a move: deleted at the next start, once nothing holds it open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_after_move: Option<PathBuf>,
}

impl UserConfig {
    /// The file's contents, or the defaults if it's missing or unreadable.
    pub fn load() -> Self {
        user_config_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    /// Writes the file atomically (temp file + rename), so a crash never leaves half a config.
    pub fn save(&self) -> Result<(), String> {
        let path = user_config_path().ok_or("There's no per-user config folder on this system.")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {} ({e}).", dir.display()))?;
        }
        let text = format!("# Written by the {PRODUCT_NAME} engine. Studio and the CLI change it with \"storage move\".\n{}", toml::to_string(self).map_err(|e| e.to_string())?);
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("Couldn't write {} ({e}).", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("Couldn't write {} ({e}).", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> { s.split_whitespace().map(String::from).collect() }

    #[test]
    fn defaults_point_into_the_repo() {
        let cfg = Config::default();
        assert!(cfg.data_dir.ends_with(".data"));
        assert_eq!(cfg.sd_server, None, "the managed runtime is the default");
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert_eq!(cfg.idle_timeout, Duration::from_secs(300));
    }

    #[test]
    fn flags_override_defaults() {
        let cfg = Config::from_args(&args("--port 9000 --idle-secs 20 --data-dir D:/x --runtime windows-vulkan --gpu arc")).unwrap();
        assert_eq!((cfg.runtime.as_deref(), cfg.gpu.as_deref()), (Some("windows-vulkan"), Some("arc")));
        assert_eq!(cfg.port, 9000);
        assert_eq!(cfg.idle_timeout, Duration::from_secs(20));
        assert_eq!(cfg.data_dir, PathBuf::from("D:/x"));
    }

    #[test]
    fn bad_flags_are_rejected() {
        assert!(Config::from_args(&args("--port abc")).is_err());
        assert!(Config::from_args(&args("--port")).is_err());
        assert!(Config::from_args(&args("--nope 1")).is_err());
    }
}
