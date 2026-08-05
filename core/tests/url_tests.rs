//! URL-state tests: `?c=` round-trip (including unknown codes) and the rule
//! that a valid `&s=` payload beats `c=`.

use cmi_timetable_core::model::{Day, Meeting, MeetingOverride, Slot};
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
    // Lowercase, stray spaces, duplicates, empties — and unknown codes are
    // preserved here (the app warns about them against the live catalog).
    assert_eq!(
        parse_c_param(" toc, QCOM ,,mfd,TOC ,XYZ"),
        codes(&["TOC", "QCOM", "MFD", "XYZ"])
    );
    assert!(parse_c_param("").is_empty());
    assert!(parse_c_param(",,,").is_empty());
}

#[test]
fn share_payload_round_trip() {
    let selection = codes(&["SVA", "MFD"]);
    let overrides = vec![MeetingOverride {
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
    }];
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
    assert_eq!(payload.o, overrides);
}

#[test]
fn s_beats_c() {
    let overrides = vec![];
    let s = encode_share(&codes(&["AML", "MAAT"]), &overrides);
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
