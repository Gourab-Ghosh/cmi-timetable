//! The clamp law, at the doors that are pure core: a share link, a
//! "my courses" file, and the stores those two write.
//!
//! One rule applied at one door only is a bug at the others. The editor
//! bounded a class time and a credit figure from the day it was written; a
//! link, a file and the browser's own storage did not, so a planner could be
//! handed a class starting at minute 65535 of the day (drawn as "1092:15",
//! exported as an eight-digit `DTSTART`), a course worth 255 credits, and a
//! change numbered 18446744073709551615 — which has no successor, so the
//! next change made in the app was given a number an existing change already
//! had (R93 S1, S5, S8).
//!
//! Proposed for `core/tests/r96_clamp_doors.rs`.

use cmi_timetable_core::export::parse_timetable_export;
use cmi_timetable_core::model::{
    Course, CreditOverride, CustomStore, Day, HiddenCourse, Meeting, MeetingOverride,
    OverridesStore, ScheduleStatus, Slot,
};
use cmi_timetable_core::share::{encode_share, resolve_url_state};

const NOW: f64 = 1_754_000_000_000.0;

fn at(day: Day, start: u16, end: u16) -> Meeting {
    Meeting {
        day,
        slot: Slot {
            start_min: start,
            end_min: end,
        },
        hall: Some("Lecture Hall 803".to_string()),
        temp_booking: false,
    }
}

fn moved(id: u64, course: &str, base: Meeting, to: Meeting) -> MeetingOverride {
    MeetingOverride {
        id,
        course: course.to_string(),
        base: Some(base),
        to: Some(to),
        created_at: NOW,
    }
}

fn course(code: &str, meetings: Vec<Meeting>) -> Course {
    Course {
        code: code.to_string(),
        name: "Reading group".to_string(),
        instructors: vec![],
        branches: vec![],
        credits: Some(4),
        starts: None,
        part_of_semester: None,
        optional_flag: false,
        status: ScheduleStatus::Scheduled,
        meetings,
    }
}

/// A time of day, and the two ways bytes from outside can fail to be one:
/// a class that ends before it starts (or at the same minute — an event with
/// no duration is not something a grid can draw) and one that runs past
/// midnight. 1440 is the last honest minute: midnight itself, as an END.
#[test]
fn only_a_real_time_of_day_is_a_time_this_app_can_draw() {
    assert!(
        Slot {
            start_min: 550,
            end_min: 625
        }
        .is_sane()
    );
    assert!(
        Slot {
            start_min: 1380,
            end_min: 1440
        }
        .is_sane(),
        "up to midnight"
    );

    assert!(
        !Slot {
            start_min: 625,
            end_min: 550
        }
        .is_sane(),
        "ends before it starts"
    );
    assert!(
        !Slot {
            start_min: 550,
            end_min: 550
        }
        .is_sane(),
        "no duration"
    );
    assert!(
        !Slot {
            start_min: 1380,
            end_min: 1441
        }
        .is_sane(),
        "past midnight"
    );
    // The one from the bug report: `Slot::label` renders it "1092:15".
    assert!(
        !Slot {
            start_min: 65535,
            end_min: 65535
        }
        .is_sane()
    );
}

/// A share link is a stranger's bytes. It may carry a class time no clock
/// has, a credit figure outside the range the editor allows, and a change
/// numbered so high that nothing can be numbered after it — and it used to
/// carry all three straight onto the reader's timetable. They are left out
/// now, one by one, and the count travels with the link so the app can say
/// how much of it did not arrive.
#[test]
fn a_link_leaves_out_what_the_app_cannot_state_and_counts_it() {
    let sent = OverridesStore {
        next_id: 0,
        items: vec![
            moved(0, "TOC", at(Day::Tue, 550, 625), at(Day::Wed, 65535, 65535)),
            moved(1, "NLP", at(Day::Thu, 840, 915), at(Day::Fri, 1020, 1095)),
            moved(
                u64::MAX,
                "ISS",
                at(Day::Tue, 550, 625),
                at(Day::Wed, 930, 1005),
            ),
        ],
        credits: vec![
            CreditOverride {
                course: "TOC".into(),
                credits: 255,
                created_at: NOW,
            },
            CreditOverride {
                course: "NLP".into(),
                credits: 3,
                created_at: NOW,
            },
        ],
        hidden: vec![HiddenCourse {
            course: "QCOM".into(),
            created_at: NOW,
            was_selected: false,
        }],
    };
    let link = encode_share(&["TOC".to_string(), "NLP".to_string()], &sent, &[]);
    let state = resolve_url_state(None, Some(&link));

    assert_eq!(
        state.set_aside, 3,
        "two impossible changes and one credit figure"
    );
    let got = state
        .overrides
        .expect("the link is readable, so it resolves");
    assert_eq!(
        got.items
            .iter()
            .map(|o| o.course.as_str())
            .collect::<Vec<_>>(),
        ["NLP"],
        "only the change the app can state survives"
    );
    assert_eq!(
        got.credits.iter().map(|c| c.credits).collect::<Vec<_>>(),
        [3],
        "a credit figure outside the editor's own range is not the reader's"
    );
    // Dropped, never clamped: moving somebody's class to an hour nobody
    // chose and then printing it as their decision is the worse lie.
    assert!(
        !got.items.iter().any(|o| o.course == "TOC"),
        "an impossible time is not repaired into a possible one"
    );
    // A deletion carries no time and no number, so it needs no rule.
    assert_eq!(got.hidden.len(), 1);
    // The selection is untouched by any of this.
    assert_eq!(state.selection, ["TOC", "NLP"]);
}

