//! R99 — the conflict answer stopped being either/or.
//!
//! CMI moving a class the reader had edited used to be answered with two
//! radio buttons: CMI's time, or theirs. That could not say "both", could not
//! say "neither", and — when CMI scheduled a previously unscheduled course at
//! several times — could not say "keep two of these three". Tick boxes can
//! say all of it, so every combination has to land somewhere sane.
//!
//! What these tests pin, beyond the plain cases:
//! - the ONE combination that must stay a replacement rather than a pair of
//!   independent entries, because an unanchored copy is a made-up time that
//!   never gets re-examined;
//! - that an answer, whatever it was, does not come back as a question on the
//!   next sync;
//! - the vector-length bug this round shipped and caught, where a short
//!   `keep_cmi` read its missing tail as "ticked" and kept a class nobody had
//!   asked for.

use cmi_timetable_core::merge::{
    Conflict, ConflictPick, ConflictShape, backfill_anchors, kept_meetings, merge_overrides,
    resolve_conflict,
};
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
        instructors: vec![],
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

fn store_of(course: &str, base: Option<Meeting>, to: Option<Meeting>) -> OverridesStore {
    let mut s = OverridesStore::default();
    s.add(course, base, to, 1.0);
    s
}

/// The everyday case: CMI ran MFD on Wednesday, the reader moved it to
/// Thursday, CMI has now moved it to Friday.
fn moved() -> (Conflict, OverridesStore, Snapshot, Meeting, Meeting) {
    let was = mtg(Day::Wed, 550, 625, "Lecture Hall 1");
    let mine = mtg(Day::Thu, 840, 915, "Lecture Hall 6");
    let cmi_new = mtg(Day::Fri, 930, 1005, "Lecture Hall 1");
    let old = snap(vec![course("MFD", vec![was.clone()])]);
    let new = snap(vec![course("MFD", vec![cmi_new.clone()])]);
    let store = store_of("MFD", Some(was), Some(mine.clone()));
    let r = merge_overrides(&old, &new, &[], &store);
    assert_eq!(r.conflicts.len(), 1);
    (r.conflicts[0].clone(), r.overrides, new, mine, cmi_new)
}

/// CMI listed no time for SVA, the reader placed it on Tuesday, and CMI has
/// now scheduled it TWICE. This is the shape radio buttons could not express.
fn newly_scheduled() -> (
    Conflict,
    OverridesStore,
    Snapshot,
    Meeting,
    Meeting,
    Meeting,
) {
    let mine = mtg(Day::Tue, 550, 625, "Seminar Hall");
    let a = mtg(Day::Mon, 550, 625, "Lecture Hall 3");
    let b = mtg(Day::Wed, 630, 705, "Lecture Hall 4");
    let old = snap(vec![course("SVA", vec![])]);
    let new = snap(vec![course("SVA", vec![a.clone(), b.clone()])]);
    let store = store_of("SVA", None, Some(mine.clone()));
    let r = merge_overrides(&old, &new, &[], &store);
    assert_eq!(r.conflicts.len(), 1);
    (r.conflicts[0].clone(), r.overrides, new, mine, a, b)
}

// ---------------------------------------------------------------------------
// The anchor — the fact the old dialog did not have
// ---------------------------------------------------------------------------

/// A conflict records the time CMI moved AWAY from. Without it the dialog can
/// only show two destinations, and the reader cannot tell whether the reason
/// they edited the class still applies.
#[test]
fn a_conflict_remembers_the_time_cmi_moved_away_from() {
    let (c, _, _, _, _) = moved();
    assert_eq!(c.was, Some(mtg(Day::Wed, 550, 625, "Lecture Hall 1")));
    assert_eq!(c.shape(), ConflictShape::Moved);
}

#[test]
fn the_four_shapes_are_told_apart() {
    let (moved_c, ..) = moved();
    assert_eq!(moved_c.shape(), ConflictShape::Moved);

    // A removal: same upstream move, but the reader had taken it off.
    let was = mtg(Day::Wed, 550, 625, "Lecture Hall 1");
    let cmi_new = mtg(Day::Fri, 930, 1005, "Lecture Hall 1");
    let old = snap(vec![course("MFD", vec![was.clone()])]);
    let new = snap(vec![course("MFD", vec![cmi_new])]);
    let r = merge_overrides(&old, &new, &[], &store_of("MFD", Some(was), None));
    assert_eq!(r.conflicts[0].shape(), ConflictShape::MovedWhatYouRemoved);

    // CMI drops one of two meetings, and it is the edited one.
    let a = mtg(Day::Wed, 550, 625, "Lecture Hall 1");
    let b = mtg(Day::Fri, 550, 625, "Lecture Hall 2");
    let mine = mtg(Day::Thu, 840, 915, "Lecture Hall 6");
    let old = snap(vec![course("MFD", vec![a.clone(), b.clone()])]);
    let new = snap(vec![course("MFD", vec![b])]);
    let r = merge_overrides(&old, &new, &[], &store_of("MFD", Some(a), Some(mine)));
    assert_eq!(r.conflicts[0].shape(), ConflictShape::Dropped);
    assert!(r.conflicts[0].theirs.is_empty());

    let (new_c, ..) = newly_scheduled();
    assert_eq!(new_c.shape(), ConflictShape::NewlyScheduled);
    assert_eq!(new_c.was, None);
}

