//! Cheap snapshot diff — feeds the "What changed since last sync" panel and
//! the three-way merge.

use crate::model::{Course, Meeting, Snapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// What sort of change a line describes. Kept apart from the values so the
/// UI can put the KIND where the eye lands first — a column of "moved",
/// "credits", "renamed" is findable in a long list; a column of full
/// sentences that happen to start differently is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    Renamed,
    Instructor,
    Credits,
    Status,
    Moved,
    MeetingAdded,
    MeetingRemoved,
}

impl ChangeKind {
    pub fn label(&self) -> &'static str {
        match self {
            ChangeKind::Renamed => "renamed",
            ChangeKind::Instructor => "instructor",
            ChangeKind::Credits => "credits",
            ChangeKind::Status => "status",
            ChangeKind::Moved => "moved",
            ChangeKind::MeetingAdded => "meeting added",
            ChangeKind::MeetingRemoved => "meeting removed",
        }
    }
}

/// One difference: its kind, the value CMI had, and the value CMI has now.
/// `before`/`after` are `None` when there is nothing on that side (a meeting
/// that only appeared, or only vanished).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeLine {
    pub kind: ChangeKind,
    pub before: Option<String>,
    pub after: Option<String>,
}

impl ChangeLine {
    fn edit(kind: ChangeKind, before: String, after: String) -> Self {
        ChangeLine {
            kind,
            before: Some(before),
            after: Some(after),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CourseChange {
    pub code: String,
    pub meetings_before: Vec<Meeting>,
    pub meetings_after: Vec<Meeting>,
    /// One line per difference, kind kept separate from the values.
    pub summary: Vec<ChangeLine>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SnapshotDiff {
    pub added: Vec<String>,
    /// Each course that left CMI's pages, exactly as CMI last published it —
    /// the new snapshot has never heard of them, so the "What changed"
    /// dialog would otherwise have nothing to show but a bare code. The
    /// WHOLE course rather than a summary of one, because the dialog can
    /// hand a dropped course back to the user as a course of their own, and
    /// anything a summary left out would have to be invented at that moment
    /// — credits above all, where a guess silently moves a credit total.
    /// The diff itself is never persisted; keeping a course is what makes
    /// one permanent.
    pub removed: Vec<Course>,
    pub changed: Vec<CourseChange>,
}

impl SnapshotDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

fn meetings_set(course: &Course) -> BTreeSet<(usize, u16, u16, Option<String>)> {
    course
        .meetings
        .iter()
        .map(|m| {
            (
                m.day.index(),
                m.slot.start_min,
                m.slot.end_min,
                m.hall.clone(),
            )
        })
        .collect()
}

fn fmt_credits(credits: Option<u8>) -> String {
    match credits {
        Some(n) => n.to_string(),
        None => "unstated".to_string(),
    }
}

fn fmt_people(people: &[String]) -> String {
    if people.is_empty() {
        "—".to_string()
    } else {
        people.join(" / ")
    }
}

fn status_words(status: crate::model::ScheduleStatus) -> &'static str {
    match status {
        crate::model::ScheduleStatus::Scheduled => "on the timetable",
        crate::model::ScheduleStatus::UnscheduledListed => "listed without a time slot",
        crate::model::ScheduleStatus::ScheduledNoBranch => "scheduled outside the branch grids",
    }
}

fn describe_course_change(old: &Course, new: &Course) -> Vec<ChangeLine> {
    let mut out = Vec::new();
    if old.name != new.name {
        out.push(ChangeLine::edit(
            ChangeKind::Renamed,
            old.name.clone(),
            new.name.clone(),
        ));
    }
    if old.instructors != new.instructors {
        out.push(ChangeLine::edit(
            ChangeKind::Instructor,
            fmt_people(&old.instructors),
            fmt_people(&new.instructors),
        ));
    }
    if old.credits != new.credits {
        out.push(ChangeLine::edit(
            ChangeKind::Credits,
            fmt_credits(old.credits),
            fmt_credits(new.credits),
        ));
    }
    if old.status != new.status {
        out.push(ChangeLine::edit(
            ChangeKind::Status,
            status_words(old.status).to_string(),
            status_words(new.status).to_string(),
        ));
    }
    let (before, after) = (meetings_set(old), meetings_set(new));
    if before != after {
        let gone: Vec<&Meeting> = old
            .meetings
            .iter()
            .filter(|m| !new.meetings.iter().any(|n| n.same_place_time(m)))
            .collect();
        let came: Vec<&Meeting> = new
            .meetings
            .iter()
            .filter(|m| !old.meetings.iter().any(|o| o.same_place_time(m)))
            .collect();
        if gone.len() == 1 && came.len() == 1 {
            out.push(ChangeLine::edit(
                ChangeKind::Moved,
                gone[0].describe(),
                came[0].describe(),
            ));
        } else {
            for m in gone {
                out.push(ChangeLine {
                    kind: ChangeKind::MeetingRemoved,
                    before: Some(m.describe()),
                    after: None,
                });
            }
            for m in came {
                out.push(ChangeLine {
                    kind: ChangeKind::MeetingAdded,
                    before: None,
                    after: Some(m.describe()),
                });
            }
        }
    }
    out
}

pub fn diff_snapshots(old: &Snapshot, new: &Snapshot) -> SnapshotDiff {
    let mut diff = SnapshotDiff::default();

    for course in &new.courses {
        match old.course(&course.code) {
            None => diff.added.push(course.code.clone()),
            Some(prev) => {
                let summary = describe_course_change(prev, course);
                if !summary.is_empty() {
                    diff.changed.push(CourseChange {
                        code: course.code.clone(),
                        meetings_before: prev.meetings.clone(),
                        meetings_after: course.meetings.clone(),
                        summary,
                    });
                }
            }
        }
    }
    for course in &old.courses {
        if new.course(&course.code).is_none() {
            diff.removed.push(course.clone());
        }
    }

    diff
}
