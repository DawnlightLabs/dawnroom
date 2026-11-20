//! What is this file? Architecture detection from tensor names and shapes, ported line by line
//! from sd.cpp's `ModelLoader::get_sd_version` at the pinned build (465adf3,
//! `src/model_loader.cpp`), so Dawnroom says yes only to what that build will load. Then the
//! Dawnroom side: which curated family a detected architecture borrows its companion files
//! (text encoders, VAE) and defaults from, and plain words for everything it can't run.

use std::collections::BTreeMap;

use crate::header::{Header, LLM_ARCHS};
use crate::registry::Role;

/// sd.cpp prefixes a `--diffusion-model` file's tensors with this (a `--model` checkpoint keeps
/// its own names), and detection runs on the prefixed names.
pub const DIFFUSION_PREFIX: &str = "model.diffusion_model.";

/// A checkpoint carries its own names for the diffusion model, VAE or text encoders.
const CHECKPOINT_MARKERS: &[&str] = &["model.diffusion_model.", "first_stage_model.", "cond_stage_model.", "conditioner.", "text_encoders."];

/// How sd.cpp would load the file: `--model` (a checkpoint) or `--diffusion-model`.
pub fn weights_role(h: &Header) -> Role {
    if h.tensors.iter().any(|(n, _)| CHECKPOINT_MARKERS.iter().any(|m| n.starts_with(m))) { Role::Checkpoint } else { Role::DiffusionModel }
}

/// The tensor names as sd.cpp sees them, sorted (its tensor map is a `std::map`, and the loop
/// returns at the first match, so the order is part of the logic).
fn names(h: &Header) -> BTreeMap<String, Vec<u64>> {
    let prefix = if weights_role(h) == Role::Checkpoint { "" } else { DIFFUSION_PREFIX };
    h.tensors.iter().map(|(n, ne)| (if n.starts_with(prefix) { n.clone() } else { format!("{prefix}{n}") }, ne.clone())).collect()
}