/// A queue stored before the anchor existed is repaired from the overrides it
/// still points at — so a plain move is never described as a course CMI had
/// no time for.
#[test]
fn a_queue_stored_without_the_anchor_is_repaired_not_guessed() {
    // The store a legacy queue actually pairs with is the PRE-merge one.
    // `moved()` returns the post-merge store, whose override R100 has already
    // re-anchored onto CMI's new time — reading `was` from that would report
    // that CMI moved the class away from where it just moved it to. The two
    // are only ever paired correctly because `backfill_anchors` runs at a
    // LOAD door, before any merge; see its doc comment.
    let (mut c, _post_merge, _, mine, _) = moved();
    let store = store_of(
        "MFD",
        Some(mtg(Day::Wed, 550, 625, "Lecture Hall 1")),
        Some(mine),
    );
    let real = c.was.clone().expect("this shape has an anchor");
    c.was = None; // exactly how an older localStorage blob deserialises
    assert_eq!(
        c.shape(),
        ConflictShape::NewlyScheduled,
        "without the anchor the shape is misread — which is what the repair is for"
    );
    let mut queue = vec![c];
    backfill_anchors(&mut queue, &store);
    assert_eq!(queue[0].was, Some(real));
    assert_eq!(queue[0].shape(), ConflictShape::Moved);
}

/// The anchor is `#[serde(default)]`, so a blob written before it existed
/// still loads instead of throwing the whole deferred queue away.
#[test]
fn a_conflict_without_the_anchor_still_deserialises() {
    let json = r#"{"override_id":3,"course":"MFD","mine":null,"theirs":[]}"#;
    let c: Conflict = serde_json::from_str(json).expect("older queues must still load");
    assert_eq!(c.override_id, 3);
    assert_eq!(c.was, None);
}

// ---------------------------------------------------------------------------
// The boxes
// ---------------------------------------------------------------------------

/// The bug this round shipped and caught. `empty_pick` must be as long as
/// CMI's list of times, because `cmi()` reads a missing entry as TICKED — so
/// a short vector silently keeps a class the reader never ticked.
#[test]
fn an_empty_pick_is_as_long_as_cmis_list_of_times() {
    let (c, ..) = newly_scheduled();
    assert_eq!(c.theirs.len(), 2);
    let p = c.empty_pick();
    assert_eq!(p.keep_cmi.len(), 2);
    assert!(
        !p.cmi(0) && !p.cmi(1),
        "an untouched row has nothing ticked"
    );
    // And the backstop it exists to keep out of reach.
    let short = ConflictPick {
        keep_cmi: vec![false],
        keep_mine: false,
    };
    assert!(
        short.cmi(1),
        "out of range must read as KEPT — a malformed pick may not invent a removal"
    );
}

// ---------------------------------------------------------------------------
// Every combination, and what it leaves in the store
// ---------------------------------------------------------------------------

/// The answer the old dialog could not give: keep CMI's new time AND the one
/// the reader set. CMI's meeting must be left alone (no removal written) and
/// the reader's must become a class of their own.
#[test]
fn keeping_both_suppresses_nothing_and_adds_the_readers_time() {
    let (c, store, new, mine, _cmi_new) = moved();
    let mut s = store;
    resolve_conflict(
        &mut s,
        &c,
        &ConflictPick {
            keep_cmi: vec![true],
            keep_mine: true,
        },
        9.0,
    );
    assert_eq!(s.items.len(), 1, "only the reader's own time needs storing");
    assert_eq!(s.items[0].base, None, "it replaces nothing");
    assert_eq!(s.items[0].to, Some(mine));
    assert!(
        !s.items.iter().any(|o| o.is_removal()),
        "CMI's new time is kept, so nothing may be written to hide it"
    );
    let again = merge_overrides(&new, &new, &[], &s);
    assert!(again.conflicts.is_empty(), "an answer must not come back");
}

/// Keeping only the reader's time stays a REPLACEMENT — the one combination
/// whose storage shape matters beyond what it draws. Anchored, CMI moving the
/// class again asks the reader again; unanchored, they would keep a made-up
/// time forever and never be told.
#[test]
fn keeping_only_my_time_stays_anchored_to_cmis_meeting() {
    let (c, store, new, mine, cmi_new) = moved();
    let mut s = store;
    resolve_conflict(
        &mut s,
        &c,
        &ConflictPick {
            keep_cmi: vec![false],
            keep_mine: true,
        },
        9.0,
    );
    assert_eq!(s.items.len(), 1);
    assert_eq!(s.items[0].base, Some(cmi_new.clone()), "still anchored");
    assert_eq!(s.items[0].to, Some(mine));
    assert!(
        merge_overrides(&new, &new, &[], &s).conflicts.is_empty(),
        "answered, so quiet"
    );

    // …and the anchor is what makes a LATER upstream move ask again.
    let moved_again = snap(vec![course(
        "MFD",
        vec![mtg(Day::Sat, 480, 555, "Lecture Hall 1")],
    )]);
    let r = merge_overrides(&new, &moved_again, &[], &s);
    assert_eq!(
        r.conflicts.len(),
        1,
        "an anchored change is re-examined when CMI moves it again"
    );
    assert_eq!(r.conflicts[0].was, Some(cmi_new));
}

