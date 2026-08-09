//! URL-state tests: `?c=` round-trip (including unknown codes) and the rule
//! that a valid `&s=` payload beats `c=`.

use cmi_timetable_core::model::{
    CreditOverride, Day, HiddenCourse, Meeting, MeetingOverride, OverridesStore, Slot,
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
    // Percent-encoding is accepted wherever it turns up — links get
    // rewritten by all sorts of software between one person and the next.
    // Separators, in either case:
    assert_eq!(parse_c_param("TOC%2CQCOM"), codes(&["TOC", "QCOM"]));
    assert_eq!(
        parse_c_param("TOC%2cQCOM,MFD"),
        codes(&["TOC", "QCOM", "MFD"])
    );
    // …and codes, including the characters a query string reads as syntax:
    assert_eq!(
        parse_c_param("A%2BB,C%26D,E%23F"),
        codes(&["A+B", "C&D", "E#F"])
    );
    assert_eq!(parse_c_param("MY%20COURSE"), codes(&["MY COURSE"]));
    // Multi-byte characters survive as characters, not as broken bytes.
    assert_eq!(
        parse_c_param("%E0%AE%A4%E0%AE%AE%E0%AE%BF"),
        codes(&["தமி"])
    );
    // A stray '%' is text, not an error, and encoded values still dedupe
    // against their plain twins.
    assert_eq!(parse_c_param("100%,TOC,%2CTOC"), codes(&["100%", "TOC"]));
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
            to: Some(Meeting {
                day: Day::Thu,
                slot: Slot::new(840, 915),
                hall: Some("Lecture Hall 6".to_string()),
                temp_booking: false,
            }),
            created_at: 1_754_000_000_000.0,
        }],
        credits: vec![CreditOverride {
            course: "SVA".to_string(),
            credits: 2,
            created_at: 1_754_000_000_000.0,
        }],
        hidden: vec![HiddenCourse {
            course: "QCOM".to_string(),
            created_at: 1_754_000_000_000.0,
        }],
    };
    let encoded = encode_share(&selection, &overrides, &[]);
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
    assert_eq!(payload.d, overrides.hidden);

    // The resolved state rebuilds a usable store: next_id past every item,
    // credit overrides and deleted courses carried along.
    let state = resolve_url_state(None, Some(&encoded));
    let store = state.overrides.expect("s payload present");
    assert_eq!(store.next_id, 4);
    assert_eq!(store.items, overrides.items);
    assert_eq!(store.credits_for("SVA"), Some(2));
    assert!(
        store.is_hidden("qcom"),
        "a deleted course travels, any casing"
    );
}

#[test]
fn share_payload_without_deletions_still_decodes() {
    // Payloads from before deleting a CMI course existed have no `d` field,
    // and a store with nothing hidden must not grow one on the way out.
    let json = r#"{"v":1,"c":["TOC"],"o":[],"k":[]}"#;
    let encoded = lz_str::compress_to_encoded_uri_component(json);
    let payload = decode_share(&encoded).expect("old payload decodes");
    assert!(payload.d.is_empty());
    let store = resolve_url_state(None, Some(&encoded))
        .overrides
        .expect("s payload present");
    assert!(store.hidden.is_empty());
    assert!(store.is_empty(), "no items, no credits, nothing hidden");
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
    let s = encode_share(&codes(&["AML", "MAAT"]), &OverridesStore::default(), &[]);
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

/// Removals (`to: null`) survive the share round trip, and pre-removal data
/// — every stored override and share link made before removals existed —
/// still deserializes: a present meeting simply becomes `Some`.
#[test]
fn removals_round_trip_and_old_payloads_still_load() {
    use cmi_timetable_core::model::{Meeting, MeetingOverride, OverridesStore};

    let base = Meeting {
        day: Day::Wed,
        slot: Slot::new(840, 915),
        hall: Some("Lecture Hall 6".to_string()),
        temp_booking: false,
    };
    let mut overrides = OverridesStore::default();
    overrides.add("MFD", Some(base), None, 0.0);
    let encoded = encode_share(&["MFD".to_string()], &overrides, &[]);
    let payload = decode_share(&encoded).expect("removal payload decodes");
    assert_eq!(payload.o.len(), 1);
    assert!(payload.o[0].is_removal());

    // The exact JSON shape written before `to` became optional.
    let legacy = r#"{
        "id": 3, "course": "TOC",
        "base": null,
        "to": {"day":"Tue","slot":{"start_min":550,"end_min":625},
               "hall":"Lecture Hall 803","temp_booking":false},
        "created_at": 0.0
    }"#;
    let o: MeetingOverride = serde_json::from_str(legacy).expect("legacy JSON loads");
    assert!(!o.is_removal());
    assert_eq!(
        o.to.as_ref().and_then(|m| m.hall.clone()).as_deref(),
        Some("Lecture Hall 803")
    );
}

/// The user's own courses ride the share payload and come back whole; links
/// from before customs existed (no `x` field) still decode.
#[test]
fn custom_courses_ride_the_share_payload() {
    use cmi_timetable_core::model::{Course, CustomStore, Meeting, ScheduleStatus};

    let yoga = Course::custom(
        "YOGA".to_string(),
        "Evening yoga".to_string(),
        vec!["S. Iyer".to_string()],
        0,
        vec![
            Meeting {
                day: Day::Sat,
                slot: Slot::new(18 * 60, 19 * 60),
                hall: Some("Sports annexe".to_string()),
                temp_booking: false,
            },
            Meeting {
                day: Day::Tue,
                slot: Slot::new(550, 625),
                hall: None,
                temp_booking: false,
            },
        ],
    );
    // The constructor sorts meetings (day, then start) and derives status.
    assert_eq!(yoga.meetings[0].day, Day::Tue);
    assert_eq!(yoga.status, ScheduleStatus::Scheduled);
    assert_eq!(yoga.credits, Some(0));

    let timeless = Course::custom("RG".into(), "Reading group".into(), vec![], 2, vec![]);
    assert_eq!(timeless.status, ScheduleStatus::UnscheduledListed);

    let selection = codes(&["TOC", "YOGA"]);
    let encoded = encode_share(
        &selection,
        &OverridesStore::default(),
        std::slice::from_ref(&yoga),
    );
    let state = resolve_url_state(None, Some(&encoded));
    assert_eq!(state.selection, selection);
    assert_eq!(state.customs, vec![yoga.clone()]);

    // Pre-customs payloads carry no `x` field and must keep decoding.
    let json = r#"{"v":1,"c":["TOC"],"o":[]}"#;
    let old = decode_share(&lz_str::compress_to_encoded_uri_component(json))
        .expect("pre-customs payload decodes");
    assert!(old.x.is_empty());
    // A `c=`-only URL never carries definitions.
    assert!(resolve_url_state(Some("TOC,YOGA"), None).customs.is_empty());

    // Store semantics: case-insensitive lookup, upsert replaces, remove
    // reports whether anything went.
    let mut store = CustomStore::default();
    store.upsert(yoga);
    assert!(store.get("yoga").is_some());
    let renamed = Course::custom("YOGA".into(), "Morning yoga".into(), vec![], 1, vec![]);
    store.upsert(renamed);
    assert_eq!(store.courses.len(), 1);
    assert_eq!(store.get("YOGA").unwrap().name, "Morning yoga");
    assert!(store.remove("Yoga"));
    assert!(!store.remove("YOGA"));
    assert!(store.is_empty());
}
