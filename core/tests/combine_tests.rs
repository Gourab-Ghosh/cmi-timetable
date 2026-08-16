//! Two students combining timetables: the merge rules, and the file half
//! that carries a week from one browser to another.

use cmi_timetable_core::combine::{clear_for_courses, merge_overrides, purge_custom_overrides};
use cmi_timetable_core::export::{MyChanges, parse_timetable_export};
use cmi_timetable_core::model::{
    Course, CreditOverride, CustomStore, Day, Meeting, MeetingOverride, OverridesStore,
    ScheduleStatus, Slot,
};
use serde_json::json;

const NOW: f64 = 1_755_000_000_000.0;

fn meeting(day: Day, start: u16, hall: &str) -> Meeting {
    Meeting {
        day,
        slot: Slot::new(start, start + 75),
        hall: Some(hall.to_string()),
        temp_booking: false,
    }
}

/// One edit, with the id the receiving store must ignore.
fn ovr(id: u64, course: &str, base: Option<Meeting>, to: Option<Meeting>) -> MeetingOverride {
    MeetingOverride {
        id,
        course: course.to_string(),
        base,
        to,
        created_at: NOW,
    }
}

fn store(items: Vec<MeetingOverride>, credits: Vec<CreditOverride>) -> OverridesStore {
    OverridesStore {
        next_id: items.iter().map(|o| o.id + 1).max().unwrap_or(0),
        items,
        credits,
        hidden: Vec::new(),
    }
}

fn credit(course: &str, credits: u8) -> CreditOverride {
    CreditOverride {
        course: course.to_string(),
        credits,
        created_at: NOW,
    }
}

fn custom(code: &str) -> Course {
    Course {
        code: code.to_string(),
        name: format!("{code} reading group"),
        instructors: vec!["Me".to_string()],
        branches: vec![],
        credits: Some(2),
        starts: None,
        part_of_semester: None,
        optional_flag: false,
        status: ScheduleStatus::Scheduled,
        meetings: vec![meeting(Day::Thu, 900, "Seminar Room")],
    }
}

/// The headline case: two people who each moved a different class end up
/// with both moves, renumbered into one sequence, and neither loses work.
#[test]
fn different_changes_both_survive() {
    let mut mine = store(
        vec![ovr(
            0,
            "TOC",
            Some(meeting(Day::Tue, 550, "803")),
            Some(meeting(Day::Wed, 1020, "803")),
        )],
        vec![],
    );
    let theirs = store(
        vec![ovr(
            0, // their store numbers from zero too — the collision is the point
            "RDBM",
            Some(meeting(Day::Mon, 550, "601")),
            Some(meeting(Day::Fri, 700, "601")),
        )],
        vec![credit("RDBM", 2)],
    );

    let stats = merge_overrides(&mut mine, &theirs);

    assert_eq!(stats.meetings_added, 1);
    assert_eq!(stats.credits_added, 1);
    assert!(stats.kept_yours.is_empty());
    assert_eq!(mine.items.len(), 2);
    let ids: Vec<u64> = mine.items.iter().map(|o| o.id).collect();
    assert_eq!(
        ids,
        vec![0, 1],
        "the incoming id is reassigned, not trusted"
    );
    assert_eq!(mine.next_id, 2);
    assert_eq!(mine.credits_for("RDBM"), Some(2));
}

/// The same move made on both sides is one move — importing a file you
/// yourself exported changes nothing.
#[test]
fn identical_changes_collapse() {
    let one = ovr(
        7,
        "TOC",
        Some(meeting(Day::Tue, 550, "803")),
        Some(meeting(Day::Wed, 1020, "803")),
    );
    let mut mine = store(vec![one.clone()], vec![credit("TOC", 3)]);
    // Same edit, different id and a code typed in another case.
    let theirs = store(
        vec![ovr(0, "toc", one.base.clone(), one.to)],
        vec![credit("toc", 3)],
    );

    let stats = merge_overrides(&mut mine, &theirs);

    assert!(stats.is_empty(), "{stats:?}");
    assert_eq!(mine.items.len(), 1);
    assert_eq!(mine.credits.len(), 1);
}

