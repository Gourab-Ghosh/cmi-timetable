//! Cheap snapshot diff — feeds the "What changed since last sync" panel and
//! the three-way merge.

use crate::model::{Course, Meeting, Snapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CourseChange {
    pub code: String,
    pub meetings_before: Vec<Meeting>,
    pub meetings_after: Vec<Meeting>,
    /// Human-readable one-liners ("name changed …", "moved …").
    pub summary: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SnapshotDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
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
        .map(|m| (m.day.index(), m.slot.start_min, m.slot.end_min, m.hall.clone()))
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

fn describe_course_change(old: &Course, new: &Course) -> Vec<String> {
    let mut out = Vec::new();
    if old.name != new.name {
        out.push(format!("renamed: {} → {}", old.name, new.name));
    }
    if old.instructors != new.instructors {
        out.push(format!(
            "instructor: {} → {}",
            fmt_people(&old.instructors),
            fmt_people(&new.instructors)
        ));
    }
    if old.credits != new.credits {
        out.push(format!(
            "credits: {} → {}",
            fmt_credits(old.credits),
            fmt_credits(new.credits)
        ));
    }
    if old.status != new.status {
        out.push(format!(
            "{} → {}",
            status_words(old.status),
            status_words(new.status)
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
            out.push(format!(
                "moved: {} → {}",
                gone[0].describe(),
                came[0].describe()
            ));
        } else {
            for m in gone {
                out.push(format!("meeting removed: {}", m.describe()));
            }
            for m in came {
                out.push(format!("meeting added: {}", m.describe()));
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
            diff.removed.push(course.code.clone());
        }
    }

    diff
}
