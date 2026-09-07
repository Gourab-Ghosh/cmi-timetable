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
    /// CMI's time the change was anchored to — the meeting the reader
    /// actually edited, before CMI moved it. `None` only for the
    /// newly-scheduled shape, where the course had no official time at all
    /// and the reader placed one themselves.
    ///
    /// The dialog cannot ask an answerable question without this. A reader
    /// who moved a class off Friday because it clashed needs to be told that
    /// Friday is what changed; shown only "CMI: Tue 09:10" and "yours: Wed
    /// 17:00" they have no way to tell whether their reason still holds.
    /// R99 added it, so `#[serde(default)]`: conflicts are persisted (both
    /// `cmitt.v1.conflicts` and a backup file's `pending_conflicts`), and a
    /// queue stored before this field existed must still load — it simply
    /// reads as the newly-scheduled shape, which is the honest answer when
    /// the anchor was never recorded.
    #[serde(default)]
    pub was: Option<Meeting>,
}

/// The boxes a reader has ticked for one conflict.
///
/// Every time in play gets its own box — each of CMI's new times, and the
/// time the reader set themselves — and ANY combination is legal, including
/// none of them. That is the whole point: a class CMI now runs twice can be
/// kept once, and a reader who wants both their time and CMI's can say so
/// instead of being made to choose. The old dialog offered two radio
/// buttons, which could express neither.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConflictPick {
    /// One flag per entry of [`Conflict::theirs`], in the same order.
    ///
    /// Build it with [`Conflict::empty_pick`] and never by hand: a vector
    /// SHORTER than `theirs` reads its missing tail through [`Self::cmi`]'s
    /// fallback, and R99 shipped a bug doing exactly that. A row with two of
    /// CMI's times, ticked at index 0, was stored as `[true]` — so index 1
    /// read as ticked, drew itself ticked, and kept a class the reader had
    /// never asked for.
    pub keep_cmi: Vec<bool>,
    /// Keep [`Conflict::mine`]. Ignored when the reader had REMOVED the
    /// class, because then there is no time of theirs to keep.
    pub keep_mine: bool,
}

impl ConflictPick {
    /// Whether CMI's `i`th time is ticked.
    ///
    /// Out of range reads as TICKED, which is the lossless direction: a
    /// malformed pick then keeps a class rather than quietly writing a
    /// removal override for one the reader never mentioned. It is a
    /// backstop, not a mechanism — [`Conflict::empty_pick`] makes every
    /// vector the right length, so a correct caller never reaches it.
    pub fn cmi(&self, i: usize) -> bool {
        self.keep_cmi.get(i).copied().unwrap_or(true)
    }
}

/// Which of the four situations a conflict is, so the dialog can say what
/// happened in a sentence instead of showing two bare times and leaving the
/// reader to infer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictShape {
    /// CMI moved a class the reader had moved somewhere else.
    Moved,
    /// CMI moved a class the reader had taken OFF their timetable — a real
    /// question, because the move may well fix whatever made them remove it.
    MovedWhatYouRemoved,
    /// CMI no longer lists a class the reader had moved. Nothing of CMI's is
    /// left to tick.
    Dropped,
    /// The course had no official time at all, so the reader placed one
    /// themselves — and CMI has now scheduled it.
    NewlyScheduled,
}

impl Conflict {
    pub fn shape(&self) -> ConflictShape {
        match (&self.was, self.theirs.is_empty(), self.mine.is_some()) {
            // No anchor means the override replaced nothing, which is only
            // true of the newly-scheduled shape. `backfill_anchors` restores
            // the anchor on any queue stored before `was` existed, so this
            // arm can be trusted rather than merely assumed.
            (None, _, _) => ConflictShape::NewlyScheduled,
            (Some(_), true, _) => ConflictShape::Dropped,
            (Some(_), false, true) => ConflictShape::Moved,
            (Some(_), false, false) => ConflictShape::MovedWhatYouRemoved,
        }
    }