/// Both moved the SAME class, to different places. A class cannot be in two
/// places at once, so the reader's own week wins — and the file's loss is
/// counted by name, never dropped in silence.
#[test]
fn a_contested_class_keeps_the_readers_own() {
    let base = Some(meeting(Day::Tue, 550, "803"));
    let mut mine = store(
        vec![ovr(
            0,
            "TOC",
            base.clone(),
            Some(meeting(Day::Wed, 1020, "803")),
        )],
        vec![credit("TOC", 3)],
    );
    let theirs = store(
        vec![
            ovr(0, "TOC", base.clone(), Some(meeting(Day::Fri, 700, "803"))),
            // A strike-out of that same class is the same disagreement.
            ovr(1, "TOC", base, None),
        ],
        vec![credit("TOC", 4)],
    );

    let stats = merge_overrides(&mut mine, &theirs);

    assert_eq!(stats.meetings_added, 0);
    assert_eq!(stats.credits_added, 0);
    assert_eq!(stats.kept_yours, vec!["TOC".to_string()], "named once");
    assert_eq!(mine.items.len(), 1, "no second TOC class appears");
    assert_eq!(mine.items[0].to, Some(meeting(Day::Wed, 1020, "803")));
    assert_eq!(mine.credits_for("TOC"), Some(3));
}

/// Classes one side created out of nothing are additions, not competitors:
/// two different ones are two classes, two identical ones are one.
#[test]
fn created_classes_are_additive() {
    let mut mine = store(
        vec![ovr(0, "SEM", None, Some(meeting(Day::Mon, 600, "Room A")))],
        vec![],
    );
    let theirs = store(
        vec![
            ovr(0, "SEM", None, Some(meeting(Day::Mon, 600, "Room A"))),
            ovr(1, "SEM", None, Some(meeting(Day::Thu, 900, "Room B"))),
        ],
        vec![],
    );

    let stats = merge_overrides(&mut mine, &theirs);

    assert_eq!(stats.meetings_added, 1);
    assert!(stats.kept_yours.is_empty());
    assert_eq!(mine.items.len(), 2);
}

/// A strike-out of a class nobody else touched is a real change and travels
/// like any other.
#[test]
fn a_struck_out_class_travels() {
    let mut mine = store(vec![], vec![]);
    let theirs = store(
        vec![ovr(3, "RDBM", Some(meeting(Day::Mon, 550, "601")), None)],
        vec![],
    );

    let stats = merge_overrides(&mut mine, &theirs);

    assert_eq!(stats.meetings_added, 1);
    assert!(mine.items[0].is_removal());
}

/// Whole-course deletions are not this module's business — merging never
/// hides a course, and never un-hides one either.
#[test]
fn deletions_are_untouched_by_a_merge() {
    let mut mine = OverridesStore::default();
    mine.hide("QCOM", true, NOW);
    let mut theirs = OverridesStore::default();
    theirs.hide("TOC", false, NOW);

    merge_overrides(&mut mine, &theirs);

    assert!(mine.is_hidden("QCOM"));
    assert!(
        !mine.is_hidden("TOC"),
        "the file's deletions stay the file's"
    );
}

/// "Replace" clears the way for the file's changes — but only across the
/// courses the file is about. Work on the rest of the week survives, and so
/// do deletions.
#[test]
fn clearing_is_scoped_to_the_named_courses() {
    let mut mine = store(
        vec![
            ovr(0, "TOC", Some(meeting(Day::Tue, 550, "803")), None),
            ovr(1, "QCOM", Some(meeting(Day::Mon, 550, "601")), None),
        ],
        vec![credit("TOC", 3), credit("QCOM", 2)],
    );
    mine.hide("MFD", false, NOW);

    clear_for_courses(&mut mine, &["toc".to_string()]);

    assert_eq!(mine.items.len(), 1);
    assert_eq!(mine.items[0].course, "QCOM");
    assert_eq!(mine.credits_for("TOC"), None);
    assert_eq!(mine.credits_for("QCOM"), Some(2));
    assert!(mine.is_hidden("MFD"));
}

/// A change aimed at a code that names one of the reader's OWN courses would
/// render as a class belonging to nothing.
#[test]
fn changes_aimed_at_your_own_courses_are_dropped() {
    let customs = CustomStore {
        courses: vec![custom("READ")],
    };
    let mut ovs = store(
        vec![
            ovr(0, "read", Some(meeting(Day::Mon, 550, "601")), None),
            ovr(1, "TOC", Some(meeting(Day::Tue, 550, "803")), None),
        ],
        vec![credit("READ", 2), credit("TOC", 3)],
    );

    purge_custom_overrides(&customs, &mut ovs);

    assert_eq!(ovs.items.len(), 1);
    assert_eq!(ovs.items[0].course, "TOC");
    assert_eq!(ovs.credits.len(), 1);
    assert_eq!(ovs.credits[0].course, "TOC");
}

