//! What this machine has: every GPU (any vendor), its memory, the NVIDIA driver, system RAM.
//! Windows lists adapters with DXGI; Linux uses nvidia-smi, `vulkaninfo --summary` and
//! /sys/class/drm. nvidia-smi stays the source of NVIDIA VRAM numbers (the measurements in the
//! registry use its 8151 MiB, not DXGI's). Detected once at start; RAM availability is live.

use serde::Serialize;

/// Live NVIDIA numbers from nvidia-smi, for health and the "fits this GPU" answer.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub vram_total_mib: u64,
    pub vram_used_mib: u64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

impl Vendor {
    pub fn from_pci(id: u32) -> Self {
        match id {
            0x10de => Vendor::Nvidia,
            0x1002 | 0x1022 => Vendor::Amd,
            0x8086 => Vendor::Intel,
            _ => Vendor::Other,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Vendor::Nvidia => "NVIDIA",
            Vendor::Amd => "AMD",
            Vendor::Intel => "Intel",
            Vendor::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GpuInfo {
    pub name: String,
    pub vendor: Vendor,
    /// Integrated GPUs work from system RAM; `shared_mib` is how much of it they may use.
    pub integrated: bool,
    /// Dedicated video memory (an iGPU's small carve-out, if any).
    pub vram_mib: u64,
    /// System memory the GPU may use. For an iGPU this is its real budget.
    pub shared_mib: u64,
    /// Driver version as the vendor writes it (NVIDIA: "596.36"; others: the Windows driver version).
    pub driver: Option<String>,
    /// CUDA compute capability, NVIDIA only (from nvidia-smi), e.g. "12.0".
    pub compute_cap: Option<String>,
    /// Where these numbers came from: "dxgi", "nvidia-smi", "sysfs", "vulkaninfo".
    pub source: &'static str,
    /// The memory number is a guess (e.g. half of RAM for an Intel iGPU on Linux).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub memory_estimated: bool,
    /// Windows adapter LUID as the GPU performance counters name it (`0x00000000_0x000135b1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub luid: Option<String>,
}

impl GpuInfo {
    /// The memory a model can use on this GPU: VRAM for a discrete card, the shared budget
    /// (plus any carve-out) for an integrated one.
    pub fn memory_mib(&self) -> u64 {
        if self.integrated { self.vram_mib + self.shared_mib } else { self.vram_mib }
    }

    /// The `Gpu` the fit logic uses.
    pub fn as_gpu(&self) -> Gpu {
        Gpu { name: self.name.clone(), vram_total_mib: self.memory_mib(), vram_used_mib: 0 }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Ram {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Hardware {
    pub os: &'static str,
    pub os_version: String,
    pub cpu: Option<String>,
    /// Linux only: the C library version (the runtime build needs a recent one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glibc: Option<String>,
    pub gpus: Vec<GpuInfo>,
    pub ram: Ram,
    /// Set when `DAWNROOM_SIMULATE` changed what was detected (dev and tests only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simulated: Option<String>,
}

/// The glibc this process runs on (`2.35`), on Linux with glibc; `None` elsewhere (and on musl).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn glibc_version() -> Option<String> {
    // SAFETY: gnu_get_libc_version returns a pointer to a static NUL-terminated string.
    let p = unsafe { libc::gnu_get_libc_version() };
    Some(unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned())
}
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn glibc_version() -> Option<String> {
    None
}

/// Everything at once. Runs a few small commands; call it once at start.
pub async fn detect() -> Hardware {
    let nvidia = nvidia_smi().await;
    let mut gpus = platform_gpus(&nvidia).await;
    // An NVIDIA card the platform list missed (e.g. no vulkaninfo on Linux) still counts.
    for n in &nvidia {
        if !gpus.iter().any(|g| g.vendor == Vendor::Nvidia && g.name == n.name) {
            gpus.push(nvidia_info(n));
        }
    }
    let mut hw = Hardware { os: os_name(), os_version: os_version().await, cpu: cpu_name().await, glibc: glibc_version(), gpus, ram: ram(), simulated: None };
    if let Ok(sim) = std::env::var("DAWNROOM_SIMULATE") {
        simulate(&mut hw, sim.trim());
    }
    hw
}

/// Dev/test switch: `DAWNROOM_SIMULATE=no-gpu | old-driver | igpu-only`.
pub fn simulate(hw: &mut Hardware, what: &str) {
    match what {
        "no-gpu" => hw.gpus.clear(),
        "old-driver" => hw.gpus.iter_mut().filter(|g| g.vendor == Vendor::Nvidia).for_each(|g| g.driver = Some("531.18".into())),
        "no-driver" => hw.gpus.iter_mut().filter(|g| g.vendor == Vendor::Nvidia).for_each(|g| g.driver = None),
        "igpu-only" => hw.gpus.retain(|g| g.integrated),
        "" => return,
        other => {
            eprintln!("[engine] DAWNROOM_SIMULATE={other:?} isn't known (no-gpu, old-driver, no-driver, igpu-only); ignored");
            return;
        }
    }
    eprintln!("[engine] simulating hardware: {what}");
    hw.simulated = Some(what.to_string());
}

pub fn os_name() -> &'static str {
    if cfg!(windows) { "windows" } else if cfg!(target_os = "linux") { "linux" } else { std::env::consts::OS }
}

// ---------- nvidia-smi ----------

#[derive(Debug, Clone, PartialEq)]
pub struct NvidiaRow {
    pub name: String,
    pub driver: String,
    pub total_mib: u64,
    pub used_mib: u64,
    pub compute_cap: Option<String>,
}

async fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let mut c = tokio::process::Command::new(cmd);
    c.args(args).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    c.creation_flags(0x0800_0000 /* CREATE_NO_WINDOW */);
    let out = c.output().await.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Every NVIDIA GPU nvidia-smi reports (empty without an NVIDIA driver).
pub async fn nvidia_smi() -> Vec<NvidiaRow> {
    const Q: &str = "--format=csv,noheader,nounits";
    // compute_cap is a newer query field; older drivers reject the whole query, so retry without it.
    if let Some(t) = run("nvidia-smi", &["--query-gpu=name,driver_version,memory.total,memory.used,compute_cap", Q]).await {
        return parse_nvidia(&t);
    }
    run("nvidia-smi", &["--query-gpu=name,driver_version,memory.total,memory.used", Q]).await.map(|t| parse_nvidia(&t)).unwrap_or_default()
}

pub fn parse_nvidia(text: &str) -> Vec<NvidiaRow> {
    text.lines()
        .filter_map(|line| {
            let p: Vec<&str> = line.split(',').map(str::trim).collect();
            Some(NvidiaRow {
                name: p.first().filter(|s| !s.is_empty())?.to_string(),
                driver: p.get(1)?.to_string(),
                total_mib: p.get(2)?.parse().ok()?,
                used_mib: p.get(3)?.parse().ok()?,
                compute_cap: p.get(4).filter(|s| s.chars().next().is_some_and(|c| c.is_ascii_digit())).map(|s| s.to_string()),
            })
        })
        .collect()
}

fn nvidia_info(n: &NvidiaRow) -> GpuInfo {
    GpuInfo {
        name: n.name.clone(),
        vendor: Vendor::Nvidia,
        integrated: false,
        vram_mib: n.total_mib,
        shared_mib: 0,
        driver: Some(n.driver.clone()),
        compute_cap: n.compute_cap.clone(),
        source: "nvidia-smi",
        memory_estimated: false,
        luid: None,
    }
}

/// The first NVIDIA GPU, live (used VRAM changes), or `None`.
pub async fn nvidia_live() -> Option<Gpu> {
    nvidia_smi().await.into_iter().next().map(|n| Gpu { name: n.name, vram_total_mib: n.total_mib, vram_used_mib: n.used_mib })
}

/// iGPUs carve out little or no dedicated memory; discrete cards have gigabytes.
/// (A big BIOS carve-out on a Ryzen AI Max then counts as dedicated, which is how it behaves.)
#[cfg_attr(not(windows), allow(dead_code))] // Linux reads the device type from vulkaninfo instead
fn looks_integrated(vendor: Vendor, dedicated_mib: u64) -> bool {
    vendor != Vendor::Nvidia && dedicated_mib < 2048
}

// ---------- Windows ----------

#[cfg(windows)]
async fn platform_gpus(nvidia: &[NvidiaRow]) -> Vec<GpuInfo> {
    let mut gpus = tokio::task::spawn_blocking(dxgi_adapters).await.unwrap_or_default();
    // NVIDIA: nvidia-smi's VRAM and driver version replace DXGI's numbers.
    for g in gpus.iter_mut().filter(|g| g.vendor == Vendor::Nvidia) {
        match nvidia.iter().find(|n| n.name == g.name) {
            Some(n) => *g = GpuInfo { luid: g.luid.take(), ..nvidia_info(n) },
            None => g.driver = None, // the card is there but nvidia-smi isn't: driver missing or broken
        }
    }
    gpus
}

#[cfg(windows)]
fn dxgi_adapters() -> Vec<GpuInfo> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIDevice, IDXGIFactory1};
    use windows::core::Interface;
    let mut out = Vec::new();
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return out };
    let mut seen = std::collections::HashSet::new();
    for i in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else { break };
        let Ok(d) = (unsafe { adapter.GetDesc1() }) else { continue };
        if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || d.VendorId == 0x1414 {
            continue; // Microsoft Basic Render Driver
        }
        if !seen.insert((d.AdapterLuid.HighPart, d.AdapterLuid.LowPart)) {
            continue;
        }
        let len = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
        let name = String::from_utf16_lossy(&d.Description[..len]).trim().to_string();
        let vendor = Vendor::from_pci(d.VendorId);
        let dedicated = (d.DedicatedVideoMemory as u64) >> 20;
        // The UMD version, a.b.c.d in four 16-bit parts (e.g. 32.0.101.6881).
        let driver = unsafe { adapter.CheckInterfaceSupport(&IDXGIDevice::IID) }.ok().map(|v| {
            let v = v as u64;
            format!("{}.{}.{}.{}", v >> 48, (v >> 32) & 0xffff, (v >> 16) & 0xffff, v & 0xffff)
        });
        out.push(GpuInfo {
            name,
            vendor,
            integrated: looks_integrated(vendor, dedicated),
            vram_mib: dedicated,
            shared_mib: (d.SharedSystemMemory as u64) >> 20,
            driver,
            compute_cap: None,
            source: "dxgi",
            memory_estimated: false,
            luid: Some(format!("0x{:08x}_0x{:08x}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart)),
        });
    }
    out
}

#[cfg(windows)]
pub fn ram() -> Ram {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    match unsafe { GlobalMemoryStatusEx(&mut m) } {
        Ok(()) => Ram { total_bytes: m.ullTotalPhys, available_bytes: m.ullAvailPhys },
        Err(_) => Ram { total_bytes: 0, available_bytes: 0 },
    }
}

#[cfg(windows)]
async fn os_version() -> String {
    // "Microsoft Windows [Version 10.0.26300.1234]"
    run("cmd", &["/c", "ver"]).await.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Windows".into())
}

#[cfg(windows)]
async fn cpu_name() -> Option<String> {
    let out = run("reg", &["query", r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0", "/v", "ProcessorNameString"]).await?;
    out.lines().find_map(|l| l.split("REG_SZ").nth(1)).map(|s| s.trim().to_string())
}

// ---------- Linux ----------

#[cfg(not(windows))]
async fn platform_gpus(_nvidia: &[NvidiaRow]) -> Vec<GpuInfo> {
    let ram_mib = ram().total_bytes >> 20;
    let drm = drm_cards();
    let mut gpus: Vec<GpuInfo> = match run("vulkaninfo", &["--summary"]).await {
        Some(text) => parse_vulkaninfo(&text),
        None => drm.iter().map(|c| GpuInfo {
            name: format!("{} GPU ({:04x}:{:04x})", Vendor::from_pci(c.vendor).label(), c.vendor, c.device),
            vendor: Vendor::from_pci(c.vendor),
            integrated: c.vram_mib.is_none_or(|v| v < 2048),
            vram_mib: 0,
            shared_mib: 0,
            driver: None,
            compute_cap: None,
            source: "sysfs",
            memory_estimated: false,
            luid: None,
        }).collect(),
    };
    // NVIDIA rows come from nvidia-smi (added by the caller); drop Vulkan's copy of them.
    gpus.retain(|g| g.vendor != Vendor::Nvidia);
    for g in gpus.iter_mut() {
        let card = drm.iter().find(|c| Vendor::from_pci(c.vendor) == g.vendor);
        match card.and_then(|c| c.vram_mib.map(|v| (v, c.gtt_mib.unwrap_or(0)))) {
            // amdgpu reports its carve-out (VRAM) and the GTT window into system RAM.
            Some((vram, gtt)) => {
                g.vram_mib = vram;
                g.shared_mib = if g.integrated { gtt } else { 0 };
                g.source = "sysfs";
            }
            // i915/xe don't publish a budget: assume half of RAM, and say it's a guess.
            None if g.integrated => {
                g.shared_mib = ram_mib / 2;
                g.memory_estimated = true;
            }
            None => {}
        }
    }
    gpus
}

#[cfg(not(windows))]
struct DrmCard {
    vendor: u32,
    device: u32,
    vram_mib: Option<u64>,
    gtt_mib: Option<u64>,
}

#[cfg(not(windows))]
fn drm_cards() -> Vec<DrmCard> {
    let read = |p: std::path::PathBuf| std::fs::read_to_string(p).ok().map(|s| s.trim().to_string());
    let hex = |s: Option<String>| s.and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok());
    let mib = |s: Option<String>| s.and_then(|s| s.parse::<u64>().ok()).map(|b| b >> 20);
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else { return vec![] };
    let mut cards: Vec<DrmCard> = entries
        .filter_map(|e| e.ok())
        .filter(|e| { let n = e.file_name().to_string_lossy().into_owned(); n.starts_with("card") && !n.contains('-') })
        .filter_map(|e| {
            let d = e.path().join("device");
            Some(DrmCard {
                vendor: hex(read(d.join("vendor")))?,
                device: hex(read(d.join("device"))).unwrap_or(0),
                vram_mib: mib(read(d.join("mem_info_vram_total"))),
                gtt_mib: mib(read(d.join("mem_info_gtt_total"))),
            })
        })
        .collect();
    cards.sort_by_key(|c| c.vendor);
    cards
}

/// `vulkaninfo --summary`: one "GPUn:" block per device with vendorID, deviceType, deviceName.
pub fn parse_vulkaninfo(text: &str) -> Vec<GpuInfo> {
    let mut out = Vec::new();
    /// One "GPUn:" block as it's read: vendorID, deviceType, deviceName, driverInfo.
    type Block = (Option<u32>, Option<String>, Option<String>, Option<String>);
    let mut cur: Option<Block> = None;
    let flush = |c: Option<Block>, out: &mut Vec<GpuInfo>| {
        if let Some((Some(vid), Some(kind), Some(name), driver)) = c {
            if kind.contains("CPU") {
                return; // llvmpipe / software
            }
            out.push(GpuInfo {
                name,
                vendor: Vendor::from_pci(vid),
                integrated: kind.contains("INTEGRATED"),
                vram_mib: 0,
                shared_mib: 0,
                driver,
                compute_cap: None,
                source: "vulkaninfo",
                memory_estimated: false,
                luid: None,
            });
        }
    };
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("GPU") && t.ends_with(':') && t[3..t.len() - 1].chars().all(|c| c.is_ascii_digit()) {
            flush(cur.take(), &mut out);
            cur = Some((None, None, None, None));
            continue;
        }
        let Some(c) = cur.as_mut() else { continue };
        let Some((k, v)) = t.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "vendorID" => c.0 = u32::from_str_radix(v.trim_start_matches("0x"), 16).ok(),
            "deviceType" => c.1 = Some(v.to_string()),
            "deviceName" => c.2 = Some(v.to_string()),
            "driverInfo" => c.3 = Some(v.to_string()),
            _ => {}
        }
    }
    flush(cur.take(), &mut out);
    out
}

#[cfg(not(windows))]
pub fn ram() -> Ram {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb = |key: &str| text.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * 1024;
    Ram { total_bytes: kb("MemTotal:"), available_bytes: kb("MemAvailable:") }
}

#[cfg(not(windows))]
async fn os_version() -> String {
    let pretty = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string())))
        .unwrap_or_else(|| "Linux".into());
    match run("uname", &["-r"]).await {
        Some(k) => format!("{pretty}, kernel {}", k.trim()),
        None => pretty,
    }
}

