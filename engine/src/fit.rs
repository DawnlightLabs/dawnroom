//! Does a model run on this machine? One formula, the same one the site uses (site/src/lib/fit.ts):
//!
//!   need  = the model's measured memory need (MiB): `need_mib` where it was measured with offload,
//!           else its measured peak VRAM. Integrated GPUs use integrated measurements when there
//!           are some, else the discrete need as an estimate.
//!   fits                if need + HEADROOM <= VRAM
//!   offload             if need + HEADROOM <= VRAM + usable RAM        (discrete GPUs only)
//!   needs_more_memory   otherwise
//!   usable RAM = total RAM - RAM_RESERVE
//!
//! Integrated GPUs keep their shared-memory rule: their "VRAM" is the shared budget, and there's
//! no offload tier (it would be the same RAM).
//!
//! No measurement at all: an estimate, the sum of the variant's file sizes + COMPUTE_RESERVE_MIB,
//! through the same formula, always marked `estimate` ("Should fit (estimate)"), never as measured.

use serde::Serialize;

use crate::registry::{Manifest, Task};

/// Left free on the GPU for Windows, the browser and the driver (same as the site).
pub const HEADROOM_MIB: u64 = 512;
/// System RAM kept for Windows and other apps when offloading. Measured on the 24 GB laptop
/// (Slice 6a Part 2): while Qwen-Image ran with auto-fit, everything else (Windows, Defender,
/// Claude) was squeezed to 5.7 GB; idle with apps closed it used 10 GB, which Windows trims
/// under pressure. 6 GiB keeps that floor. Mirrored in site/src/lib/fit.ts.
pub const RAM_RESERVE_MIB: u64 = 6 * 1024;
/// Working memory on top of the weights for an untested model's estimate: the compute reserve
/// sd.cpp itself planned for Qwen-Image (Slice 6a Part 2). Mirrored in site/src/lib/fit.ts.
pub const COMPUTE_RESERVE_MIB: u64 = 2048;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Fits,
    /// Runs with part of the weights in system RAM (sd.cpp auto-fit): slower.
    Offload,
    NeedsMoreMemory,
    NoGpu,
}

/// What the fit is decided against.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Machine {
    /// VRAM, or the shared budget of an integrated GPU.
    pub gpu_mib: u64,
    pub integrated: bool,
    pub ram_total_mib: u64,
}

impl Machine {
    pub fn usable_ram_mib(&self) -> u64 {
        if self.integrated { 0 } else { self.ram_total_mib.saturating_sub(RAM_RESERVE_MIB) }
    }
}

/// The tier plus the numbers behind it, for the API and messages.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Fit {
    pub tier: Tier,
    pub need_mib: Option<u64>,
    pub gpu_mib: Option<u64>,
    pub usable_ram_mib: Option<u64>,
    /// The need isn't a measurement on this kind of GPU: file sizes + compute reserve (untested),
    /// or a discrete card's peak used for an integrated GPU.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub estimate: bool,
}

/// What a variant's need is based on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    Measured,
    /// A discrete card's measurement used for an integrated GPU.
    OtherGpu,
    /// Nobody measured it: its files + COMPUTE_RESERVE_MIB.
    Files,
}

/// The estimate for a model nobody measured.
pub fn estimate_mib(m: &Manifest) -> u64 {
    m.total_size().div_ceil(1024 * 1024) + COMPUTE_RESERVE_MIB
}

/// The task a model's fit is shown for: generate if it can, else edit. Each task's fit uses only
/// that task's measurements (an edit holds input images too, so it can peak higher).
pub fn main_task(m: &Manifest) -> Task {
    if m.supports(Task::Generate) { Task::Generate } else { Task::Edit }
}

/// The model's memory need for its main task on this kind of GPU, and what it's based on.
pub fn need_mib(m: &Manifest, integrated: bool) -> (u64, Basis) {
    need_mib_for(m, main_task(m), integrated)
}

/// The memory need for one task. No measurement of that task: the estimate (files + reserve).
pub fn need_mib_for(m: &Manifest, task: Task, integrated: bool) -> (u64, Basis) {
    let need = |igpu: bool| m.measured.iter().filter(|x| x.integrated == igpu && x.task() == task).map(|x| x.need_mib.unwrap_or(x.peak_vram_mib)).max();
    match (integrated, need(integrated)) {
        (_, Some(n)) => (n, Basis::Measured),
        (true, None) => need(false).map(|n| (n, Basis::OtherGpu)).unwrap_or((estimate_mib(m), Basis::Files)),
        (false, None) => (estimate_mib(m), Basis::Files),
    }
}

pub fn fit(m: &Manifest, machine: Option<&Machine>) -> Fit {
    fit_for(m, main_task(m), machine)
}

