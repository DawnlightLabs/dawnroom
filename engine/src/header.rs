//! Reads a model file's header (tensor names and shapes) without its weights: GGUF's key/value
//! section and tensor infos, or safetensors' JSON header. The parsers work on a prefix of the
//! file and say how many more bytes they need, so a check reads a few hundred KB over HTTP Range
//! (or from disk) instead of gigabytes.

use std::collections::HashMap;

/// What a header says about a file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Header {
    pub format: Format,
    /// Tensor name -> dims in ggml order (`ne`: the fastest-moving dimension first, the reverse of
    /// safetensors' `shape`), so shape checks read the same as sd.cpp's.
    pub tensors: Vec<(String, Vec<u64>)>,
    /// GGUF string values (`general.architecture`, `general.name`…) and safetensors `__metadata__`.
    pub meta: HashMap<String, String>,
    /// Bytes of the file the header took.
    pub header_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Gguf,
    Safetensors,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Gguf => "GGUF",
            Format::Safetensors => "safetensors",
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Parse {
    /// The prefix ends inside the header: read at least this many bytes from the start.
    NeedBytes(u64),
    /// Not a valid header, in plain words.
    Bad(String),
}

/// The most of a file a check reads. Real headers are far smaller (an LLM GGUF's tokenizer is the
/// biggest seen, a few MB); anything past this is refused rather than downloaded.
pub const MAX_HEADER_BYTES: u64 = 64 << 20;

/// Language-model architectures in GGUF's `general.architecture` (llama.cpp's names). Seeing one
/// ends the read early: an LLM's tokenizer alone can be megabytes.
pub const LLM_ARCHS: &[&str] = &[
    "llama", "llama4", "qwen", "qwen2", "qwen2moe", "qwen2vl", "qwen3", "qwen3moe", "qwen3vl", "qwen3vlmoe", "gemma", "gemma2", "gemma3", "gemma3n", "mistral3", "phi2", "phi3",
    "phimoe", "gpt2", "gptneox", "gpt-oss", "falcon", "mpt", "starcoder", "starcoder2", "baichuan", "bloom", "stablelm", "internlm2", "minicpm", "minicpm3", "deepseek",
    "deepseek2", "command-r", "cohere2", "olmo", "olmo2", "olmoe", "granite", "granitemoe", "exaone", "glm4", "chatglm", "bert", "nomic-bert", "jina-bert-v2", "mamba",
    "rwkv6", "rwkv7", "smollm3", "hunyuan-moe", "ernie4_5", "ernie4_5-moe", "t5", "t5encoder",
];

/// Magic bytes say which parser to use: `GGUF`, or a safetensors length + `{`. Pickle-based
/// formats (a zip for torch, 0x80 for a raw pickle) are named so the refusal can say why.
pub fn sniff(prefix: &[u8]) -> Result<Format, String> {
    if prefix.starts_with(b"GGUF") {
        return Ok(Format::Gguf);
    }
    if prefix.starts_with(b"PK\x03\x04") {
        return Err("a PyTorch zip (pickle) file".into());
    }
    if prefix.first() == Some(&0x80) {
        return Err("a Python pickle file".into());
    }
    if prefix.len() >= 9 && prefix[8] == b'{' {
        return Ok(Format::Safetensors);
    }
    Err("neither GGUF nor safetensors".into())
}

pub fn parse(prefix: &[u8]) -> Result<Header, Parse> {
    match sniff(prefix) {
        Ok(Format::Gguf) => parse_gguf(prefix),
        Ok(Format::Safetensors) => parse_safetensors(prefix),
        Err(what) => Err(Parse::Bad(format!("The file is {what}."))),
    }
}

// ---------- safetensors ----------

