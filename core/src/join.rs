//! Join the two parsed pages into the course catalog: branch membership and
//! meetings from the timetable page, halls from the hall-allocation page,
//! names/instructors canonically from the halls legend.

use crate::model::{
    Branch, Course, Day, ParseStats, ScheduleStatus, Slot,
};
use crate::parse::{HallsPage, TimetablePage};
use regex_lite::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

static STARTS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\(\s*starts?\s+(\d{1,2})(?:st|nd|rd|th)?\s+([A-Za-z]{3,})\.?\s*\)").unwrap()
});
static CREDITS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\(\s*(\d+)\s*credits?\s*\)").unwrap());
static PAREN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(([^()]*)\)").unwrap());

/// Structured fields extracted from a verbatim course name. The name itself
/// is always preserved for display.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NameNotes {
    pub starts: Option<(u8, String)>,
    pub credits: Option<u8>,
    pub part_of_semester: Option<String>,
}

/// Read a parenthesized note as a month span. Accepts what a hand-edited
/// page might plausibly say — "Oct-Nov", "Oct – Nov", "October to November",
/// "Sept." — and rejects everything else by requiring every token to BE a
/// month word (so "(Haskell)" and "(Maroon)" never match). Normalized to
/// the app's "Oct-Nov" / "Oct" form, which date-consuming code (ics) and
/// `Course::months_span` both understand.
fn parse_month_span(inner: &str) -> Option<String> {
    let trimmed = inner.trim();
    // A dangling separator ("Oct-", "-Nov", "until Nov") is an INCOMPLETE
    // range, not a one-month note — misreading it would shrink the course
    // to one month of credits and calendar. Leave it unparsed.
    if trimmed.starts_with(['-', '\u{2013}', '\u{2014}'])
        || trimmed.ends_with(['-', '\u{2013}', '\u{2014}'])
    {
        return None;
    }
    let tokens: Vec<&str> = trimmed
        .split(|c: char| c == '-' || c == '\u{2013}' || c == '\u{2014}' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .collect();
    let month = crate::date::month_from_word;
    let name = crate::date::month_short_name;
    match tokens.as_slice() {
        [a] => Some(name(month(a)?).to_string()),
        [a, b] => Some(format!("{}-{}", name(month(a)?), name(month(b)?))),
        [a, sep, b] if ["to", "till", "until", "through"]
            .contains(&sep.to_ascii_lowercase().as_str()) =>
        {
            Some(format!("{}-{}", name(month(a)?), name(month(b)?)))
        }
        _ => None,
    }
}

pub fn extract_name_notes(name: &str) -> NameNotes {
    let mut notes = NameNotes::default();
    if let Some(caps) = STARTS_RE.captures(name) {
        if let Ok(d) = caps.get(1).unwrap().as_str().parse::<u8>() {
            notes.starts = Some((d, caps.get(2).unwrap().as_str().to_string()));
        }
    }
    if let Some(caps) = CREDITS_RE.captures(name) {
        notes.credits = caps.get(1).unwrap().as_str().parse::<u8>().ok();
    }
    // Month spans: check every parenthesized note, not one rigid "(Xxx-Yyy)"
    // shape. The starts/credits notes never parse as pure month spans (they
    // carry digits/words), so order doesn't matter.
    for caps in PAREN_RE.captures_iter(name) {
        if let Some(span) = parse_month_span(caps.get(1).unwrap().as_str()) {
            notes.part_of_semester = Some(span);
            break;
        }
    }
    notes
}

pub fn split_instructors(raw: &str) -> Vec<String> {
    raw.split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Default)]
struct CourseBuilder {
    tt_name: Option<String>,
    tt_instr: Option<String>,
    halls_name: Option<String>,
    halls_instr: Option<String>,
    branches: Vec<String>,
    per_branch: BTreeMap<String, BTreeSet<(Day, Slot)>>,
    grid_meetings: BTreeSet<(Day, Slot)>,
    optional: bool,
}

impl CourseBuilder {
    fn add_branch(&mut self, branch: &str) {
        if !self.branches.iter().any(|b| b == branch) {
            self.branches.push(branch.to_string());
        }
    }
}

pub struct Joined {
    pub branches: Vec<Branch>,
    pub courses: Vec<Course>,
    pub halls: Vec<String>,
    pub slot_grid: Vec<Slot>,
    pub hall_bookings: Vec<crate::model::HallBooking>,
    pub stats: ParseStats,
    pub warnings: Vec<String>,
}

