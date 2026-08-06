//! Parser tests 1–11 from the build spec, run against the committed fixtures
//! (fetched from cmi.ac.in on 5 Aug 2026).

use cmi_timetable_core::extract::{extract_pre_blocks, parse_html_pages};
use cmi_timetable_core::join::join_pages;
use cmi_timetable_core::model::{Day, ScheduleStatus, Slot, Snapshot, SourceTier};
use cmi_timetable_core::parse::{parse_halls_page, parse_timetable_page};
use std::sync::LazyLock;

const TT: &str = include_str!("../fixtures/timetable.php.html");
const HALLS: &str = include_str!("../fixtures/lecturehalls.php.html");

static SNAPSHOT: LazyLock<Snapshot> = LazyLock::new(|| {
    let out = parse_html_pages(TT, HALLS, 0.0, SourceTier::Bundled, false);
    assert!(
        out.report.gate_passed(),
        "validation gate must pass on the fixtures: {:#?}",
        out.report.gate
    );
    out.snapshot.expect("snapshot present when gate passes")
});

fn slot(start: u16, end: u16) -> Slot {
    Slot::new(start, end)
}

/// Test 1 — 18 branches; semester label verbatim (and normalized for display).
#[test]
fn t01_branches_and_semester_label() {
    let snap = &*SNAPSHOT;
    assert_eq!(snap.branches.len(), 18, "branches: {:?}", snap.branches);
    let codes: Vec<&str> = snap.branches.iter().map(|b| b.code.as_str()).collect();
    assert_eq!(
        codes,
        [
            "BM1", "BM2", "BP2", "HUM", "MC1", "MD1", "MD2", "MM1", "MO", "OCS1", "OCS2",
            "OCS3", "OM1", "OM2", "OM3", "OP1", "OP2", "OPDS1"
        ]
    );
    assert_eq!(snap.semester_label, "August--November 2026");
    assert_eq!(snap.semester_label_display(), "August\u{2013}November 2026");
    assert_eq!(snap.branch("BM1").unwrap().title, "B.S I year");
    assert_eq!(snap.branch("OP2").unwrap().title, "Phy Electives [PhD]");
}

/// Test 2 — ≥ 70 unique courses after legend merge.
#[test]
fn t02_course_count() {
    assert!(
        SNAPSHOT.courses.len() >= 70,
        "only {} courses",
        SNAPSHOT.courses.len()
    );
}

/// Test 3 — duplicate codes across branches merge into one course with the
/// union of branches, and the meetings agree between the branches.
#[test]
fn t03_duplicates_merge() {
    let toc = SNAPSHOT.course("TOC").unwrap();
    assert_eq!(toc.branches, ["BM2", "MC1"]);
    let qcom = SNAPSHOT.course("QCOM").unwrap();
    assert_eq!(qcom.branches, ["OCS1", "OP1"]);

    // Meetings must be identical across branches: check at the section level.
    let tt = parse_timetable_page(&extract_pre_blocks(TT));
    let occ = |branch: &str, code: &str| {
        let mut v: Vec<(Day, Slot)> = tt
            .sections
            .iter()
            .find(|s| s.branch.code == branch)
            .unwrap()
            .occurrences
            .iter()
            .filter(|o| o.code == code)
            .map(|o| (o.day, o.slot))
            .collect();
        v.sort();
        v
    };
    assert_eq!(occ("BM2", "TOC"), occ("MC1", "TOC"));
    assert_eq!(occ("OCS1", "QCOM"), occ("OP1", "QCOM"));
    for code in ["ALG3", "ANA2", "CAL2"] {
        let c = SNAPSHOT.course(code).unwrap();
        assert_eq!(c.branches, ["BM2", "BP2"], "{code}");
        assert_eq!(occ("BM2", code), occ("BP2", code), "{code}");
    }
    let netw = SNAPSHOT.course("NETW").unwrap();
    assert_eq!(netw.branches, ["OCS3", "OPDS1"]);
}

/// Test 4 — the halls legend is canonical, resolving the name/instructor
/// ambiguity: "Reinforcement Learning" taught by "I Murugeswari".
#[test]
fn t04_rflr_ambiguity() {
    let rflr = SNAPSHOT.course("RFLR").unwrap();
    assert_eq!(rflr.name, "Reinforcement Learning");
    assert_eq!(rflr.instructors, ["I Murugeswari"]);
}

