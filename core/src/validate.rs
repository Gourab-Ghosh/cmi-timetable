//! The validation gate: a freshly fetched snapshot may replace cached data
//! only if every rule passes. On failure the old cache stays untouched.

use crate::join::{Joined, join_pages};
use crate::model::{GateCheck, PARSER_VERSION, ParseReport, RawHtml, Snapshot, SourceTier};
use crate::parse::{PreBlock, parse_halls_page, parse_timetable_page};

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
    // Any run of dash characters compares equal — "August--November",
    // "August–November" and "August-November" are the same label.
    let mut out = String::with_capacity(label.len());
    let mut prev_dash = false;
    for c in label.to_ascii_lowercase().chars() {
        let is_dash = matches!(c, '-' | '\u{2012}' | '\u{2013}' | '\u{2014}');
        if is_dash {
            if !prev_dash {
                out.push('-');
            }
        } else {
            out.push(c);
        }
        prev_dash = is_dash;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The meaning of a semester label: its month set and year. Lets rule 1
/// accept two pages that PHRASE the same semester differently ("Aug--Nov
/// 2026" vs "August-November 2026") while still failing when they genuinely
/// name different terms — the real stale-page signal.
fn label_semantics(label: &str) -> Option<(Vec<u8>, i32)> {
    let mut months: Vec<u8> = Vec::new();
    let mut year: Option<i32> = None;
    for token in label.split(|c: char| !c.is_ascii_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        if let Some(m) = crate::date::month_from_word(token) {
            if !months.contains(&m) {
                months.push(m);
            }
        } else if let Ok(y) = token.parse::<i32>()
            && (1900..2200).contains(&y)
        {
            year = Some(y);
        }
    }
    months.sort_unstable();
    match (months.is_empty(), year) {
        (false, Some(y)) => Some((months, y)),
        _ => None,
    }
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
        branch_stats: tt
            .sections
            .iter()
            .map(|s| crate::model::BranchStat {
                code: s.branch.code.clone(),
                title: s.branch.title.clone(),
                day_rows: s.days.len(),
                slots: s.slots.len(),
                occurrences: s.occurrences.len(),
                legend_entries: s.legend.len(),
            })
            .collect(),
    };
    report.warnings.extend(tt.warnings.iter().cloned());
    report.warnings.extend(hp.warnings.iter().cloned());
    report.warnings.extend(joined.warnings.iter().cloned());
    report
        .classifications
        .extend(tt.classifications.iter().cloned());
    report
        .classifications
        .extend(hp.classifications.iter().cloned());

    run_gate(&tt.semester_label, &hp.semester_label, &joined, &mut report);
    report.gate.push(per_grid_checks(&tt));
    report.gate.push(halls_page_completeness(&tt, &hp, &joined));

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
            report.errors.push(format!(
                "gate rule failed — {}: {}",
                check.rule, check.detail
            ));
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
            } else if label_semantics(t).is_some() && label_semantics(t) == label_semantics(h) {
                // Same months, same year, different phrasing (independently
                // edited pages) — pass with a warning. If semantics can't be
                // extracted from BOTH labels, fall through to the hard fail:
                // never pass-by-default.
                report.warnings.push(format!(
                    "semester labels are phrased differently but name the same \
                     term: {t:?} vs {h:?}"
                ));
                GateCheck {
                    rule: "semester label".into(),
                    passed: true,
                    detail: format!("{t:?} ≈ {h:?} (same months and year)"),
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
                "no semester label found on the timetable page; using the halls page's".to_string(),
            );
            GateCheck {
                rule: "semester label".into(),
                passed: true,
                detail: format!("halls page says {h:?} (timetable page has none)"),
            }
        }
        (None, None) => {
            // The label is display-only metadata; rules 2–6 guard the data
            // itself. Rewording of the heading must not block a fresh
            // semester, so this is warn-only — CONFLICTING labels still fail.
            report
                .warnings
                .push("no semester label found on either page; continuing without one".to_string());
            GateCheck {
                rule: "semester label".into(),
                passed: true,
                detail: "no label on either page (warn only — labels only fail \
                         this rule when the two pages disagree)"
                    .into(),
            }
        }
    };
    report.gate.push(rule1);

    // Rule 2 — enough branch grids (per-grid substance is a separate check,
    // appended by the caller via `per_grid_checks`). The floors here are
    // garbage detectors, not semester-size estimates: an error page or a
    // half-rendered fetch parses to zeros, while a legitimately small term
    // (a January minisemester) must not be rejected for being small — the
    // scale-free rules (legend resolution, per-grid substance, slot sanity)
    // carry the data-quality burden.
    let grids = joined.stats.branch_grids;
    report.gate.push(GateCheck {
        rule: "branch grid count".into(),
        passed: grids >= 3,
        detail: format!("{grids} branch grids parsed (need ≥ 3)"),
    });

    // Rule 3 — enough courses.
    report.gate.push(GateCheck {
        rule: "course count".into(),
        passed: joined.stats.unique_courses >= 10,
        detail: format!(
            "{} unique courses after merging legends (need ≥ 10)",
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

    // Rule 5 — hall grid substance (same garbage-detection sizing as
    // rules 2–3).
    report.gate.push(GateCheck {
        rule: "hall grid".into(),
        passed: joined.stats.hall_days >= 3 && joined.stats.halls >= 3,
        detail: format!(
            "{} days and {} halls parsed (need ≥ 3 days, ≥ 3 halls)",
            joined.stats.hall_days, joined.stats.halls
        ),
    });

    // Rule 7 (order kept after rule 5 for display) — cross-page
    // consistency: courses scheduled ONLY via the hall grid
    // (ScheduledNoBranch) are legitimate in ones and twos (DSEM), but when
    // a large share of the schedule has no branch grid behind it, the
    // timetable page was truncated mid-transfer while the halls page
    // survived — a shape the count floors can't catch at any size.
    let scheduled_total = joined
        .courses
        .iter()
        .filter(|c| !c.meetings.is_empty())
        .count();
    let no_branch = joined
        .courses
        .iter()
        .filter(|c| c.status == crate::model::ScheduleStatus::ScheduledNoBranch)
        .count();
    report.gate.push(GateCheck {
        rule: "cross-page consistency".into(),
        passed: no_branch * 4 <= scheduled_total,
        detail: format!(
            "{no_branch}/{scheduled_total} scheduled courses appear in no branch grid \
             (need ≤ 25% — more means a truncated timetable page)",
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
        // Lexicographic (start, end): two branches can legitimately share a
        // start with different end times (join keeps the union), but exact
        // duplicates and decreasing order are still parse garbage.
        if (pair[0].start_min, pair[0].end_min) >= (pair[1].start_min, pair[1].end_min) {
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

/// Rule 8 — the halls page arrived whole.
///
/// Rule 7 catches a truncated TIMETABLE page; nothing was symmetric for the
/// halls page, which fails in a quieter way. A transfer cut short still
/// parses: the day sections simply stop, so every class after the cut keeps
/// its time and loses its room, while the count floors (≥ 3 days, ≥ 3
/// halls) stay satisfied and the snapshot replaces the good cached one. A
/// measured 50 % cut of the live page left 60 of 146 classes reading "Hall
/// TBA" with the gate perfectly happy.
///
/// The signal is a whole DAY that the timetable schedules and the halls page
/// never mentions — weighed by how much of the week it carries, so the rule
/// is scale-free and cannot fire on the innocent version of the same shape:
/// one Saturday make-up class with no room listed is 1 meeting in 150, while
/// a lost Friday is 24. Below that (a cut landing inside the last day) some
/// classes simply read "Hall TBA", which is what the page now says, not
/// something invented.
fn halls_page_completeness(
    tt: &crate::parse::TimetablePage,
    hp: &crate::parse::HallsPage,
    joined: &Joined,
) -> GateCheck {
    let missing: std::collections::BTreeSet<crate::model::Day> = tt
        .sections
        .iter()
        .flat_map(|s| s.days.iter().copied())
        .filter(|d| !hp.days.contains(d))
        .collect();
    let total = joined.stats.meetings_total;
    let stranded = joined
        .courses
        .iter()
        .flat_map(|c| &c.meetings)
        .filter(|m| missing.contains(&m.day))
        .count();

    let named: Vec<&str> = missing.iter().map(|d| d.short()).collect();
    let passed = missing.is_empty() || total == 0 || stranded * 10 < total;
    GateCheck {
        rule: "halls page completeness".into(),
        passed,
        detail: if missing.is_empty() {
            format!("the halls page covers every day the timetable schedules ({total} classes)")
        } else if passed {
            format!(
                "the halls page never mentions {}, but only {stranded} of {total} classes                  meet then — a room CMI has not allocated, not a missing page",
                named.join(", ")
            )
        } else {
            format!(
                "the halls page never mentions {}, where {stranded} of {total} classes                  meet — it looks cut short",
                named.join(", ")
            )
        },
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_normalize_dashes() {
        assert_eq!(norm_label("Aug–Nov 2026"), norm_label("aug-nov 2026"));
        assert_eq!(norm_label("Aug--Nov 2026"), norm_label("Aug-Nov 2026"));
        assert_eq!(
            norm_label("Aug\u{2014}Nov  2026"),
            norm_label("AUG-NOV 2026")
        );
        assert_ne!(norm_label("Aug-Nov 2026"), norm_label("Aug-Nov 2027"));
    }

    #[test]
    fn label_semantics_compare_terms_not_phrasing() {
        let s = label_semantics;
        assert_eq!(
            s("Timetable for August--November 2026"),
            s("aug to nov 2026")
        );
        assert_eq!(s("August--November 2026"), Some((vec![8, 11], 2026)));
        // Different year or different months: genuinely different terms.
        assert_ne!(s("Aug-Nov 2026"), s("Aug-Nov 2027"));
        assert_ne!(s("Aug-Nov 2026"), s("Jan-Apr 2026"));
        // No months or no year → no semantics (rule 1 then hard-fails on
        // mismatch, never passes by default).
        assert_eq!(s("Timetable 2026"), None);
        assert_eq!(s("August--November"), None);
    }
}