pub fn join_pages(tt: &TimetablePage, hp: &HallsPage) -> Joined {
    let mut warnings = Vec::new();
    let mut builders: BTreeMap<String, CourseBuilder> = BTreeMap::new();

    // 1. Branch legends: membership + fallback name/instructor.
    for section in &tt.sections {
        for entry in &section.legend {
            let b = builders.entry(entry.code.clone()).or_default();
            b.add_branch(&section.branch.code);
            match &b.tt_name {
                Some(existing) if *existing != entry.name => warnings.push(format!(
                    "{}: timetable legends disagree on the name ({existing:?} vs {:?})",
                    entry.code, entry.name
                )),
                _ => b.tt_name = Some(entry.name.clone()),
            }
            if b.tt_instr.is_none() {
                b.tt_instr = entry.instructors_raw.clone();
            }
        }
    }

    // 2. Grid occurrences: membership + meetings.
    for section in &tt.sections {
        for occ in &section.occurrences {
            let b = builders.entry(occ.code.clone()).or_default();
            b.add_branch(&section.branch.code);
            b.per_branch
                .entry(section.branch.code.clone())
                .or_default()
                .insert((occ.day, occ.slot));
            b.grid_meetings.insert((occ.day, occ.slot));
            if occ.optional {
                b.optional = true;
            }
        }
    }

    // Duplicate codes across branches: verify the meetings agree; on
    // disagreement the union is already recorded — add a data warning.
    for (code, b) in &builders {
        let sets: Vec<&BTreeSet<(Day, Slot)>> = b.per_branch.values().collect();
        if sets.len() > 1 && sets.windows(2).any(|w| w[0] != w[1]) {
            warnings.push(format!(
                "{code}: meetings differ between branches {:?}; using the union",
                b.branches
            ));
        }
    }

    // 3. Halls legend (canonical names/instructors; also the full catalog).
    for entry in &hp.legend {
        let b = builders.entry(entry.code.clone()).or_default();
        if let Some(tt_name) = &b.tt_name {
            if *tt_name != entry.name {
                warnings.push(format!(
                    "{}: legends disagree on the name — timetable {tt_name:?}, halls {:?} (halls kept)",
                    entry.code, entry.name
                ));
            }
        }
        b.halls_name = Some(entry.name.clone());
        b.halls_instr = entry.instructors_raw.clone();
    }

    // 4. Hall lookup table from the hall grid.
    let mut hall_by_key: BTreeMap<(String, Day, Slot), Vec<(String, bool)>> = BTreeMap::new();
    for entry in &hp.entries {
        for code in &entry.codes {
            hall_by_key
                .entry((code.clone(), entry.day, entry.slot))
                .or_default()
                .push((entry.hall.clone(), entry.temp));
        }
    }
    let mut consumed: BTreeSet<(String, Day, Slot)> = BTreeSet::new();

    // 5. Assemble courses.
    let mut courses = Vec::new();
    let mut stats = ParseStats::default();

    // Make sure hall-grid-only codes (category c) get builders too.
    for entry in &hp.entries {
        for code in &entry.codes {
            builders.entry(code.clone()).or_default();
        }
    }

    for (code, b) in &builders {
        let name = b
            .halls_name
            .clone()
            .or_else(|| b.tt_name.clone())
            .unwrap_or_else(|| {
                warnings.push(format!(
                    "{code}: no legend entry on either page; using the code as its name"
                ));
                code.clone()
            });
        let instructors_raw = b.halls_instr.clone().or_else(|| b.tt_instr.clone());
        let notes = extract_name_notes(&name);

        let mut meetings = Vec::new();
        for (day, slot) in &b.grid_meetings {
            let key = (code.clone(), *day, *slot);
            let hall_matches = hall_by_key.get(&key);
            let (hall, temp) = match hall_matches {
                Some(halls) => {
                    consumed.insert(key.clone());
                    if halls.len() > 1 {
                        warnings.push(format!(
                            "{code}: multiple halls allocated for {} {} ({:?}); using the first",
                            day.short(),
                            slot.label(),
                            halls.iter().map(|(h, _)| h.clone()).collect::<Vec<_>>()
                        ));
                    }
                    (Some(halls[0].0.clone()), halls[0].1)
                }
                None => {
                    // The two pages are edited independently, so their slot
                    // times can drift by a few minutes. Before giving up,
                    // take the nearest OVERLAPPING booking for the same
                    // course and day (and mark it consumed so it isn't
                    // re-reported as an ignored extra).
                    let overlap = hall_by_key
                        .iter()
                        .filter(|((c, d, s), _)| {
                            c == code && d == day && s.overlaps(slot)
                        })
                        .min_by_key(|((_, _, s), _)| s.start_min.abs_diff(slot.start_min))
                        .map(|(k, v)| (k.clone(), v.clone()));
                    match overlap {
                        Some((k, halls)) => {
                            warnings.push(format!(
                                "{code}: hall matched by overlap for {} — pages say {} vs {}",
                                day.short(),
                                k.2.label(),
                                slot.label()
                            ));
                            consumed.insert(k);
                            (Some(halls[0].0.clone()), halls[0].1)
                        }
                        None => {
                            warnings.push(format!(
                                "{code}: no hall found for {} {} — shown as Hall TBA",
                                day.short(),
                                slot.label()
                            ));
                            stats.meetings_without_hall += 1;
                            (None, false)
                        }
                    }
                }
            };
            meetings.push(crate::model::Meeting {
                day: *day,
                slot: *slot,
                hall,
                temp_booking: temp,
            });
        }

        // Hall-grid entries not matched by any branch-grid meeting.
        let extra: Vec<(Day, Slot)> = hall_by_key
            .keys()
            .filter(|(c, d, s)| c == code && !consumed.contains(&(c.clone(), *d, *s)))
            .map(|(_, d, s)| (*d, *s))
            .collect();

        let status = if !b.grid_meetings.is_empty() {
            if !extra.is_empty() {
                for (d, s) in &extra {
                    warnings.push(format!(
                        "{code}: hall grid lists an extra booking at {} {} that no branch grid mentions; ignored",
                        d.short(),
                        s.label()
                    ));
                }
            }
            ScheduleStatus::Scheduled
        } else if !extra.is_empty() {
            // Scheduled in the hall grid but in no branch grid (e.g. DSEM).
            for (d, s) in &extra {
                let key = (code.clone(), *d, *s);
                let halls = &hall_by_key[&key];
                consumed.insert(key.clone());
                meetings.push(crate::model::Meeting {
                    day: *d,
                    slot: *s,
                    hall: Some(halls[0].0.clone()),
                    temp_booking: halls[0].1,
                });
            }
            ScheduleStatus::ScheduledNoBranch
        } else {
            ScheduleStatus::UnscheduledListed
        };

        meetings.sort_by_key(|m| (m.day.index(), m.slot.start_min, m.slot.end_min));
        stats.meetings_total += meetings.len();

        courses.push(Course {
            code: code.clone(),
            name,
            instructors: instructors_raw
                .as_deref()
                .map(split_instructors)
                .unwrap_or_default(),
            branches: b.branches.clone(),
            credits: notes.credits,
            starts: notes.starts,
            part_of_semester: notes.part_of_semester,
            optional_flag: b.optional,
            status,
            meetings,
        });
    }

    // 6. Canonical slot grid: from the branch sections (verify agreement),
    // cross-checked against the halls page.
    let mut slot_grid: Vec<Slot> = Vec::new();
    for section in &tt.sections {
        if slot_grid.is_empty() {
            slot_grid = section.slots.clone();
        } else if slot_grid != section.slots {
            warnings.push(format!(
                "branch {} uses different slot columns than earlier branches; using the union",
                section.branch.code
            ));
            for s in &section.slots {
                if !slot_grid.contains(s) {
                    slot_grid.push(*s);
                }
            }
            slot_grid.sort_by_key(|s| (s.start_min, s.end_min));
        }
    }
    if slot_grid.is_empty() {
        slot_grid = hp.slots.clone();
    } else if !hp.slots.is_empty() && slot_grid != hp.slots {
        warnings.push(
            "the timetable and hall pages disagree on slot columns; using the timetable's"
                .to_string(),
        );
    }

    // 7. Stats for the validation gate.
    let mut grid_codes: BTreeSet<String> = BTreeSet::new();
    for section in &tt.sections {
        for occ in &section.occurrences {
            grid_codes.insert(occ.code.clone());
        }
    }
    for entry in &hp.entries {
        for code in &entry.codes {
            grid_codes.insert(code.clone());
        }
    }
    let mut legend_codes: BTreeSet<&str> = BTreeSet::new();
    for section in &tt.sections {
        for e in &section.legend {
            legend_codes.insert(&e.code);
        }
    }
    for e in &hp.legend {
        legend_codes.insert(&e.code);
    }
    stats.grid_codes = grid_codes.len();
    stats.grid_codes_resolved = grid_codes
        .iter()
        .filter(|c| legend_codes.contains(c.as_str()))
        .count();
    stats.branch_grids = tt.sections.len();
    stats.branch_legends = tt.sections.iter().filter(|s| !s.legend.is_empty()).count();
    stats.unique_courses = courses.len();
    stats.halls = hp.halls.len();
    stats.hall_days = hp.days.len();

    Joined {
        branches: tt.sections.iter().map(|s| s.branch.clone()).collect(),
        courses,
        halls: hp.halls.clone(),
        slot_grid,
        hall_bookings: hp
            .entries
            .iter()
            .map(|e| crate::model::HallBooking {
                hall: e.hall.clone(),
                day: e.day,
                slot: e.slot,
                codes: e.codes.clone(),
                temp: e.temp,
            })
            .collect(),
        stats,
        warnings,
    }
}