/// Test 5 — legend-only courses are UnscheduledListed; RDBM carries credits.
#[test]
fn t05_unscheduled_listed() {
    for code in ["RDBM", "SVA", "MATH", "MPML", "CSEM"] {
        let c = SNAPSHOT.course(code).unwrap_or_else(|| panic!("{code} missing"));
        assert_eq!(
            c.status,
            ScheduleStatus::UnscheduledListed,
            "{code} should be unscheduled-listed"
        );
        assert!(c.meetings.is_empty(), "{code} should have no meetings");
    }
    assert_eq!(SNAPSHOT.course("RDBM").unwrap().credits, Some(2));
    assert_eq!(SNAPSHOT.course("RDBM").unwrap().effective_credits(), 2);
    // Unstated credits default to 4 (but stay distinguishable as assumed).
    let toc = SNAPSHOT.course("TOC").unwrap();
    assert_eq!(toc.credits, None);
    assert_eq!(toc.effective_credits(), 4);
    assert!(toc.credits_assumed());
    // Branch membership still comes from the timetable legend.
    assert_eq!(SNAPSHOT.course("RDBM").unwrap().branches, ["MD1"]);
    assert_eq!(SNAPSHOT.course("SVA").unwrap().branches, ["OCS1"]);
    assert_eq!(SNAPSHOT.course("MATH").unwrap().branches, ["OCS2"]);
    assert_eq!(SNAPSHOT.course("MPML").unwrap().branches, ["OCS3"]);
    assert!(SNAPSHOT.course("CSEM").unwrap().branches.is_empty());
}

/// Test 6 — DSEM is in the hall grid but no branch grid: ScheduledNoBranch,
/// Friday 14:00–15:15 at the NKN AV Hall.
#[test]
fn t06_dsem_scheduled_no_branch() {
    let dsem = SNAPSHOT.course("DSEM").unwrap();
    assert_eq!(dsem.status, ScheduleStatus::ScheduledNoBranch);
    assert!(dsem.branches.is_empty());
    assert_eq!(dsem.meetings.len(), 1);
    let m = &dsem.meetings[0];
    assert_eq!(m.day, Day::Fri);
    assert_eq!(m.slot, slot(840, 915));
    assert_eq!(m.hall.as_deref(), Some("NKN AV Hall"));
    assert!(!m.temp_booking);
}

/// Test 7 — the join resolves halls: ENV on Friday 15:30–16:45 is in the
/// Seminar Hall.
#[test]
fn t07_env_hall_join() {
    let env = SNAPSHOT.course("ENV").unwrap();
    let m = env
        .meetings
        .iter()
        .find(|m| m.day == Day::Fri && m.slot == slot(930, 1005))
        .expect("ENV Friday 15:30-16:45 exists");
    assert_eq!(m.hall.as_deref(), Some("Seminar Hall"));
    assert_eq!(env.instructors, ["Speaker", "Movie"]);
}

/// Test 8 — structured note extraction, names kept verbatim.
#[test]
fn t08_name_notes() {
    let cm1 = SNAPSHOT.course("CM1").unwrap();
    assert_eq!(cm1.starts, Some((12, "Aug".to_string())));
    assert_eq!(cm1.name, "Classical Mechanics I(starts 12 Aug)");

    let math = SNAPSHOT.course("MATH").unwrap();
    assert_eq!(math.part_of_semester.as_deref(), Some("Oct-Nov"));

    let prog = SNAPSHOT.course("PROG").unwrap();
    assert_eq!(prog.name, "Introduction to Programming(Haskell)");
    assert_eq!(prog.instructors, ["S P Suresh"]);

    // Capitalisation variant "(Starts 14 Aug)" and multi-instructor "A/B".
    let papf = SNAPSHOT.course("PAPF").unwrap();
    assert_eq!(papf.starts, Some((14, "Aug".to_string())));
    assert_eq!(papf.instructors, ["Purusottam Rath", "Dhanayjay Sahu"]);

    // Upstream typos are displayed verbatim, never "fixed".
    let aat = SNAPSHOT.course("AAT").unwrap();
    assert_eq!(aat.name, "Applied Algebaric Topology");
    // Apostrophes and ampersands survive.
    assert_eq!(SNAPSHOT.course("CALG").unwrap().instructors, ["Clare D'Cruz"]);
    assert_eq!(
        SNAPSHOT.course("ALGO").unwrap().name,
        "Design & Analysis of Algorithms"
    );
}

