//! The one planned seam: anything that can turn a request into PNG bytes.

use async_trait::async_trait;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct GenRequest {
    /// Which model to run it on. The backend switches models when this differs from the loaded one.
    #[serde(skip)]
    pub model: ModelSpec,
    /// Set to ask the backend to stop this job; it then returns the `cancelled` error.
    #[serde(skip)]
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// generate | edit (recorded in the job's params).
    pub task: crate::registry::Task,
    /// Edits: the sha256 of each input image, the image to edit first, then the references.
    /// Recorded in the job's params and the PNG's recipe; the files stay in `<data>/inputs/`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    /// The stored files of `inputs`, in the same order: sent to sd-server as `ref_images`.
    #[serde(skip)]
    pub input_paths: Vec<std::path::PathBuf>,
    pub prompt: String,
    pub negative_prompt: String,
    pub width: u32,
    pub height: u32,
    pub seed: i64,
    pub steps: u32,
    pub cfg: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampler: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distilled_guidance: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flow_shift: Option<f32>,
    /// A fixed sigma schedule from the model's recipe; its length sets the real step count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigmas: Option<Vec<f32>>,
}

/// What the backend needs to load a model: its id, its files, the runtime's server exe and the
/// full launch arguments (runtime args such as `--backend Vulkan1` first, then the model's).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelSpec {
    pub id: String,
    /// Set by the queue from the installed runtime just before the job runs.
    pub exe: std::path::PathBuf,
    pub files: Vec<std::path::PathBuf>,
    pub args: Vec<String>,
}

/// Stage + step, straight from the backend. Stages: starting, switching_model, conditioning,
/// sampling (step n of total), vae_decode, encoding. No fake overall percentage.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Progress {
    pub stage: String,
    pub step: Option<u32>,
    pub total: Option<u32>,
    /// Only set when a new preview frame arrives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<Preview>,
}

/// A step preview frame. Its step can run ahead of `Progress::step`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Preview {
    pub step: u32,
    pub total: u32,
    /// Base64 PNG (small; 128x128 with the `proj` previewer).
    pub b64_png: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BackendState {
    Stopped,
    Loading,
    Ready,
    Busy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendError {
    pub code: String,
    /// Plain words, ending with what to do next.
    pub message: String,
}

impl BackendError {
    pub fn new(code: &str) -> Self {
        Self { code: code.to_string(), message: friendly_message(code) }
    }
    pub fn with_message(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into() }
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

const LOG_HINT: &str = "Details are in .data/logs/sd-server.log.";

/// Maps sd-server's JSON error codes (and the engine's own) to "what to do next" messages.
pub fn friendly_message(code: &str) -> String {
    match code {
        "model_not_loaded" => "The model isn't loaded. Try again; the engine reloads it automatically.".into(),
        "model_loading" => "The model is still loading. Try again in a few seconds.".into(),
        "model_busy" => "The image engine is busy with another job. Wait for it to finish, then try again.".into(),
        "invalid_model_config" => "The model files don't match what the engine expects. Delete the model and pull it again.".into(),
        "model_load_failed" => format!(
            "The model failed to load. Check that its files are complete and that the GPU has about 4.5 GB free (close games or other AI apps), then try again. {LOG_HINT}"
        ),
        "generation_failed" => format!("Generation failed. Try again; if it keeps failing, restart the engine. {LOG_HINT}"),
        "cancelled" => "The job was cancelled.".into(),
        "backend_crashed" => format!("The image engine stopped unexpectedly. Try again; it restarts on its own. {LOG_HINT}"),
        "backend_unreachable" => format!("Couldn't reach the image engine. Try again; if it keeps happening, restart the engine. {LOG_HINT}"),
        "backend_missing" => "The image runtime (sd-server) wasn't found. Pull a model again to reinstall it, or check the --sd-server path.".into(),
        other => format!("Something went wrong in the image engine ({other}). Try again. {LOG_HINT}"),
    }
}

#[async_trait]
pub trait Backend: Send + Sync {
    /// Generate one image and return PNG bytes. Reports progress as it goes.
    /// Starts the backend on demand if it is stopped.
    async fn generate(&self, req: &GenRequest, progress: &(dyn Fn(Progress) + Send + Sync)) -> Result<Vec<u8>, BackendError>;
    fn state(&self) -> BackendState;
    /// Id of the model currently loaded, if any.
    fn loaded_model(&self) -> Option<String>;
    /// Release the GPU entirely (stop the child process). Safe to call when already stopped.
    async fn stop(&self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_codes_map_to_next_steps() {
        let busy = BackendError::new("model_busy");
        assert_eq!(busy.code, "model_busy");
        assert!(busy.message.contains("Wait for it to finish"));
        assert!(friendly_message("model_not_loaded").contains("Try again"));
        assert!(friendly_message("invalid_model_config").contains("pull it again"));
        assert!(friendly_message("model_load_failed").contains("GPU"));
    }

    #[test]
    fn unknown_codes_still_say_what_to_do() {
        let m = friendly_message("weird_new_code");
        assert!(m.contains("weird_new_code") && m.contains("Try again"));
    }
}
