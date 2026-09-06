//! Where a class is DRAWN on a week grid, and where it only reaches.
//!
//! The rule (R97): a class belongs to the column holding most of it. An exact
//! start match wins outright, so every meeting CMI publishes stays exactly
//! where it has always been drawn and the new rule only ever moves a free-form
//! time somebody typed or dragged.

use cmi_timetable_core::model::{Placement, Slot, home_column, place_meeting};

/// CMI's real teaching grid, which is what nearly every case below is judged
/// against. Note the 15-minute gap between 16:45 and 17:00 — several of the
/// interesting cases live inside it.
fn grid() -> Vec<Slot> {
    vec![
        Slot::new(550, 625),   // 09:10-10:25
        Slot::new(630, 705),   // 10:30-11:45
        Slot::new(710, 785),   // 11:50-13:05
        Slot::new(840, 915),   // 14:00-15:15
        Slot::new(930, 1005),  // 15:30-16:45
        Slot::new(1020, 1095), // 17:00-18:15
    ]
}

fn home(start: u16, end: u16) -> Option<u16> {
    home_column(&grid(), &Slot::new(start, end))
}

fn placed(start: u16, end: u16) -> Placement {
    let mut p = place_meeting(&grid(), &Slot::new(start, end));
    p.covered.sort_unstable();
    p
}

// -- the case that prompted the rule ---------------------------------------

#[test]
fn a_class_is_drawn_where_most_of_it_happens() {
    // 16:40-18:00: five minutes in the 15:30 column, a full hour in the 17:00
    // one. It is a 17:00 class that reaches back, not a 15:30 class.
    assert_eq!(home(1000, 1080), Some(1020));
    assert_eq!(placed(1000, 1080).covered, vec![930]);
}

#[test]
fn the_earlier_column_still_wins_when_it_holds_more() {
    // 16:00-17:30: 45 minutes before the gap, 30 after it.
    assert_eq!(home(960, 1050), Some(930));
    assert_eq!(placed(960, 1050).covered, vec![1020]);
}

#[test]
fn a_dead_heat_goes_to_the_earlier_column() {
    // 16:15-17:30: 30 minutes in each. A class split evenly reads as starting
    // in the first — and the answer must be STABLE, not whichever the
    // iterator reached first.
    assert_eq!(
        Slot::new(930, 1005).overlap_minutes(&Slot::new(975, 1050)),
        30
    );
    assert_eq!(
        Slot::new(1020, 1095).overlap_minutes(&Slot::new(975, 1050)),
        30
    );
    assert_eq!(home(975, 1050), Some(930));
}

// -- what must NOT move ----------------------------------------------------

#[test]
fn an_exact_start_wins_outright_however_the_minutes_fall() {
    // Every meeting CMI publishes starts on a boundary. 09:10-14:00 spends
    // more minutes in later columns than in its own, and must still be drawn
    // in the column it starts in — this is the whole official timetable.
    assert_eq!(home(550, 840), Some(550));
    assert_eq!(placed(550, 840).covered, vec![630, 710]);
}

#[test]
fn an_ordinary_class_covers_nothing() {
    let p = placed(630, 705);
    assert_eq!(p.home, Some(630));
    assert!(p.covered.is_empty(), "{:?}", p.covered);
}

#[test]
fn a_class_inside_one_column_stays_there() {
    // 10:40-11:20, wholly inside 10:30-11:45.
    assert_eq!(home(640, 680), Some(630));
    assert!(placed(640, 680).covered.is_empty());
}

// -- the edges -------------------------------------------------------------

#[test]
fn touching_a_boundary_is_not_overlapping_it() {
    // 16:45-18:00 starts exactly where the 15:30 column ends. Half-open: it
    // shares nothing with it, so no band is owed there.
    assert_eq!(home(1005, 1080), Some(1020));
    assert!(placed(1005, 1080).covered.is_empty());
    // ...and 09:10-10:30 ends exactly where the next column starts.
    assert!(placed(550, 630).covered.is_empty());
}