/// The guard on the test above: a link carrying nothing impossible must
/// arrive whole and set NOTHING aside, or "it was left out" would be a
/// sentence the app says about every link it reads.
#[test]
fn a_link_that_says_nothing_impossible_arrives_whole() {
    let sent = OverridesStore {
        next_id: 2,
        items: vec![moved(
            1,
            "NLP",
            at(Day::Thu, 840, 915),
            at(Day::Fri, 1020, 1095),
        )],
        credits: vec![CreditOverride {
            course: "NLP".into(),
            credits: 20,
            created_at: NOW,
        }],
        hidden: vec![],
    };
    let link = encode_share(&["NLP".to_string()], &sent, &[]);
    let state = resolve_url_state(None, Some(&link));
    assert_eq!(state.set_aside, 0);
    let got = state.overrides.expect("resolves");
    assert_eq!(got.items.len(), 1);
    assert_eq!(
        got.credits[0].credits, 20,
        "20 credits is the top of the editor's range"
    );
}

/// A course of the sender's own rides along with the link. One of its
/// classes being undrawable makes the whole course unusable — a course
/// quietly missing a meeting is lying by omission, so it loses its whole
/// entry and its code becomes the same dismissible "unknown code" chip a
/// link naming a retired course already produces.
#[test]
fn a_course_of_your_own_arrives_whole_or_not_at_all() {
    let mut customs = CustomStore {
        courses: vec![
            course(
                "READ",
                vec![at(Day::Mon, 600, 660), at(Day::Wed, 65535, 65535)],
            ),
            course("SEM", vec![at(Day::Fri, 600, 660)]),
        ],
    };
    assert!(!customs.is_sane());
    assert_eq!(customs.retain_sane(), 1);
    assert_eq!(
        customs
            .courses
            .iter()
            .map(|c| c.code.as_str())
            .collect::<Vec<_>>(),
        ["SEM"],
        "the good class of a bad course is not kept on its own"
    );
    assert!(customs.is_sane());
}

/// The store hands out a number for every change the reader makes, and a
/// hand-made file can leave that counter behind its own changes. The repair
/// moves the COUNTER, never an id: a postponed sync question
/// (`merge::Conflict::override_id`) points INTO these items by number, so
/// renumbering them would silently re-aim a deferred "use CMI's new time" at
/// a different change.
#[test]
fn the_counter_moves_on_and_the_changes_keep_their_numbers() {
    let mut store = OverridesStore {
        next_id: 0,
        items: vec![
            moved(7, "TOC", at(Day::Tue, 550, 625), at(Day::Wed, 1020, 1095)),
            moved(3, "NLP", at(Day::Thu, 840, 915), at(Day::Fri, 1020, 1095)),
        ],
        credits: vec![],
        hidden: vec![],
    };
    store.bump_next_id();
    assert_eq!(
        store.next_id, 8,
        "the next change gets a number nothing else has"
    );
    assert_eq!(
        store.items.iter().map(|o| o.id).collect::<Vec<_>>(),
        [7, 3],
        "the changes themselves are never renumbered"
    );

    // And the whole point of the counter: the number it hands out is free.
    let fresh = store.add("ISS", None, Some(at(Day::Mon, 600, 660)), NOW);
    assert_eq!(fresh, 8);
    assert!(
        store.items.iter().filter(|o| o.id == fresh).count() == 1,
        "two changes sharing a number means removing one removes both"
    );

    // A counter already ahead of its items is left exactly where it is.
    let mut ahead = OverridesStore {
        next_id: 99,
        items: vec![moved(
            3,
            "NLP",
            at(Day::Thu, 840, 915),
            at(Day::Fri, 1020, 1095),
        )],
        credits: vec![],
        hidden: vec![],
    };
    ahead.bump_next_id();
    assert_eq!(ahead.next_id, 99);
}