/// Test 9 — the slot columns are derived from the header, in order.
#[test]
fn t09_slot_grid() {
    let expected = [
        slot(550, 625),   // 09:10-10:25
        slot(630, 705),   // 10:30-11:45
        slot(710, 785),   // 11:50-13:05
        slot(840, 915),   // 14:00-15:15
        slot(930, 1005),  // 15:30-16:45
        slot(1020, 1095), // 17:00-18:15
    ];
    assert_eq!(SNAPSHOT.slot_grid, expected);
    // And the halls parsed dynamically, in grid order.
    assert_eq!(
        SNAPSHOT.halls,
        [
            "Seminar Hall",
            "Lecture Hall 1",
            "Lecture Hall 2",
            "Lecture Hall 3",
            "Lecture Hall 4",
            "Lecture Hall 5",
            "Lecture Hall 6",
            "Lecture Hall 801",
            "Lecture Hall 802",
            "Lecture Hall 803",
            "Lecture Hall 804",
            "Physics Lab",
            "NKN AV Hall",
            "Lecture Hall 202"
        ]
    );
}

/// Test 10 — synthetic fixtures: a "QCOM+" cell sets optional_flag; an
/// "ABC TMP*" hall cell sets temp_booking.
#[test]
fn t10_synthetic_flags() {
    let syn_tt = r#"<html><body>
<pre>
Timetable for August--November 2026
Test Branch

==============================================
 TST |09:10-10:25|10:30-11:45|11:50-13:05|14:00-15:15|
=====+===========+===========+===========+===========+
 Mon | QCOM+     |           |           |           |
 Tue |           | ABC       |           |           |
 Wed |           |           |           |           |
==============================================
</pre>
<pre>QCOM: Quantum Computing        Bijita Sarma
ABC : Test Course              Some One
</pre>
</body></html>"#;
    let syn_halls = r#"<html><body>
<pre>
          |09:10-10:25|10:30-11:45|11:50-13:05|14:00-15:15|
----------+-----------+-----------+-----------+-----------+
Monday    |           |           |           |           |
  Hall A  | QCOM      |           |           |           |
Tuesday   |           |           |           |           |
  Hall A  |           | ABC TMP*  |           |           |
</pre>
<pre>QCOM : Quantum Computing : Bijita Sarma
ABC  : Test Course : Some One
</pre>
</body></html>"#;

    let tt = parse_timetable_page(&extract_pre_blocks(syn_tt));
    let hp = parse_halls_page(&extract_pre_blocks(syn_halls));
    let joined = join_pages(&tt, &hp);

    let qcom = joined.courses.iter().find(|c| c.code == "QCOM").unwrap();
    assert!(qcom.optional_flag, "trailing '+' sets optional_flag");
    assert_eq!(qcom.meetings[0].hall.as_deref(), Some("Hall A"));
    assert!(!qcom.meetings[0].temp_booking);

    let abc = joined.courses.iter().find(|c| c.code == "ABC").unwrap();
    assert!(!abc.optional_flag);
    assert_eq!(abc.meetings.len(), 1);
    assert!(abc.meetings[0].temp_booking, "TMP* sets temp_booking");
    assert_eq!(abc.meetings[0].hall.as_deref(), Some("Hall A"));
}