// ---------------------------------------------------------------------------
// The file half
// ---------------------------------------------------------------------------

fn export_file(my_changes: Option<serde_json::Value>) -> String {
    let mut file = json!({
        "format": "cmi-timetable-export",
        "format_version": "1.1.0",
        "courses": [{"code": "TOC"}, {"code": "RDBM"}, {"code": " toc "}],
    });
    if let Some(changes) = my_changes {
        file["my_changes"] = changes;
    }
    file.to_string()
}

/// Codes come back trimmed, deduped case-insensitively, in file order — and
/// the changes come back as a store whose ids start from zero.
#[test]
fn a_file_round_trips_into_a_plan() {
    let changes = MyChanges::build(
        &[
            ovr(
                41,
                "TOC",
                Some(meeting(Day::Tue, 550, "803")),
                Some(meeting(Day::Wed, 1020, "803")),
            ),
            ovr(99, "SEM", None, Some(meeting(Day::Thu, 900, "Room B"))),
        ],
        &[credit("TOC", 3)],
        &[custom("SEM")],
    );
    let text = export_file(Some(serde_json::to_value(&changes).unwrap()));

    let plan = parse_timetable_export(&text).expect("parses");

    assert_eq!(plan.codes, vec!["TOC".to_string(), "RDBM".to_string()]);
    assert!(plan.has_changes());
    assert_eq!(
        plan.overrides
            .items
            .iter()
            .map(|o| o.id)
            .collect::<Vec<_>>(),
        vec![0, 1],
        "the sender's ids are given up at the door"
    );
    assert_eq!(plan.overrides.next_id, 2);
    assert_eq!(plan.overrides.credits_for("TOC"), Some(3));
    assert!(plan.overrides.hidden.is_empty());
    assert_eq!(plan.customs, vec![custom("SEM")]);

    // And the changes it carries land on an empty store whole.
    let mut fresh = OverridesStore::default();
    let stats = merge_overrides(&mut fresh, &plan.overrides);
    assert_eq!(stats.meetings_added, 2);
    assert_eq!(stats.credits_added, 1);
}

/// A file from before the section existed is a course list, and imports as
/// exactly that.
#[test]
fn a_file_without_changes_is_still_a_timetable() {
    let plan = parse_timetable_export(&export_file(None)).expect("parses");
    assert_eq!(plan.codes.len(), 2);
    assert!(!plan.has_changes());
    assert!(plan.overrides.is_empty());
}

/// Everything refused, and refused with the sentence that names the reason.
#[test]
fn file_refusals() {
    let msg = |text: &str| parse_timetable_export(text).unwrap_err();

    assert!(msg("not json {").contains("couldn't be read"));
    assert!(
        msg(&json!({"format": "cmi-planner-backup"}).to_string()).contains("Import everything"),
        "a backup is named and redirected, not called foreign"
    );
    assert!(msg(&json!({"format": "someone-elses"}).to_string()).contains("Export my courses"));
    assert!(msg(&json!({"format": "cmi-timetable-export"}).to_string()).contains("no course list"));
    assert!(
        msg(&json!({"format": "cmi-timetable-export", "courses": [{"code": "  "}]}).to_string())
            .contains("no courses at all")
    );

    // A file that SAYS it carries changes and then doesn't parse is refused
    // whole — importing "the courses only" would drop the half that was the
    // reason for sending it.
    let broken = export_file(Some(json!({"meeting_changes": [{"nope": 1}]})));
    assert!(
        msg(&broken).contains("shape this app can read"),
        "{}",
        msg(&broken)
    );

    // Well-formed JSON that means nothing: a class on the 99th weekday.
    let nonsense = export_file(Some(json!({"meeting_changes": [{
        "course": "TOC", "kind": "added", "from": null,
        "to": {"day": "Funday", "iso_weekday": 99,
               "start": {"minutes": 550}, "end": {"minutes": 625}, "hall": null},
        "made_at": "", "made_at_ms": 0.0,
    }]})));
    assert!(
        msg(&nonsense).contains("shape this app can read"),
        "{}",
        msg(&nonsense)
    );

    // A class that ends before it starts is refused for the same reason.
    let backwards = export_file(Some(json!({"meeting_changes": [{
        "course": "TOC", "kind": "added", "from": null,
        "to": {"day": "Mon", "iso_weekday": 1,
               "start": {"minutes": 700}, "end": {"minutes": 550}, "hall": null},
        "made_at": "", "made_at_ms": 0.0,
    }]})));
    assert!(msg(&backwards).contains("shape this app can read"));

    // An explicit null is "no changes", not a damaged file.
    assert!(parse_timetable_export(&export_file(Some(json!(null)))).is_ok());
}

