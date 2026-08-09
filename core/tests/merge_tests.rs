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

/// The week the student actually sees: CMI's meetings with their changes
/// layered on, the same rule /app applies. Used to state the property that
/// matters most about a merge — a class they never edited is still there,
/// where CMI puts it.
fn effective(store: &OverridesStore, code: &str, official: &[Meeting]) -> Vec<Meeting> {
    let ovs: Vec<&_> = store.for_course(code).collect();
    let mut out: Vec<Meeting> = Vec::new();
    for m in official {
        match ovs
            .iter()
            .find(|o| o.base.as_ref().is_some_and(|b| b.same_place_time(m)))
        {
            // A change to this meeting: its new time, or nothing if struck out.
            Some(o) => out.extend(o.to.clone()),
            None => out.push(m.clone()),
        }
    }
    // Times of their own, replacing nothing.
    for o in &ovs {
        if o.base.is_none() {
            out.extend(o.to.clone());
        }
    }
    out
}

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
    let new = snap(vec![course(
        "MFD",
        vec![mtg(Day::Thu, 930, 1005, "Lecture Hall 803")],
    )]);
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
    let store = store_with("MFD", Some(official), mine);
    let r = merge_overrides(&old, &new, &[], &store);
    assert!(r.conflicts.is_empty());
    assert_eq!(r.dropped_matching.len(), 1);
    assert_eq!(r.dropped_matching[0].course, "MFD");
    assert!(
        r.overrides.items.is_empty(),
        "override removed from the store"
    );
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
    let store = store_with("MFD", Some(official), mine);
    let r = merge_overrides(&old, &new, &[], &store);
    let conflict = r.conflicts[0].clone();

    // Keep mine → base becomes CMI's new meeting; re-merging is quiet.
    let mut kept = r.overrides.clone();
    resolve_conflict(&mut kept, &conflict, true);
    assert_eq!(kept.items[0].base, Some(cmi_new));
    let r2 = merge_overrides(&new, &new, &[], &kept);
    assert!(r2.conflicts.is_empty());
    assert_eq!(r2.overrides.items.len(), 1);

    // Use CMI's → override removed.
    let mut dropped = r.overrides;
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
    assert_eq!(
        r.overrides.items.len(),
        1,
        "override kept for the badge flow"
    );
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
    let store = store_with("MFD", Some(official), mine);
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
        ..official
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

/// A change whose meeting is in NEITHER snapshot has lost the thing it was
/// about: CMI has not run that class for at least a term. It may not be
/// silently dropped (that is how a struck-out class comes back without a
/// word) and it may not be re-pointed at whatever the course runs now
/// (nothing there is a class the student edited). So it LAPSES and is
/// reported: a removal goes, a move keeps its destination as a time of the
/// student's own. A base that still matches a CURRENT official meeting is a
/// different thing entirely — it is doing its job, and survives.
#[test]
fn stale_changes_lapse_and_are_reported() {
    let phantom = mtg(Day::Mon, 550, 625, "Lecture Hall 1");
    let real = WED_OFFICIAL();

    // A removal of a class CMI no longer runs: nothing left to suppress.
    let old = snap(vec![course("MFD", vec![real.clone()])]);
    let store = removal_store("MFD", phantom.clone());
    let r = merge_overrides(&old, &old, &[], &store);
    assert!(
        r.conflicts.is_empty(),
        "never a question: {:#?}",
        r.conflicts
    );
    assert_eq!(r.lapsed.len(), 1, "but never silent either");
    assert!(r.overrides.items.is_empty());
    // Above all: the class the student never touched is untouched.
    assert_eq!(
        effective(&r.overrides, "MFD", std::slice::from_ref(&real)),
        vec![real.clone()],
        "CMI's Wednesday lecture must still be in their week"
    );

    // A MOVE of a class CMI no longer runs keeps where they put it.
    let mine = mtg(Day::Sat, 1020, 1095, "Seminar Hall");
    let store = store_with("MFD", Some(phantom), mine.clone());
    let r = merge_overrides(&old, &old, &[], &store);
    assert!(r.conflicts.is_empty());
    assert_eq!(r.lapsed.len(), 1);
    assert_eq!(r.overrides.items.len(), 1);
    assert_eq!(
        r.overrides.items[0].base, None,
        "it becomes a time of their own, claiming to replace nothing"
    );
    let mut still = effective(&r.overrides, "MFD", std::slice::from_ref(&real));
    still.sort_by_key(|m| (m.day.index(), m.slot.start_min));
    assert_eq!(
        still,
        vec![real.clone(), mine],
        "their Saturday slot stays AND CMI's lecture is still there"
    );
    // And it has stopped being stale, so the next sync is quiet.
    let again = merge_overrides(&old, &old, &[], &r.overrides);
    assert!(again.lapsed.is_empty());
    assert!(again.conflicts.is_empty());

    // Meaningful: base missing from the OLD snapshot but present in the new.
    let old_without = snap(vec![course("MFD", vec![])]);
    let new_with = snap(vec![course("MFD", vec![real.clone()])]);
    let store = removal_store("MFD", real);
    let r = merge_overrides(&old_without, &new_with, &[], &store);
    assert!(r.conflicts.is_empty());
    assert!(r.lapsed.is_empty());
    assert_eq!(r.overrides.items.len(), 1, "active removal must survive");
    assert!(r.overrides.items[0].is_removal());
}

