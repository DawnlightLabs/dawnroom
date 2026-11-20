//! Input images for edits, stored in the data folder by sha256 (`<data>/inputs/<sha256>.<ext>`):
//! the original bytes, never re-encoded, so the hash of a Library PNG passed back in matches its
//! output hash (that's how an edit knows what it was edited from). Nothing here leaves the machine.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Bigger than any photo a phone takes; keeps a mistaken upload from filling memory.
pub const MAX_BYTES: usize = 50 * 1024 * 1024;
/// Longest side accepted. sd.cpp resizes references itself; this only stops absurd inputs.
pub const MAX_SIDE: u32 = 8192;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Input {
    pub sha256: String,
    /// png | jpeg | webp
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    #[serde(skip)]
    pub path: PathBuf,
}

fn ext_of(f: image::ImageFormat) -> Option<&'static str> {
    match f {
        image::ImageFormat::Png => Some("png"),
        image::ImageFormat::Jpeg => Some("jpeg"),
        image::ImageFormat::WebP => Some("webp"),
        _ => None,
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Checks the bytes are a PNG, JPEG or WebP image of a sane size and stores them (once) by hash.
pub fn store(dir: &Path, bytes: &[u8]) -> Result<Input, String> {
    if bytes.is_empty() {
        return Err("The image is empty.".into());
    }
    if bytes.len() > MAX_BYTES {
        return Err(format!("The image is {:.0} MB; the limit is {} MB.", bytes.len() as f64 / 1e6, MAX_BYTES / 1024 / 1024));
    }
    let format = image::guess_format(bytes).ok().and_then(ext_of).ok_or("That isn't a PNG, JPEG or WebP image.")?;
    let reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::from_extension(format).unwrap_or(image::ImageFormat::Png));
    let (width, height) = reader.into_dimensions().map_err(|e| format!("Couldn't read the image ({e})."))?;
    if width == 0 || height == 0 || width.max(height) > MAX_SIDE {
        return Err(format!("The image is {width}x{height}; each side must be between 1 and {MAX_SIDE} pixels."));
    }
    let sha256 = sha256_hex(bytes);
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {} ({e}).", dir.display()))?;
    let path = dir.join(format!("{sha256}.{format}"));
    if !path.is_file() {
        // Temp + rename: a half-written input never sits under its hash.
        let tmp = dir.join(format!(".{sha256}.tmp"));
        std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("Couldn't save the image ({e}). Check free disk space."))?;
    }
    Ok(Input { sha256, format: format.into(), width, height, bytes: bytes.len() as u64, path })
}

/// A stored input by its hash.
pub fn get(dir: &Path, sha256: &str) -> Result<Input, String> {
    if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
        return Err(format!("{sha256:?} isn't an input id (64 lowercase hex characters from POST /api/inputs)."));
    }
    for ext in ["png", "jpeg", "webp"] {
        let path = dir.join(format!("{sha256}.{ext}"));
        if let Ok(meta) = std::fs::metadata(&path) {
            let (width, height) = image::image_dimensions(&path).map_err(|e| format!("Couldn't read input {sha256} ({e})."))?;
            return Ok(Input { sha256: sha256.into(), format: ext.into(), width, height, bytes: meta.len(), path });
        }
    }
    Err(format!("There's no input image {sha256}. Send it to POST /api/inputs first."))
}

/// The input as sd-server takes it: base64 of a PNG or JPEG (WebP is re-encoded as PNG here; the
/// stored bytes and their hash stay the original).
pub fn b64_for_backend(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {} ({e}).", path.display()))?;
    let bytes = match image::guess_format(&bytes) {
        Ok(image::ImageFormat::Png | image::ImageFormat::Jpeg) => bytes,
        _ => {
            let img = image::load_from_memory(&bytes).map_err(|e| format!("Couldn't decode {} ({e}).", path.display()))?;
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| format!("Couldn't convert {} to PNG ({e}).", path.display()))?;
            out.into_inner()
        }
    };
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The output size for an edit when none is asked for: the input's aspect ratio at about the
/// model's own area (its default width x height), each side a multiple of 16 within 256..2048.
pub fn edit_size(in_w: u32, in_h: u32, area: u32) -> (u32, u32) {
    let aspect = in_w as f64 / in_h as f64;
    let h = (area as f64 / aspect).sqrt();
    let w = h * aspect;
    let snap = |v: f64| ((v / 16.0).round() as u32 * 16).clamp(256, 2048);
    (snap(w), snap(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_fn(w, h, |x, y| image::Rgb([x as u8, y as u8, 7])).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn inputs_are_stored_once_by_hash_and_read_back() {
        let dir = std::env::temp_dir().join(format!("inputs-{}", uuid::Uuid::new_v4()));
        let bytes = png(40, 30);
        let a = store(&dir, &bytes).unwrap();
        assert_eq!((a.width, a.height, a.format.as_str()), (40, 30, "png"));
        assert_eq!(a.sha256, sha256_hex(&bytes));
        let b = store(&dir, &bytes).unwrap();
        assert_eq!(a, b, "the same bytes are the same input");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert_eq!(get(&dir, &a.sha256).unwrap(), a);
        assert!(get(&dir, &"0".repeat(64)).unwrap_err().contains("POST /api/inputs"));
        assert!(get(&dir, "../../etc").is_err(), "only hashes, never paths");
        assert!(store(&dir, b"GIF89a....").is_err());
        assert!(store(&dir, b"").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_size_keeps_the_aspect_near_the_model_area() {
        assert_eq!(edit_size(1024, 1024, 1024 * 1024), (1024, 1024));
        let (w, h) = edit_size(4000, 3000, 1024 * 1024);
        assert!(w % 16 == 0 && h % 16 == 0);
        assert!(((w as f64 / h as f64) - 4.0 / 3.0).abs() < 0.03, "{w}x{h}");
        assert!(((w * h) as f64 / (1024.0 * 1024.0) - 1.0).abs() < 0.05, "{w}x{h}");
        assert_eq!(edit_size(100, 8000, 1024 * 1024).0, 256, "clamped");
    }
}