pub fn parse_safetensors(b: &[u8]) -> Result<Header, Parse> {
    if b.len() < 8 {
        return Err(Parse::NeedBytes(8));
    }
    let n = u64::from_le_bytes(b[..8].try_into().unwrap());
    if n <= 2 || n > MAX_HEADER_BYTES {
        return Err(Parse::Bad(format!("The safetensors header says it's {n} bytes, which isn't a real header.")));
    }
    let end = 8 + n;
    if (b.len() as u64) < end {
        return Err(Parse::NeedBytes(end));
    }
    let json: serde_json::Value = serde_json::from_slice(&b[8..end as usize]).map_err(|e| Parse::Bad(format!("The safetensors header isn't valid JSON ({e}).")))?;
    let obj = json.as_object().ok_or_else(|| Parse::Bad("The safetensors header isn't a JSON object.".into()))?;
    let mut h = Header { format: Format::Safetensors, header_bytes: end, ..Default::default() };
    for (name, v) in obj {
        if name == "__metadata__" {
            for (k, v) in v.as_object().into_iter().flatten() {
                if let Some(s) = v.as_str() {
                    h.meta.insert(k.clone(), s.to_string());
                }
            }
            continue;
        }
        let shape: Vec<u64> = v["shape"].as_array().map(|a| a.iter().filter_map(|d| d.as_u64()).collect()).unwrap_or_default();
        h.tensors.push((name.clone(), shape.into_iter().rev().collect()));
    }
    Ok(h)
}

