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

fn describe_course_change(old: &Course, new: &Course) -> Vec<String> {
    let mut out = Vec::new();
    if old.name != new.name {
        out.push(format!("name: {:?} → {:?}", old.name, new.name));
    }
    if old.instructors != new.instructors {
        out.push(format!(
            "instructors: {} → {}",
            old.instructors.join("/"),
            new.instructors.join("/")
        ));
    }
    if old.credits != new.credits {
        out.push("credits changed".to_string());
    }
    if old.status != new.status {
        out.push("schedule status changed".to_string());
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
