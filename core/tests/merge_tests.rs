//! Three-way-merge tests — one per row of the spec §5 decision table, plus
//! removed-course, new-course and unscheduled-course cases.

use cmi_timetable_core::merge::{merge_overrides, resolve_conflict};
use cmi_timetable_core::model::{
    Course, Day, Meeting, OverridesStore, ScheduleStatus, Slot, Snapshot, SourceTier,
};

fn mtg(day: Day, start: u16, end: u16, hall: &str) -> Meeting {
    Meeting {
        day,
        slot: Slot::new(start, end),
        hall: Some(hall.to_string()),
        temp_booking: false,
    }
}

fn course(code: &str, meetings: Vec<Meeting>) -> Course {
    Course {
        code: code.to_string(),
        name: format!("{code} name"),
        instructors: vec!["Someone".to_string()],
        branches: vec!["BM1".to_string()],
        credits: None,
        starts: None,
        part_of_semester: None,
        optional_flag: false,
        status: if meetings.is_empty() {
            ScheduleStatus::UnscheduledListed
        } else {
            ScheduleStatus::Scheduled
        },
        meetings,
    }
}

fn snap(courses: Vec<Course>) -> Snapshot {
    Snapshot {
        semester_label: "August--November 2026".to_string(),
        fetched_at: 0.0,
        source: SourceTier::Bundled,
        parser_version: 1,
        branches: vec![],
        courses,
        halls: vec![],
        slot_grid: vec![],
        hall_bookings: vec![],
        raw_html_gz: None,
    }
}

fn store_with(course: &str, base: Option<Meeting>, to: Meeting) -> OverridesStore {
    let mut store = OverridesStore::default();
    store.add(course, base, Some(to), 0.0);
    store
}

const WED_OFFICIAL: fn() -> Meeting = || mtg(Day::Wed, 840, 915, "Lecture Hall 6");

/// Row 1 — CMI unchanged, no override: nothing happens.
#[test]
fn row1_no_change_no_override() {
    let old = snap(vec![course("MFD", vec![WED_OFFICIAL()])]);
    let new = snap(vec![course("MFD", vec![WED_OFFICIAL()])]);
    let r = merge_overrides(&old, &new, &[], &OverridesStore::default());
    assert!(r.conflicts.is_empty());
    assert!(r.dropped_matching.is_empty());
    assert!(r.diff.is_empty());
    assert!(r.removed_selected.is_empty());
}

/// Row 2 — CMI changed, no override: applied silently, listed in the digest.
#[test]
fn row2_cmi_changed_no_override() {
    let old = snap(vec![course("MFD", vec![WED_OFFICIAL()])]);
    let new = snap(vec![course("MFD", vec![mtg(Day::Thu, 930, 1005, "Lecture Hall 803")])]);
    let r = merge_overrides(&old, &new, &[], &OverridesStore::default());
    assert!(r.conflicts.is_empty());
    assert!(r.dropped_matching.is_empty());
    assert_eq!(r.diff.changed.len(), 1);
    assert_eq!(r.diff.changed[0].code, "MFD");
}

/// Row 3 — CMI unchanged, override exists: override kept.
#[test]
fn row3_no_change_override_kept() {
    let official = WED_OFFICIAL();
    let mine = mtg(Day::Thu, 840, 915, "Lecture Hall 6");
    let old = snap(vec![course("MFD", vec![official.clone()])]);
    let new = snap(vec![course("MFD", vec![official.clone()])]);
    let store = store_with("MFD", Some(official), mine);
    let r = merge_overrides(&old, &new, &[], &store);
    assert!(r.conflicts.is_empty());
    assert!(r.dropped_matching.is_empty());
    assert_eq!(r.overrides.items.len(), 1);
}

/// Row 4 — CMI changed to exactly what the user chose: override dropped
/// silently (toast-worthy).
#[test]
fn row4_cmi_matches_override_dropped() {
    let official = WED_OFFICIAL();
    let mine = mtg(Day::Thu, 840, 915, "Lecture Hall 6");
    let old = snap(vec![course("MFD", vec![official.clone()])]);
    let new = snap(vec![course("MFD", vec![mine.clone()])]);
    let store = store_with("MFD", Some(official), mine.clone());
    let r = merge_overrides(&old, &new, &[], &store);
    assert!(r.conflicts.is_empty());
    assert_eq!(r.dropped_matching.len(), 1);
    assert_eq!(r.dropped_matching[0].course, "MFD");
    assert!(r.overrides.items.is_empty(), "override removed from the store");
}

/// Row 5 — CMI changed to something else: conflict queued, never
/// auto-resolved.
#[test]
fn row5_conflict_queued() {
    let official = WED_OFFICIAL();
    let mine = mtg(Day::Thu, 840, 915, "Lecture Hall 6");
    let cmi_new = mtg(Day::Wed, 930, 1005, "Lecture Hall 803");
    let old = snap(vec![course("MFD", vec![official.clone()])]);
    let new = snap(vec![course("MFD", vec![cmi_new.clone()])]);
    let store = store_with("MFD", Some(official), mine.clone());
    let r = merge_overrides(&old, &new, &[], &store);
    assert_eq!(r.conflicts.len(), 1);
    let c = &r.conflicts[0];
    assert_eq!(c.course, "MFD");
    assert_eq!(c.mine, Some(mine));
    assert_eq!(c.theirs, vec![cmi_new]);
    // The override stays until the user decides.
    assert_eq!(r.overrides.items.len(), 1);
}