/// Ticking nothing is a real answer — "CMI moved it somewhere impossible and
/// my workaround is void, so take it off" — and it must actually take it off.
#[test]
fn ticking_nothing_takes_the_class_off_the_timetable() {
    let (c, store, new, _, cmi_new) = moved();
    let mut s = store;
    resolve_conflict(&mut s, &c, &c.empty_pick(), 9.0);
    assert_eq!(s.items.len(), 1);
    assert!(s.items[0].is_removal(), "CMI's time is hidden");
    assert_eq!(s.items[0].base, Some(cmi_new));
    assert!(
        merge_overrides(&new, &new, &[], &s).conflicts.is_empty(),
        "answered, so quiet"
    );
}

/// The case that needed tick boxes: CMI now runs the course twice and the
/// reader wants ONE of those, plus their own. Exactly the unticked one gets
/// hidden — not both, not the wrong one.
#[test]
fn one_of_cmis_two_times_can_be_kept_and_the_other_dropped() {
    let (c, store, new, mine, a, b) = newly_scheduled();
    let mut s = store;
    resolve_conflict(
        &mut s,
        &c,
        &ConflictPick {
            keep_cmi: vec![true, false],
            keep_mine: true,
        },
        9.0,
    );
    let removals: Vec<_> = s.items.iter().filter(|o| o.is_removal()).collect();
    assert_eq!(removals.len(), 1, "exactly one of CMI's times is hidden");
    assert_eq!(
        removals[0].base,
        Some(b.clone()),
        "and it is the unticked one"
    );
    assert!(
        !s.items
            .iter()
            .any(|o| o.is_removal() && o.base.as_ref() == Some(&a)),
        "the ticked one is left alone"
    );
    assert!(
        s.items
            .iter()
            .any(|o| o.base.is_none() && o.to.as_ref() == Some(&mine)),
        "the reader's own time is kept beside it"
    );
    assert!(merge_overrides(&new, &new, &[], &s).conflicts.is_empty());
}

/// Whatever was ticked, the next sync is quiet. A dialog that asked the same
/// question every time the app synced would be worse than no dialog.
#[test]
fn every_combination_is_answered_for_good() {
    for cmi_a in [false, true] {
        for cmi_b in [false, true] {
            for keep_mine in [false, true] {
                let (c, store, new, ..) = newly_scheduled();
                let mut s = store;
                let pick = ConflictPick {
                    keep_cmi: vec![cmi_a, cmi_b],
                    keep_mine,
                };
                resolve_conflict(&mut s, &c, &pick, 9.0);
                let again = merge_overrides(&new, &new, &[], &s);
                assert!(
                    again.conflicts.is_empty(),
                    "({cmi_a},{cmi_b},{keep_mine}) came back as a question"
                );
                assert!(
                    s.is_sane(),
                    "({cmi_a},{cmi_b},{keep_mine}) wrote an entry the app cannot hold"
                );
                // What the dialog promised is what got stored: every kept
                // time is present, and no kept time was hidden.
                for m in kept_meetings(&c, &pick) {
                    assert!(
                        !s.items
                            .iter()
                            .any(|o| o.is_removal() && o.base.as_ref() == Some(&m)),
                        "({cmi_a},{cmi_b},{keep_mine}) hid a time it promised to keep"
                    );
                }
            }
        }
    }
}

/// `kept_meetings` is what the dialog reads out, so it has to agree with the
/// boxes exactly — in reading order, and counting the reader's time only when
/// they have one.
#[test]
fn the_promise_reads_the_boxes_and_nothing_else() {
    let (c, _, _, mine, a, b) = newly_scheduled();
    let all = kept_meetings(
        &c,
        &ConflictPick {
            keep_cmi: vec![true, true],
            keep_mine: true,
        },
    );
    assert_eq!(
        all,
        vec![a.clone(), mine.clone(), b.clone()],
        "reading order"
    );
    assert!(kept_meetings(&c, &c.empty_pick()).is_empty());

    // A reader who had REMOVED the class has no time to keep, whatever the
    // flag says.
    let was = mtg(Day::Wed, 550, 625, "Lecture Hall 1");
    let cmi_new = mtg(Day::Fri, 930, 1005, "Lecture Hall 1");
    let old = snap(vec![course("MFD", vec![was.clone()])]);
    let new = snap(vec![course("MFD", vec![cmi_new])]);
    let r = merge_overrides(&old, &new, &[], &store_of("MFD", Some(was), None));
    let kept = kept_meetings(
        &r.conflicts[0],
        &ConflictPick {
            keep_cmi: vec![false],
            keep_mine: true,
        },
    );
    assert!(
        kept.is_empty(),
        "there is no time of the reader's to keep — the class was removed"
    );
}
