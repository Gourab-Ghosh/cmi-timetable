//! URL-state tests: `?c=` round-trip (including unknown codes) and the rule
//! that a valid `&s=` payload beats `c=`.

use cmi_timetable_core::model::{
    CreditOverride, Day, Meeting, MeetingOverride, OverridesStore, Slot,
};
use cmi_timetable_core::share::{
    decode_share, encode_share, parse_c_param, resolve_url_state, selection_to_c_param,
};

fn codes(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn c_param_round_trip() {
    let selection = codes(&["TOC", "QCOM", "MFD"]);
    let c = selection_to_c_param(&selection);
    assert_eq!(c, "TOC,QCOM,MFD");
    assert_eq!(parse_c_param(&c), selection);
}

#[test]
fn c_param_is_forgiving() {
    // Stray spaces, duplicates (case-insensitive), empties. Codes are kept
    // VERBATIM — CMI's casing is unknown in advance, so the app resolves
    // them against the live catalog case-insensitively instead of forcing
    // uppercase here.
    assert_eq!(
        parse_c_param(" toc, QCOM ,,mfd,TOC ,XYZ"),
        codes(&["toc", "QCOM", "mfd", "XYZ"])
    );
    assert!(parse_c_param("").is_empty());
    assert!(parse_c_param(",,,").is_empty());
}

#[test]
fn share_payload_round_trip() {
    let selection = codes(&["SVA", "MFD"]);
    let overrides = OverridesStore {
        next_id: 4,
        items: vec![MeetingOverride {
            id: 3,
            course: "MFD".to_string(),
            base: Some(Meeting {
                day: Day::Wed,
                slot: Slot::new(840, 915),
                hall: Some("Lecture Hall 6".to_string()),
                temp_booking: false,
            }),
            to: Meeting {
                day: Day::Thu,
                slot: Slot::new(840, 915),
                hall: Some("Lecture Hall 6".to_string()),
                temp_booking: false,
            },
            created_at: 1_754_000_000_000.0,
        }],
        credits: vec![CreditOverride {
            course: "SVA".to_string(),
            credits: 2,
            created_at: 1_754_000_000_000.0,
        }],
    };
    let encoded = encode_share(&selection, &overrides);
    // Must be URI-component-safe as produced.
    assert!(
        encoded
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-$".contains(c)),
        "unexpected characters in {encoded:?}"
    );
    let payload = decode_share(&encoded).expect("decodes");
    assert_eq!(payload.c, selection);
    assert_eq!(payload.o, overrides.items);
    assert_eq!(payload.k, overrides.credits);

    // The resolved state rebuilds a usable store: next_id past every item,
    // credit overrides carried along.
    let state = resolve_url_state(None, Some(&encoded));
    let store = state.overrides.expect("s payload present");
    assert_eq!(store.next_id, 4);
    assert_eq!(store.items, overrides.items);
    assert_eq!(store.credits_for("SVA"), Some(2));
}

#[test]
fn share_payload_without_credit_overrides_still_decodes() {
    // Payloads from before credit overrides existed have no `k` field.
    let json = r#"{"v":1,"c":["TOC"],"o":[]}"#;
    let encoded = lz_str::compress_to_encoded_uri_component(json);
    let payload = decode_share(&encoded).expect("old payload decodes");
    assert_eq!(payload.c, codes(&["TOC"]));
    assert!(payload.k.is_empty());
}

#[test]
fn s_beats_c() {
    let s = encode_share(&codes(&["AML", "MAAT"]), &OverridesStore::default());
    let state = resolve_url_state(Some("TOC,QCOM"), Some(&s));
    assert_eq!(state.selection, codes(&["AML", "MAAT"]));
    assert!(state.overrides.is_some());
}

#[test]
fn malformed_s_falls_back_to_c() {
    let state = resolve_url_state(Some("TOC,QCOM"), Some("!!!not-a-payload!!!"));
    assert_eq!(state.selection, codes(&["TOC", "QCOM"]));
    assert!(state.overrides.is_none());

    let state = resolve_url_state(None, Some("!!!"));
    assert!(state.selection.is_empty());
}

#[test]
fn no_params_is_empty_state() {
    let state = resolve_url_state(None, None);
    assert!(state.selection.is_empty());
    assert!(state.overrides.is_none());
}