/// Conflict resolution: "keep mine" re-bases so the next sync is quiet;
/// "use CMI's" drops the override.
#[test]
fn conflict_resolution_paths() {
    let official = WED_OFFICIAL();
    let mine = mtg(Day::Thu, 840, 915, "Lecture Hall 6");
    let cmi_new = mtg(Day::Wed, 930, 1005, "Lecture Hall 803");
    let old = snap(vec![course("MFD", vec![official.clone()])]);
    let new = snap(vec![course("MFD", vec![cmi_new.clone()])]);
    let store = store_with("MFD", Some(official), mine.clone());
    let r = merge_overrides(&old, &new, &[], &store);
    let conflict = r.conflicts[0].clone();

    // Keep mine → base becomes CMI's new meeting; re-merging is quiet.
    let mut kept = r.overrides.clone();
    resolve_conflict(&mut kept, &conflict, true);
    assert_eq!(kept.items[0].base, Some(cmi_new.clone()));
    let r2 = merge_overrides(&new, &new, &[], &kept);
    assert!(r2.conflicts.is_empty());
    assert_eq!(r2.overrides.items.len(), 1);

    // Use CMI's → override removed.
    let mut dropped = r.overrides.clone();
    resolve_conflict(&mut dropped, &conflict, false);
    assert!(dropped.items.is_empty());
}

/// Course removed upstream while selected → reported; override untouched.
#[test]
fn removed_course_reported() {
    let official = WED_OFFICIAL();
    let old = snap(vec![course("GONE", vec![official.clone()])]);
    let new = snap(vec![]);
    let store = store_with("GONE", Some(official), mtg(Day::Fri, 840, 915, "Hall X"));
    let selection = vec!["GONE".to_string()];
    let r = merge_overrides(&old, &new, &selection, &store);
    assert_eq!(r.removed_selected, vec!["GONE".to_string()]);
    assert_eq!(r.diff.removed, vec!["GONE".to_string()]);
    assert_eq!(r.overrides.items.len(), 1, "override kept for the badge flow");
    assert!(r.conflicts.is_empty());
}

/// New course upstream → appears in the digest.
#[test]
fn new_course_reported() {
    let old = snap(vec![]);
    let new = snap(vec![course("NEW", vec![WED_OFFICIAL()])]);
    let r = merge_overrides(&old, &new, &[], &OverridesStore::default());
    assert_eq!(r.diff.added, vec!["NEW".to_string()]);
}

/// A user-created meeting (base = None) for an unscheduled course:
/// CMI scheduling it at the same time drops the override silently…
#[test]
fn user_created_meeting_matching_upstream() {
    let mine = mtg(Day::Mon, 550, 625, "Seminar Hall");
    let old = snap(vec![course("SVA", vec![])]);
    let new = snap(vec![course("SVA", vec![mine.clone()])]);
    let store = store_with("SVA", None, mine);
    let r = merge_overrides(&old, &new, &[], &store);
    assert!(r.conflicts.is_empty());
    assert_eq!(r.dropped_matching.len(), 1);
    assert!(r.overrides.items.is_empty());
}

/// …and CMI scheduling it at a different time queues a conflict.
#[test]
fn user_created_meeting_conflicting_upstream() {
    let mine = mtg(Day::Mon, 550, 625, "Seminar Hall");
    let cmi = mtg(Day::Tue, 630, 705, "Lecture Hall 2");
    let old = snap(vec![course("SVA", vec![])]);
    let new = snap(vec![course("SVA", vec![cmi.clone()])]);
    let store = store_with("SVA", None, mine.clone());
    let r = merge_overrides(&old, &new, &[], &store);
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0].mine, Some(mine));
    assert_eq!(r.conflicts[0].theirs, vec![cmi]);
    // While the course stays unscheduled, the created meeting is left alone.
    let r2 = merge_overrides(&old, &old, &[], &store);
    assert!(r2.conflicts.is_empty());
    assert_eq!(r2.overrides.items.len(), 1);
}

/// CMI deleting the overridden meeting entirely queues a conflict with no
/// upstream candidate.
#[test]
fn meeting_deleted_upstream() {
    let official = WED_OFFICIAL();
    let other = mtg(Day::Fri, 550, 625, "Lecture Hall 1");
    let mine = mtg(Day::Thu, 840, 915, "Lecture Hall 6");
    let old = snap(vec![course("MFD", vec![official.clone(), other.clone()])]);
    let new = snap(vec![course("MFD", vec![other])]);
    let store = store_with("MFD", Some(official), mine.clone());
    let r = merge_overrides(&old, &new, &[], &store);
    assert_eq!(r.conflicts.len(), 1);
    assert!(r.conflicts[0].theirs.is_empty());

    // "Keep mine" then turns it into a user-created meeting.
    let mut kept = r.overrides.clone();
    resolve_conflict(&mut kept, &r.conflicts[0], true);
    assert_eq!(kept.items[0].base, None);
}

