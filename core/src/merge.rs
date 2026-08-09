//! Three-way merge between CMI's fresh snapshot and the user's meeting
//! overrides, per the decision table:
//!
//! | CMI changed vs base? | Override exists? | to == cmi_new? | Action                     |
//! |----------------------|------------------|----------------|----------------------------|
//! | no                   | no               | —              | nothing                    |
//! | yes                  | no               | —              | apply CMI silently         |
//! | no                   | yes              | —              | keep override              |
//! | yes                  | yes              | yes            | drop override silently     |
//! | yes                  | yes              | no             | queue a conflict           |
//!
//! Because official meetings always come straight from the snapshot and
//! overrides are layered on top, "apply CMI silently" needs no work here —
//! it only has to show up in the "What changed" digest.

use crate::diff::{diff_snapshots, SnapshotDiff};
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
    /// Conflicts to put in front of the user — never auto-resolved.
    pub conflicts: Vec<Conflict>,
    /// Courses in the current selection that no longer exist upstream.
    pub removed_selected: Vec<String>,
    /// Full snapshot diff for the "What changed since last sync" panel.
    pub diff: SnapshotDiff,
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
    let sort_key =
        |m: &&Meeting| (m.day.index(), m.slot.start_min, m.slot.end_min, m.hall.clone());
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

    for ov in &overrides.items {
        let old_meetings = old.course(&ov.course).map(|c| c.meetings.as_slice());
        let new_meetings = new.course(&ov.course).map(|c| c.meetings.as_slice());

        match &ov.base {
            Some(base) => {
                let (Some(old_m), Some(new_m)) = (old_meetings, new_meetings) else {
                    // Course removed upstream (or absent from the old cache):
                    // keep the override; the removed-course badge handles UX.
                    continue;
                };
                match counterpart(base, old_m, new_m) {
                    Ok(Some(cmi_new)) => {
                        if cmi_new.same_place_time(base) {
                            // CMI unchanged → keep override.
                        } else if ov.to.as_ref().is_some_and(|to| cmi_new.same_place_time(to)) {
                            // CMI now matches the user's change → drop silently.
                            drop_ids.push(ov.id);
                            result.dropped_matching.push(ov.clone());
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
                        if ov.is_removal() {
                            // Base missing from the OLD snapshot. If it still
                            // matches a CURRENT official meeting (overrides
                            // imported via a share link against fresher
                            // data), the removal is meaningful — keep it. If
                            // not, it is inert: it suppresses nothing, there
                            // is nothing to "keep", and rebasing onto an
                            // arbitrary candidate could strike a meeting the
                            // user never touched. Drop it silently; the
                            // visible timetable is unchanged either way.
                            if !new_m.iter().any(|m| m.same_place_time(base)) {
                                drop_ids.push(ov.id);
                            }
                        } else {
                            // Stale base (not in the old snapshot). Offer the
                            // new official meetings as candidates.
                            result.conflicts.push(Conflict {
                                override_id: ov.id,
                                course: ov.course.clone(),
                                mine: ov.to.clone(),
                                theirs: new_m.to_vec(),
                            });
                        }
                    }
                }
            }
            None => {
                // User-created meeting for an unscheduled course.
                let newly_scheduled = new_meetings.is_some_and(|m| !m.is_empty())
                    && old_meetings.is_none_or(|m| m.is_empty());
                if newly_scheduled {
                    let new_m = new_meetings.unwrap();
                    let matches_mine = ov
                        .to
                        .as_ref()
                        .is_some_and(|to| new_m.iter().any(|m| m.same_place_time(to)));
                    if matches_mine {
                        drop_ids.push(ov.id);
                        result.dropped_matching.push(ov.clone());
                    } else {
                        result.conflicts.push(Conflict {
                            override_id: ov.id,
                            course: ov.course.clone(),
                            mine: ov.to.clone(),
                            theirs: new_m.to_vec(),
                        });
                    }
                }
            }
        }
    }

    for id in drop_ids {
        result.overrides.remove(id);
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
        if let Some(ov) = store.items.iter_mut().find(|o| o.id == conflict.override_id) {
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
