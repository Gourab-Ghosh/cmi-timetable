//! Page-level parsing: classify `<pre>` blocks by content and turn the two
//! CMI pages into intermediate structures that `join` merges into a Snapshot.
//!
//! The input is a list of pre-extracted `<pre>` texts (plus the nearest
//! preceding heading text) so the same logic runs under wasm (DOMParser
//! extraction in /app) and natively (scraper extraction in the tests).

use crate::model::{Branch, Day, PreClassification, Slot};
use crate::textgrid::{RawGrid, RawRow, parse_cell, parse_grid};
use regex_lite::Regex;
use std::sync::LazyLock;

/// One `<pre>` block's raw text plus the nearest preceding heading text.
#[derive(Debug, Clone, Default)]
pub struct PreBlock {
    pub text: String,
    pub heading: String,
}

impl PreBlock {
    pub fn new(text: impl Into<String>, heading: impl Into<String>) -> PreBlock {
        PreBlock {
            text: text.into(),
            heading: heading.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreKind {
    BranchGrid,
    HallGrid,
    Legend,
    Other,
}

impl PreKind {
    pub fn label(&self) -> &'static str {
        match self {
            PreKind::BranchGrid => "branch grid",
            PreKind::HallGrid => "hall grid",
            PreKind::Legend => "legend",
            PreKind::Other => "other",
        }
    }
}

// Deliberately loose: case-insensitive, "Time Table"/"Timetable", the
// "for"/"of" connective optional — CMI can reword the heading any semester.
//
// The label ends at the first year, EXCEPT when a second month and year
// follow a dash: a term that crosses New Year is written with both years
// ("December 2026--March 2027"), and stopping at the first would keep only
// half of it — a truncated label the halls page would then contradict.
static SEMESTER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)time\s*-?\s*table\s+(?:(?:for|of)\s+)?(.+?\d{4}(?:\s*(?:--|[-\u{2013}\u{2014}])\s*[A-Za-z.]+\s+\d{4})?)",
    )
    .unwrap()
});

// Fallback signal when the phrasing changes entirely: a short line carrying
// a plausible calendar year.
static YEAR_LINE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:19|20)\d{2}\b").unwrap());

// Codes are 2–5 chars today; {1,12} leaves generous headroom so a longer
// future code isn't silently dropped from the legend.
static LEGEND_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\S{1,12})\s*:\s*(.*)$").unwrap());

pub fn find_semester_label(text: &str) -> Option<String> {
    SEMESTER_RE
        .captures(text)
        .map(|c| c.get(1).unwrap().as_str().trim().to_string())
}

/// Classify a `<pre>` by content, never by DOM position or CSS.
///
/// Grid kind is decided by COMPOSITION, not by which spelling of the day
/// name appears (a page edit from "Mon" to "Monday" must not flip the
/// kind): in a branch grid the day-labeled rows carry the cell data; in the
/// hall grid the day lines are bare section headers and the data lives in
/// the (more numerous) hall-labeled rows beneath them.
pub fn classify(text: &str) -> (PreKind, Option<RawGrid>) {
    if let Some(grid) = parse_grid(text) {
        let has_data = |r: &&RawRow| r.cells.iter().any(|c| !c.trim().is_empty());
        let day_rows = grid
            .rows
            .iter()
            .filter(|r| Day::from_label(&r.label).is_some())
            .count();
        if day_rows == 0 {
            return (PreKind::Other, None);
        }
        let day_data_rows = grid
            .rows
            .iter()
            .filter(|r| Day::from_label(&r.label).is_some())
            .filter(has_data)
            .count();
        let non_day_data_rows = grid
            .rows
            .iter()
            .filter(|r| !r.label.is_empty() && Day::from_label(&r.label).is_none())
            .filter(has_data)
            .count();
        // Whoever carries MORE of the data names the kind; ties go to the
        // day rows, so a stray non-day row in a branch grid (skipped with a
        // warning downstream) can't flip it, while one accidental cell on a
        // hall grid's day line (warned + ignored downstream) can't either.
        if day_data_rows < non_day_data_rows {
            return (PreKind::HallGrid, Some(grid));
        }
        return (PreKind::BranchGrid, Some(grid));
    }
    let non_empty: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    // Decoration lines ("* Note …", "+ Optional course.") are neither
    // legend entries nor evidence against a legend — keep them out of the
    // ratio's denominator so a short legend with a footnote still counts.
    let substantive: Vec<&str> = non_empty
        .iter()
        .filter(|l| !matches!(l.trim_start().chars().next(), Some('*' | '+')))
        .copied()
        .collect();
    if !substantive.is_empty() {
        let matching = substantive
            .iter()
            .filter(|l| LEGEND_LINE_RE.is_match(l))
            .count();
        let single_clean = matching == 1 && non_empty.len() <= 2 && {
            // One-line legends exist (a branch with one course): accept a
            // lone match only when the code LOOKS like a course code —
            // all caps/digits ("TOC", "C01"), never a prose word, so a
            // stray "Note: rooms may change" block can't become a course.
            substantive.iter().any(|l| {
                LEGEND_LINE_RE.captures(l).is_some_and(|c| {
                    let code = c.get(1).unwrap().as_str();
                    let name = c.get(2).unwrap().as_str();
                    code.chars()
                        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
                        && !name.trim().is_empty()
                })
            })
        };
        if (matching >= 2 && matching * 10 >= substantive.len() * 6) || single_clean {
            return (PreKind::Legend, None);
        }
    }
    (PreKind::Other, None)
}

