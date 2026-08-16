//! Putting one person's timetable file on top of another's.
//!
//! "Export my courses" writes a week: the courses, the classes moved, added
//! or struck out, the credit corrections, the courses the student wrote
//! themselves. "Import my courses" has to land that week in a browser that
//! already holds a week of its own — most often somebody else's, because the
//! point of the file is two people combining timetables.
//!
//! The rules, in one sentence each:
//!
//! - The same change made on both sides is ONE change, not two.
//! - A change to a class the other side never touched is taken.
//! - A change to a class the other side DID touch keeps theirs — a class
//!   cannot be in two places at once, and the browser doing the importing is
//!   the one whose week has to stay usable. Every such skip is counted and
//!   named, never dropped in silence.
//! - Anything genuinely additional — a class one side created that the other
//!   doesn't have — survives on both sides.
//!
//! Deletions of whole COURSES ([`OverridesStore::hidden`]) are not part of
//! this at all: a deleted course is by definition off the timetable, and the
//! file describes a timetable. Nothing here ever adds or removes one.
//!
//! Incoming ids are meaningless — both stores number from zero — so every
//! adopted change is renumbered into the receiving store's sequence.

use crate::model::{CustomStore, Meeting, MeetingOverride, OverridesStore};

/// What an import actually did, for the sentence the student is shown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CombineStats {
    /// Classes moved, created or struck out that were taken from the file.
    pub meetings_added: usize,
    /// Credit corrections taken from the file.
    pub credits_added: usize,
    /// Courses where the file's change lost to one of the reader's own.
    /// Codes, deduped, in the order they were met.
    pub kept_yours: Vec<String>,
    /// Courses whose changes were dropped because the code now names a
    /// course somebody wrote themselves, which carries its own schedule.
    /// Filled in by the caller from [`purge_custom_overrides`].
    pub dropped_for_own_course: Vec<String>,
}

impl CombineStats {
    pub fn changes_added(&self) -> usize {
        self.meetings_added + self.credits_added
    }

    pub fn is_empty(&self) -> bool {
        self.changes_added() == 0
            && self.kept_yours.is_empty()
            && self.dropped_for_own_course.is_empty()
    }
}

/// Two meeting slots that mean the same class at the same place and time —
/// or two absences. `None` is a struck-out class, and two strike-outs of the
/// same class are the same decision.
fn same_target(a: &Option<Meeting>, b: &Option<Meeting>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.same_place_time(y),
        _ => false,
    }
}

/// The same edit: same course, aimed at the same official class, with the
/// same outcome. Ids and timestamps say who wrote it down first, which is
/// not what makes two edits one.
fn same_change(a: &MeetingOverride, b: &MeetingOverride) -> bool {
    a.course.eq_ignore_ascii_case(&b.course)
        && same_target(&a.base, &b.base)
        && same_target(&a.to, &b.to)
}

/// Both edits claim the same official class, and disagree about where it
/// goes. Created classes (`base == None`) are never in competition: they are
/// additions, and two different additions are two classes.
fn contests_same_class(mine: &MeetingOverride, theirs: &MeetingOverride) -> bool {
    mine.course.eq_ignore_ascii_case(&theirs.course)
        && mine.base.is_some()
        && same_target(&mine.base, &theirs.base)
}

fn note_kept(stats: &mut CombineStats, code: &str) {
    if !stats
        .kept_yours
        .iter()
        .any(|c| c.eq_ignore_ascii_case(code))
    {
        stats.kept_yours.push(code.to_string());
    }
}

/// Fold `theirs` into `mine` by the rules at the top of this module. `mine`
/// is left holding every change it started with.
pub fn merge_overrides(mine: &mut OverridesStore, theirs: &OverridesStore) -> CombineStats {
    let mut stats = CombineStats::default();

    for incoming in &theirs.items {
        if mine.items.iter().any(|m| same_change(m, incoming)) {
            // Both of us moved the same class the same way. One change.
            continue;
        }
        if mine.items.iter().any(|m| contests_same_class(m, incoming)) {
            note_kept(&mut stats, &incoming.course);
            continue;
        }
        let id = mine.next_id;
        mine.next_id += 1;
        mine.items.push(MeetingOverride {
            id,
            ..incoming.clone()
        });
        stats.meetings_added += 1;
    }

    for incoming in &theirs.credits {
        match mine.credits_for(&incoming.course) {
            // Same number on both sides — nothing to decide, nothing to say.
            Some(existing) if existing == incoming.credits => {}
            Some(_) => note_kept(&mut stats, &incoming.course),
            None => {
                mine.credits.push(incoming.clone());
                stats.credits_added += 1;
            }
        }
    }

    stats
}

/// Forget every change aimed at these courses — the first half of "replace
/// my timetable with the file's", which then merges the file's own changes
/// into the space this clears. Deletions of whole courses are left alone
/// (see the module note); changes to courses NOT named here are the
/// reader's work on a different part of their week and stay untouched.
pub fn clear_for_courses(store: &mut OverridesStore, codes: &[String]) {
    let named = |course: &str| codes.iter().any(|c| c.eq_ignore_ascii_case(course));
    store.items.retain(|o| !named(&o.course));
    store.credits.retain(|c| !named(&c.course));
}

/// A course of the user's own IS its own schedule — it never carries
/// overrides. A store arriving from somewhere else (a share link, a file)
/// may still aim changes at a code that names one of these courses here, and
/// those would render as classes belonging to nothing. Drop them.
///
/// Returns the course codes it dropped changes for, deduped. Usually empty,
/// and usually the changes were the incoming store's own — but not always:
/// a file bringing a course of the sender's own under a code the READER had
/// changes saved for (a course CMI dropped, whose classes live on as their
/// overrides) loses the reader's work here. That is the right resolution —
/// the code now names a course that carries its own schedule — but it is
/// not something to do quietly, so the caller is told which courses.
pub fn purge_custom_overrides(customs: &CustomStore, ovs: &mut OverridesStore) -> Vec<String> {
    let mut dropped: Vec<String> = Vec::new();
    let mut note = |code: &str| {
        if !dropped.iter().any(|c| c.eq_ignore_ascii_case(code)) {
            dropped.push(code.to_string());
        }
    };
    ovs.items.retain(|o| {
        let keep = customs.get(&o.course).is_none();
        if !keep {
            note(&o.course);
        }
        keep
    });
    ovs.credits.retain(|c| {
        let keep = customs.get(&c.course).is_none();
        if !keep {
            note(&c.course);
        }
        keep
    });
    dropped
}
