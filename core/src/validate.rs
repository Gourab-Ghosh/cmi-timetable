//! The validation gate: a freshly fetched snapshot may replace cached data
//! only if every rule passes. On failure the old cache stays untouched.

use crate::join::{join_pages, Joined};
use crate::model::{
    GateCheck, ParseReport, RawHtml, Snapshot, SourceTier, PARSER_VERSION,
};
use crate::parse::{parse_halls_page, parse_timetable_page, PreBlock};

pub struct SnapshotMeta {
    /// Milliseconds since the Unix epoch.
    pub fetched_at: f64,
    pub source: SourceTier,
    /// Uncompressed raw HTML of (timetable.php, lecturehalls.php); stored
    /// compressed inside the snapshot when present.
    pub raw_html: Option<(String, String)>,
}

pub struct ParseOutcome {
    /// `Some` only when the validation gate passed.
    pub snapshot: Option<Snapshot>,
    pub report: ParseReport,
}

fn norm_label(label: &str) -> String {
    label
        .to_ascii_lowercase()
        .replace('\u{2013}', "--")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse both pages' pre-extracted `<pre>` blocks, join them, run the
/// validation gate, and produce a report.
pub fn parse_and_validate(
    tt_blocks: &[PreBlock],
    hall_blocks: &[PreBlock],
    meta: SnapshotMeta,
) -> ParseOutcome {
    let tt = parse_timetable_page(tt_blocks);
    let hp = parse_halls_page(hall_blocks);
    let joined = join_pages(&tt, &hp);

    let mut report = ParseReport {
        stats: joined.stats.clone(),
        warnings: Vec::new(),
        errors: Vec::new(),
        gate: Vec::new(),
        classifications: Vec::new(),
    };
    report.warnings.extend(tt.warnings.iter().cloned());
    report.warnings.extend(hp.warnings.iter().cloned());
    report.warnings.extend(joined.warnings.iter().cloned());
    report.classifications.extend(tt.classifications.iter().cloned());
    report.classifications.extend(hp.classifications.iter().cloned());

    run_gate(&tt.semester_label, &hp.semester_label, &joined, &mut report);
    report.gate.push(per_grid_checks(&tt));

    let snapshot = if report.gate_passed() {
        Some(Snapshot {
            semester_label: tt
                .semester_label
                .clone()
                .or_else(|| hp.semester_label.clone())
                .unwrap_or_default(),
            fetched_at: meta.fetched_at,
            source: meta.source,
            parser_version: PARSER_VERSION,
            branches: joined.branches,
            courses: joined.courses,
            halls: joined.halls,
            slot_grid: joined.slot_grid,
            hall_bookings: joined.hall_bookings,
            raw_html_gz: meta.raw_html.map(|(tt_html, halls_html)| RawHtml {
                timetable_b64: crate::rawhtml::compress_to_b64(&tt_html),
                lecturehalls_b64: crate::rawhtml::compress_to_b64(&halls_html),
            }),
        })
    } else {
        for check in report.gate.iter().filter(|c| !c.passed) {
            report
                .errors
                .push(format!("gate rule failed — {}: {}", check.rule, check.detail));
        }
        None
    };

    ParseOutcome { snapshot, report }
}

fn run_gate(
    tt_label: &Option<String>,
    hp_label: &Option<String>,
    joined: &Joined,
    report: &mut ParseReport,
) {
    // Rule 1 — semester label. The spec expects the label on both pages, but
    // the live lecturehalls.php (as of Aug 2026) carries no label at all, so
    // a missing halls label is warn-only; a *conflicting* label is an error.
    let rule1 = match (tt_label, hp_label) {
        (Some(t), Some(h)) => {
            if t == h {
                GateCheck {
                    rule: "semester label".into(),
                    passed: true,
                    detail: format!("both pages say {t:?}"),
                }
            } else if norm_label(t) == norm_label(h) {
                report.warnings.push(format!(
                    "semester labels differ only in case/whitespace: {t:?} vs {h:?}"
                ));
                GateCheck {
                    rule: "semester label".into(),
                    passed: true,
                    detail: format!("{t:?} ≈ {h:?}"),
                }
            } else {
                GateCheck {
                    rule: "semester label".into(),
                    passed: false,
                    detail: format!("pages disagree: {t:?} vs {h:?}"),
                }
            }
        }
        (Some(t), None) => {
            report.warnings.push(
                "no semester label found on the lecture-halls page; using the timetable's"
                    .to_string(),
            );
            GateCheck {
                rule: "semester label".into(),
                passed: true,
                detail: format!("timetable page says {t:?} (halls page has none)"),
            }
        }
        (None, Some(h)) => {
            report.warnings.push(
                "no semester label found on the timetable page; using the halls page's"
                    .to_string(),
            );
            GateCheck {
                rule: "semester label".into(),
                passed: true,
                detail: format!("halls page says {h:?} (timetable page has none)"),
            }
        }
        (None, None) => GateCheck {
            rule: "semester label".into(),
            passed: false,
            detail: "no semester label found on either page".into(),
        },
    };
    report.gate.push(rule1);

    // Rule 2 — enough branch grids (per-grid substance is a separate check,
    // appended by the caller via `per_grid_checks`).
    let grids = joined.stats.branch_grids;
    report.gate.push(GateCheck {
        rule: "branch grid count".into(),
        passed: grids >= 10,
        detail: format!("{grids} branch grids parsed (need ≥ 10)"),
    });

    // Rule 3 — enough courses.
    report.gate.push(GateCheck {
        rule: "course count".into(),
        passed: joined.stats.unique_courses >= 40,
        detail: format!(
            "{} unique courses after merging legends (need ≥ 40)",
            joined.stats.unique_courses
        ),
    });

    // Rule 4 — grid codes resolve to legend entries.
    let (total, resolved) = (joined.stats.grid_codes, joined.stats.grid_codes_resolved);
    let ok = total == 0 || resolved * 10 >= total * 9;
    report.gate.push(GateCheck {
        rule: "legend resolution".into(),
        passed: ok && total > 0,
        detail: format!("{resolved}/{total} grid codes have a legend entry (need ≥ 90%)"),
    });

    // Rule 5 — hall grid substance.
    report.gate.push(GateCheck {
        rule: "hall grid".into(),
        passed: joined.stats.hall_days >= 4 && joined.stats.halls >= 8,
        detail: format!(
            "{} days and {} halls parsed (need ≥ 4 days, ≥ 8 halls)",
            joined.stats.hall_days, joined.stats.halls
        ),
    });

    // Rule 6 — slots are valid, increasing time ranges.
    let mut slots_ok = !joined.slot_grid.is_empty();
    let mut detail = String::new();
    for s in &joined.slot_grid {
        if s.start_min >= s.end_min {
            slots_ok = false;
            detail = format!("slot {} has a non-positive duration", s.label());
        }
    }
    for pair in joined.slot_grid.windows(2) {
        if pair[0].start_min >= pair[1].start_min {
            slots_ok = false;
            detail = format!(
                "slots {} and {} are not in increasing order",
                pair[0].label(),
                pair[1].label()
            );
        }
    }
    if detail.is_empty() {
        detail = format!(
            "{} slot columns, all valid and increasing",
            joined.slot_grid.len()
        );
    }
    report.gate.push(GateCheck {
        rule: "slot sanity".into(),
        passed: slots_ok,
        detail,
    });
}

/// Extra rule-2 detail: verify every branch grid has ≥ 3 day rows and ≥ 4
/// slot columns. Returns gate checks to append.
pub fn per_grid_checks(tt: &crate::parse::TimetablePage) -> GateCheck {
    let mut bad: Vec<String> = Vec::new();
    for s in &tt.sections {
        if s.days.len() < 3 || s.slots.len() < 4 {
            bad.push(format!(
                "{} ({} day rows, {} slots)",
                s.branch.code,
                s.days.len(),
                s.slots.len()
            ));
        }
    }
    if bad.is_empty() {
        GateCheck {
            rule: "branch grid substance".into(),
            passed: true,
            detail: "every branch grid has ≥ 3 day rows and ≥ 4 slots".into(),
        }
    } else {
        GateCheck {
            rule: "branch grid substance".into(),
            passed: false,
            detail: format!("too thin: {}", bad.join(", ")),
        }
    }
}
