//! Fully client-side .ics generation: one VEVENT per weekly meeting, weekly
//! RRULE until the semester end, VTIMEZONE for Asia/Kolkata (fixed +05:30,
//! no DST).

use crate::date::{CivilDate, MONTH_SHORT, last_day_of_month, month_from_token};
use crate::model::{Course, Day, Meeting};

#[derive(Debug, Clone)]
pub struct IcsCourse {
    pub code: String,
    pub name: String,
    pub instructors: Vec<String>,
    pub branches: Vec<String>,
    /// Effective meetings (after user overrides).
    pub meetings: Vec<Meeting>,
    pub starts: Option<(u8, String)>,
    pub part_of_semester: Option<String>,
}

impl IcsCourse {
    pub fn from_course(course: &Course, effective_meetings: Vec<Meeting>) -> IcsCourse {
        IcsCourse {
            code: course.code.clone(),
            // The display name: a calendar event repeating "(2 credits)"
            // every week says nothing a calendar reader can use.
            name: course.display_name(),
            instructors: course.instructors.clone(),
            branches: course.branches.clone(),
            meetings: effective_meetings,
            starts: course.starts.clone(),
            part_of_semester: course.part_of_semester.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IcsOptions {
    pub range_start: CivilDate,
    pub range_end: CivilDate,
    /// Add a 10-minute display alarm to every event.
    pub alarm: bool,
    pub app_url: String,
    /// UTC timestamp for DTSTAMP, e.g. "20260805T120000Z" — passed in so
    /// output is deterministic and js_sys::Date stays at the edges.
    pub dtstamp: String,
    pub calendar_name: String,
}

/// RFC 5545 TEXT escaping.
fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Fold a content line at 74 octets (RFC 5545 §3.1), splitting only at char
/// boundaries; continuation lines start with a single space.
fn fold_line(line: &str, out: &mut String) {
    const LIMIT: usize = 74;
    let mut budget = LIMIT;
    let mut current = String::new();
    for c in line.chars() {
        let w = c.len_utf8();
        if current.len() + w > budget {
            out.push_str(&current);
            out.push_str("\r\n ");
            current.clear();
            budget = LIMIT - 1; // continuation lines lose one octet to the space
        }
        current.push(c);
    }
    out.push_str(&current);
    out.push_str("\r\n");
}

fn push(out: &mut String, line: &str) {
    fold_line(line, out);
}

fn fmt_time(min: u16) -> String {
    format!("{:02}{:02}00", min / 60, min % 60)
}

/// Resolve a month token to a concrete date inside `[lo, hi]`. CMI notes
/// name only months, never years; the year is whichever one puts the date
/// inside the range, so year-crossing semesters (e.g. Dec–Mar) work too.
fn month_date_in_range(
    lo: CivilDate,
    hi: CivilDate,
    m: u8,
    day_of: impl Fn(i32) -> u8,
) -> Option<CivilDate> {
    (lo.y..=hi.y)
        .map(|y| CivilDate::new(y, m, day_of(y)))
        .find(|d| *d >= lo && *d <= hi)
}

/// Per-course date range: honor "(starts 12 Aug)" and part-of-semester
/// notes like "(Oct-Nov)" when parseable, clamped to the requested range.
/// A note that cannot be placed inside the range is ignored — a slightly
/// over-covering calendar beats silently missing events.
fn course_range(
    course: &IcsCourse,
    range_start: CivilDate,
    range_end: CivilDate,
) -> (CivilDate, CivilDate) {
    let mut start = range_start;
    let mut end = range_end;
    if let Some((d, mon)) = &course.starts
        && let Some(m) = month_from_token(mon)
    {
        let candidate = month_date_in_range(range_start, range_end, m, |y| {
            (*d).clamp(1, last_day_of_month(y, m))
        });
        if let Some(candidate) = candidate
            && candidate > start
        {
            start = candidate;
        }
    }
    if let Some(part) = &course.part_of_semester {
        let mut months = part.split(['-', '\u{2013}']);
        let m1 = months.next().and_then(month_from_token);
        if let Some(m1) = m1
            && let Some(candidate) = month_date_in_range(range_start, range_end, m1, |_| 1)
            && candidate > start
        {
            start = candidate;
        }
        // A single-month note ("(Sep)") ends in its own month; a range ends
        // in its second month.
        let second = months.next();
        let m2 = match second {
            Some(tok) => month_from_token(tok),
            None => m1,
        };
        if let Some(m2) = m2 {
            // Anchor the end month at or after the resolved start so a month
            // that occurs twice in a long range picks the right year.
            let candidate = month_date_in_range(start, range_end, m2, |y| last_day_of_month(y, m2));
            if let Some(candidate) = candidate
                && candidate < end
            {
                end = candidate;
            }
        }
    }
    // Fail-safe: an inverted clamp must widen, never drop events silently.
    if start > end {
        (range_start, range_end)
    } else {
        (start, end)
    }
}

pub fn build_ics(courses: &[IcsCourse], opts: &IcsOptions) -> String {
    let mut out = String::new();
    push(&mut out, "BEGIN:VCALENDAR");
    push(&mut out, "VERSION:2.0");
    push(
        &mut out,
        "PRODID:-//cmi-timetable//CMI Timetable Planner//EN",
    );
    push(&mut out, "CALSCALE:GREGORIAN");
    push(&mut out, "METHOD:PUBLISH");
    push(
        &mut out,
        &format!("X-WR-CALNAME:{}", escape_text(&opts.calendar_name)),
    );
    push(&mut out, "X-WR-TIMEZONE:Asia/Kolkata");
    push(&mut out, "BEGIN:VTIMEZONE");
    push(&mut out, "TZID:Asia/Kolkata");
    push(&mut out, "BEGIN:STANDARD");
    push(&mut out, "DTSTART:19700101T000000");
    push(&mut out, "TZOFFSETFROM:+0530");
    push(&mut out, "TZOFFSETTO:+0530");
    push(&mut out, "TZNAME:IST");
    push(&mut out, "END:STANDARD");
    push(&mut out, "END:VTIMEZONE");

    let mut sorted: Vec<&IcsCourse> = courses.iter().collect();
    sorted.sort_by(|a, b| a.code.cmp(&b.code));

    // UIDs must be unique across the file; a course CAN meet twice at the
    // same day+start (different halls, or user-added meetings), so the UID
    // covers day, start, end and hall — plus a counter for exact repeats.
    let mut seen_uids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for course in sorted {
        let (start, end) = course_range(course, opts.range_start, opts.range_end);
        let mut meetings: Vec<&Meeting> = course.meetings.iter().collect();
        meetings.sort_by_key(|m| (m.day.index(), m.slot.start_min, m.slot.end_min));

        for meeting in meetings {
            let first = start.first_on_or_after(meeting.day);
            if first > end {
                continue; // never occurs inside the range
            }
            let date = first.to_compact();
            // 23:59:59 IST on the last day == 18:29:59 UTC the same day.
            let until = format!("{}T182959Z", end.to_compact());
            let hall_slug: String = meeting
                .hall
                .as_deref()
                .unwrap_or("tba")
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            let base_uid = format!(
                "{}-{}-{:02}{:02}{:02}{:02}-{}@cmi-timetable",
                course.code,
                meeting.day.short(),
                meeting.slot.start_min / 60,
                meeting.slot.start_min % 60,
                meeting.slot.end_min / 60,
                meeting.slot.end_min % 60,
                hall_slug,
            );
            let mut uid = base_uid.clone();
            let mut n = 1;
            while !seen_uids.insert(uid.clone()) {
                n += 1;
                uid = format!("{base_uid}-{n}");
            }

            push(&mut out, "BEGIN:VEVENT");
            push(&mut out, &format!("UID:{}", escape_text(&uid)));
            push(&mut out, &format!("DTSTAMP:{}", opts.dtstamp));
            push(
                &mut out,
                &format!(
                    "DTSTART;TZID=Asia/Kolkata:{date}T{}",
                    fmt_time(meeting.slot.start_min)
                ),
            );
            push(
                &mut out,
                &format!(
                    "DTEND;TZID=Asia/Kolkata:{date}T{}",
                    fmt_time(meeting.slot.end_min)
                ),
            );
            push(&mut out, &format!("RRULE:FREQ=WEEKLY;UNTIL={until}"));
            push(
                &mut out,
                &format!(
                    "SUMMARY:{}",
                    escape_text(&format!("{}: {}", course.code, course.name))
                ),
            );
            push(
                &mut out,
                &format!(
                    "LOCATION:{}",
                    escape_text(meeting.hall.as_deref().unwrap_or("Hall TBA"))
                ),
            );
            let mut desc_parts: Vec<String> = Vec::new();
            if !course.instructors.is_empty() {
                desc_parts.push(format!("Instructor(s): {}", course.instructors.join(", ")));
            }
            if !course.branches.is_empty() {
                desc_parts.push(format!("Branches: {}", course.branches.join(", ")));
            }
            if !opts.app_url.is_empty() {
                desc_parts.push(opts.app_url.clone());
            }
            if !desc_parts.is_empty() {
                push(
                    &mut out,
                    &format!("DESCRIPTION:{}", escape_text(&desc_parts.join("\n"))),
                );
            }
            if opts.alarm {
                push(&mut out, "BEGIN:VALARM");
                push(&mut out, "ACTION:DISPLAY");
                push(
                    &mut out,
                    &format!(
                        "DESCRIPTION:{}",
                        escape_text(&format!("{} starts in 10 minutes", course.code))
                    ),
                );
                push(&mut out, "TRIGGER:-PT10M");
                push(&mut out, "END:VALARM");
            }
            push(&mut out, "END:VEVENT");
        }
    }

    push(&mut out, "END:VCALENDAR");
    out
}

/// "August--November 2026" → "cmi-timetable-aug-nov-2026.ics".
pub fn ics_filename(semester_label: &str) -> String {
    if let Some(caps) =
        regex_lite::Regex::new(r"([A-Za-z]{3,})\s*(?:--|\u{2013}|-)\s*([A-Za-z]{3,})\s+(\d{4})")
            .ok()
            .and_then(|re| re.captures(semester_label))
    {
        let short = |i: usize| {
            month_from_token(caps.get(i).unwrap().as_str())
                .map(|m| MONTH_SHORT[(m - 1) as usize].to_string())
        };
        if let (Some(a), Some(b)) = (short(1), short(2)) {
            return format!(
                "cmi-timetable-{a}-{b}-{}.ics",
                caps.get(3).unwrap().as_str()
            );
        }
    }
    "cmi-timetable.ics".to_string()
}

/// A meeting's day as it appears in aria labels / dialogs, kept here so the
/// exporter and UI agree on wording.
pub fn describe_day(day: Day) -> &'static str {
    day.full()
}
