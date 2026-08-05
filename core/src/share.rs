//! URL state: the `?c=` course-code list and the `&s=` compressed share
//! payload (selection + overrides). When both are present, `s` wins.

use crate::model::MeetingOverride;
use serde::{Deserialize, Serialize};

/// Canonical `?c=` value: uppercase codes, comma-separated, order preserved.
pub fn selection_to_c_param(selection: &[String]) -> String {
    selection.join(",")
}

/// Parse a `?c=` value: uppercase, trim, drop empties, dedupe keeping order.
pub fn parse_c_param(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in raw.split(',') {
        let code = token.trim().to_ascii_uppercase();
        if !code.is_empty() && !out.contains(&code) {
            out.push(code);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharePayload {
    pub v: u8,
    pub c: Vec<String>,
    #[serde(default)]
    pub o: Vec<MeetingOverride>,
}

/// Compress selection + overrides into a URI-component-safe string.
pub fn encode_share(selection: &[String], overrides: &[MeetingOverride]) -> String {
    let payload = SharePayload {
        v: 1,
        c: selection.to_vec(),
        o: overrides.to_vec(),
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
    pub overrides: Option<Vec<MeetingOverride>>,
}

/// Resolve the two query parameters into one state. A malformed `s=` falls
/// back to `c=` rather than breaking anything.
pub fn resolve_url_state(c: Option<&str>, s: Option<&str>) -> UrlState {
    if let Some(encoded) = s {
        if let Some(payload) = decode_share(encoded) {
            let mut selection = Vec::new();
            for code in payload.c {
                let code = code.trim().to_ascii_uppercase();
                if !code.is_empty() && !selection.contains(&code) {
                    selection.push(code);
                }
            }
            return UrlState {
                selection,
                overrides: Some(payload.o),
            };
        }
    }
    UrlState {
        selection: c.map(parse_c_param).unwrap_or_default(),
        overrides: None,
    }
}
