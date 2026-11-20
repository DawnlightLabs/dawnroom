//! Auto, the default model choice, and measured timings. Pure functions over plain inputs, so
//! every rule is unit-tested; `queue::Engine::auto` feeds them the live state.
//!
//! Auto picks, for one task (generate or edit):
//! 1. Only candidates that are installed, curated (never custom), tested by us for that task,
//!    able to do the task, and that fit or run with RAM offload on this machine. It never
//!    downloads anything.
//! 2. The model that will be loaded when the job runs (the last queued job's, else sd-server's)
//!    if it qualifies and fits on the GPU (or nothing that fits qualifies): no needless switch, but
//!    a loaded RAM-offload model never blocks a much faster one that fits.
//! 3. Otherwise, in order: fits before RAM offload (offload is several times slower), commercially
//!    clean before restricted or non-commercial, the ranking below, the bigger tag, then the id.
//!    Same inputs and state, same pick.
//!
//! It always says what it picked and why, in one line.

use serde::Serialize;

use crate::fit::Tier;
use crate::registry::{Manifest, Task, Testing};

/// Best first, per task, with the one thing each is best at. Models not listed rank after these,
/// fastest measured first. The one place this order lives.
pub const RANK: &[(Task, &str, &str)] = &[
    (Task::Generate, "z-image-turbo", "best text"),
    (Task::Generate, "flux2-klein-4b", "fastest"),
    (Task::Generate, "qwen-image", "strong text, big"),
    (Task::Generate, "flux1-schnell", "fast FLUX"),
    (Task::Generate, "sd35-medium", "good prompt-following"),
    (Task::Generate, "realvisxl-v5", "photoreal"),
    (Task::Generate, "sd15", "small and quick"),
    (Task::Edit, "flux2-klein-4b", "fast edits that keep the rest of the image"),
];

/// What Auto needs to know about one variant.
#[derive(Debug, Clone)]
pub struct Candidate<'a> {
    pub m: &'a Manifest,
    pub installed: bool,
    pub fit: Tier,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Pick {
    pub task: Task,
    pub model: String,
    pub name: String,
    /// One line: "Z-Image Turbo: already loaded, best text, about 19 s here".
    pub reason: String,
    /// It is the model already loaded (or the one the queue will have loaded).
    pub loaded: bool,
}

/// Nothing installed qualifies. `suggest` is the model to pull: the best tested, commercially
/// clean, ungated one that fits here (else runs with offload); `None` if there's no such model.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NoPick {
    pub task: Task,
    pub message: String,
    pub suggest: Option<Suggest>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Suggest {
    pub model: String,
    pub name: String,
    pub download_bytes: u64,
}

fn rank_of(task: Task, family: &str) -> Option<(usize, &'static str)> {
    RANK.iter().filter(|(t, ..)| *t == task).position(|(_, f, _)| *f == family).map(|i| {
        let (_, _, why) = RANK.iter().filter(|(t, ..)| *t == task).nth(i).unwrap();
        (i, *why)
    })
}

fn clean(m: &Manifest) -> bool {
    m.licence.commercial && !m.licence.use_restrictions
}

fn qualifies(task: Task, c: &Candidate) -> bool {
    c.installed && !c.m.is_custom() && c.m.supports(task) && c.m.testing_for(task) == Testing::Tested && matches!(c.fit, Tier::Fits | Tier::Offload)
}

/// The sort key: smaller is better. Deterministic, ends on the unique id.
fn key<'a>(task: Task, c: &Candidate<'a>, timing: &dyn Fn(&Manifest) -> Option<f64>) -> (u8, u8, usize, u64, u64, &'a str) {
    let rank = rank_of(task, &c.m.family).map(|(i, _)| i).unwrap_or(usize::MAX);
    // Unlisted models: fastest measured first (milliseconds keep it an integer, so it sorts totally).
    let speed = if rank == usize::MAX { timing(c.m).map(|s| (s * 1000.0) as u64).unwrap_or(u64::MAX) } else { 0 };
    (if c.fit == Tier::Fits { 0 } else { 1 }, if clean(c.m) { 0 } else { 1 }, rank, speed, u64::MAX - c.m.total_size(), c.m.id.as_str())
}