/// The number 18446744073709551615 is the one number a counter cannot follow:
/// `add` computes the next one, so a change wearing it overflows the counter
/// (panicking in a debug build) and, short of that, forces the counter to
/// collide with a change that already exists.
#[test]
fn a_change_numbered_higher_than_anything_can_follow_is_not_kept() {
    let live = moved(4, "NLP", at(Day::Thu, 840, 915), at(Day::Fri, 1020, 1095));
    let impossible = moved(
        u64::MAX,
        "TOC",
        at(Day::Tue, 550, 625),
        at(Day::Wed, 930, 1005),
    );
    assert!(live.is_sane());
    assert!(!impossible.is_sane());

    let mut store = OverridesStore {
        next_id: 0,
        items: vec![live, impossible],
        credits: vec![],
        hidden: vec![],
    };
    assert_eq!(store.retain_sane(), 1);
    assert_eq!(store.items.len(), 1);
    assert_eq!(store.next_id, 5, "and the counter is past what is left");

    // An entry naming no class at all is not an entry either — the rule the
    // file door has always had, now asked at every door.
    let says_nothing = MeetingOverride {
        id: 0,
        course: "TOC".into(),
        base: None,
        to: None,
        created_at: NOW,
    };
    assert!(!says_nothing.is_sane());
}

/// The credits editor takes a whole number from 0 to 20 and always has. A
/// "my courses" file made by hand could say 255, and the app then told the
/// reader "You set the credits on 2 courses yourself" and totted the term up
/// to 459 credits — a decision they never made. A figure outside that range
/// is not one this app can state, so the file is refused whole, the way this
/// door refuses every other value it cannot read.
#[test]
fn a_courses_file_cannot_hand_out_credits_the_editor_would_not_take() {
    let file = |credits: u8| {
        format!(
            r#"{{"format":"cmi-timetable-export","format_version":"1.1.0",
                 "courses":[{{"code":"NLP"}}],
                 "my_changes":{{"meeting_changes":[],
                   "credit_changes":[{{"course":"NLP","credits":{credits},
                     "made_at":"2025-07-31T22:13:20Z","made_at_ms":1754000000000.0}}],
                   "my_own_courses":[]}}}}"#
        )
    };
    let refused = parse_timetable_export(&file(255)).expect_err("255 credits is refused");
    assert!(
        refused.contains("aren't the shape this app can read"),
        "{refused}"
    );

    // The guard: the same file at the top of the editor's range is read.
    let plan = parse_timetable_export(&file(20)).expect("20 credits is a number the editor takes");
    assert_eq!(plan.overrides.credits[0].credits, 20);
    assert_eq!(
        CreditOverride::MAX,
        20,
        "the range every door enforces is the editor's"
    );
}

/// A class time in the same file gets the same treatment, and the two rules
/// are the same rule: `Slot::is_sane`, asked once.
#[test]
fn a_courses_file_cannot_hand_out_a_class_time_no_clock_has() {
    let file = |end_min: u16| {
        format!(
            r#"{{"format":"cmi-timetable-export","format_version":"1.1.0",
                 "courses":[{{"code":"NLP"}}],
                 "my_changes":{{"meeting_changes":[{{"course":"NLP","kind":"moved",
                     "from":{{"day":"Thu","iso_weekday":4,
                       "start":{{"minutes":840,"hhmm":"14:00"}},
                       "end":{{"minutes":915,"hhmm":"15:15"}},"hall":"Lecture Hall 801"}},
                     "to":{{"day":"Fri","iso_weekday":5,
                       "start":{{"minutes":1020,"hhmm":"17:00"}},
                       "end":{{"minutes":{end_min},"hhmm":"17:00"}},"hall":"Lecture Hall 801"}},
                     "made_at":"2025-07-31T22:13:20Z","made_at_ms":1754000000000.0}}],
                   "credit_changes":[],"my_own_courses":[]}}}}"#
        )
    };
    let refused =
        parse_timetable_export(&file(1441)).expect_err("a class past midnight is refused");
    assert!(
        refused.contains("aren't the shape this app can read"),
        "{refused}"
    );
    let plan = parse_timetable_export(&file(1095)).expect("a real time of day is read");
    assert_eq!(plan.overrides.items.len(), 1);
}