/// sd.cpp's `SDVersion` for this file, as its enum name in lowercase without `VERSION_`
/// (`z_image`, `flux2_klein`, `sdxl`…), or `None` (VERSION_COUNT: not a model it knows).
pub fn sd_version(h: &Header) -> Option<&'static str> {
    let map = names(h);
    let has = |n: &str| map.contains_key(n);
    let ne = |n: &str, i: usize| map.get(n).and_then(|v| v.get(i).copied()).unwrap_or(1);

    if has("model.diffusion_model.perceiver.latent_queries") {
        return Some("fibo2");
    }
    if has("model.diffusion_model.emb_in.image.weight") && has("model.diffusion_model.txt_mode_blocks.0.q_proj.weight") {
        return Some("flux3");
    }
    if has("model.diffusion_model.video_embedder.weight") && has("model.diffusion_model.shared_block_modulation.weight") {
        return Some("moss_image");
    }

    let (mut has_multiple_encoders, mut is_unet, mut is_xl, mut is_flux, mut is_flux2) = (false, false, false, false, false);
    let (mut has_single_block_47, mut is_wan, mut is_s2v, mut has_img_emb) = (false, false, false, false);
    let (mut has_middle_block_1, mut has_output_block_311, mut has_output_block_71, mut has_attn_1024) = (false, false, false, false);
    let (mut has_distilled_guidance, mut has_sdxl_add_embedding) = (false, false);
    let mut patch_embedding_channels = 0u64;
    let (mut token_embedding, mut input_block, mut context_embedding): (Option<&str>, Option<&str>, Option<&str>) = (None, None, None);

    for (name, shape) in &map {
        let f = |s: &str| name.contains(s);
        if f("model.diffusion_model.double_blocks.") || f("model.diffusion_model.single_transformer_blocks.") {
            is_flux = true;
        }
        if f("model.diffusion_model.net.lq_proj.latent_proj.0.weight") {
            return Some("pid");
        }
        if f("embed_image_indicator.weight") {
            return Some("ideogram4");
        }
        if f("model.diffusion_model.txtfusion.projector.weight") || f("model.diffusion_model.text_fusion.projector.weight") {
            return Some("krea2");
        }
        if f("model.diffusion_model.nerf_final_layer_conv.") {
            return Some("chroma_radiance");
        }
        if f("distilled_guidance_layer.in_proj.weight") {
            has_distilled_guidance = true;
        }
        if f("model.diffusion_model.joint_blocks.") {
            return Some("sd3");
        }
        if f("model.x_embedder.proj1.weight") && has("model.language_model.layers.0.self_attn.q_proj.weight") {
            return Some("hidream_o1");
        }
        if f("model.diffusion_model.transformer_blocks.0.attn.norm_added_q.weight") && has("model.diffusion_model.transformer_blocks.0.img_mlp.w1.weight") {
            return Some("lens");
        }
        if f("net.img_embedder.proj1.weight") {
            return Some("minit2i");
        }
        if f("language_model.model.layers.0.self_attn.q_proj_mot_gen.weight") {
            return Some("sensenova_u1_5");
        }
        if name == "model.diffusion_model.txt_in.text_norm.weight" {
            return Some("qwen_image_2_1");
        }
        if f("model.diffusion_model.transformer_blocks.0.img_mod.1.weight") {
            if has("model.diffusion_model.img_in.weight") && ne("model.diffusion_model.img_in.weight", 0) == 128 {
                return Some("mage_flow");
            }
            if has("model.diffusion_model.time_text_embed.addition_t_embedding.weight") {
                return Some("qwen_image_layered");
            }
            return Some("qwen_image");
        }
        if f("model.diffusion_model.txt_in.individual_token_refiner.blocks.0.adaLN_modulation.1.weight") {
            return Some("hunyuan_video");
        }
        if f("llm_adapter.blocks.0.cross_attn.q_proj.weight") {
            return Some("anima");
        }
        if f("model.diffusion_model.double_stream_modulation_img.lin.weight") {
            is_flux2 = true;
        }
        if f("dual_time_embed.semantic_embedder.linear_1.weight") {
            return Some("sefi_image");
        }
        if f("single_blocks.47.linear1.weight") {
            has_single_block_47 = true;
        }
        if f("model.diffusion_model.double_blocks.0.img_mlp.gate_proj.weight") {
            return Some("ovis_image");
        }
        if f("model.diffusion_model.sigvq_embedder.1.weight") {
            return Some("llada_image");
        }
        if f("model.diffusion_model.cap_embedder.0.weight") {
            if has("text_encoders.llm.connector.layers.0.self_attn.q_proj.weight") {
                return Some("ming_image");
            }
            if has("model.diffusion_model.local_decoder.out_conv.weight") {
                return Some("z_image_l2p");
            }
            return Some("z_image");
        }
        if f("double_stream_layers.0.img_instruct_attn.processor.img_to_q.weight") {
            return Some("boogu_image");
        }
        if f("model.diffusion_model.layers.0.adaLN_sa_ln.weight") {
            return Some("ernie_image");
        }
        if f("model.diffusion_model.t_block.1.weight") && has("model.diffusion_model.x_embedder.proj.weight") && !has("model.diffusion_model.audio_patchify_proj.weight") {
            return Some("pixart");
        }
        if f("model.diffusion_model.adaln_single.emb.timestep_embedder.linear_1.bias") {
            if has("model.diffusion_model.pos_embed.proj.weight") && has("model.diffusion_model.adaln_single.linear.weight") && !has("model.diffusion_model.audio_patchify_proj.weight") {
                return Some("pixart");
            }
            return Some("ltxav");
        }
        if f("model.diffusion_model.video_patch_proj.weight") && has("model.diffusion_model.audio_patch_proj.weight") {
            let decoder = ["first_stage_model.decoder.proj_out.weight", "vae.decoder.proj_out.weight"].into_iter().find(|n| has(n));
            if decoder.is_some_and(|d| ne(d, 1) == 768) {
                return Some("p_h3_image");
            }
            for prefix in ["", "embeddings_connectors."] {
                if has(&format!("{prefix}text_encoders.ph3_image_connector.proj.weight")) || has(&format!("{prefix}text_encoders.llm.text_connector.proj.weight")) {
                    return Some("p_h3_image");
                }
            }
            if has("embeddings_connectors.proj.weight")
                && ne("embeddings_connectors.proj.weight", 0) == 12800
                && ne("embeddings_connectors.proj.weight", 1) == 5120
                && has("embeddings_connectors.sink_embedding")
            {
                return Some("p_h3_image");
            }
            return Some("minimax_h3");
        }
        if name == "model.diffusion_model.y_embedder.layer_pool.weight" && has("model.diffusion_model.modulation_cores.adaln_img.weight") {
            return Some("iris");
        }
        if f("model.diffusion_model.blocks.0.cross_attn.norm_k.weight") {
            is_wan = true;
        }
        if f("casual_audio_encoder.weights") || f("audio_injector.injector.0.q.weight") {
            is_s2v = true;
        }
        if f("model.diffusion_model.patch_embedder.weight") {
            return Some("lingbot_video");
        }
        if f("model.diffusion_model.patch_embedding.weight") {
            patch_embedding_channels = shape.get(3).copied().unwrap_or(1);
        }
        if f("model.diffusion_model.img_emb") {
            has_img_emb = true;
        }
        if f("model.diffusion_model.input_blocks.") || f("model.diffusion_model.down_blocks.") || f("unet.down_blocks.") {
            is_unet = true;
            if has_multiple_encoders {
                is_xl = true;
            }
        }
        if f("conditioner.embedders.1") || f("cond_stage_model.1") || f("te.1") {
            has_multiple_encoders = true;
            if is_unet {
                is_xl = true;
            }
        }
        if f("model.diffusion_model.add_embedding.") || f("model.diffusion_model.label_emb.0.0.") {
            has_sdxl_add_embedding = true;
        }
        if f("model.diffusion_model.input_blocks.8.0.time_mixer.mix_factor") {
            return Some("svd");
        }
        if f("model.diffusion_model.middle_block.1.") || f("model.diffusion_model.mid_block.resnets.1.") || f("unet.mid_block.resnets.1.") {
            has_middle_block_1 = true;
        }
        if f("model.diffusion_model.output_blocks.3.1.transformer_blocks.1") || f("model.diffusion_model.up_blocks.1.attentions.0.transformer_blocks.1") || f("unet.up_blocks.1.attentions.0.transformer_blocks.1") {
            has_output_block_311 = true;
        }
        if f("model.diffusion_model.output_blocks.7.1") || f("model.diffusion_model.up_blocks.2.attentions.1") || f("unet.up_blocks.2.attentions.1") {
            has_output_block_71 = true;
            if f("model.diffusion_model.output_blocks.7.1.transformer_blocks.0.attn1.to_k.weight") && shape.first() == Some(&1024) {
                has_attn_1024 = true;
            }
        }
        if matches!(
            name.as_str(),
            "cond_stage_model.transformer.text_model.embeddings.token_embedding.weight"
                | "cond_stage_model.model.token_embedding.weight"
                | "text_model.embeddings.token_embedding.weight"
                | "te.text_model.embeddings.token_embedding.weight"
                | "conditioner.embedders.0.model.token_embedding.weight"
                | "conditioner.embedders.0.transformer.text_model.embeddings.token_embedding.weight"
        ) {
            token_embedding = Some(name);
        }
        if matches!(name.as_str(), "model.diffusion_model.input_blocks.0.0.weight" | "model.diffusion_model.conv_in.weight" | "model.diffusion_model.img_in.weight" | "unet.conv_in.weight") {
            input_block = Some(name);
        }
        if matches!(name.as_str(), "model.diffusion_model.txt_in.weight" | "model.diffusion_model.context_embedder.weight") {
            context_embedding = Some(name);
        }
    }
    if is_wan {
        if is_s2v {
            return Some("wan2_2_s2v");
        }
        if patch_embedding_channels == 184320 && !has_img_emb {
            return Some("wan2_2_i2v");
        }
        if patch_embedding_channels == 147456 && !has_img_emb {
            return Some("wan2_2_ti2v");
        }
        return Some("wan2");
    }
    let dim = |n: Option<&str>, i: usize| n.map(|n| ne(n, i)).unwrap_or(1);
    let is_inpaint = dim(input_block, 2) == 9;
    let is_ip2p = dim(input_block, 2) == 8;
    if is_unet && has_sdxl_add_embedding {
        is_xl = true;
    }
    if is_xl {
        if is_inpaint {
            return Some("sdxl_inpaint");
        }
        if is_ip2p {
            return Some("sdxl_pix2pix");
        }
        if !has_middle_block_1 {
            return Some(if has_output_block_311 { "sdxl_ssd1b" } else { "sdxl_vega" });
        }
        return Some("sdxl");
    }
    if is_flux && !is_flux2 {
        if has_distilled_guidance {
            return Some("chroma");
        }
        if dim(context_embedding, 0) == 3584 {
            return Some("longcat");
        }
        return Some(match dim(input_block, 0) {
            384 => "flux_fill",
            128 => "flux_controls",
            196 => "flex_2",
            _ => "flux",
        });
    }
    if is_flux2 {
        return Some(if has_single_block_47 { "flux2" } else { "flux2_klein" });
    }
    match dim(token_embedding, 0) {
        768 => Some(if is_inpaint {
            "sd1_inpaint"
        } else if is_ip2p {
            "sd1_pix2pix"
        } else if !has_middle_block_1 {
            if has_output_block_71 { "sd1_tiny_unet" } else { "sdxs_512_ds" }
        } else {
            "sd1"
        }),
        1024 => Some(if is_inpaint {
            "sd2_inpaint"
        } else if !has_middle_block_1 {
            if has_attn_1024 { "sdxs_09" } else { "sd2_tiny_unet" }
        } else {
            "sd2"
        }),
        _ => None,
    }
}