    /// Nothing ticked, at the right length — the state a row is in the moment
    /// the reader first touches it, and the ONLY way the app should ever build
    /// a pick from scratch. See [`ConflictPick::keep_cmi`] for what building
    /// one by hand cost.
    ///
    /// Nothing is ticked for the reader, and that is the shipped behaviour:
    /// `t29` asserts "no conflict row may come pre-answered" and `t293`
    /// asserts that an untouched row and a deliberately emptied one are told
    /// apart in WORDS rather than by their ticks.
    ///
    /// This doc comment is written out because of what stood here before it.
    /// Seventeen lines describing a DIFFERENT function — "Everything starts
    /// TICKED … untick what you don't want" — sat directly above this one,
    /// which returns nothing ticked, while the function they actually
    /// described (`default_pick`) sat below with no doc at all and no
    /// production caller. R99's `keep_cmi` bug was born in exactly that gap:
    /// a contract stated in one place and implemented in another. R100
    /// deleted the dead function and moved its reasoning to the code that now
    /// carries the policy — the re-anchor in [`merge_overrides`], which is
    /// what actually keeps a removal removed while its question waits.
    pub fn empty_pick(&self) -> ConflictPick {
        ConflictPick {
            keep_cmi: vec![false; self.theirs.len()],
            keep_mine: false,
        }
    }
}

/// Fill in [`Conflict::was`] for a queue stored before that field existed.
///
/// A deferred conflict still points at its override by id, and that override
/// still carries the anchor in `base` — so the fact is recoverable and does
/// not have to be guessed. Without this, an old stored conflict would arrive
/// with `was: None` and be described to the reader as "CMI listed no time
/// for this class, so you placed one yourself", which for a plain move is
/// simply false. Running it at every door that loads a queue is what lets
/// [`Conflict::shape`] treat a missing anchor as a fact rather than a maybe.
///
/// # Call this at a LOAD door and nowhere else
///
/// `base` is only the original anchor until a merge has seen it. Since R100,
/// raising a question RE-ANCHORS its override onto the meeting CMI moved it
/// to, so after a merge `base` is CMI's NEW time and reading `was` from it
/// would tell the reader that CMI moved the class away from the place it just
/// moved it to. That is safe today only because of an ordering fact, and the
/// ordering is the whole guarantee: the queues that need repair are the ones
/// written before `was` existed, by a build that did not re-anchor, and they
/// are repaired at boot (`app.rs`) and at import (`export.rs`) BEFORE any
/// merge runs. Do not call this after a sync, and do not "simplify" it into
/// running unconditionally — a queue that already has its anchor must keep
/// it. `r99_conflict_choices::a_queue_stored_without_the_anchor_is_repaired_not_guessed`
/// pins the pairing this function is for.
pub fn backfill_anchors(conflicts: &mut [Conflict], store: &OverridesStore) {
    for c in conflicts.iter_mut().filter(|c| c.was.is_none()) {
        if let Some(ov) = store.items.iter().find(|o| o.id == c.override_id) {
            c.was = ov.base.clone();
        }
    }
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
    // Overrides to RE-ANCHOR onto the meeting CMI moved them to, while the
    // question about them stays unanswered. See `reanchor` below for why.
    let mut reanchor: Vec<(u64, Meeting)> = Vec::new();

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
                                theirs: vec![cmi_new.clone()],
                                // The ORIGINAL anchor, captured before the
                                // re-anchor below — it is what the dialog
                                // reads out, and it must stay the time the
                                // reader actually edited.
                                was: Some(base.clone()),
                            });
                            // ASKING IS NOT ANSWERING, so asking must not
                            // change the week (R100).
                            //
                            // The override still points at CMI's OLD meeting,
                            // which this sync has just replaced. Left that
                            // way it goes stale, and a stale override is not
                            // inert: a removal suppresses nothing, so a class
                            // the reader had struck out REAPPEARS the moment
                            // they press "Decide later" — and the notice told
                            // them "there's nothing left to remove". A stale
                            // move floats free, so CMI's new time shows
                            // beside theirs. Either way the app has answered
                            // for them, while its own dialog promises
                            // "nothing changes until you press Save".
                            //
                            // There is no neutral state — the week must draw
                            // something — so the choice is which default is
                            // safer, and the honesty law answers it: the
                            // reader's own work comes first, so their edit
                            // stays in force until they replace it. So the
                            // override follows the class CMI moved, and the
                            // question stands.
                            //
                            // THIS is where that policy lives. R100 deleted
                            // `Conflict::default_pick`, which stated it in a
                            // doc comment and had no production caller,
                            // because a contract implemented somewhere other
                            // than where it is written is exactly how R99's
                            // `keep_cmi` bug happened.
                            //
                            // `merge_tests::an_unanswered_removal_lapses_out_loud`
                            // used to pin the opposite, on a premise that has
                            // since expired ("unanswered conflicts are not
                            // persisted" — R87 made them survive reloads) and
                            // on an objection to re-aiming a removal at a
                            // class the student never removed. That objection
                            // is about doing it SILENTLY INSTEAD OF ASKING;
                            // here the question is still asked, and answering
                            // "keep it removed" performs this very re-anchor
                            // (`resolve_conflict`). Doing it up front only
                            // makes the pending state agree with the answer
                            // the dialog offers by default.
                            reanchor.push((ov.id, cmi_new));
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
                                was: Some(base.clone()),
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
                        // No anchor exists: CMI listed no time for this
                        // course, which is why the reader placed one.
                        was: None,
                    });
                }
            }
        }
    }

    for id in drop_ids {
        result.overrides.remove(id);
    }
    for (id, to) in reanchor {
        if let Some(o) = result.overrides.items.iter_mut().find(|o| o.id == id) {
            o.base = Some(to);
        }
    }
    for id in unanchor_ids {
        if let Some(o) = result.overrides.items.iter_mut().find(|o| o.id == id) {
            o.base = None;
        }
    }

    result
}

