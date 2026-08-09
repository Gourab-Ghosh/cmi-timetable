//! URL state: the `?c=` course-code list and the `&s=` compressed share
//! payload (selection + overrides). When both are present, `s` wins.

use crate::model::{Course, CreditOverride, MeetingOverride, OverridesStore};
use serde::{Deserialize, Serialize};

/// Canonical `?c=` value: uppercase codes, comma-separated, order preserved.
pub fn selection_to_c_param(selection: &[String]) -> String {
    selection.join(",")
}

/// Parse a `?c=` value: trim, drop empties, dedupe case-insensitively while
/// keeping order. Codes are kept VERBATIM — course codes come from CMI's
/// pages and their casing is whatever CMI uses; the app canonicalizes
/// against the live catalog case-insensitively at lookup time.
pub fn parse_c_param(raw: &str) -> Vec<String> {
    // Percent-encoding is accepted anywhere, not just in the shapes this app
    // writes: links are retyped, quoted, wrapped and re-encoded by mail
    // clients and chat apps on the way from one person to the next, and the
    // only thing that matters is that the codes come back.
    //
    // Separators first (a `%2C` that survived the browser's own decoding —
    // i.e. arrived double-encoded — still separates), then each code, so a
    // code carrying '+', '&' or '#' is restored as it was written.
    let mut out: Vec<String> = Vec::new();
    for token in normalize_separators(raw).split(',') {
        let code = percent_decode(token.trim());
        let code = code.trim();
        if !code.is_empty() && !out.iter().any(|c| c.eq_ignore_ascii_case(code)) {
            out.push(code.to_string());
        }
    }
    out
}

/// Turn any still-encoded comma into a real one, so it separates.
fn normalize_separators(raw: &str) -> String {
    raw.replace("%2C", ",").replace("%2c", ",")
}

/// Decode `%XX` escapes. Bytes first, then UTF-8, so a multi-byte character
/// split across escapes ("%E2%82%B9") comes back whole. Anything that isn't
/// a valid escape is left exactly as it was — a stray '%' is not an error.
fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push(hi * 16 + lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharePayload {
    pub v: u8,
    pub c: Vec<String>,
    #[serde(default)]
    pub o: Vec<MeetingOverride>,
    /// Credit overrides — absent in payloads made before they existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub k: Vec<CreditOverride>,
    /// Custom (user-created) courses riding along, so a shared timetable
    /// renders complete on the recipient's browser — absent in payloads
    /// made before customs existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub x: Vec<Course>,
}

/// Compress selection + overrides + the selection's custom courses into a
/// URI-component-safe string.
pub fn encode_share(
    selection: &[String],
    overrides: &OverridesStore,
    customs: &[Course],
) -> String {
    let payload = SharePayload {
        v: 1,
        c: selection.to_vec(),
        o: overrides.items.clone(),
        k: overrides.credits.clone(),
        x: customs.to_vec(),
    };
    let json = serde_json::to_string(&payload).expect("share payload serializes");
    lz_str::compress_to_encoded_uri_component(json.as_str())
}

pub fn decode_share(encoded: &str) -> Option<SharePayload> {
    let wide = lz_str::decompress_from_encoded_uri_component(encoded)?;
    let json = String::from_utf16(&wide).ok()?;
    let payload: SharePayload = serde_json::from_str(&json).ok()?;
    if payload.v != 1 {
        return None;
    }
    Some(payload)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UrlState {
    pub selection: Vec<String>,
    /// `Some` only when a valid `s=` payload was present (it wins over `c=`).
    pub overrides: Option<OverridesStore>,
    /// Custom courses carried by an `s=` payload (empty for `c=`-only URLs).
    pub customs: Vec<Course>,
}

/// Resolve the two query parameters into one state. A malformed `s=` falls
/// back to `c=` rather than breaking anything.
pub fn resolve_url_state(c: Option<&str>, s: Option<&str>) -> UrlState {
    if let Some(encoded) = s
        && let Some(payload) = decode_share(encoded)
    {
        let mut selection: Vec<String> = Vec::new();
        for code in payload.c {
            let code = code.trim().to_string();
            if !code.is_empty() && !selection.iter().any(|c| c.eq_ignore_ascii_case(&code)) {
                selection.push(code);
            }
        }
        let next_id = payload.o.iter().map(|o| o.id + 1).max().unwrap_or(0);
        return UrlState {
            selection,
            overrides: Some(OverridesStore {
                next_id,
                items: payload.o,
                credits: payload.k,
            }),
            customs: payload.x,
        };
    }
    UrlState {
        selection: c.map(parse_c_param).unwrap_or_default(),
        overrides: None,
        customs: Vec::new(),
    }
}
