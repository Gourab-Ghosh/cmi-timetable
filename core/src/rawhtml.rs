//! DEFLATE + base64 helpers for storing the raw page HTML inside a snapshot
//! (so a shipped parser fix can re-parse without refetching).

use base64::prelude::*;

pub fn compress_to_b64(text: &str) -> String {
    let compressed = miniz_oxide::deflate::compress_to_vec(text.as_bytes(), 8);
    BASE64_STANDARD.encode(compressed)
}

pub fn decompress_from_b64(b64: &str) -> Option<String> {
    let bytes = BASE64_STANDARD.decode(b64.trim()).ok()?;
    // 32 MB ceiling — both pages are ~30 KB; anything bigger is corrupt.
    let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(&bytes, 32 * 1024 * 1024).ok()?;
    String::from_utf8(raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let input = "<html><pre> BM1 |09:10-10:25|\n Mon | PROG |</pre></html>";
        let b64 = compress_to_b64(input);
        assert_eq!(decompress_from_b64(&b64).as_deref(), Some(input));
    }

    #[test]
    fn garbage_is_none() {
        assert_eq!(decompress_from_b64("not base64!!!"), None);
        assert_eq!(decompress_from_b64("aGVsbG8="), None); // valid b64, not deflate
    }
}
