//! Three-way merge between CMI's fresh snapshot and the user's meeting
//! overrides, per the decision table:
//!
//! | CMI changed vs base? | Override exists? | to == cmi_new? | Action                     |
//! |----------------------|------------------|----------------|----------------------------|
//! | no                   | no               | —              | nothing                    |
//! | yes                  | no               | —              | apply CMI silently         |
//! | no                   | yes              | —              | keep override              |
//! | yes                  | yes              | yes            | drop override (announced)  |
//! | yes                  | yes              | no             | queue a conflict           |
//!
//! Because official meetings always come straight from the snapshot and
//! overrides are layered on top, "apply CMI silently" needs no work here —
//! it only has to show up in the "What changed" digest.
//!
//! Two rules deliberately do NOT need the old snapshot, because a share
//! link can arrive in a browser that has never synced (`old` is an empty
//! placeholder), and "we have no history" must never be read as "CMI
//! changed something":
//!
//! - **Convergence.** An override whose destination is a meeting CMI now
//!   runs officially (and whose base, if any, CMI no longer runs) says
//!   nothing the timetable doesn't already say — worse, layering it on
//!   would draw the same class twice. It is dropped and announced,
//!   whether or not there is any history to compare against.
//! - **"Newly scheduled" requires knowing the course was unscheduled.**
//!   A user-created meeting raises a conflict only when the OLD snapshot
//!   knew the course with no meetings and the new one gives it some. A
//!   course the old snapshot never heard of proves nothing about what CMI
//!   changed — treating "missing" as "was unscheduled" asked share-link
//!   recipients to resolve a change that never happened.

use crate::diff::{SnapshotDiff, diff_snapshots};
use crate::model::{Meeting, MeetingOverride, OverridesStore, Snapshot};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    pub override_id: u64,
    pub course: String,
    /// The user's side: their meeting ("Keep my time: …"), or `None` when
    /// they had REMOVED the meeting ("Keep it removed").
    pub mine: Option<Meeting>,
    /// CMI's new meeting(s) ("Use CMI's new time: …"). Usually one; empty
    /// when CMI deleted the meeting entirely; several when CMI scheduled a
    /// previously unscheduled course the user had placed manually.
    pub theirs: Vec<Meeting>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MergeResult {
    /// The overrides store after the merge (matching overrides dropped).
    pub overrides: OverridesStore,
    /// Overrides dropped because CMI now matches the user's change —
    /// announce with a toast.
    pub dropped_matching: Vec<MeetingOverride>,
    /// Changes whose meeting is in NEITHER snapshot: CMI has not run that
    /// class for at least a term, so there is nothing left for the change to
    /// attach to. A move keeps its destination (it becomes a time of the
    /// user's own); a removal has nothing left to suppress and goes. Both
    /// are ANNOUNCED — the one thing that must not happen is a silent
    /// reinterpretation of what the student asked for.
    pub lapsed: Vec<MeetingOverride>,
    /// Conflicts to put in front of the user — never auto-resolved.
    pub conflicts: Vec<Conflict>,
    /// Courses in the current selection that no longer exist upstream.
    pub removed_selected: Vec<String>,
    /// Full snapshot diff for the "What changed since last sync" panel.
    pub diff: SnapshotDiff,
}

/// Hall equality the way people write halls: trimmed and case-insensitive,
/// with "no hall" only equal to "no hall". Used ONLY by the convergence
/// check — a destination hall typed as "lecture hall 6" and CMI's
/// "Lecture Hall 6" are the same room, and missing that match would leave
/// the same class drawn twice forever. `same_place_time` itself stays
/// byte-exact: everywhere else both sides come from CMI's own pages.
fn same_hall_loose(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.trim().eq_ignore_ascii_case(b.trim()),
        (None, None) => true,
        _ => false,
    }
}

/// Does `official` realize the user's destination `to`? Day and times must
/// match exactly; the hall matches the way users type halls.
fn realizes(official: &Meeting, to: &Meeting) -> bool {
    official.day == to.day
        && official.slot == to.slot
        && same_hall_loose(official.hall.as_deref(), to.hall.as_deref())
}

