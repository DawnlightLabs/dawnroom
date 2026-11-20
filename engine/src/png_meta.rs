//! Embeds the recipe in a PNG as iTXt chunks (UTF-8, so any prompt survives), right after IHDR.

const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Returns a copy of `png` with one uncompressed iTXt chunk per (keyword, text) pair.
pub fn with_text_chunks(png: &[u8], entries: &[(&str, String)]) -> Result<Vec<u8>, String> {
    // Signature (8) + IHDR chunk (4 len + 4 type + 13 data + 4 crc).
    const IHDR_END: usize = 8 + 25;
    if png.len() < IHDR_END || &png[..8] != SIGNATURE || &png[12..16] != b"IHDR" {
        return Err("not a PNG".into());
    }
    let mut out = Vec::with_capacity(png.len() + 512);
    out.extend_from_slice(&png[..IHDR_END]);
    for (key, text) in entries {
        // keyword \0 compression-flag(0) compression-method(0) language-tag \0 translated-keyword \0 text
        let mut data = Vec::with_capacity(key.len() + text.len() + 5);
        data.extend_from_slice(key.as_bytes());
        data.extend_from_slice(&[0, 0, 0, 0, 0]);
        data.extend_from_slice(text.as_bytes());
        push_chunk(&mut out, b"iTXt", &data);
    }
    out.extend_from_slice(&png[IHDR_END..]);
    Ok(out)
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

/// A minimal PNG (signature, 1x1 IHDR, IEND) for tests.
#[cfg(test)]
pub fn tiny_png() -> Vec<u8> {
    let mut v = SIGNATURE.to_vec();
    push_chunk(&mut v, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
    push_chunk(&mut v, b"IEND", &[]);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_itxt_after_ihdr() {
        let out = with_text_chunks(&tiny_png(), &[("prompt", "café ☀".to_string())]).unwrap();
        assert_eq!(&out[37..41], b"iTXt");
        assert!(out.windows("café ☀".len()).any(|w| w == "café ☀".as_bytes()));
        assert!(out.ends_with(&tiny_png()[33..]));
    }
}
