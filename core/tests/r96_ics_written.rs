// R96, slice t3-calendar-share — the native half of R93 S3.
//
// PASTE-READY: drop the `#[test] fn` below (and, if it is not already there,
// nothing else — `mtg`, `IcsCourse`, `IcsOptions`, `CivilDate` and `Day` are
// all already imported at the top) into `core/tests/ics_tests.rs`.
//
// The file it was verified in is reproduced whole here so it can be run on
// its own: `cp` this to core/tests/r96_ics_written.rs and
// `CARGO_TARGET_DIR=~/.rust-cache/timetable-e2e cargo test -p cmi-timetable-core
//  --test r96_ics_written`.

use cmi_timetable_core::date::CivilDate;
use cmi_timetable_core::ics::{IcsCourse, IcsOptions, build_ics};
use cmi_timetable_core::model::{Day, Meeting, Slot};

fn mtg(day: Day, start: u16, end: u16, hall: &str) -> Meeting {
    Meeting {
        day,
        slot: Slot::new(start, end),
        hall: Some(hall.to_string()),
        temp_booking: false,
    }
}

fn course(code: &str, meetings: Vec<Meeting>) -> IcsCourse {
    IcsCourse {
        code: code.to_string(),
        name: format!("{code} course"),
        instructors: vec![],
        branches: vec![],
        meetings,
        starts: None,
        part_of_semester: None,
    }
}

fn opts(start: CivilDate, end: CivilDate) -> IcsOptions {
    IcsOptions {
        range_start: start,
        range_end: end,
        alarm_minutes: None,
        app_url: String::new(),
        dtstamp: "20260805T120000Z".to_string(),
        calendar_name: "test".to_string(),
    }
}

/// The file and the account of what went into it are ONE answer.
///
/// A meeting whose first occurrence falls past the last date writes nothing,
/// so "three courses chosen" and "three courses in the file" are different
/// facts, and only the writer knows which is which. The export dialog drives
/// both its refusal and its "isn't in it" sentence off this list — take the
/// count anywhere else and the sentence starts describing a file that was
/// never written, which is how a narrowed range came to drop a course in
/// silence (R93 S3).
#[test]
fn build_ics_reports_what_each_course_put_in_the_file() {
    // 11 Aug 2026 is a Tuesday. A range of that one day can hold TUE's
    // Tuesday class and nothing else.
    let one_tuesday = opts(CivilDate::new(2026, 8, 11), CivilDate::new(2026, 8, 11));
    let courses = || {
        vec![
            course("MON", vec![mtg(Day::Mon, 550, 625, "Lecture Hall 803")]),
            course(
                "TUE",
                vec![
                    mtg(Day::Tue, 550, 625, "Lecture Hall 803"),
                    mtg(Day::Thu, 630, 705, "Lecture Hall 802"),
                ],
            ),
            course("NIL", vec![]),
        ]
    };

    let (ics, written) = build_ics(&courses(), &one_tuesday);
    assert_eq!(ics.matches("BEGIN:VEVENT").count(), 1, "{ics}");
    // EVERY course passed in gets an entry, a zero included: the caller walks
    // its own list and looks each code up, so a missing entry would read as a
    // zero for a course that is in the file.
    assert_eq!(written.len(), 3, "{written:?}");
    let count = |code: &str| {
        written
            .iter()
            .find(|(c, _)| c == code)
            .map(|(_, n)| *n)
            .unwrap_or_else(|| panic!("{code} has no entry in {written:?}"))
    };
    assert_eq!(count("TUE"), 1, "{written:?}");
    // The whole point: a course with real classes that the DATES excluded
    // must report 0 — not the number of meetings it has, and not silence.
    assert_eq!(
        count("MON"),
        0,
        "a course the range dropped must report nothing written: {written:?}"
    );
    assert_eq!(count("NIL"), 0, "{written:?}");
    // The counts are of what the FILE holds, so they must add up to it.
    let total: usize = written.iter().map(|(_, n)| *n).sum();
    assert_eq!(total, ics.matches("BEGIN:VEVENT").count(), "{written:?}");

    // Widen the range by a week and the answer changes with the file, not
    // with the courses: MON is back, and TUE's second class comes with it.
    let a_fortnight = opts(CivilDate::new(2026, 8, 11), CivilDate::new(2026, 8, 24));
    let (wide, written) = build_ics(&courses(), &a_fortnight);
    let count = |code: &str| {
        written
            .iter()
            .find(|(c, _)| c == code)
            .map(|(_, n)| *n)
            .unwrap_or_else(|| panic!("{code} has no entry in {written:?}"))
    };
    assert_eq!(count("MON"), 1, "{written:?}");
    assert_eq!(count("TUE"), 2, "{written:?}");
    assert_eq!(count("NIL"), 0, "{written:?}");
    let total: usize = written.iter().map(|(_, n)| *n).sum();
    assert_eq!(total, wide.matches("BEGIN:VEVENT").count(), "{written:?}");
}