/// Test 11 — fail closed: truncated or mangled pages fail the validation
/// gate and no snapshot is produced (so a cache replacement is refused).
#[test]
fn t11_fail_closed() {
    // Truncated timetable page (first 4 KB — most branch grids gone).
    let truncated = &TT[..4000.min(TT.len())];
    let out = parse_html_pages(truncated, HALLS, 0.0, SourceTier::Direct, false);
    assert!(!out.report.gate_passed(), "truncated page must fail the gate");
    assert!(out.snapshot.is_none());
    assert!(!out.report.errors.is_empty());

    // Mangled timetable page: every '|' gone — no grids parse at all.
    let mangled = TT.replace('|', " ");
    let out = parse_html_pages(&mangled, HALLS, 0.0, SourceTier::Direct, false);
    assert!(!out.report.gate_passed(), "mangled page must fail the gate");
    assert!(out.snapshot.is_none());

    // Mangled halls page only: the hall-grid rule must fail.
    let mangled_halls = HALLS.replace('|', " ");
    let out = parse_html_pages(TT, &mangled_halls, 0.0, SourceTier::Direct, false);
    assert!(!out.report.gate_passed(), "mangled halls page must fail the gate");
    assert!(out.snapshot.is_none());

    // Both pages empty.
    let out = parse_html_pages("", "", 0.0, SourceTier::Direct, false);
    assert!(!out.report.gate_passed());
    assert!(out.snapshot.is_none());
}

/// The stored raw HTML round-trips, enabling the re-parse path.
#[test]
fn raw_html_reparse_path() {
    let out = parse_html_pages(TT, HALLS, 42.0, SourceTier::Direct, true);
    let snap = out.snapshot.unwrap();
    let raw = snap.raw_html_gz.as_ref().unwrap();
    let tt_html = cmi_timetable_core::rawhtml::decompress_from_b64(&raw.timetable_b64).unwrap();
    let halls_html =
        cmi_timetable_core::rawhtml::decompress_from_b64(&raw.lecturehalls_b64).unwrap();
    assert_eq!(tt_html, TT);
    assert_eq!(halls_html, HALLS);

    // Re-parsing the stored HTML yields the same catalog.
    let again = parse_html_pages(&tt_html, &halls_html, 42.0, SourceTier::Direct, false);
    let snap2 = again.snapshot.unwrap();
    assert_eq!(snap.courses, snap2.courses);
    assert_eq!(snap.branches, snap2.branches);
}

/// Snapshot JSON round-trip (localStorage / mirror format stability).
#[test]
fn snapshot_serde_round_trip() {
    let snap = &*SNAPSHOT;
    let json = serde_json::to_string(snap).unwrap();
    let back: Snapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(*snap, back);
}

/// The semester heading is matched loosely — CMI can reword it — and course
/// names may contain colons (the halls legend splits at the LAST colon).
#[test]
fn label_and_legend_survive_rewording() {
    use cmi_timetable_core::parse::{find_semester_label, parse_halls_legend_line};

    // Loosened phrasing all resolve to a label.
    assert_eq!(
        find_semester_label("Timetable for August--November 2026").as_deref(),
        Some("August--November 2026")
    );
    assert_eq!(
        find_semester_label("Time Table of the Odd Semester 2026").as_deref(),
        Some("the Odd Semester 2026")
    );
    assert_eq!(
        find_semester_label("TIMETABLE FOR JAN-APR 2027").as_deref(),
        Some("JAN-APR 2027")
    );

    // A colon inside the course name must not truncate it.
    let entry =
        parse_halls_legend_line("TQI : Topics: Quantum Information : R Rao").unwrap();
    assert_eq!(entry.code, "TQI");
    assert_eq!(entry.name, "Topics: Quantum Information");
    assert_eq!(entry.instructors_raw.as_deref(), Some("R Rao"));

    // The plain two-field form still works.
    let plain = parse_halls_legend_line("RFLR : Reinforcement Learning : I Murugeswari")
        .unwrap();
    assert_eq!(plain.name, "Reinforcement Learning");
    assert_eq!(plain.instructors_raw.as_deref(), Some("I Murugeswari"));
}

/// A page pair whose headings were reworded beyond recognition still passes
/// the gate: the label is display-only, so (None, None) is warn-only.
#[test]
fn missing_labels_are_warn_only() {
    let tt_reworded = TT.replace("Timetable", "Schedule").replace("timetable", "schedule");
    let halls_reworded = HALLS.replace("Timetable", "Schedule").replace("timetable", "schedule");
    let out = parse_html_pages(&tt_reworded, &halls_reworded, 0.0, SourceTier::Direct, false);
    // Either the year-line fallback found a label, or the gate passed
    // without one — a heading reword must never block a fresh semester.
    assert!(
        out.report.gate_passed(),
        "reworded headings must not fail the gate: {:#?}",
        out.report.gate
    );
    assert!(out.snapshot.is_some());
}
