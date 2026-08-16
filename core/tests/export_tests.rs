//! The `cmi-planner-backup` format: write → read round trip, and every
//! honest refusal. Run against the committed fixtures, like the parser
//! tests — the snapshot inside a backup is a real parsed snapshot.

use cmi_timetable_core::export::{ImportError, parse_planner_backup, planner_backup_json};
use cmi_timetable_core::extract::parse_html_pages;
use cmi_timetable_core::model::{Snapshot, SourceTier};
use serde_json::json;
use std::sync::LazyLock;

const TT: &str = include_str!("../fixtures/timetable.php.html");
const HALLS: &str = include_str!("../fixtures/lecturehalls.php.html");
const NOW: f64 = 1_755_000_000_000.0;

static SNAPSHOT: LazyLock<Snapshot> = LazyLock::new(|| {
    let out = parse_html_pages(TT, HALLS, NOW - 3_600_000.0, SourceTier::Direct, false);
    out.snapshot.expect("fixtures parse")
});

fn backup_text() -> String {
    planner_backup_json(
        &SNAPSHOT,
        json!(["TOC", "QCOM"]),
        json!({"next_id": 1, "items": [], "credits": []}),
        json!({"courses": []}),
        json!({"theme": "Dark"}),
        json!([]),
        "test",
        "deadbeef",
        NOW,
    )
}

/// Everything that goes in comes back out — the snapshot equal (minus the
/// raw pages, which are deliberately stripped), the store values verbatim.
#[test]
fn backup_round_trips() {
    let parsed = parse_planner_backup(&backup_text(), NOW).expect("round trip");
    assert_eq!(parsed.snapshot.courses, SNAPSHOT.courses);
    assert_eq!(parsed.snapshot.fetched_at, SNAPSHOT.fetched_at);
    assert_eq!(parsed.snapshot.semester_label, SNAPSHOT.semester_label);
    assert!(parsed.snapshot.raw_html_gz.is_none(), "raw pages stripped");
    assert_eq!(parsed.selection, json!(["TOC", "QCOM"]));
    assert_eq!(
        parsed.overrides,
        json!({"next_id": 1, "items": [], "credits": []})
    );
    assert_eq!(parsed.prefs, json!({"theme": "Dark"}));
    assert_eq!(parsed.pending_conflicts, json!([]));
}

/// Every refusal is the right refusal — the message a student sees names
/// what the file actually was.
#[test]
fn backup_refusals() {
    // Not JSON at all.
    assert_eq!(
        parse_planner_backup("not json {", NOW).unwrap_err(),
        ImportError::NotJson
    );

    // The other format the app writes, named and redirected.
    let timetable = json!({"format": "cmi-timetable-export", "courses": []}).to_string();
    let err = parse_planner_backup(&timetable, NOW).unwrap_err();
    assert_eq!(
        err,
        ImportError::WrongFormat("cmi-timetable-export".to_string())
    );
    assert!(
        err.message().contains("Import my courses"),
        "{}",
        err.message()
    );

    // Some other file entirely.
    let stranger = json!({"format": "someone-elses"}).to_string();
    let err = parse_planner_backup(&stranger, NOW).unwrap_err();
    assert!(
        err.message().contains("Export everything"),
        "{}",
        err.message()
    );

    // A future major version is refused, not half-read.
    let mut v: serde_json::Value = serde_json::from_str(&backup_text()).unwrap();
    v["format_version"] = json!("2.0.0");
    assert_eq!(
        parse_planner_backup(&v.to_string(), NOW).unwrap_err(),
        ImportError::NewerFormat
    );

    // A missing section is named, not mistaken for a foreign file.
    let mut v: serde_json::Value = serde_json::from_str(&backup_text()).unwrap();
    v.as_object_mut().unwrap().remove("selection");
    let err = parse_planner_backup(&v.to_string(), NOW).unwrap_err();
    assert_eq!(err, ImportError::MissingPart("course selection"));
    assert!(err.message().contains("damaged"), "{}", err.message());

    // Snapshot sanity: no courses.
    let mut v: serde_json::Value = serde_json::from_str(&backup_text()).unwrap();
    v["snapshot"]["courses"] = json!([]);
    assert!(matches!(
        parse_planner_backup(&v.to_string(), NOW).unwrap_err(),
        ImportError::BadSnapshot(_)
    ));

    // Snapshot sanity: fetched in the future.
    let mut v: serde_json::Value = serde_json::from_str(&backup_text()).unwrap();
    v["snapshot"]["fetched_at"] = json!(NOW + 172_800_000.0);
    assert!(matches!(
        parse_planner_backup(&v.to_string(), NOW).unwrap_err(),
        ImportError::BadSnapshot(_)
    ));

    // A file whose format field says it IS ours, but whose version stamp
    // was lost in a hand-edit: named honestly, not called a foreign file.
    let mut v: serde_json::Value = serde_json::from_str(&backup_text()).unwrap();
    v.as_object_mut().unwrap().remove("format_version");
    let err = parse_planner_backup(&v.to_string(), NOW).unwrap_err();
    assert_eq!(err, ImportError::BadEnvelope);
    assert!(
        err.message().contains("which version of the app made it"),
        "{}",
        err.message()
    );

    // Same when the stamp is mistyped to a non-string.
    let mut v: serde_json::Value = serde_json::from_str(&backup_text()).unwrap();
    v["format_version"] = json!(1);
    assert_eq!(
        parse_planner_backup(&v.to_string(), NOW).unwrap_err(),
        ImportError::BadEnvelope
    );

    // And when it is a string that says nothing this build can compare. A
    // git-style "v1.1.0" is not a newer app, so "reload the page to get the
    // newest version" would send somebody to fix a version that is fine.
    for stamp in ["v1.1.0", "one.1.0", ""] {
        let mut v: serde_json::Value = serde_json::from_str(&backup_text()).unwrap();
        v["format_version"] = json!(stamp);
        let err = parse_planner_backup(&v.to_string(), NOW).unwrap_err();
        assert_eq!(err, ImportError::BadEnvelope, "stamp {stamp:?}");
        assert!(
            !err.message().contains("newer version"),
            "stamp {stamp:?} must not be blamed on an out-of-date app: {}",
            err.message()
        );
    }
}

/// A minor-version bump from a future build still loads (unknown keys are
/// ignored); the major gate alone decides.
#[test]
fn backup_minor_versions_load() {
    let mut v: serde_json::Value = serde_json::from_str(&backup_text()).unwrap();
    v["format_version"] = json!("1.9.3");
    v["some_future_key"] = json!({"the app": "ignores this"});
    assert!(parse_planner_backup(&v.to_string(), NOW).is_ok());
}