/// `loaded`: the model that will be loaded when this job runs. `timing` gives the warm (else cold)
/// seconds measured for this task, for the reason line and to rank unlisted models.
pub fn pick(task: Task, cands: &[Candidate], loaded: Option<&str>, timing: &dyn Fn(&Manifest) -> Option<Timing>) -> Result<Pick, NoPick> {
    let secs = |m: &Manifest| timing(m).and_then(|t| t.warm_s.or(t.cold_s));
    let ok: Vec<&Candidate> = cands.iter().filter(|c| qualifies(task, c)).collect();
    // The loaded model wins only if it fits on the GPU, or if nothing that fits qualifies: switching
    // costs a few seconds, while a RAM-offload model can take minutes per image (Qwen-Image-Edit
    // streamed from disk: 750 s for an edit klein does in 27 s cold).
    let any_fits = ok.iter().any(|c| c.fit == Tier::Fits);
    let chosen = ok
        .iter()
        .find(|c| Some(c.m.id.as_str()) == loaded && (c.fit == Tier::Fits || !any_fits))
        .copied()
        .map(|c| (c, true))
        .or_else(|| ok.iter().min_by_key(|c| key(task, c, &secs)).map(|c| (*c, false)));
    let Some((c, is_loaded)) = chosen else { return Err(no_pick(task, cands, &secs)) };
    let mut parts: Vec<String> = Vec::new();
    parts.push(if is_loaded {
        "already loaded".into()
    } else if ok.len() == 1 {
        format!("the only installed model for {}", words(task))
    } else {
        format!("the best installed model for {}", words(task))
    });
    if let Some((_, why)) = rank_of(task, &c.m.family) {
        parts.push(why.into());
    }
    if c.fit == Tier::Offload {
        parts.push("runs with RAM offload (slower)".into());
    }
    if !c.m.licence.commercial {
        parts.push("non-commercial licence".into());
    }
    parts.push(match timing(c.m) {
        Some(t) => t.about(),
        None => "no timing yet".into(),
    });
    Ok(Pick { task, model: c.m.id.clone(), name: c.m.name.clone(), reason: format!("{}: {}", c.m.name, parts.join(", ")), loaded: is_loaded })
}

fn words(task: Task) -> &'static str {
    match task {
        Task::Generate => "making images",
        Task::Edit => "editing",
    }
}

fn no_pick(task: Task, cands: &[Candidate], secs: &dyn Fn(&Manifest) -> Option<f64>) -> NoPick {
    // What to pull: tested for this task, curated, clean, ungated, and it runs here.
    let pullable: Vec<&Candidate> = cands
        .iter()
        .filter(|c| !c.m.is_custom() && c.m.supports(task) && c.m.testing_for(task) == Testing::Tested && clean(c.m) && !c.m.licence.accept_required && matches!(c.fit, Tier::Fits | Tier::Offload))
        .collect();
    let best = pullable.iter().min_by_key(|c| key(task, c, secs));
    let installed_untested = cands.iter().any(|c| c.installed && c.m.supports(task) && !qualifies(task, c));
    let what = match task {
        Task::Generate => "No tested model for making images is installed",
        Task::Edit => "No tested model for editing is installed",
    };
    let also = if installed_untested { " (models that are untested for this, custom or don't fit don't count; pick one by name to use it anyway)" } else { "" };
    match best {
        Some(c) => {
            let suggest = Suggest { model: c.m.id.clone(), name: c.m.name.clone(), download_bytes: c.m.total_size() };
            NoPick { task, message: format!("{what}{also}. Pull {} ({:.1} GB): `dawnroom pull {}`.", c.m.name, c.m.total_size() as f64 / 1e9, c.m.id), suggest: Some(suggest) }
        }
        None if !cands.iter().any(|c| !c.m.is_custom() && c.m.testing_for(task) == Testing::Tested) => {
            NoPick { task, message: format!("{what}{also}, and Dawnroom hasn't tested any model for {} yet.", words(task)), suggest: None }
        }
        None => NoPick { task, message: format!("{what}{also}, and none of the tested ones runs on this machine."), suggest: None },
    }
}

// ---------- Timings ----------

/// A measured time for one model, tag and task, and where it was measured. Never estimated:
/// without a measurement there is no `Timing` ("no timing yet").
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Timing {
    pub cold_s: Option<f64>,
    pub warm_s: Option<f64>,
    /// The GPU it was measured on.
    pub gpu: String,
    /// The output size it was measured at (`1024x1024`), when recorded.
    pub size: Option<String>,
    /// Measured on this machine's GPU (same name, same kind).
    pub here: bool,
    /// "on your GPU" or "on <gpu>".
    pub label: String,
}

