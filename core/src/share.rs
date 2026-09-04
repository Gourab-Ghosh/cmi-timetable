//! URL state: the `?c=` course-code list and the `&s=` compressed share
//! payload (selection + overrides). When both are present, `s` wins.

use crate::model::{
    Course, CreditOverride, CustomStore, HiddenCourse, MeetingOverride, OverridesStore,
};
use serde::{Deserialize, Serialize};

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

/// Can this course code travel in a `?c=` list?
///
/// THE definition. `?c=` joins codes with plain commas and percent-encodes
/// each one (`domx::c_param`), while `parse_c_param` below accepts
/// percent-encoding ANYWHERE — deliberately, because mail clients and chat
/// apps re-encode links on the way from one person to the next. Each rule is
/// right on its own and together they unescape once too often, so a code
/// carrying a comma or a percent sign has no round trip: `CM,X` is written
/// `CM%2CX` and read back as TWO codes, `CM` and `X`. One reload then rewrote
/// the student's stored selection to the mis-decoding — and where the second
/// half named a real CMI course, it put a course they never picked on their
/// timetable (R93 M5).
///
/// Every door that writes a course code asks this — the editor, both imports,
/// a share payload, a dropped course kept as your own — and every code the app
/// puts in a URL has passed it. Three hand-rolled copies of this test used to
/// live in `app/src/ui.rs`; they now delegate here.
pub fn code_is_url_safe(code: &str) -> bool {
    !code.contains(',') && !code.contains('%')
}

/// Why such a code is refused, in the words every door uses. One sentence, so
/// the editor, the import dialog and the link dialog cannot drift apart.
pub const CODE_NOT_URL_SAFE: &str = "A code can't contain a comma or a % sign — \
                                     they'd break the links that share your timetable.";

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
    /// Courses the sender deleted. A share link carries the sender's whole
    /// planner, and a course they struck out is as much a part of it as a
    /// meeting they moved — the recipient can restore any of it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub d: Vec<HiddenCourse>,
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
        d: overrides.hidden.clone(),
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
    /// Parts of a readable `s=` were set aside because the app cannot state
    /// them: a class whose start is at or after its end or which runs past
    /// midnight, a credit figure outside the range the editor allows, or an
    /// override id with no number after it. Counted, not clamped — moving
    /// somebody's class to an hour nobody chose and printing it as their own
    /// decision is worse than saying it was set aside (R93 S1, S5, S8).
    pub set_aside: usize,
    /// An `s=` was present but could not be decoded. The caller must not
    /// treat the fallback as the link's true content: with no `c=` beside it
    /// the fallback is an EMPTY selection, and applying that wiped the
    /// reader's timetable while looking like a no-op (final sweep,
    /// share-import-1).
    pub damaged: bool,
}

/// Resolve the two query parameters into one state. A malformed `s=` falls
/// back to `c=` rather than breaking anything — and says so via `damaged`.
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
        // Everything past this point is a stranger's bytes, so it goes
        // through the SAME rules the editor enforces on the reader's own
        // input — the clamp law: a rule applied at one door only is a bug at
        // the others. `retain_sane` sets aside a class the app cannot draw
        // (`start_min: 65535` drew "1092:15" columns on three grids and wrote
        // nine digits into a `DTSTART`), a credit figure outside the editor's
        // 0..=20 ("459 credits in total", from a number the reader never
        // typed), and an `id: u64::MAX` whose successor `add` cannot compute
        // (R93 S1, S5, S8). It also calls `bump_next_id`, which replaces the
        // `saturating_add(1)` line that used to live here — that line's own
        // comment said it existed so a hand-crafted link could not hand the
        // store a colliding `next_id`, and `u64::MAX.saturating_add(1) ==
        // u64::MAX`, so it handed out exactly that collision.
        //
        // Dropped, never clamped, and the count travels so the caller can
        // say so.
        let mut overrides = OverridesStore {
            next_id: 0,
            items: payload.o,
            credits: payload.k,
            hidden: payload.d,
        };
        let mut set_aside = overrides.retain_sane();
        // A course carrying a class the app cannot draw goes whole: a course
        // silently missing one of its meetings is lying by omission. Its code
        // then resolves to nothing and becomes the same dismissible "unknown
        // code" chip a link naming a retired course already produces.
        let mut customs = CustomStore { courses: payload.x };
        set_aside += customs.retain_sane();
        return UrlState {
            selection,
            overrides: Some(overrides),
            customs: customs.courses,
            set_aside,
            damaged: false,
        };
    }
    UrlState {
        selection: c.map(parse_c_param).unwrap_or_default(),
        overrides: None,
        customs: Vec::new(),
        set_aside: 0,
        damaged: s.is_some(),
    }
}