/// Find `base`'s counterpart among the new official meetings.
///
/// Meetings that survived unchanged pair with themselves. The remaining old
/// meetings pair positionally (sorted by day/time) with the remaining new
/// meetings — the standard heuristic for "CMI moved this meeting".
/// Returns `Ok(Some(meeting))` for a (possibly identical) counterpart,
/// `Ok(None)` when the meeting was deleted upstream, and `Err(())` when the
/// base meeting cannot be found in the old snapshot at all (stale override —
/// treated as "changed" with the whole new set as counterpart candidates).
fn counterpart(
    base: &Meeting,
    old_meetings: &[Meeting],
    new_meetings: &[Meeting],
) -> Result<Option<Meeting>, ()> {
    if new_meetings.iter().any(|m| m.same_place_time(base)) {
        return Ok(Some(base.clone()));
    }
    let mut old_unmatched: Vec<&Meeting> = old_meetings
        .iter()
        .filter(|o| !new_meetings.iter().any(|n| n.same_place_time(o)))
        .collect();
    let mut new_unmatched: Vec<&Meeting> = new_meetings
        .iter()
        .filter(|n| !old_meetings.iter().any(|o| o.same_place_time(n)))
        .collect();
    let sort_key = |m: &&Meeting| {
        (
            m.day.index(),
            m.slot.start_min,
            m.slot.end_min,
            m.hall.clone(),
        )
    };
    old_unmatched.sort_by_key(sort_key);
    new_unmatched.sort_by_key(sort_key);

    match old_unmatched.iter().position(|o| o.same_place_time(base)) {
        Some(idx) => Ok(new_unmatched.get(idx).map(|m| (*m).clone())),
        None => Err(()),
    }
}