#[test]
fn a_class_in_the_gap_falls_to_the_nearest_column_and_covers_nothing() {
    // 16:50-16:55 lives entirely in the 15-minute gap: it overlaps NOTHING,
    // so the nearest column answers and no band is drawn anywhere. A band
    // must only ever appear where the class genuinely runs.
    let p = placed(1010, 1015);
    assert_eq!(p.home, Some(1020));
    assert!(p.covered.is_empty(), "{:?}", p.covered);
}

#[test]
fn a_class_spanning_three_columns_is_drawn_in_the_fullest_and_bands_the_rest() {
    // 10:00-12:30: 25 min of the 09:10 column, 75 of the 10:30 one, 40 of the
    // 11:50 one.
    let p = placed(600, 750);
    assert_eq!(p.home, Some(630));
    assert_eq!(p.covered, vec![550, 710]);
}

#[test]
fn a_band_before_the_home_column_is_allowed_and_is_the_point() {
    // The reader's own case, stated as the invariant: the chip is later than
    // one of its bands. This is what R83's old "LATER only" floor forbade,
    // and forbidding it is what left the fullest column showing a band while
    // the chip sat in a column the class had nearly left.
    let p = placed(1000, 1080);
    assert!(p.covered.iter().all(|c| Some(*c) < p.home), "{p:?}");
}

#[test]
fn home_is_never_also_a_band() {
    // The one invariant that makes the two answers safe to use together —
    // swept over every start and length on the real grid.
    for start in (540..1100).step_by(5) {
        for len in [5u16, 30, 75, 90, 150, 300] {
            let p = place_meeting(&grid(), &Slot::new(start, start + len));
            if let Some(h) = p.home {
                assert!(!p.covered.contains(&h), "start {start} len {len}: {p:?}");
            }
            for c in &p.covered {
                assert!(
                    grid().iter().any(|s| s.start_min == *c),
                    "band in a column that does not exist: {p:?}"
                );
            }
        }
    }
}

#[test]
fn every_covered_column_genuinely_overlaps_the_class() {
    // A band claims the class is running there. It had better be.
    for start in (540..1100).step_by(5) {
        for len in [5u16, 45, 120, 400] {
            let m = Slot::new(start, start + len);
            for c in place_meeting(&grid(), &m).covered {
                let col = grid().into_iter().find(|s| s.start_min == c).unwrap();
                assert!(
                    col.overlaps(&m),
                    "band at {c} for {m:?} that does not overlap"
                );
            }
        }
    }
}

#[test]
fn an_empty_grid_places_nothing() {
    let p = place_meeting(&[], &Slot::new(600, 700));
    assert_eq!(p, Placement::default());
    assert_eq!(p.home, None);
}

#[test]
fn a_one_column_grid_always_answers_that_column() {
    let one = [Slot::new(630, 705)];
    for (s, e) in [
        (0u16, 10u16),
        (600, 640),
        (630, 705),
        (700, 900),
        (1400, 1440),
    ] {
        assert_eq!(home_column(&one, &Slot::new(s, e)), Some(630), "{s}-{e}");
    }
}

#[test]
fn overlapping_columns_do_not_double_count() {
    // Personal grids mint extra columns for typed times, and two of them can
    // overlap each other. The class still has exactly one home, and the other
    // column gets a band — never two homes.
    let g = [Slot::new(1230, 1305), Slot::new(1290, 1365)];
    let p = place_meeting(&g, &Slot::new(1300, 1360));
    assert_eq!(p.home, Some(1290));
    assert_eq!(p.covered, vec![1230]);
}

#[test]
fn overlap_minutes_is_symmetric_and_half_open() {
    let a = Slot::new(600, 700);
    let b = Slot::new(650, 750);
    assert_eq!(a.overlap_minutes(&b), 50);
    assert_eq!(b.overlap_minutes(&a), 50);
    assert_eq!(a.overlap_minutes(&Slot::new(700, 800)), 0);
    assert_eq!(a.overlap_minutes(&Slot::new(500, 600)), 0);
    assert_eq!(a.overlap_minutes(&a), 100);
}