// ---------- Dawnroom's side ----------

/// A detected image model Dawnroom can run.
#[derive(Debug, Clone, PartialEq)]
pub struct Family {
    /// sd.cpp's version name.
    pub version: &'static str,
    /// What to call the architecture.
    pub label: &'static str,
    /// The curated model whose companions and defaults it borrows (`None`: a self-contained
    /// checkpoint with nothing to borrow, e.g. SD 2.x).
    pub base: Option<&'static str>,
}

/// Why a file can't be added, in plain words. `code` is for clients.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub code: &'static str,
    /// "a LoRA (…)", "a language model (qwen3)"… the "what it is" part.
    pub what: String,
    pub message: String,
}

fn refuse(code: &'static str, what: impl Into<String>, why: &str) -> Refusal {
    let what = what.into();
    Refusal { code, message: format!("This is {what}, not an image model Dawnroom can run. {why}"), what }
}

/// Image architectures with a curated family to borrow from. Variant picks (FLUX.1 schnell or
/// dev, SD 3.5 Large or Medium, klein 4B or 9B) read the tensors, since the version alone
/// doesn't tell them apart.
fn family_for(version: &'static str, h: &Header) -> Option<Family> {
    let map = names(h);
    let fam = |label, base| Some(Family { version, label, base });
    match version {
        "sd1" => fam("Stable Diffusion 1.x", Some("sd15")),
        "sd2" => fam("Stable Diffusion 2.x", None),
        "sdxl" | "sdxl_ssd1b" | "sdxl_vega" => fam("Stable Diffusion XL", Some("sdxl")),
        "sd3" => {
            // SD 3.5 Large has 38 joint blocks, Medium (and SD3 Medium) 24.
            let blocks = map.keys().filter_map(|n| n.strip_prefix("model.diffusion_model.joint_blocks.")?.split('.').next()?.parse::<u32>().ok()).max().unwrap_or(0) + 1;
            if blocks >= 38 { fam("Stable Diffusion 3.5 Large", Some("sd35-large")) } else { fam("Stable Diffusion 3.5 Medium", Some("sd35-medium")) }
        }
        // Only FLUX.1 dev has the guidance embedding.
        "flux" if map.keys().any(|n| n.contains("guidance_in.")) => fam("FLUX.1 dev", Some("flux1-dev")),
        "flux" => fam("FLUX.1 schnell", Some("flux1-schnell")),
        "flux2" => fam("FLUX.2 dev", Some("flux2-dev")),
        "flux2_klein" => {
            // klein 4B's hidden size is 3072, 9B's 4096 (they need different Qwen3 encoders).
            let hidden = map.get("model.diffusion_model.double_blocks.0.img_attn.qkv.weight").and_then(|ne| ne.first().copied()).unwrap_or(0);
            if hidden > 3072 { fam("FLUX.2 klein 9B", Some("flux2-klein-9b")) } else { fam("FLUX.2 klein 4B", Some("flux2-klein-4b")) }
        }
        "flux3" => fam("FLUX.3 klein", Some("flux3-klein")),
        "chroma" => fam("Chroma", Some("chroma1-hd")),
        "chroma_radiance" => fam("Chroma Radiance", Some("chroma1-radiance")),
        "longcat" => fam("LongCat-Image", Some("longcat-image")),
        "qwen_image" => fam("Qwen-Image", Some("qwen-image")),
        "qwen_image_2_1" => fam("Qwen-Image 2.1", Some("qwen-image-2-1")),
        "z_image" => fam("Z-Image", Some("z-image-turbo")),
        "ovis_image" => fam("Ovis-Image", Some("ovis-image")),
        "ernie_image" => fam("ERNIE-Image", Some("ernie-image")),
        "pixart" => fam("PixArt-Σ", Some("pixart-sigma")),
        "krea2" => fam("Krea 2", Some("krea2-turbo")),
        "hidream_o1" => fam("HiDream-O1-Image", Some("hidream-o1-dev")),
        "ideogram4" => fam("Ideogram 4", Some("ideogram4")),
        "fibo2" => fam("Fibo 2.0", Some("fibo2-dev")),
        "moss_image" => fam("MOSS-Image", Some("moss-image")),
        _ => None,
    }
}