pub fn merge_overrides(
    old: &Snapshot,
    new: &Snapshot,
    selection: &[String],
    overrides: &OverridesStore,
) -> MergeResult {
    let mut result = MergeResult {
        overrides: overrides.clone(),
        diff: diff_snapshots(old, new),
        ..Default::default()
    };

    result.removed_selected = selection
        .iter()
        .filter(|code| old.course(code).is_some() && new.course(code).is_none())
        .cloned()
        .collect();

    let mut drop_ids: Vec<u64> = Vec::new();
    // Overrides that keep their destination but lose their anchor.
    let mut unanchor_ids: Vec<u64> = Vec::new();

    for ov in &overrides.items {
        let old_meetings = old.course(&ov.course).map(|c| c.meetings.as_slice());
        let new_meetings = new.course(&ov.course).map(|c| c.meetings.as_slice());

        // Convergence, judged against the NEW snapshot alone (see module
        // docs): the user's destination is now official, and the meeting it
        // replaced (if any) is gone. The override has nothing left to say —
        // and keeping it would render the same class twice, once from the
        // snapshot and once from the override layer.
        if let (Some(new_m), Some(to)) = (new_meetings, ov.to.as_ref()) {
            let cmi_runs_mine = new_m.iter().any(|m| realizes(m, to));
            let base_gone = ov
                .base
                .as_ref()
                .is_none_or(|b| !new_m.iter().any(|m| m.same_place_time(b)));
            if cmi_runs_mine && base_gone {
                drop_ids.push(ov.id);
                result.dropped_matching.push(ov.clone());
                continue;
            }
        }

        match &ov.base {
            Some(base) => {
                let Some(new_m) = new_meetings else {
                    // Course removed upstream entirely: keep the override;
                    // the removed-course badge handles UX.
                    continue;
                };
                // A course the OLD snapshot never heard of gets an empty
                // history, not a free pass: if its base is still official,
                // `counterpart` returns it unchanged and the override is
                // kept (the share-link case); if the base is in neither
                // snapshot, the change lapses NOW, out loud — instead of
                // surviving one sync as a zombie and lapsing later with
                // copy that blames a "recent" CMI edit.
                let old_m = old_meetings.unwrap_or(&[]);
                match counterpart(base, old_m, new_m) {
                    Ok(Some(cmi_new)) => {
                        if cmi_new.same_place_time(base) {
                            // CMI unchanged → keep override.
                            //
                            // (The "CMI now matches the user's change" case
                            // cannot reach this match: a counterpart equal to
                            // `to` means `to` is official and `base` is not,
                            // which is exactly the convergence check above.)
                        } else {
                            // CMI moved a meeting the user had moved — or one
                            // they had removed ("keep it removed?" is a real
                            // question: the move may fix why they removed it).
                            result.conflicts.push(Conflict {
                                override_id: ov.id,
                                course: ov.course.clone(),
                                mine: ov.to.clone(),
                                theirs: vec![cmi_new],
                            });
                        }
                    }
                    Ok(None) => {
                        if ov.is_removal() {
                            // CMI deleted the meeting the user had removed —
                            // both sides agree; drop silently.
                            drop_ids.push(ov.id);
                            result.dropped_matching.push(ov.clone());
                        } else {
                            // CMI deleted the meeting the user had moved.
                            result.conflicts.push(Conflict {
                                override_id: ov.id,
                                course: ov.course.clone(),
                                mine: ov.to.clone(),
                                theirs: Vec::new(),
                            });
                        }
                    }
                    Err(()) => {
                        // The base is in neither snapshot — CMI has not run
                        // this class for at least a term. (It cannot be in
                        // the new one: `counterpart` returns `Ok` for that.)
                        //
                        // Asking about it was tried and is not safe: the
                        // question's only candidates are the classes the
                        // course runs NOW, none of which the student edited,
                        // and `resolve_conflict` would re-point the override
                        // at one of them — "keep it removed" striking out a
                        // lecture they never touched, "keep mine" hiding one.
                        // Dropping it silently is not acceptable either: that
                        // is how a struck-out class comes back with no word.
                        // So the change LAPSES, and is announced.
                        result.lapsed.push(ov.clone());
                        if ov.is_removal() {
                            // Nothing left to suppress.
                            drop_ids.push(ov.id);
                        } else {
                            // Their placement is real and stays put — as a
                            // time of their own, with nothing claimed about
                            // what it replaces.
                            unanchor_ids.push(ov.id);
                        }
                    }
                }
            }
            None => {
                // User-created meeting for an unscheduled course. "Newly
                // scheduled" needs the OLD snapshot to have KNOWN the course
                // with no meetings — a course the old snapshot never heard
                // of (an empty first-boot placeholder, a share link opened
                // in a fresh browser) proves nothing about what CMI changed,
                // and used to raise a bogus "CMI changed times you
                // customised" conflict on the very first sync.
                // (The matching case — CMI now runs the user's meeting — was
                // handled by the convergence check above.)
                let newly_scheduled = new_meetings.is_some_and(|m| !m.is_empty())
                    && old_meetings.is_some_and(|m| m.is_empty());
                if newly_scheduled {
                    result.conflicts.push(Conflict {
                        override_id: ov.id,
                        course: ov.course.clone(),
                        mine: ov.to.clone(),
                        theirs: new_meetings.unwrap().to_vec(),
                    });
                }
            }
        }
    }

    for id in drop_ids {
        result.overrides.remove(id);
    }
    for id in unanchor_ids {
        if let Some(o) = result.overrides.items.iter_mut().find(|o| o.id == id) {
            o.base = None;
        }
    }

    result
}

/// Apply one conflict resolution to an overrides store.
/// `keep_mine == true` re-bases the override onto CMI's new meeting so the
/// same conflict doesn't re-trigger on every future sync; `false` drops the
/// override so CMI's official time shows through.
pub fn resolve_conflict(store: &mut OverridesStore, conflict: &Conflict, keep_mine: bool) {
    if keep_mine {
        let mut drop = false;
        if let Some(ov) = store
            .items
            .iter_mut()
            .find(|o| o.id == conflict.override_id)
        {
            ov.base = if conflict.theirs.len() == 1 {
                Some(conflict.theirs[0].clone())
            } else {
                // Meeting deleted upstream (or several candidates): the
                // user's meeting is now effectively user-created.
                None
            };
            // "Keep it removed" of a meeting that no longer exists is a
            // no-op override — the removal is already a fact; drop it.
            drop = ov.is_removal() && ov.base.is_none();
        }
        if drop {
            store.remove(conflict.override_id);
        }
    } else {
        store.remove(conflict.override_id);
    }
}