/// What one answer will draw, in reading order — the sentence the dialog
/// shows under the boxes, and what a native test can pin without a browser.
///
/// An empty result means the class will not appear at all, which is a legal
/// answer (CMI moved it somewhere impossible and the old workaround is void)
/// and the one the dialog has to say out loud.
pub fn kept_meetings(conflict: &Conflict, pick: &ConflictPick) -> Vec<Meeting> {
    let mut out: Vec<Meeting> = conflict
        .theirs
        .iter()
        .enumerate()
        .filter(|(i, _)| pick.cmi(*i))
        .map(|(_, m)| m.clone())
        .collect();
    if pick.keep_mine
        && let Some(mine) = &conflict.mine
    {
        out.push(mine.clone());
    }
    out.sort_by_key(|m| (m.day.index(), m.slot.start_min));
    out
}

/// Apply one answer to an overrides store.
///
/// The question is settled either way, so the override that raised it always
/// goes and the ticked times are written back as fresh entries. Rebuilding
/// rather than patching is deliberate: ONE code path produces all sixteen
/// shapes of answer (any subset of CMI's times × the reader's time × whether
/// they had removed the class), and there is no combination it can only
/// half-express. The old two-way version patched `base` in place and
/// therefore could not say "both" at all.
///
/// Three rules do the work:
/// - an UNTICKED time of CMI's becomes a removal override, which is exactly
///   how the rest of the app hides an official meeting;
/// - a TICKED time of the reader's is written with `base: None`, i.e. a class
///   of their own, which is what lets it sit BESIDE CMI's instead of
///   replacing it;
/// - the one case that stays a replacement is "my time instead of CMI's" —
///   exactly one official time, unticked, with the reader's time ticked.
///   Both forms draw the same week, but a replacement is still ANCHORED to
///   CMI's meeting, so if CMI moves it again the reader is asked again. The
///   unanchored pair would leave them with a made-up time that quietly
///   drifts out of date forever, and that is the shape this dialog exists to
///   prevent.
pub fn resolve_conflict(
    store: &mut OverridesStore,
    conflict: &Conflict,
    pick: &ConflictPick,
    now: f64,
) {
    store.remove(conflict.override_id);

    let dropped: Vec<&Meeting> = conflict
        .theirs
        .iter()
        .enumerate()
        .filter(|(i, _)| !pick.cmi(*i))
        .map(|(_, m)| m)
        .collect();
    // A reader who had REMOVED the class has no time of their own to keep,
    // whatever the flag says.
    let mine = pick.keep_mine.then_some(conflict.mine.as_ref()).flatten();

    if let Some(mine) = mine
        && conflict.theirs.len() == 1
        && dropped.len() == 1
    {
        // "My time instead of CMI's" — keep it anchored (see above).
        store.add(
            &conflict.course,
            Some(conflict.theirs[0].clone()),
            Some(mine.clone()),
            now,
        );
        return;
    }

    for m in dropped {
        store.add(&conflict.course, Some(m.clone()), None, now);
    }
    if let Some(mine) = mine {
        store.add(&conflict.course, None, Some(mine.clone()), now);
    }
}