const VIDEO: &[&str] = &["wan2", "wan2_2_i2v", "wan2_2_ti2v", "wan2_2_s2v", "hunyuan_video", "ltxav", "lingbot_video", "minimax_h3", "svd"];
const EDIT: &[&str] = &["sd1_inpaint", "sd1_pix2pix", "sd2_inpaint", "sdxl_inpaint", "sdxl_pix2pix", "flux_fill", "flux_controls", "flex_2", "qwen_image_layered"];

/// The verdict on a header: an image family Dawnroom can run, or why not.
pub fn classify(h: &Header) -> Result<Family, Refusal> {
    let has = |s: &str| h.tensors.iter().any(|(n, _)| n.contains(s));
    let starts = |s: &str| h.tensors.iter().any(|(n, _)| n.starts_with(s));
    let only = |prefixes: &[&str]| !h.tensors.is_empty() && h.tensors.iter().all(|(n, _)| prefixes.iter().any(|p| n.starts_with(p)));
    // A LoRA's names can contain a full model's (FLUX double_blocks…), so it goes first.
    if ["lora_up.", "lora_down.", ".lora_A.", ".lora_B.", "lora_unet_", "lora_te", ".hada_w1", ".lokr_w1"].iter().any(|s| has(s)) {
        return Err(refuse("lora", "a LoRA (a small add-on that changes a model's style or subject)", "It isn't a model on its own, and Dawnroom doesn't apply LoRAs yet."));
    }
    if let Some(arch) = h.meta.get("general.architecture").filter(|a| LLM_ARCHS.contains(&a.as_str())) {
        if arch.starts_with("t5") {
            return Err(refuse("text_encoder", format!("a text encoder ({arch})"), "Dawnroom adds the text encoder an image model needs by itself."));
        }
        return Err(refuse(
            "not_image_model",
            format!("a language model (LLM, architecture {arch})"),
            "Image models use some LLMs as their text encoder, and Dawnroom adds the right one by itself.",
        ));
    }
    if let Some(v) = sd_version(h) {
        if let Some(f) = family_for(v, h) {
            return Ok(f);
        }
        if VIDEO.contains(&v) {
            return Err(refuse("video_model", format!("a video model (sd.cpp: {v})"), "Video comes in a later version of Dawnroom."));
        }
        if EDIT.contains(&v) {
            return Err(Refusal {
                code: "unsupported_family",
                what: format!("an inpainting, control or layered model (sd.cpp: {v})"),
                message: format!("This is an inpainting, control or layered model (sd.cpp: {v}). Dawnroom runs text-to-image models; these need an input image it can't pass yet."),
            });
        }
        return Err(Refusal {
            code: "unsupported_family",
            what: format!("an image model of a family Dawnroom doesn't carry yet (sd.cpp: {v})"),
            message: format!("This is an image model sd.cpp knows (its {v} family), but Dawnroom's library has no model of that family to take its text encoder and VAE from, so it can't set it up yet."),
        });
    }
    if has("token_embd.weight") || has("blk.0.attn_q") || starts("model.layers.0.self_attn") || starts("model.embed_tokens") || starts("lm_head.") {
        return Err(refuse("not_image_model", "a language model (LLM)", "Image models use some LLMs as their text encoder, and Dawnroom adds the right one by itself."));
    }
    if starts("text_model.encoder.layers.") || starts("encoder.block.0.layer.0.SelfAttention") || starts("shared.weight") {
        return Err(refuse("text_encoder", "a text encoder (CLIP or T5)", "Dawnroom adds the text encoder an image model needs by itself."));
    }
    if has("control_model.") || has("controlnet_") || has("input_hint_block") {
        return Err(refuse("not_image_model", "a ControlNet (it guides a model with a pose or edge image)", "Dawnroom doesn't use ControlNets yet."));
    }
    // SD/FLUX VAEs, plus the SDXL VAE's `model_ema.*` leftovers and the Wan/Qwen-Image VAE's `conv1`/`conv2`.
    if only(&["encoder.", "decoder.", "quant_conv.", "post_quant_conv.", "first_stage_model.", "vae.", "bn.", "model_ema.", "conv1.", "conv2."]) {
        return Err(refuse("vae_only", "a VAE (the part that turns latents into pixels)", "It's one piece of a model, not a whole one; Dawnroom adds the VAE an image model needs by itself."));
    }
    if has("conv_first.") || has("RRDB") || (has("model.0.weight") && has("model.1.sub.")) {
        return Err(refuse("not_image_model", "an upscaler (ESRGAN)", "Upscaling comes in a later version of Dawnroom."));
    }
    Err(Refusal {
        code: "unknown_architecture",
        what: "a model Dawnroom couldn't identify".into(),
        message: "Dawnroom couldn't tell what kind of model this is from its tensor names, so the pinned sd.cpp build most likely can't run it.".into(),
    })
}