/// A legend entry from either page.
#[derive(Debug, Clone, PartialEq)]
pub struct LegendEntry {
    pub code: String,
    pub name: String,
    pub instructors_raw: Option<String>,
}

static TWO_SPACES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}").unwrap());

/// Parse one line of the timetable page's legend:
/// `CM1 : Classical Mechanics I(starts 12 Aug)        K G Arun`
/// Name and instructor are separated by a run of ≥2 spaces in the raw text;
/// if no such run exists the whole remainder is the name (the halls legend
/// supplies the instructor). This format is ambiguous in edge cases, which
/// is why the halls legend is canonical.
pub fn parse_timetable_legend_line(line: &str) -> Option<LegendEntry> {
    let caps = LEGEND_LINE_RE.captures(line)?;
    let code = caps.get(1).unwrap().as_str().to_string();
    let rest = caps.get(2).unwrap().as_str();
    match TWO_SPACES_RE.find(rest) {
        Some(m) => {
            let name = rest[..m.start()].trim().to_string();
            let instr = rest[m.end()..].trim().to_string();
            Some(LegendEntry {
                code,
                name,
                instructors_raw: if instr.is_empty() { None } else { Some(instr) },
            })
        }
        None => Some(LegendEntry {
            code,
            name: rest.trim().to_string(),
            instructors_raw: None,
        }),
    }
}

// The middle group is GREEDY so the instructor splits off at the LAST
// colon: instructor lists never contain colons, but course names plausibly
// do ("Topics: Quantum Information").
static HALLS_LEGEND_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\S{1,12})\s*:\s*(.*)\s*:\s*(.*)$").unwrap());

/// Parse one line of the halls page's colon-separated legend:
/// `RFLR : Reinforcement Learning : I Murugeswari`
/// Falls back to the two-space format when only one colon is present.
pub fn parse_halls_legend_line(line: &str) -> Option<LegendEntry> {
    if let Some(caps) = HALLS_LEGEND_RE.captures(line) {
        let instr = caps.get(3).unwrap().as_str().trim().to_string();
        return Some(LegendEntry {
            code: caps.get(1).unwrap().as_str().to_string(),
            name: caps.get(2).unwrap().as_str().trim().to_string(),
            instructors_raw: if instr.is_empty() { None } else { Some(instr) },
        });
    }
    parse_timetable_legend_line(line)
}

// ---------------------------------------------------------------------------
// Timetable page
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Occurrence {
    pub day: Day,
    pub slot: Slot,
    pub code: String,
    pub optional: bool,
}