/// A hall-only change upstream still counts as a CMI change (row 5 with the
/// same day/slot but a different hall).
#[test]
fn hall_change_is_a_change() {
    let official = WED_OFFICIAL();
    let rehalled = Meeting {
        hall: Some("Lecture Hall 801".to_string()),
        ..official.clone()
    };
    let mine = mtg(Day::Thu, 840, 915, "Lecture Hall 6");
    let old = snap(vec![course("MFD", vec![official.clone()])]);
    let new = snap(vec![course("MFD", vec![rehalled.clone()])]);
    let store = store_with("MFD", Some(official), mine);
    let r = merge_overrides(&old, &new, &[], &store);
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0].theirs, vec![rehalled]);
}

// ---------------------------------------------------------------------------
// Meeting removals (to == None)
// ---------------------------------------------------------------------------

fn removal_store(course: &str, base: Meeting) -> OverridesStore {
    let mut store = OverridesStore::default();
    store.add(course, Some(base), None, 0.0);
    store
}

/// CMI unchanged: the removal stays in force, silently.
#[test]
fn removal_kept_while_cmi_unchanged() {
    let official = WED_OFFICIAL();
    let old = snap(vec![course("MFD", vec![official.clone()])]);
    let store = removal_store("MFD", official);
    let r = merge_overrides(&old, &old, &[], &store);
    assert!(r.conflicts.is_empty());
    assert!(r.dropped_matching.is_empty());
    assert_eq!(r.overrides.items.len(), 1);
    assert!(r.overrides.items[0].is_removal());
}

/// CMI deleted the meeting the user had removed: both sides agree — the
/// override is dropped without a conflict.
#[test]
fn removal_auto_resolves_when_cmi_deletes_too() {
    let official = WED_OFFICIAL();
    let other = mtg(Day::Fri, 840, 915, "Lecture Hall 6");
    let old = snap(vec![course("MFD", vec![official.clone(), other.clone()])]);
    let new = snap(vec![course("MFD", vec![other])]);
    let store = removal_store("MFD", official);
    let r = merge_overrides(&old, &new, &[], &store);
    assert!(r.conflicts.is_empty());
    assert_eq!(r.dropped_matching.len(), 1);
    assert!(r.overrides.items.is_empty());
}

/// CMI moved the meeting the user had removed: a real question — the move
/// may fix whatever made them remove it. Keep-mine rebases the removal onto
/// the new meeting so it doesn't re-conflict on the next sync.
#[test]
fn removal_conflicts_when_cmi_moves_the_meeting() {
    let official = WED_OFFICIAL();
    let cmi_new = mtg(Day::Wed, 930, 1005, "Lecture Hall 803");
    let old = snap(vec![course("MFD", vec![official.clone()])]);
    let new = snap(vec![course("MFD", vec![cmi_new.clone()])]);
    let store = removal_store("MFD", official);

    let r = merge_overrides(&old, &new, &[], &store);
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0].mine, None);
    assert_eq!(r.conflicts[0].theirs, vec![cmi_new.clone()]);

    let mut kept = r.overrides.clone();
    resolve_conflict(&mut kept, &r.conflicts[0], true);
    assert_eq!(kept.items.len(), 1);
    assert!(kept.items[0].is_removal());
    assert_eq!(kept.items[0].base, Some(cmi_new));
    let r2 = merge_overrides(&new, &new, &[], &kept);
    assert!(r2.conflicts.is_empty(), "keep-mine must not re-conflict");

    let mut dropped = r.overrides.clone();
    resolve_conflict(&mut dropped, &r.conflicts[0], false);
    assert!(dropped.items.is_empty(), "use-CMI's restores the meeting");
}

/// A removal whose base is in NEITHER snapshot is inert (suppresses
/// nothing): dropped silently, no conflict. One whose base still matches a
/// CURRENT official meeting (e.g. share-imported against fresher data)
/// keeps suppressing it and must survive.
#[test]
fn stale_removals_drop_only_when_truly_inert() {
    let phantom = mtg(Day::Mon, 550, 625, "Lecture Hall 1");
    let real = WED_OFFICIAL();

    // Inert: base matches nothing anywhere.
    let old = snap(vec![course("MFD", vec![real.clone()])]);
    let store = removal_store("MFD", phantom);
    let r = merge_overrides(&old, &old, &[], &store);
    assert!(r.conflicts.is_empty());
    assert!(r.overrides.items.is_empty(), "inert removal must be dropped");

    // Meaningful: base missing from the OLD snapshot but present in the new.
    let old_without = snap(vec![course("MFD", vec![])]);
    let new_with = snap(vec![course("MFD", vec![real.clone()])]);
    let store = removal_store("MFD", real);
    let r = merge_overrides(&old_without, &new_with, &[], &store);
    assert!(r.conflicts.is_empty());
    assert_eq!(r.overrides.items.len(), 1, "active removal must survive");
    assert!(r.overrides.items[0].is_removal());
}