/// Which companion roles the file already carries itself (an all-in-one checkpoint), so they
/// aren't added twice: sd.cpp refuses duplicate tensors.
pub fn carries(h: &Header, role: Role) -> bool {
    let starts = |ps: &[&str]| h.tensors.iter().any(|(n, _)| ps.iter().any(|p| n.starts_with(p)));
    match role {
        Role::Vae => starts(&["first_stage_model.", "vae."]),
        Role::ClipL => starts(&["text_encoders.clip_l.", "cond_stage_model.transformer.", "conditioner.embedders.0."]),
        Role::ClipG => starts(&["text_encoders.clip_g.", "cond_stage_model.1.", "conditioner.embedders.1."]),
        Role::T5xxl => starts(&["text_encoders.t5xxl."]),
        Role::Llm | Role::LlmVision => starts(&["text_encoders.llm."]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(names: &[(&str, &[u64])]) -> Header {
        Header { tensors: names.iter().map(|(n, ne)| (n.to_string(), ne.to_vec())).collect(), ..Default::default() }
    }

    #[test]
    fn each_curated_family_is_recognised_from_its_tensor_names() {
        type Case<'a> = (&'a [(&'a str, &'a [u64])], &'a str, Option<&'a str>);
        let cases: &[Case] = &[
            (&[("cap_embedder.0.weight", &[3840])], "z_image", Some("z-image-turbo")),
            (&[("double_blocks.0.img_attn.qkv.weight", &[3072, 9216]), ("img_in.weight", &[64, 3072])], "flux", Some("flux1-schnell")),
            (&[("double_blocks.0.img_attn.qkv.weight", &[3072, 9216]), ("guidance_in.in_layer.weight", &[256, 3072])], "flux", Some("flux1-dev")),
            (&[("double_stream_modulation_img.lin.weight", &[3072]), ("double_blocks.0.img_attn.qkv.weight", &[3072, 9216])], "flux2_klein", Some("flux2-klein-4b")),
            (&[("double_stream_modulation_img.lin.weight", &[4096]), ("double_blocks.0.img_attn.qkv.weight", &[4096, 12288])], "flux2_klein", Some("flux2-klein-9b")),
            (&[("double_stream_modulation_img.lin.weight", &[6144]), ("single_blocks.47.linear1.weight", &[6144])], "flux2", Some("flux2-dev")),
            (&[("transformer_blocks.0.img_mod.1.weight", &[3072]), ("img_in.weight", &[64, 3072])], "qwen_image", Some("qwen-image")),
            (&[("txt_in.text_norm.weight", &[3584])], "qwen_image_2_1", Some("qwen-image-2-1")),
            (&[("joint_blocks.0.x_block.attn.qkv.weight", &[1536]), ("joint_blocks.23.context_block.attn.qkv.weight", &[1536])], "sd3", Some("sd35-medium")),
            (&[("model.diffusion_model.joint_blocks.37.x_block.attn.qkv.weight", &[2432]), ("first_stage_model.decoder.conv_in.weight", &[3])], "sd3", Some("sd35-large")),
            (&[("double_blocks.0.img_attn.qkv.weight", &[3072]), ("distilled_guidance_layer.in_proj.weight", &[64])], "chroma", Some("chroma1-hd")),
            (&[("layers.0.adaLN_sa_ln.weight", &[4096])], "ernie_image", Some("ernie-image")),
            (&[("t_block.1.weight", &[1152]), ("x_embedder.proj.weight", &[1152])], "pixart", Some("pixart-sigma")),
        ];
        for (names, version, base) in cases {
            let hd = h(names);
            assert_eq!(sd_version(&hd), Some(*version), "{names:?}");
            assert_eq!(classify(&hd).map(|f| f.base).ok().flatten(), *base, "{names:?}");
        }
    }

    #[test]
    fn checkpoints_keep_their_names_and_unets_need_their_text_encoder() {
        // An SD 1.5 checkpoint: UNet + CLIP-L (token embedding 768 wide) + VAE.
        let sd1 = h(&[
            ("model.diffusion_model.input_blocks.0.0.weight", &[3, 3, 4, 320]),
            ("model.diffusion_model.middle_block.1.norm.weight", &[320]),
            ("cond_stage_model.transformer.text_model.embeddings.token_embedding.weight", &[768, 49408]),
            ("first_stage_model.decoder.conv_in.weight", &[3, 3, 4, 512]),
        ]);
        assert_eq!(weights_role(&sd1), Role::Checkpoint);
        assert_eq!(sd_version(&sd1), Some("sd1"));
        assert_eq!(classify(&sd1).unwrap().base, Some("sd15"));
        assert!(carries(&sd1, Role::Vae) && carries(&sd1, Role::ClipL) && !carries(&sd1, Role::T5xxl));
        // Inpainting UNet (9 input channels): sd.cpp knows it, Dawnroom says what it is.
        let inpaint = h(&[
            ("model.diffusion_model.input_blocks.0.0.weight", &[3, 3, 9, 320]),
            ("model.diffusion_model.middle_block.1.norm.weight", &[320]),
            ("cond_stage_model.transformer.text_model.embeddings.token_embedding.weight", &[768, 49408]),
        ]);
        assert_eq!(sd_version(&inpaint), Some("sd1_inpaint"));
        assert!(classify(&inpaint).unwrap_err().message.contains("inpainting"));
    }

    #[test]
    fn non_image_models_are_named() {
        let lora = h(&[("double_blocks.0.img_attn.qkv.lora_A.weight", &[16, 3072])]);
        assert_eq!(classify(&lora).unwrap_err().code, "lora");
        let mut llm = Header::default();
        llm.meta.insert("general.architecture".into(), "qwen3".into());
        let r = classify(&llm).unwrap_err();
        assert_eq!(r.code, "not_image_model");
        assert!(r.message.contains("language model (LLM, architecture qwen3)") && r.message.contains("not an image model"), "{}", r.message);
        let vae = h(&[("encoder.conv_in.weight", &[3]), ("decoder.conv_out.weight", &[3]), ("quant_conv.weight", &[1])]);
        assert_eq!(classify(&vae).unwrap_err().code, "vae_only");
        let qwen_vae = h(&[("conv1.weight", &[1]), ("decoder.head.0.gamma", &[1]), ("encoder.conv1.weight", &[1])]);
        assert_eq!(classify(&qwen_vae).unwrap_err().code, "vae_only");
        let wan = h(&[("blocks.0.cross_attn.norm_k.weight", &[5120]), ("patch_embedding.weight", &[1, 2, 2, 147456])]);
        assert_eq!(sd_version(&wan), Some("wan2_2_ti2v"));
        assert_eq!(classify(&wan).unwrap_err().code, "video_model");
        let clip = h(&[("text_model.encoder.layers.0.mlp.fc1.weight", &[768])]);
        assert_eq!(classify(&clip).unwrap_err().code, "text_encoder");
        let lens = h(&[("transformer_blocks.0.attn.norm_added_q.weight", &[128]), ("transformer_blocks.0.img_mlp.w1.weight", &[1])]);
        assert_eq!(classify(&lens).unwrap_err().code, "unsupported_family", "sd.cpp knows Lens, Dawnroom has no Lens to borrow from");
        assert_eq!(classify(&h(&[("foo.bar", &[1])])).unwrap_err().code, "unknown_architecture");
    }
}
