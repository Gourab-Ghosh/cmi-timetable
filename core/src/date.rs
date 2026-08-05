//! Minimal proleptic-Gregorian date math (Howard Hinnant's algorithms) so
//! the .ics exporter needs no chrono/js-sys. Weeks run Mon=0 … Sun=6.

use crate::model::Day;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CivilDate {
    pub y: i32,
    pub m: u8,
    pub d: u8,
}

impl CivilDate {
    pub fn new(y: i32, m: u8, d: u8) -> CivilDate {
        CivilDate { y, m, d }
    }

    /// Days since 1970-01-01.
    pub fn to_days(self) -> i64 {
        let y = if self.m <= 2 { self.y - 1 } else { self.y } as i64;
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = ((self.m as i64) + 9) % 12;
        let doy = (153 * mp + 2) / 5 + (self.d as i64) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    pub fn from_days(days: i64) -> CivilDate {
        let z = days + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        CivilDate {
            y: (if m <= 2 { y + 1 } else { y }) as i32,
            m,
            d,
        }
    }

    pub fn weekday(self) -> Day {
        // 1970-01-01 was a Thursday (Mon=0 ⇒ index 3).
        let idx = (self.to_days() + 3).rem_euclid(7) as usize;
        Day::ALL[idx]
    }

    pub fn add_days(self, n: i64) -> CivilDate {
        CivilDate::from_days(self.to_days() + n)
    }

    /// First date ≥ self whose weekday is `day`.
    pub fn first_on_or_after(self, day: Day) -> CivilDate {
        let cur = self.weekday().index() as i64;
        let want = day.index() as i64;
        self.add_days((want - cur).rem_euclid(7))
    }

    /// "2026-08-03"
    pub fn to_iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }

    /// "20260803"
    pub fn to_compact(self) -> String {
        format!("{:04}{:02}{:02}", self.y, self.m, self.d)
    }

    pub fn parse_iso(s: &str) -> Option<CivilDate> {
        let mut parts = s.trim().splitn(3, '-');
        let y = parts.next()?.parse::<i32>().ok()?;
        let m = parts.next()?.parse::<u8>().ok()?;
        let d = parts.next()?.parse::<u8>().ok()?;
        if !(1..=12).contains(&m) || d < 1 || d > last_day_of_month(y, m) {
            return None;
        }
        Some(CivilDate::new(y, m, d))
    }
}

pub fn is_leap_year(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn last_day_of_month(y: i32, m: u8) -> u8 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// "Aug", "august", "AUGUST" → 8. Matches on the first three letters.
pub fn month_from_token(token: &str) -> Option<u8> {
    let t = token.trim().to_ascii_lowercase();
    let t3 = t.get(..3)?;
    Some(match t3 {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

pub const MONTH_SHORT: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// "August--November 2026" → (first Monday of August 2026, 30 Nov 2026).
/// Tolerates `--`, en dash or a single hyphen between the months.
pub fn semester_range_from_label(label: &str) -> Option<(CivilDate, CivilDate)> {
    let re = regex_lite::Regex::new(
        r"([A-Za-z]{3,})\s*(?:--|\u{2013}|-)\s*([A-Za-z]{3,})\s+(\d{4})",
    )
    .ok()?;
    let caps = re.captures(label)?;
    let m1 = month_from_token(caps.get(1)?.as_str())?;
    let m2 = month_from_token(caps.get(2)?.as_str())?;
    let year = caps.get(3)?.as_str().parse::<i32>().ok()?;
    let start = CivilDate::new(year, m1, 1).first_on_or_after(Day::Mon);
    let end_year = if m2 < m1 { year + 1 } else { year };
    let end = CivilDate::new(end_year, m2, last_day_of_month(end_year, m2));
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_math() {
        let d = CivilDate::new(2026, 8, 5);
        assert_eq!(d.weekday(), Day::Wed);
        assert_eq!(d.to_iso(), "2026-08-05");
        assert_eq!(CivilDate::from_days(d.to_days()), d);
        assert_eq!(d.first_on_or_after(Day::Wed), d);
        assert_eq!(d.first_on_or_after(Day::Mon).to_iso(), "2026-08-10");
    }

    #[test]
    fn semester_range() {
        let (start, end) = semester_range_from_label("August--November 2026").unwrap();
        assert_eq!(start.to_iso(), "2026-08-03"); // first Monday of Aug 2026
        assert_eq!(end.to_iso(), "2026-11-30");
        let (s2, e2) = semester_range_from_label("January\u{2013}April 2027").unwrap();
        assert_eq!(s2.to_iso(), "2027-01-04");
        assert_eq!(e2.to_iso(), "2027-04-30");
    }
}
