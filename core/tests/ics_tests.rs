//! .ics golden-file test for a two-course selection including a course with
//! a "(starts 12 Aug)" note. Regenerate the golden with:
//! `UPDATE_GOLDEN=1 cargo test -p cmi-timetable-core --test ics_tests`

use cmi_timetable_core::date::CivilDate;
use cmi_timetable_core::ics::{build_ics, ics_filename, IcsCourse, IcsOptions};
use cmi_timetable_core::model::{Day, Meeting, Slot};

fn mtg(day: Day, start: u16, end: u16, hall: &str) -> Meeting {
    Meeting {
        day,
        slot: Slot::new(start, end),
        hall: Some(hall.to_string()),
        temp_booking: false,
    }
}

#[test]
fn golden_two_courses() {
    let cm1 = IcsCourse {
        code: "CM1".to_string(),
        name: "Classical Mechanics I(starts 12 Aug)".to_string(),
        instructors: vec!["K G Arun".to_string()],
        branches: vec!["BM1".to_string()],
        meetings: vec![
            mtg(Day::Wed, 840, 915, "Lecture Hall 802"),
            mtg(Day::Thu, 630, 705, "Lecture Hall 802"),
        ],
        starts: Some((12, "Aug".to_string())),
        part_of_semester: None,
    };
    let mfd = IcsCourse {
        code: "MFD".to_string(),
        name: "Matchings and Fair Division".to_string(),
        instructors: vec!["Keshav Ranjan".to_string()],
        branches: vec!["OCS2".to_string()],
        meetings: vec![
            mtg(Day::Wed, 840, 915, "Lecture Hall 6"),
            mtg(Day::Fri, 840, 915, "Lecture Hall 6"),
        ],
        starts: None,
        part_of_semester: None,
    };
    let opts = IcsOptions {
        range_start: CivilDate::new(2026, 8, 3), // first Monday of Aug 2026
        range_end: CivilDate::new(2026, 11, 30),
        alarm: true,
        app_url: "https://example.github.io/timetable/?c=CM1,MFD".to_string(),
        dtstamp: "20260805T120000Z".to_string(),
        calendar_name: "CMI Timetable August\u{2013}November 2026".to_string(),
    };
    let ics = build_ics(&[mfd, cm1], &opts);

    let golden_path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/two_courses.ics");
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::create_dir_all(std::path::Path::new(golden_path).parent().unwrap()).unwrap();
        std::fs::write(golden_path, &ics).unwrap();
    }
    let golden = std::fs::read_to_string(golden_path)
        .expect("golden file missing — run with UPDATE_GOLDEN=1 once");
    assert_eq!(ics, golden);
}

#[test]
fn starts_note_shifts_first_occurrence() {
    let course = IcsCourse {
        code: "CM1".to_string(),
        name: "Classical Mechanics I(starts 12 Aug)".to_string(),
        instructors: vec![],
        branches: vec![],
        // Thursdays — 6 Aug is before the (starts 12 Aug) note, so the first
        // event must fall on 13 Aug.
        meetings: vec![mtg(Day::Thu, 630, 705, "Lecture Hall 802")],
        starts: Some((12, "Aug".to_string())),
        part_of_semester: None,
    };
    let opts = IcsOptions {
        range_start: CivilDate::new(2026, 8, 3),
        range_end: CivilDate::new(2026, 11, 30),
        alarm: false,
        app_url: String::new(),
        dtstamp: "20260805T120000Z".to_string(),
        calendar_name: "test".to_string(),
    };
    let ics = build_ics(&[course], &opts);
    assert!(
        ics.contains("DTSTART;TZID=Asia/Kolkata:20260813T103000"),
        "{ics}"
    );
    assert!(ics.contains("RRULE:FREQ=WEEKLY;UNTIL=20261130T182959Z"));
}

#[test]
fn part_of_semester_clamps_range() {
    let course = IcsCourse {
        code: "MATH".to_string(),
        name: "Matroid Theory(Oct-Nov)".to_string(),
        instructors: vec![],
        branches: vec![],
        meetings: vec![mtg(Day::Mon, 550, 625, "Lecture Hall 1")],
        starts: None,
        part_of_semester: Some("Oct-Nov".to_string()),
    };
    let opts = IcsOptions {
        range_start: CivilDate::new(2026, 8, 3),
        range_end: CivilDate::new(2026, 11, 30),
        alarm: false,
        app_url: String::new(),
        dtstamp: "20260805T120000Z".to_string(),
        calendar_name: "test".to_string(),
    };
    let ics = build_ics(&[course], &opts);
    // First Monday on/after 1 Oct 2026 is 5 Oct.
    assert!(ics.contains("DTSTART;TZID=Asia/Kolkata:20261005T091000"), "{ics}");
}

#[test]
fn escaping_and_structure() {
    let course = IcsCourse {
        code: "ALGO".to_string(),
        name: "Design & Analysis of Algorithms; with, commas".to_string(),
        instructors: vec!["Prajakta Nimbhorkar".to_string()],
        branches: vec!["BM2".to_string()],
        meetings: vec![Meeting {
            day: Day::Mon,
            slot: Slot::new(630, 705),
            hall: None, // Hall TBA
            temp_booking: false,
        }],
        starts: None,
        part_of_semester: None,
    };
    let opts = IcsOptions {
        range_start: CivilDate::new(2026, 8, 3),
        range_end: CivilDate::new(2026, 11, 30),
        alarm: false,
        app_url: String::new(),
        dtstamp: "20260805T120000Z".to_string(),
        calendar_name: "test".to_string(),
    };
    let ics = build_ics(&[course], &opts);
    assert!(ics.contains("SUMMARY:ALGO: Design & Analysis of Algorithms\\; with\\, commas"));
    assert!(ics.contains("LOCATION:Hall TBA"));
    assert!(ics.starts_with("BEGIN:VCALENDAR\r\n"));
    assert!(ics.ends_with("END:VCALENDAR\r\n"));
    // Every line ends with CRLF and no unfolded line exceeds 75 octets.
    for line in ics.split("\r\n") {
        assert!(line.len() <= 75, "line too long: {line:?}");
    }
}

#[test]
fn filename_from_label() {
    assert_eq!(
        ics_filename("August--November 2026"),
        "cmi-timetable-aug-nov-2026.ics"
    );
    assert_eq!(
        ics_filename("January\u{2013}April 2027"),
        "cmi-timetable-jan-apr-2027.ics"
    );
    assert_eq!(ics_filename("gibberish"), "cmi-timetable.ics");
}