#[derive(Debug, Clone)]
pub struct BranchSection {
    pub branch: Branch,
    pub slots: Vec<Slot>,
    /// Distinct day rows seen in the grid, in row order.
    pub days: Vec<Day>,
    pub occurrences: Vec<Occurrence>,
    pub legend: Vec<LegendEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct TimetablePage {
    pub semester_label: Option<String>,
    pub sections: Vec<BranchSection>,
    pub warnings: Vec<String>,
    pub classifications: Vec<PreClassification>,
}

fn classification_for(page: &str, index: usize, kind: PreKind, text: &str) -> PreClassification {
    let first_line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(80)
        .collect();
    PreClassification {
        page: page.to_string(),
        index,
        kind: kind.label().to_string(),
        first_line,
        line_count: text.lines().count(),
    }
}

pub fn parse_timetable_page(blocks: &[PreBlock]) -> TimetablePage {
    let mut page = TimetablePage::default();

    for (i, block) in blocks.iter().enumerate() {
        let (kind, grid) = classify(&block.text);
        page.classifications
            .push(classification_for("timetable", i, kind, &block.text));

        match kind {
            PreKind::BranchGrid => {
                let grid = grid.unwrap();
                page.warnings.extend(grid.warnings.iter().cloned());

                // Semester label: leading lines inside the <pre>, else the
                // nearest preceding heading.
                let label = grid
                    .leading
                    .iter()
                    .find_map(|l| find_semester_label(l))
                    .or_else(|| find_semester_label(&block.heading));
                if let Some(label) = label {
                    match &page.semester_label {
                        None => page.semester_label = Some(label),
                        Some(existing) if *existing != label => page.warnings.push(format!(
                            "semester label differs between sections: {existing:?} vs {label:?}"
                        )),
                        _ => {}
                    }
                }

                let code = if grid.label0.is_empty() {
                    page.warnings
                        .push(format!("branch grid #{i} has an empty header label"));
                    format!("BRANCH{i}")
                } else {
                    grid.label0.clone()
                };

                // Branch title: last leading line that is not the semester
                // label; heading text as fallback; the code as a last resort.
                let title = grid
                    .leading
                    .iter()
                    .rfind(|l| find_semester_label(l).is_none())
                    .cloned()
                    .or_else(|| {
                        let h = block.heading.trim();
                        (!h.is_empty()).then(|| h.to_string())
                    })
                    .unwrap_or_else(|| code.clone());

                let mut days = Vec::new();
                let mut occurrences = Vec::new();
                for row in &grid.rows {
                    let Some(day) = Day::from_label(&row.label) else {
                        page.warnings.push(format!(
                            "branch {code}: row label {:?} is not a day; row skipped",
                            row.label
                        ));
                        continue;
                    };
                    if !days.contains(&day) {
                        days.push(day);
                    }
                    for (cell, slot) in row.cells.iter().zip(grid.slots.iter()) {
                        let tokens = parse_cell(cell);
                        if tokens.temp {
                            page.warnings.push(format!(
                                "branch {code}: unexpected TMP* marker in a branch grid cell"
                            ));
                        }
                        for (course, plus) in tokens.codes {
                            occurrences.push(Occurrence {
                                day,
                                slot: *slot,
                                code: course,
                                optional: plus,
                            });
                        }
                    }
                }

                page.sections.push(BranchSection {
                    branch: Branch { code, title },
                    slots: grid.slots.clone(),
                    days,
                    occurrences,
                    legend: Vec::new(),
                });
            }
            PreKind::Legend => {
                let entries: Vec<LegendEntry> = block
                    .text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .filter_map(parse_timetable_legend_line)
                    .collect();
                match page.sections.last_mut() {
                    Some(section) => section.legend.extend(entries),
                    None => page
                        .warnings
                        .push(format!("legend block #{i} appears before any branch grid")),
                }
            }
            PreKind::HallGrid => {
                page.warnings.push(format!(
                    "unexpected hall-style grid (block #{i}) on the timetable page; ignored"
                ));
            }
            PreKind::Other => {}
        }
    }

    // Fallback when CMI rewords the heading beyond what SEMESTER_RE covers:
    // any short heading/leading line carrying a calendar year beats having
    // no label (it is display-only metadata). Timetable page only — a wrong
    // guess here must not manufacture a label CONFLICT with the halls page.
    if page.semester_label.is_none() {
        let year_line = |l: &str| {
            let t = l.trim();
            (!t.is_empty() && t.len() <= 80 && YEAR_LINE_RE.is_match(t)).then(|| t.to_string())
        };
        let fallback = blocks.iter().find_map(|b| {
            year_line(&b.heading).or_else(|| b.text.lines().take(5).find_map(&year_line))
        });
        if let Some(label) = fallback {
            page.warnings.push(format!(
                "semester heading did not match the usual \"Timetable for …\" \
                 phrasing; using {label:?} from the page as the label"
            ));
            page.semester_label = Some(label);
        }
    }

    page
}

// ---------------------------------------------------------------------------
// Lecture halls page
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HallEntry {
    pub day: Day,
    pub slot: Slot,
    pub hall: String,
    pub codes: Vec<String>,
    pub temp: bool,
}

#[derive(Debug, Clone, Default)]
pub struct HallsPage {
    pub semester_label: Option<String>,
    pub slots: Vec<Slot>,
    /// In grid order.
    pub halls: Vec<String>,
    pub entries: Vec<HallEntry>,
    pub legend: Vec<LegendEntry>,
    pub days: Vec<Day>,
    pub footnotes: Vec<String>,
    pub warnings: Vec<String>,
    pub classifications: Vec<PreClassification>,
}

pub fn parse_halls_page(blocks: &[PreBlock]) -> HallsPage {
    let mut page = HallsPage::default();

    for (i, block) in blocks.iter().enumerate() {
        let (kind, grid) = classify(&block.text);
        page.classifications
            .push(classification_for("lecturehalls", i, kind, &block.text));

        match kind {
            PreKind::HallGrid => {
                let grid = grid.unwrap();
                page.warnings.extend(grid.warnings.iter().cloned());
                page.footnotes.extend(grid.trailing.iter().cloned());

                if page.semester_label.is_none() {
                    page.semester_label = grid
                        .leading
                        .iter()
                        .find_map(|l| find_semester_label(l))
                        .or_else(|| find_semester_label(&block.heading));
                }

                if page.slots.is_empty() {
                    page.slots = grid.slots.clone();
                } else if page.slots != grid.slots {
                    page.warnings.push(
                        "hall grid blocks disagree on slot columns; using the first block's"
                            .to_string(),
                    );
                }

                let mut current_day: Option<Day> = None;
                for row in &grid.rows {
                    if let Some(day) = Day::from_label(&row.label) {
                        current_day = Some(day);
                        if !page.days.contains(&day) {
                            page.days.push(day);
                        }
                        if row.cells.iter().any(|c| !c.trim().is_empty()) {
                            page.warnings.push(format!(
                                "hall grid: day row {} unexpectedly has cell content; ignored",
                                day.full()
                            ));
                        }
                        continue;
                    }
                    if row.label.is_empty() {
                        if row.cells.iter().any(|c| !c.trim().is_empty()) {
                            page.warnings.push(
                                "hall grid: row with empty hall name has cell content; ignored"
                                    .to_string(),
                            );
                        }
                        continue;
                    }
                    let Some(day) = current_day else {
                        page.warnings.push(format!(
                            "hall grid: hall row {:?} appears before any day line; skipped",
                            row.label
                        ));
                        continue;
                    };
                    let hall = row.label.clone();
                    if !page.halls.contains(&hall) {
                        page.halls.push(hall.clone());
                    }
                    for (cell, slot) in row.cells.iter().zip(grid.slots.iter()) {
                        let tokens = parse_cell(cell);
                        if tokens.codes.is_empty() && !tokens.temp {
                            continue;
                        }
                        page.entries.push(HallEntry {
                            day,
                            slot: *slot,
                            hall: hall.clone(),
                            codes: tokens.codes.into_iter().map(|(c, _)| c).collect(),
                            temp: tokens.temp,
                        });
                    }
                }
            }
            PreKind::Legend => {
                let entries: Vec<LegendEntry> = block
                    .text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .filter_map(parse_halls_legend_line)
                    .collect();
                page.legend.extend(entries);
            }
            PreKind::BranchGrid => {
                page.warnings.push(format!(
                    "unexpected branch-style grid (block #{i}) on the halls page; ignored"
                ));
            }
            PreKind::Other => {}
        }
    }

    page
}