/// The file is meant to be read and WRITTEN by other programs, so the shape
/// carries its meaning on its face: every list is present, every time says
/// both what to compute with and what to read, every change says in a word
/// what kind it is.
#[test]
fn the_file_says_what_it_means_without_this_app() {
    let changes = MyChanges::build(
        &[
            ovr(
                0,
                "TOC",
                Some(meeting(Day::Tue, 550, "803")),
                Some(meeting(Day::Wed, 1020, "803")),
            ),
            ovr(1, "RDBM", Some(meeting(Day::Mon, 550, "601")), None),
            ovr(2, "SEM", None, Some(meeting(Day::Thu, 900, "Room B"))),
        ],
        &[credit("TOC", 3)],
        &[custom("SEM")],
    );
    let v = serde_json::to_value(&changes).unwrap();

    for key in ["meeting_changes", "credit_changes", "my_own_courses"] {
        assert!(v[key].is_array(), "{key} is always a list");
    }
    let kinds: Vec<&str> = v["meeting_changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["moved", "removed", "added"]);

    let moved = &v["meeting_changes"][0];
    assert_eq!(moved["course"], "TOC");
    assert_eq!(moved["from"]["day"], "Tue");
    assert_eq!(moved["from"]["iso_weekday"], 2);
    assert_eq!(moved["to"]["start"]["minutes"], 1020);
    assert_eq!(moved["to"]["start"]["hhmm"], "17:00");
    assert_eq!(moved["to"]["end"]["hhmm"], "18:15");
    assert_eq!(moved["to"]["hall"], "803");
    assert_eq!(moved["to"]["temporary_booking"], false);
    assert!(moved["made_at"].as_str().unwrap().ends_with('Z'));
    assert!(moved["made_at_ms"].is_number());
    assert!(v["meeting_changes"][1]["to"].is_null(), "struck out");
    assert!(v["meeting_changes"][2]["from"].is_null(), "added");

    let own = &v["my_own_courses"][0];
    assert_eq!(own["code"], "SEM");
    assert_eq!(own["status"], "scheduled");
    assert_eq!(own["credits"], 2);
    assert_eq!(own["meetings"][0]["day"], "Thu");
    assert_eq!(own["meetings"][0]["start"]["hhmm"], "15:00");
}

/// …and a file written BY another program, carrying only the load-bearing
/// fields, still loads. The decoration this app writes — `hhmm`, `kind`,
/// `made_at`, `status`, `iso_weekday` beside a good `day` — is for readers,
/// not for the parser.
#[test]
fn a_minimal_hand_written_file_loads() {
    let text = export_file(Some(json!({
        "meeting_changes": [{
            "course": "toc",
            "from": {"day": "Tue", "start": {"minutes": 550}, "end": {"minutes": 625},
                     "hall": "803", "iso_weekday": 2},
            "to": {"iso_weekday": 5, "start": {"minutes": 700}, "end": {"minutes": 775},
                   "hall": null},
            "made_at_ms": 0.0,
        }],
        "credit_changes": [{"course": "TOC", "credits": 2, "made_at_ms": 0.0}],
    })));

    let plan = parse_timetable_export(&text).expect("a minimal file is still a file");

    let change = &plan.overrides.items[0];
    assert_eq!(change.course, "toc");
    assert_eq!(change.base.as_ref().unwrap().day, Day::Tue);
    // Day taken from iso_weekday alone, since `day` was left out.
    let to = change.to.as_ref().unwrap();
    assert_eq!(to.day, Day::Fri);
    assert_eq!(to.slot, Slot::new(700, 775));
    assert_eq!(to.hall, None);
    assert_eq!(plan.overrides.credits_for("TOC"), Some(2));
}