/// The two-sync story behind [`stale_changes_lapse_and_are_reported`].
///
/// A student strikes out one of a course's classes. CMI then moves that
/// class, so the next sync asks whether to keep it removed — and they close
/// the dialog without answering. Unanswered conflicts are not persisted and
/// the cached snapshot has already moved on, so by the following sync the
/// override's base is in neither snapshot. What must NOT happen then is the
/// removal quietly disappearing (the class reappears with no word) or being
/// re-aimed at the class CMI moved it to (which the student never removed).
#[test]
fn an_unanswered_removal_lapses_out_loud() {
    let monday = mtg(Day::Mon, 550, 625, "Lecture Hall 1");
    let moved = mtg(Day::Mon, 630, 705, "Lecture Hall 2");
    let other = mtg(Day::Thu, 840, 915, "Lecture Hall 1");

    let before = snap(vec![course("MFD", vec![monday.clone(), other.clone()])]);
    let after = snap(vec![course("MFD", vec![moved.clone(), other.clone()])]);
    let selection = vec!["MFD".to_string()];
    let store = removal_store("MFD", monday);

    // Sync 1 — CMI moved the class they had struck out, so they are asked.
    let first = merge_overrides(&before, &after, &selection, &store);
    assert_eq!(first.conflicts.len(), 1);
    assert_eq!(first.overrides.items.len(), 1);
    assert!(first.lapsed.is_empty());

    // They close the dialog. The snapshot is stored anyway; the question is
    // not. Sync 2 sees a base that is in neither snapshot.
    let second = merge_overrides(&after, &after, &selection, &first.overrides);
    assert!(second.conflicts.is_empty(), "{:#?}", second.conflicts);
    assert_eq!(second.lapsed.len(), 1, "it must be said out loud");
    assert_eq!(second.lapsed[0].course, "MFD");
    assert!(second.overrides.items.is_empty());

    // Both of CMI's classes are in their week, untouched. In particular the
    // class CMI moved is NOT struck out — they never removed that one.
    let mut still = effective(&second.overrides, "MFD", &[moved.clone(), other.clone()]);
    still.sort_by_key(|m| (m.day.index(), m.slot.start_min));
    assert_eq!(still, vec![moved, other]);

    // And it does not come round again.
    let third = merge_overrides(&after, &after, &selection, &second.overrides);
    assert!(third.lapsed.is_empty());
    assert!(third.conflicts.is_empty());
}