// ---------- GGUF ----------

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], Parse> {
        // Asking for more than the cap means a corrupt length, not a big header.
        if (self.at as u64).saturating_add(n as u64) > MAX_HEADER_BYTES {
            return Err(Parse::Bad("The GGUF header is larger than any real model header.".into()));
        }
        if self.at + n > self.b.len() {
            // Read well past the gap so a long tokenizer array doesn't take one round trip per string.
            return Err(Parse::NeedBytes(((self.at + n) as u64).max(self.b.len() as u64 * 2)));
        }
        let s = &self.b[self.at..self.at + n];
        self.at += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, Parse> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, Parse> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String, Parse> {
        let n = self.u64()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    /// Skips (or, for strings, returns) one value of GGUF type `t`.
    fn value(&mut self, t: u32) -> Result<Option<String>, Parse> {
        let fixed = match t {
            0 | 1 | 7 => 1,
            2 | 3 => 2,
            4..=6 => 4,
            10..=12 => 8,
            8 => return self.string().map(Some),
            9 => {
                let (et, count) = (self.u32()?, self.u64()?);
                if count > MAX_HEADER_BYTES {
                    return Err(Parse::Bad("A GGUF array is longer than any real header.".into()));
                }
                match et {
                    0 | 1 | 7 => self.take(count as usize)?,
                    2 | 3 => self.take(count as usize * 2)?,
                    4..=6 => self.take(count as usize * 4)?,
                    10..=12 => self.take(count as usize * 8)?,
                    _ => {
                        for _ in 0..count {
                            self.value(et)?;
                        }
                        return Ok(None);
                    }
                };
                return Ok(None);
            }
            _ => return Err(Parse::Bad(format!("Unknown GGUF value type {t}."))),
        };
        self.take(fixed)?;
        Ok(None)
    }
}

pub fn parse_gguf(b: &[u8]) -> Result<Header, Parse> {
    let mut r = Reader { b, at: 0 };
    if r.take(4)? != b"GGUF" {
        return Err(Parse::Bad("Not a GGUF file.".into()));
    }
    let version = r.u32()?;
    if !(2..=3).contains(&version) {
        return Err(Parse::Bad(format!("GGUF version {version} isn't supported (2 and 3 are).")));
    }
    let (n_tensors, n_kv) = (r.u64()?, r.u64()?);
    if n_tensors > 1_000_000 || n_kv > 1_000_000 {
        return Err(Parse::Bad("The GGUF header counts are corrupt.".into()));
    }
    let mut h = Header { format: Format::Gguf, ..Default::default() };
    for _ in 0..n_kv {
        let key = r.string()?;
        let t = r.u32()?;
        if let Some(v) = r.value(t)? {
            // A language model: that's all the check needs to know.
            let llm = key == "general.architecture" && LLM_ARCHS.contains(&v.as_str());
            h.meta.insert(key, v);
            if llm {
                h.header_bytes = r.at as u64;
                return Ok(h);
            }
        }
    }
    for _ in 0..n_tensors {
        let name = r.string()?;
        let n_dims = r.u32()?;
        if n_dims > 8 {
            return Err(Parse::Bad(format!("Tensor {name} has {n_dims} dimensions.")));
        }
        let mut ne = Vec::with_capacity(n_dims as usize);
        for _ in 0..n_dims {
            ne.push(r.u64()?);
        }
        let _type = r.u32()?;
        let _offset = r.u64()?;
        h.tensors.push((name, ne));
    }
    h.header_bytes = r.at as u64;
    Ok(h)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A GGUF header with these string keys and tensors (type f16, offset 0).
    pub(crate) fn gguf(kv: &[(&str, &str)], tensors: &[(&str, &[u64])]) -> Vec<u8> {
        let mut b = b"GGUF".to_vec();
        b.extend(3u32.to_le_bytes());
        b.extend((tensors.len() as u64).to_le_bytes());
        b.extend((kv.len() as u64).to_le_bytes());
        let s = |b: &mut Vec<u8>, s: &str| {
            b.extend((s.len() as u64).to_le_bytes());
            b.extend(s.as_bytes());
        };
        for (k, v) in kv {
            s(&mut b, k);
            b.extend(8u32.to_le_bytes());
            s(&mut b, v);
        }
        for (name, ne) in tensors {
            s(&mut b, name);
            b.extend((ne.len() as u32).to_le_bytes());
            for d in *ne {
                b.extend(d.to_le_bytes());
            }
            b.extend(1u32.to_le_bytes());
            b.extend(0u64.to_le_bytes());
        }
        b
    }

    /// A safetensors header (shapes in safetensors order) followed by `pad` zero bytes.
    pub(crate) fn safetensors(tensors: &[(&str, &[u64])], meta: &[(&str, &str)], pad: usize) -> Vec<u8> {
        let mut obj = serde_json::Map::new();
        if !meta.is_empty() {
            obj.insert("__metadata__".into(), serde_json::Value::Object(meta.iter().map(|(k, v)| (k.to_string(), serde_json::json!(v))).collect()));
        }
        for (name, shape) in tensors {
            obj.insert(name.to_string(), serde_json::json!({ "dtype": "F16", "shape": shape, "data_offsets": [0, 0] }));
        }
        let json = serde_json::to_vec(&obj).unwrap();
        let mut b = (json.len() as u64).to_le_bytes().to_vec();
        b.extend(json);
        b.extend(std::iter::repeat_n(0u8, pad));
        b
    }

    #[test]
    fn gguf_tensors_and_keys_are_read_and_a_short_prefix_asks_for_more() {
        let b = gguf(&[("general.architecture", "flux")], &[("double_blocks.0.img_attn.qkv.weight", &[3072, 9216])]);
        let h = parse(&b).unwrap();
        assert_eq!(h.format, Format::Gguf);
        assert_eq!(h.meta["general.architecture"], "flux");
        assert_eq!(h.tensors, vec![("double_blocks.0.img_attn.qkv.weight".to_string(), vec![3072, 9216])]);
        assert_eq!(h.header_bytes, b.len() as u64);
        match parse(&b[..b.len() - 3]) {
            Err(Parse::NeedBytes(n)) => assert!(n >= b.len() as u64),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_llm_gguf_stops_at_its_architecture() {
        let mut b = gguf(&[("general.architecture", "qwen3"), ("tokenizer.ggml.model", "gpt2")], &[("token_embd.weight", &[1024, 151936])]);
        let at = b.len();
        b.truncate(at - 40); // the rest isn't needed
        let h = parse(&b).unwrap();
        assert_eq!(h.meta["general.architecture"], "qwen3");
        assert!(h.tensors.is_empty());
    }

    #[test]
    fn safetensors_shapes_are_reversed_into_ggml_order() {
        let b = safetensors(&[("model.diffusion_model.input_blocks.0.0.weight", &[320, 4, 3, 3])], &[("modelspec.license", "MIT")], 100);
        let h = parse(&b).unwrap();
        assert_eq!(h.format, Format::Safetensors);
        assert_eq!(h.tensors[0].1, vec![3, 3, 4, 320]);
        assert_eq!(h.meta["modelspec.license"], "MIT");
        assert_eq!(parse(&b[..20]), Err(Parse::NeedBytes(h.header_bytes)));
    }

    #[test]
    fn pickles_and_junk_are_named() {
        assert!(sniff(b"PK\x03\x04rest").unwrap_err().contains("PyTorch"));
        assert!(sniff(&[0x80, 2, 0x7d]).unwrap_err().contains("pickle"));
        assert!(matches!(parse(b"<html><body>"), Err(Parse::Bad(_))));
        let mut b = 10u64.to_le_bytes().to_vec();
        b.extend(b"{not json}");
        assert!(matches!(parse(&b), Err(Parse::Bad(_))));
        let mut huge = (MAX_HEADER_BYTES + 1).to_le_bytes().to_vec();
        huge.push(b'{');
        assert!(matches!(parse(&huge), Err(Parse::Bad(_))));
    }
}