/// The fit for one task (Auto uses the task it picks for).
pub fn fit_for(m: &Manifest, task: Task, machine: Option<&Machine>) -> Fit {
    let Some(mc) = machine else { return Fit { tier: Tier::NoGpu, need_mib: None, gpu_mib: None, usable_ram_mib: None, estimate: false } };
    let (n, basis) = need_mib_for(m, task, mc.integrated);
    let tier = if n + HEADROOM_MIB <= mc.gpu_mib {
        Tier::Fits
    } else if !mc.integrated && n + HEADROOM_MIB <= mc.gpu_mib + mc.usable_ram_mib() {
        Tier::Offload
    } else {
        Tier::NeedsMoreMemory
    };
    Fit { tier, need_mib: Some(n), gpu_mib: Some(mc.gpu_mib), usable_ram_mib: Some(mc.usable_ram_mib()), estimate: basis != Basis::Measured }
}

/// Plain words for a refusal, with both numbers.
pub fn needs_more_message(name: &str, f: &Fit, integrated: bool) -> String {
    let gb = |mib: u64| format!("{:.1} GB", mib as f64 / 1024.0);
    let need = gb(f.need_mib.unwrap_or(0) + HEADROOM_MIB);
    if integrated {
        format!("{name} needs about {need} of memory, and this computer's built-in graphics may use {}. It needs a computer with more RAM or a graphics card.", gb(f.gpu_mib.unwrap_or(0)))
    } else {
        format!(
            "{name} needs about {need} of graphics and system memory together, and this computer has {} of VRAM plus {} of RAM it can spare. It needs a bigger GPU or more RAM.",
            gb(f.gpu_mib.unwrap_or(0)),
            gb(f.usable_ram_mib.unwrap_or(0))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use std::path::Path;

    fn reg() -> Registry {
        Registry::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("registry")).unwrap()
    }

    fn laptop(ram_gb: u64) -> Machine {
        Machine { gpu_mib: 8151, integrated: false, ram_total_mib: ram_gb * 1024 }
    }

    #[test]
    fn eight_gb_card_tiers() {
        let r = reg();
        let t = |id: &str, mc: &Machine| fit(r.get(id).unwrap(), Some(mc)).tier;
        assert_eq!(t("z-image-turbo:q4", &laptop(24)), Tier::Fits);
        assert_eq!(t("flux2-klein-4b:q8", &laptop(24)), Tier::Fits);
        // 18785 + 512 needs VRAM + RAM: 8151 + (24 GB - 6 GB) = 26583 MiB.
        assert_eq!(t("qwen-image:q4", &laptop(24)), Tier::Offload);
        // 16 GB: 8151 + (16 GB - 6 GB) = 18391 MiB < 19297 needed.
        assert_eq!(t("qwen-image:q4", &laptop(16)), Tier::NeedsMoreMemory);
        assert_eq!(t("qwen-image:q4", &laptop(32)), Tier::Offload);
        assert_eq!(t("qwen-image:q4", &laptop(8)), Tier::NeedsMoreMemory);
        assert_eq!(t("qwen-image:q4", &Machine { gpu_mib: 24 * 1024, integrated: false, ram_total_mib: 32 * 1024 }), Tier::Fits);
        assert_eq!(fit(r.get("qwen-image:q4").unwrap(), None).tier, Tier::NoGpu);
    }

    #[test]
    fn integrated_uses_its_own_measurements_and_never_offloads() {
        let r = reg();
        let igpu = Machine { gpu_mib: 13785, integrated: true, ram_total_mib: 24 * 1024 };
        let k = fit(r.get("flux2-klein-4b:q8").unwrap(), Some(&igpu));
        assert_eq!((k.tier, k.need_mib, k.estimate), (Tier::Fits, Some(7082), false));
        let z = fit(r.get("z-image-turbo:q4").unwrap(), Some(&igpu));
        assert!(z.estimate, "no iGPU measurement: the discrete peak, as an estimate");
        assert_eq!(fit(r.get("qwen-image:q4").unwrap(), Some(&igpu)).tier, Tier::NeedsMoreMemory, "no offload tier on shared memory");
    }

    #[test]
    fn untested_variants_get_a_file_size_estimate() {
        let r = reg();
        let q8 = r.get("z-image-turbo:q8").unwrap();
        let f = fit(q8, Some(&laptop(24)));
        assert!(f.estimate);
        assert_eq!(f.need_mib, Some(q8.total_size().div_ceil(1 << 20) + COMPUTE_RESERVE_MIB));
        // 6.6 + 2.5 + 0.3 GB of files + 2 GB: more than 8 GB of VRAM, so it would offload.
        assert_eq!(f.tier, Tier::Offload);
        assert!(!fit(r.get("z-image-turbo:q4").unwrap(), Some(&laptop(24))).estimate, "measured");
    }

    #[test]
    fn refusal_says_both_numbers() {
        let r = reg();
        let f = fit(r.get("qwen-image:q4").unwrap(), Some(&laptop(8)));
        let m = needs_more_message("Qwen-Image", &f, false);
        assert!(m.contains("18.8 GB") && m.contains("8.0 GB of VRAM") && m.contains("2.0 GB of RAM"), "{m}");
    }
}