#[cfg(not(windows))]
async fn cpu_name() -> Option<String> {
    let t = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    t.lines().find_map(|l| l.strip_prefix("model name").and_then(|v| v.split_once(':')).map(|(_, v)| v.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nvidia_smi_csv() {
        let rows = parse_nvidia("NVIDIA GeForce RTX 5060 Laptop GPU, 596.36, 8151, 0, 12.0\n");
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!((r.name.as_str(), r.driver.as_str(), r.total_mib, r.used_mib), ("NVIDIA GeForce RTX 5060 Laptop GPU", "596.36", 8151, 0));
        assert_eq!(r.compute_cap.as_deref(), Some("12.0"));
        // Without compute_cap (older drivers), and "[N/A]" fields.
        assert_eq!(parse_nvidia("GTX 1060, 531.18, 6144, 10\n")[0].compute_cap, None);
        assert_eq!(parse_nvidia("GTX 1060, 531.18, 6144, 10, [N/A]\n")[0].compute_cap, None);
        assert!(parse_nvidia("").is_empty());
    }

    #[test]
    fn parses_vulkaninfo_summary() {
        let text = "Devices:\n========\nGPU0:\n\tapiVersion = 1.3.287\n\tvendorID = 0x1002\n\tdeviceID = 0x15bf\n\tdeviceType = PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU\n\tdeviceName = AMD Radeon 780M (RADV PHOENIX)\n\tdriverInfo = Mesa 24.2.8\nGPU1:\n\tvendorID = 0x10005\n\tdeviceType = PHYSICAL_DEVICE_TYPE_CPU\n\tdeviceName = llvmpipe (LLVM 19.1.1, 256 bits)\n";
        let g = parse_vulkaninfo(text);
        assert_eq!(g.len(), 1);
        assert_eq!((g[0].vendor, g[0].integrated, g[0].name.as_str()), (Vendor::Amd, true, "AMD Radeon 780M (RADV PHOENIX)"));
        assert_eq!(g[0].driver.as_deref(), Some("Mesa 24.2.8"));
    }

    #[test]
    fn integrated_memory_is_the_shared_budget() {
        let igpu = GpuInfo { name: "Intel(R) Arc(TM) 140T GPU".into(), vendor: Vendor::Intel, integrated: true, vram_mib: 128, shared_mib: 16000, driver: None, compute_cap: None, source: "dxgi", memory_estimated: false, luid: None };
        assert_eq!(igpu.memory_mib(), 16128);
        let dgpu = GpuInfo { integrated: false, vram_mib: 8151, shared_mib: 16000, vendor: Vendor::Nvidia, ..igpu };
        assert_eq!(dgpu.memory_mib(), 8151);
        assert!(looks_integrated(Vendor::Intel, 128) && !looks_integrated(Vendor::Intel, 16384) && !looks_integrated(Vendor::Nvidia, 0));
    }
}