impl Timing {
    /// "about 19 s here" / "about 19 s on an RTX 5060 Laptop GPU".
    pub fn about(&self) -> String {
        let s = self.warm_s.or(self.cold_s).unwrap_or(0.0);
        let s = if s >= 10.0 { format!("{s:.0}") } else { format!("{s:.1}") };
        if self.here { format!("about {s} s here") } else { format!("about {s} s on {}", short_gpu(&self.gpu)) }
    }
}

/// "NVIDIA GeForce RTX 5060 Laptop GPU" -> "an RTX 5060 Laptop GPU".
pub fn short_gpu(name: &str) -> String {
    let n = name.strip_prefix("NVIDIA GeForce ").unwrap_or(name);
    let article = if n.starts_with(['A', 'E', 'I', 'O', 'U', 'R']) { "an" } else { "a" };
    format!("{article} {n}")
}

/// The timing for `task` to show on a machine with this GPU (`gpu_name`, `integrated`): one
/// measured on this GPU if there is one, else one on the same kind of GPU (discrete or
/// integrated), else none. Our own measurements before community ones.
pub fn timing(m: &Manifest, task: Task, gpu_name: Option<&str>, integrated: bool) -> Option<Timing> {
    let timed: Vec<_> = m.measured.iter().filter(|x| x.task() == task && (x.cold_s.is_some() || x.warm_s.is_some())).collect();
    let same_gpu = |g: &str| gpu_name.is_some_and(|n| g == n || g.starts_with(&format!("{n} (")));
    let best = timed
        .iter()
        .filter(|x| x.integrated == integrated)
        .min_by_key(|x| (!same_gpu(&x.gpu), x.source.is_some()))
        .or_else(|| timed.iter().min_by_key(|x| x.source.is_some()))?;
    let here = same_gpu(&best.gpu) && best.integrated == integrated;
    Some(Timing {
        cold_s: best.cold_s,
        warm_s: best.warm_s,
        gpu: best.gpu.clone(),
        size: best.size.clone(),
        here,
        label: if here { "on your GPU".into() } else { format!("on {}", best.gpu) },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Measured, Registry};

    fn reg() -> Registry {
        Registry::load_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("registry")).unwrap()
    }

    const GPU: &str = "NVIDIA GeForce RTX 5060 Laptop GPU";

    fn measured(task: Task, warm: f64) -> Measured {
        Measured {
            gpu: GPU.into(),
            vram_total_mib: 8151,
            peak_vram_mib: 5000,
            cold_s: Some(warm + 5.0),
            warm_s: Some(warm),
            note: None,
            runtime: None,
            integrated: false,
            need_mib: None,
            peak_ram_mib: None,
            offload: None,
            source: None,
            task: Some(task),
            size: Some("1024x1024".into()),
        }
    }

    /// Klein measured for edit too (the registry gains it in 6b-3; the tests don't depend on that).
    fn with_edit(mut m: Manifest) -> Manifest {
        if !m.measured.iter().any(|x| x.task() == Task::Edit) {
            m.measured.push(measured(Task::Edit, 12.0));
        }
        m
    }

    fn here(m: &Manifest) -> Option<Timing> {
        timing(m, Task::Generate, Some(GPU), false)
    }

    fn cand(m: &Manifest, installed: bool, fit: Tier) -> Candidate<'_> {
        Candidate { m, installed, fit }
    }

    #[test]
    fn picks_the_best_installed_tested_model() {
        let r = reg();
        let (z, k) = (r.get("z-image-turbo:q4").unwrap(), r.get("flux2-klein-4b:q8").unwrap());
        let cands = [cand(k, true, Tier::Fits), cand(z, true, Tier::Fits)];
        let p = pick(Task::Generate, &cands, None, &here).unwrap();
        assert_eq!(p.model, "z-image-turbo:q4");
        assert!(!p.loaded);
        assert_eq!(p.reason, "Z-Image Turbo: the best installed model for making images, best text, about 19 s here");
    }

    #[test]
    fn no_switch_when_the_loaded_model_qualifies() {
        let r = reg();
        let (z, k) = (r.get("z-image-turbo:q4").unwrap(), r.get("flux2-klein-4b:q8").unwrap());
        let cands = [cand(z, true, Tier::Fits), cand(k, true, Tier::Fits)];
        let p = pick(Task::Generate, &cands, Some("flux2-klein-4b:q8"), &here).unwrap();
        assert_eq!(p.model, "flux2-klein-4b:q8");
        assert!(p.loaded && p.reason.starts_with("FLUX.2 klein 4B: already loaded, fastest, about 10 s here"), "{}", p.reason);
        // A loaded model that doesn't qualify (can't edit / not installed) doesn't win.
        let p = pick(Task::Generate, &cands, Some("qwen-image:q4"), &here).unwrap();
        assert_eq!(p.model, "z-image-turbo:q4");
    }

    #[test]
    fn never_a_custom_untested_or_unfit_model() {
        let r = reg();
        let z = r.get("z-image-turbo:q4").unwrap();
        let mut custom = z.clone();
        custom.id = "hf.co/a/b:x.gguf".into();
        custom.custom = Some(crate::registry::Custom { source: custom.id.clone(), architecture: "Z-Image".into(), sd_version: "z_image".into(), base: None, added_ms: 0, test: None });
        let untested = r.get("z-image-turbo:q8").unwrap();
        let cands = [cand(&custom, true, Tier::Fits), cand(untested, true, Tier::Fits), cand(z, true, Tier::NeedsMoreMemory)];
        let e = pick(Task::Generate, &cands, Some(custom.id.as_str()), &here).unwrap_err();
        assert!(e.message.contains("No tested model for making images"), "{}", e.message);
        assert!(e.message.contains("pick one by name"), "it says why the installed ones don't count");
        // Not installed never counts either.
        assert!(pick(Task::Generate, &[cand(z, false, Tier::Fits)], None, &here).is_err());
    }

    #[test]
    fn a_loaded_offload_model_doesnt_block_one_that_fits() {
        let r = reg();
        let k = with_edit(r.get("flux2-klein-4b:q8").unwrap().clone());
        let mut q = r.get("qwen-image-edit-2511:q4").unwrap().clone();
        q.measured.push(measured(Task::Edit, 750.0));
        let t = |m: &Manifest| timing(m, Task::Edit, Some(GPU), false);
        let cands = [cand(&q, true, Tier::Offload), cand(&k, true, Tier::Fits)];
        let p = pick(Task::Edit, &cands, Some("qwen-image-edit-2511:q4"), &t).unwrap();
        assert_eq!(p.model, "flux2-klein-4b:q8", "klein fits: worth the switch");
        assert!(!p.loaded);
        // Alone, the loaded offload model is still used (and says so).
        let p = pick(Task::Edit, &[cand(&q, true, Tier::Offload)], Some("qwen-image-edit-2511:q4"), &t).unwrap();
        assert!(p.loaded && p.reason.contains("already loaded") && p.reason.contains("RAM offload"), "{}", p.reason);
    }

    #[test]
    fn ties_break_the_same_way_every_time() {
        let r = reg();
        let a = r.get("z-image-turbo:q4").unwrap();
        let mut b = a.clone();
        b.id = "z-image-turbo:q4b".into(); // same family, same size: only the id differs
        let one = pick(Task::Generate, &[cand(a, true, Tier::Fits), cand(&b, true, Tier::Fits)], None, &here).unwrap();
        let two = pick(Task::Generate, &[cand(&b, true, Tier::Fits), cand(a, true, Tier::Fits)], None, &here).unwrap();
        assert_eq!(one, two, "input order doesn't matter");
        assert_eq!(one.model, "z-image-turbo:q4");
        for _ in 0..20 {
            assert_eq!(pick(Task::Generate, &[cand(&b, true, Tier::Fits), cand(a, true, Tier::Fits)], None, &here).unwrap(), one);
        }
    }

    #[test]
    fn fits_beats_offload_and_clean_beats_restricted() {
        let r = reg();
        let (z, q, rv) = (r.get("z-image-turbo:q4").unwrap(), r.get("qwen-image:q4").unwrap(), r.get("realvisxl-v5:fp16").unwrap());
        let p = pick(Task::Generate, &[cand(q, true, Tier::Offload), cand(rv, true, Tier::Fits)], None, &here).unwrap();
        assert_eq!(p.model, "realvisxl-v5:fp16", "fits beats offload, even ranked lower");
        let p = pick(Task::Generate, &[cand(q, true, Tier::Offload)], None, &here).unwrap();
        assert!(p.reason.contains("the only installed model") && p.reason.contains("RAM offload"), "{}", p.reason);
        let p = pick(Task::Generate, &[cand(rv, true, Tier::Fits), cand(z, true, Tier::Fits)], None, &here).unwrap();
        assert_eq!(p.model, "z-image-turbo:q4");
    }

    #[test]
    fn edit_only_picks_models_tested_for_edit() {
        let r = reg();
        let z = r.get("z-image-turbo:q4").unwrap();
        let k = r.get("flux2-klein-4b:q8").unwrap().clone();
        let mut k_gen_only = k.clone();
        k_gen_only.measured.retain(|x| x.task() == Task::Generate);
        // Klein measured only for generate: not tested for edit, so Auto won't edit with it.
        let e = pick(Task::Edit, &[cand(z, true, Tier::Fits), cand(&k_gen_only, true, Tier::Fits)], None, &|m| timing(m, Task::Edit, Some(GPU), false)).unwrap_err();
        assert!(e.message.contains("No tested model for editing"), "{}", e.message);
        let k = with_edit(k);
        let p = pick(Task::Edit, &[cand(z, true, Tier::Fits), cand(&k, true, Tier::Fits)], Some("z-image-turbo:q4"), &|m| timing(m, Task::Edit, Some(GPU), false)).unwrap();
        assert_eq!(p.model, "flux2-klein-4b:q8", "the loaded model can't edit, so it switches");
        assert!(p.reason.starts_with("FLUX.2 klein 4B: the only installed model for editing, fast edits"), "{}", p.reason);
    }

    #[test]
    fn nothing_installed_names_the_model_to_pull() {
        let r = reg();
        let cands: Vec<Candidate> = r.models.iter().map(|m| cand(m, false, Tier::Fits)).collect();
        let e = pick(Task::Generate, &cands, None, &here).unwrap_err();
        let s = e.suggest.unwrap();
        assert_eq!(s.model, "z-image-turbo:q4");
        assert!(e.message.contains("Pull Z-Image Turbo (") && e.message.contains("`dawnroom pull z-image-turbo:q4`"), "{}", e.message);
        assert!(s.download_bytes > 6_000_000_000);
        // Nothing fits: no suggestion, and it says so.
        let cands: Vec<Candidate> = r.models.iter().map(|m| cand(m, false, Tier::NeedsMoreMemory)).collect();
        let e = pick(Task::Generate, &cands, None, &here).unwrap_err();
        assert!(e.suggest.is_none() && e.message.contains("none of the tested ones runs"));
    }

    #[test]
    fn fit_is_per_task() {
        let r = reg();
        let k = r.get("flux2-klein-4b:q8").unwrap();
        let mc = crate::fit::Machine { gpu_mib: 8151, integrated: false, ram_total_mib: 24 * 1024 };
        let generate = crate::fit::fit_for(k, Task::Generate, Some(&mc));
        let edit = crate::fit::fit_for(k, Task::Edit, Some(&mc));
        assert_eq!(generate.need_mib, Some(5246), "generating doesn't inherit the edit peak");
        assert_eq!(edit.need_mib, Some(6862));
        assert_eq!(crate::fit::fit(k, Some(&mc)), generate, "a model's fit is its main task's");
        // An edit-only model's main task is edit; never measured, so it's an estimate.
        let lc = r.get("fibo2-edit-turbo:int8").unwrap();
        assert!(crate::fit::fit(lc, Some(&mc)).estimate);
    }

    #[test]
    fn timings_say_where_they_were_measured() {
        let r = reg();
        let k = r.get("flux2-klein-4b:q8").unwrap();
        let t = timing(k, Task::Generate, Some(GPU), false).unwrap();
        assert!(t.here && t.label == "on your GPU" && t.warm_s == Some(10.4) && t.size.as_deref() == Some("1024x1024"), "{t:?}");
        let t = timing(k, Task::Generate, Some("NVIDIA GeForce RTX 4090"), false).unwrap();
        assert!(!t.here && t.label == format!("on {GPU}"), "another GPU: its name, not ours");
        assert_eq!(t.about(), "about 10 s on an RTX 5060 Laptop GPU");
        // On the integrated GPU the integrated measurement is "here".
        let t = timing(k, Task::Generate, Some("Intel(R) Graphics"), true).unwrap();
        assert!(t.here && t.warm_s == Some(156.5), "{t:?}");
        // Nobody measured it: no timing, never an invented one.
        assert_eq!(timing(r.get("z-image-turbo:q8").unwrap(), Task::Generate, Some(GPU), false), None);
        assert_eq!(timing(r.get("z-image-turbo:q4").unwrap(), Task::Edit, Some(GPU), false), None);
    }
}
