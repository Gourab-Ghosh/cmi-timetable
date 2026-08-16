//! The two JSON file formats the app writes (and reads back):
//!
//! - `cmi-timetable-export` — the student's own week, to hand to another
//!   browser (their next one, or a friend's) and for programmatic analysis.
//!   It has two halves. `courses` is the readable one: every course as it is
//!   actually attended, explicit about everything (minutes AND "HH:MM",
//!   short day AND ISO weekday), owing nothing to the internal serde shapes.
//!   `my_changes` is the exact one: the classes moved, added or struck out,
//!   the credit corrections and the courses the student wrote themselves —
//!   so an import can put a week back the way it was instead of guessing it
//!   back from the readable half. It is written in the same explicit style,
//!   NOT in the app's storage shapes: a program reading the file should
//!   never have to know what this app calls things internally.
//!
//!   Both halves are always written, and both are read forgivingly: the
//!   decoration ("HH:MM" beside the minutes, the ISO weekday beside the day
//!   name, `kind` beside the two meetings it describes) is for whoever opens
//!   the file, and a program writing one can leave all of it out. Only what
//!   carries meaning is required. The readable half is built in /app, which
//!   owns course resolution; the round-trip half is defined and read HERE.
//!
//!   The two halves overlap on purpose. A file with only `courses` (this
//!   app's own, before the round-trip half existed, and anything another
//!   program writes) still imports — as the course codes it lists, which is
//!   all it ever meant.
//! - `cmi-planner-backup` — the WHOLE planner in one file: the downloaded
//!   timetable (the internal `Snapshot` serde JSON), the course selection,
//!   every override, the student's own courses, preferences and any
//!   conflicts they postponed. Importing it makes another browser look
//!   exactly like this one, years later, even if CMI's site has changed or
//!   gone. The envelope and the snapshot are validated HERE; the app-owned
//!   stores travel as opaque JSON values and the app deserializes them
//!   fail-closed on its side (they are /app types this crate cannot name).
//!
//! Import is fail-closed like everything else: nothing stored is touched
//! until every check passes, and each rejection carries copy that says what
//! the file actually was.

use crate::date::CivilDate;
use crate::model::{
    Course, CreditOverride, Day, Meeting, MeetingOverride, OverridesStore, ScheduleStatus, Slot,
    Snapshot,
};
use serde::{Deserialize, Serialize};

/// The version of the two formats this build writes. Semver: additions are
/// minor bumps, breaking changes major — and import accepts any major-1
/// file, ignoring keys it doesn't know (serde's default), so newer minor
/// files still load.
///
/// 1.1.0 added `my_changes` to `cmi-timetable-export`. Nothing was removed
/// or renamed, so 1.0.0 files import exactly as they always did and 1.1.0
/// files open in a build that predates the section.
pub const FORMAT_VERSION: &str = "1.1.0";

/// Epoch milliseconds → ISO 8601 UTC ("2026-08-14T13:02:05Z"). Pure civil
/// arithmetic on top of `date.rs`; no locale, no timezone surprises — file
/// timestamps must mean the same thing on every machine that reads them.
pub fn iso_utc(epoch_ms: f64) -> String {
    let total_secs = (epoch_ms / 1000.0).floor() as i64;
    let days = total_secs.div_euclid(86_400);
    let secs = total_secs.rem_euclid(86_400);
    // CivilDate counts days from 1970-01-01.
    let date = CivilDate::from_days(days);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        date.to_iso(),
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60,
    )
}

/// "August--November 2026" → "aug-nov-2026", the shared filename slug (the
/// same parse `ics_filename` uses). `None` when the label doesn't parse.
pub fn semester_slug(label: &str) -> Option<String> {
    let caps =
        regex_lite::Regex::new(r"([A-Za-z]{3,})\s*(?:--|\u{2013}|-)\s*([A-Za-z]{3,})\s+(\d{4})")
            .ok()?
            .captures(label)?;
    let short = |i: usize| {
        crate::date::month_from_token(caps.get(i)?.as_str())
            .map(|m| crate::date::month_short_name(m).to_ascii_lowercase())
    };
    Some(format!(
        "{}-{}-{}",
        short(1)?,
        short(2)?,
        caps.get(3)?.as_str()
    ))
}

/// `cmi-timetable-aug-nov-2026-2026-08-14.json` (or without the slug when
/// the label doesn't parse). `kind` is "timetable" or "planner"; `date` is
/// the date the NAME should carry — the export day for both (each names
/// the student's state on that day).
pub fn json_filename(kind: &str, semester_label: &str, date: CivilDate) -> String {
    match semester_slug(semester_label) {
        Some(slug) => format!("cmi-{kind}-{slug}-{}.json", date.to_iso()),
        None => format!("cmi-{kind}-{}.json", date.to_iso()),
    }
}

// ---------------------------------------------------------------------------
// `cmi-timetable-export`: the round-trip half
// ---------------------------------------------------------------------------

/// A time of day, both ways at once: `minutes` since midnight is the number
/// to compute with, `hhmm` the string to read. Reading trusts `minutes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeJson {
    pub minutes: u16,
    /// Written always; ignored on the way back in, so a program producing a
    /// file need only fill in `minutes`.
    #[serde(default)]
    pub hhmm: String,
}

impl TimeJson {
    fn new(minutes: u16) -> TimeJson {
        TimeJson {
            minutes,
            hhmm: format!("{:02}:{:02}", minutes / 60, minutes % 60),
        }
    }
}

/// One weekly class. Every key is always written — a field that appears only
/// sometimes is a field every reader has to write a branch for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingJson {
    /// "Mon" … "Sun". The truth on the way in; `iso_weekday` covers for it
    /// when a file leaves it out.
    #[serde(default)]
    pub day: String,
    /// 1 = Monday … 7 = Sunday, as ISO 8601 numbers them. Either this or
    /// `day` has to say which day it is; both is how this app writes it.
    #[serde(default)]
    pub iso_weekday: u8,
    pub start: TimeJson,
    pub end: TimeJson,
    /// `null` when CMI hasn't said where the class meets.
    #[serde(default)]
    pub hall: Option<String>,
    /// CMI marks some bookings TMP*; those are provisional rooms.
    #[serde(default)]
    pub temporary_booking: bool,
}

impl MeetingJson {
    pub fn from_meeting(m: &Meeting) -> MeetingJson {
        MeetingJson {
            day: m.day.short().to_string(),
            iso_weekday: m.day.index() as u8 + 1,
            start: TimeJson::new(m.slot.start_min),
            end: TimeJson::new(m.slot.end_min),
            hall: m.hall.clone(),
            temporary_booking: m.temp_booking,
        }
    }

    fn to_meeting(&self) -> Option<Meeting> {
        let day = Day::from_short(&self.day).or_else(|| {
            Day::ALL
                .get(self.iso_weekday.checked_sub(1)? as usize)
                .copied()
        })?;
        if self.start.minutes >= self.end.minutes || self.end.minutes > 1440 {
            return None;
        }
        Some(Meeting {
            day,
            slot: Slot::new(self.start.minutes, self.end.minutes),
            hall: self.hall.clone(),
            temp_booking: self.temporary_booking,
        })
    }
}

/// "(starts 12 Aug)", split so nobody has to parse the sentence back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartsJson {
    pub day: u8,
    /// Three-letter month, as CMI writes it: "Aug".
    pub month: String,
}

/// A course the student wrote themselves, in full — the recipient's browser
/// has never heard of it, so a bare code would mean nothing there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CourseJson {
    pub code: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub instructors: Vec<String>,
    #[serde(default)]
    pub branches: Vec<String>,
    /// `null` when no credit count was stated (the app then assumes one).
    #[serde(default)]
    pub credits: Option<u8>,
    #[serde(default)]
    pub starts: Option<StartsJson>,
    /// "Oct-Nov" for a course that runs across part of the semester.
    #[serde(default)]
    pub part_of_semester: Option<String>,
    #[serde(default)]
    pub optional_flag: bool,
    /// "scheduled", "unscheduled_listed" or "scheduled_no_branch". Anything
    /// else (or nothing) reads as "scheduled" — it decides how the app
    /// groups the course, never where its classes are.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub meetings: Vec<MeetingJson>,
}

impl CourseJson {
    pub fn from_course(c: &Course) -> CourseJson {
        CourseJson {
            code: c.code.clone(),
            name: c.name.clone(),
            instructors: c.instructors.clone(),
            branches: c.branches.clone(),
            credits: c.credits,
            starts: c.starts.as_ref().map(|(day, month)| StartsJson {
                day: *day,
                month: month.clone(),
            }),
            part_of_semester: c.part_of_semester.clone(),
            optional_flag: c.optional_flag,
            status: match c.status {
                ScheduleStatus::Scheduled => "scheduled",
                ScheduleStatus::UnscheduledListed => "unscheduled_listed",
                ScheduleStatus::ScheduledNoBranch => "scheduled_no_branch",
            }
            .to_string(),
            meetings: c.meetings.iter().map(MeetingJson::from_meeting).collect(),
        }
    }

    fn to_course(&self) -> Option<Course> {
        let code = self.code.trim();
        if code.is_empty() {
            return None;
        }
        let mut meetings: Vec<Meeting> = Vec::with_capacity(self.meetings.len());
        for m in &self.meetings {
            meetings.push(m.to_meeting()?);
        }
        meetings.sort_by_key(|m| (m.day.index(), m.slot.start_min));
        Some(Course {
            code: code.to_string(),
            name: self.name.clone(),
            instructors: self.instructors.clone(),
            branches: self.branches.clone(),
            credits: self.credits,
            starts: self.starts.as_ref().map(|s| (s.day, s.month.clone())),
            part_of_semester: self.part_of_semester.clone(),
            optional_flag: self.optional_flag,
            status: match self.status.as_str() {
                "unscheduled_listed" => ScheduleStatus::UnscheduledListed,
                "scheduled_no_branch" => ScheduleStatus::ScheduledNoBranch,
                _ => ScheduleStatus::Scheduled,
            },
            meetings,
        })
    }
}

/// One edit to one class. `kind` says in a word what `from` and `to` say in
/// full — filtering a file for every class somebody struck out should not
/// require reasoning about which field is null. Reading trusts `from`/`to`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeetingChangeJson {
    pub course: String,
    /// "moved" (CMI's class, elsewhere), "added" (a class CMI never listed)
    /// or "removed" (CMI's class, struck off this timetable).
    #[serde(default)]
    pub kind: String,
    /// The CMI class this replaces, as it stood when the change was made.
    /// `null` for an added class.
    #[serde(default)]
    pub from: Option<MeetingJson>,
    /// Where it goes. `null` means struck out. At least one of `from` and
    /// `to` has to be a class, or the entry changes nothing.
    #[serde(default)]
    pub to: Option<MeetingJson>,
    /// When the student made the change. `made_at_ms` is epoch
    /// milliseconds; `made_at` is the same instant as ISO 8601 UTC.
    #[serde(default)]
    pub made_at: String,
    #[serde(default)]
    pub made_at_ms: f64,
}

/// A course whose credit count the student corrected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditChangeJson {
    pub course: String,
    pub credits: u8,
    #[serde(default)]
    pub made_at: String,
    #[serde(default)]
    pub made_at_ms: f64,
}

/// Everything about a week that CMI's catalog cannot supply — the part of a
/// timetable that belongs to the student rather than to the campus.
///
/// Written in this format's own explicit style, not the app's internal
/// storage shapes: a program reading it should never have to know how this
/// app happens to store an "override". Every list is always present, so
/// `len(file["my_changes"]["meeting_changes"])` is safe on any file this
/// build writes.
///
/// Deletions of whole courses are deliberately absent — see
/// [`crate::combine`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MyChanges {
    /// Classes moved, created or struck out.
    #[serde(default)]
    pub meeting_changes: Vec<MeetingChangeJson>,
    #[serde(default)]
    pub credit_changes: Vec<CreditChangeJson>,
    /// Courses the student wrote themselves — a reading group, a seminar, a
    /// class at another institute.
    #[serde(default)]
    pub my_own_courses: Vec<CourseJson>,
}

impl MyChanges {
    /// Build the section from the app's stores.
    pub fn build(
        meetings: &[MeetingOverride],
        credits: &[CreditOverride],
        customs: &[Course],
    ) -> MyChanges {
        MyChanges {
            meeting_changes: meetings
                .iter()
                .map(|o| MeetingChangeJson {
                    course: o.course.clone(),
                    kind: match (&o.base, &o.to) {
                        (None, _) => "added",
                        (Some(_), None) => "removed",
                        (Some(_), Some(_)) => "moved",
                    }
                    .to_string(),
                    from: o.base.as_ref().map(MeetingJson::from_meeting),
                    to: o.to.as_ref().map(MeetingJson::from_meeting),
                    made_at: iso_utc(o.created_at),
                    made_at_ms: o.created_at,
                })
                .collect(),
            credit_changes: credits
                .iter()
                .map(|c| CreditChangeJson {
                    course: c.course.clone(),
                    credits: c.credits,
                    made_at: iso_utc(c.created_at),
                    made_at_ms: c.created_at,
                })
                .collect(),
            my_own_courses: customs.iter().map(CourseJson::from_course).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.meeting_changes.is_empty()
            && self.credit_changes.is_empty()
            && self.my_own_courses.is_empty()
    }

    /// Back into the app's stores. `None` when a change names a day that
    /// isn't a day, a class that ends before it starts, or a course with no
    /// code — load-bearing data that cannot be guessed at. The decoration
    /// (`hhmm`, `iso_weekday` beside a good `day`, `kind`, `made_at`,
    /// `status`) is never fatal.
    fn into_stores(self) -> Option<(Vec<MeetingOverride>, Vec<CreditOverride>, Vec<Course>)> {
        let mut items: Vec<MeetingOverride> = Vec::with_capacity(self.meeting_changes.len());
        for (i, c) in self.meeting_changes.iter().enumerate() {
            let course = c.course.trim();
            if course.is_empty() {
                return None;
            }
            // Neither a class to change nor a class to put in its place:
            // an entry that says nothing at all, which is a damaged file
            // rather than a change worth applying.
            if c.from.is_none() && c.to.is_none() {
                return None;
            }
            let convert = |m: &Option<MeetingJson>| match m {
                None => Some(None),
                Some(m) => m.to_meeting().map(Some),
            };
            items.push(MeetingOverride {
                // Renumbered at the door: two browsers both number from
                // zero, and `effective_meetings` tells one change from
                // another by id.
                id: i as u64,
                course: course.to_string(),
                base: convert(&c.from)?,
                to: convert(&c.to)?,
                created_at: c.made_at_ms,
            });
        }
        let mut credits: Vec<CreditOverride> = Vec::with_capacity(self.credit_changes.len());
        for c in &self.credit_changes {
            let course = c.course.trim();
            if course.is_empty() {
                return None;
            }
            credits.push(CreditOverride {
                course: course.to_string(),
                credits: c.credits,
                created_at: c.made_at_ms,
            });
        }
        let mut customs: Vec<Course> = Vec::with_capacity(self.my_own_courses.len());
        for c in &self.my_own_courses {
            customs.push(c.to_course()?);
        }
        Some((items, credits, customs))
    }
}

/// A `cmi-timetable-export` file, read back: the course codes it lists, the
/// student's changes to them, and the courses they made themselves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimetablePlan {
    /// Course codes, trimmed, deduped case-insensitively, file order kept.
    pub codes: Vec<String>,
    /// Renumbered from zero — the ids in a file are the sender's, and mean
    /// nothing in the store that receives them.
    pub overrides: OverridesStore,
    pub customs: Vec<Course>,
}

impl TimetablePlan {
    /// Did this file carry anything beyond the bare course list?
    pub fn has_changes(&self) -> bool {
        !self.overrides.is_empty() || !self.customs.is_empty()
    }
}

/// Read a `cmi-timetable-export` file. Lenient about everything it doesn't
/// need — extra keys, missing prose, another program's additions — and
/// fail-closed about everything it does: a file whose `my_changes` won't
/// parse is refused whole rather than imported as "the courses only", which
/// would quietly drop the half the student cared about.
///
/// The error is the exact student-facing sentence.
pub fn parse_timetable_export(text: &str) -> Result<TimetablePlan, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| {
        "That file couldn't be read — it may be damaged, or it may not be a \
         file this app made."
            .to_string()
    })?;
    let format = value.get("format").and_then(|f| f.as_str()).unwrap_or("");
    if format == "cmi-planner-backup" {
        return Err("That's an “Export everything” file, not a timetable — use \
             “Import everything…” under “Everything in one file” in My data \
             to load it."
            .to_string());
    }
    if format != "cmi-timetable-export" {
        return Err(
            "That file doesn't look like one this app made — nothing in it \
             says it came from “Export my courses”."
                .to_string(),
        );
    }
    let Some(courses) = value.get("courses").and_then(|c| c.as_array()) else {
        return Err("That file has no course list inside it.".to_string());
    };
    let mut codes: Vec<String> = Vec::new();
    for course in courses {
        // Trim BEFORE the duplicate check: the list stores trimmed codes,
        // so comparing an untrimmed candidate would let " TOC" slip past a
        // stored "TOC" and the same code would be reported twice.
        if let Some(code) = course.get("code").and_then(|c| c.as_str()).map(str::trim)
            && !code.is_empty()
            && !codes.iter().any(|c| c.eq_ignore_ascii_case(code))
        {
            codes.push(code.to_string());
        }
    }
    if codes.is_empty() {
        return Err("That file lists no courses at all.".to_string());
    }

    // Absent (a 1.0.0 file, or another program's) means "no changes", which
    // is different from "changes this app can't read".
    let bad_changes = || {
        "That file says it carries your changes, but they aren't the shape \
         this app can read — it may be damaged, or edited by hand. Nothing \
         was changed."
            .to_string()
    };
    let changes: MyChanges = match value.get("my_changes") {
        None | Some(serde_json::Value::Null) => MyChanges::default(),
        Some(v) => serde_json::from_value(v.clone()).map_err(|_| bad_changes())?,
    };
    let (items, credits, customs) = changes.into_stores().ok_or_else(bad_changes)?;

    Ok(TimetablePlan {
        overrides: OverridesStore {
            next_id: items.len() as u64,
            items,
            credits,
            hidden: Vec::new(),
        },
        customs,
        codes,
    })
}

/// The `cmi-planner-backup` envelope, as read. (Writing goes through
/// `planner_backup_json`, which controls key order.) The five store fields
/// are /app types this crate cannot name, so they stay `serde_json::Value`
/// here and the app deserializes each one fail-closed.
#[derive(Serialize, Deserialize)]
pub struct PlannerBackup {
    pub format: String,
    pub format_version: String,
    #[serde(default)]
    pub exported_at: String,
    // All defaulted (to JSON null) so a file missing one still parses as an
    // envelope — the parser then names the missing section honestly instead
    // of calling the whole file "not a backup".
    #[serde(default)]
    pub snapshot: serde_json::Value,
    #[serde(default)]
    pub selection: serde_json::Value,
    #[serde(default)]
    pub overrides: serde_json::Value,
    #[serde(default)]
    pub custom_courses: serde_json::Value,
    #[serde(default)]
    pub prefs: serde_json::Value,
    #[serde(default)]
    pub pending_conflicts: serde_json::Value,
}

/// Everything a validated backup gives the app: the snapshot (already
/// checked here) plus the raw store values for the app to deserialize.
#[derive(Debug)]
pub struct ParsedBackup {
    pub snapshot: Snapshot,
    pub selection: serde_json::Value,
    pub overrides: serde_json::Value,
    pub custom_courses: serde_json::Value,
    pub prefs: serde_json::Value,
    pub pending_conflicts: serde_json::Value,
}

/// Build the `cmi-planner-backup` file. `raw_html_gz` is stripped from the
/// snapshot — it multiplies the file size for a benefit (future parser
/// re-reads) the parsed data doesn't need; import tolerates its presence.
#[allow(clippy::too_many_arguments)]
pub fn planner_backup_json(
    snapshot: &Snapshot,
    selection: serde_json::Value,
    overrides: serde_json::Value,
    custom_courses: serde_json::Value,
    prefs: serde_json::Value,
    pending_conflicts: serde_json::Value,
    app_version: &str,
    git_commit: &str,
    exported_at_ms: f64,
) -> String {
    let mut slim = snapshot.clone();
    slim.raw_html_gz = None;
    serde_json::to_string_pretty(&serde_json::json!({
        "format": "cmi-planner-backup",
        "format_version": FORMAT_VERSION,
        "exported_at": iso_utc(exported_at_ms),
        "app": {
            "name": "cmi-timetable-planner",
            "version": app_version,
            "git_commit": git_commit,
        },
        "semester": {
            "label": snapshot.semester_label,
            "display": snapshot.semester_label_display(),
        },
        "snapshot": slim,
        "selection": selection,
        "overrides": overrides,
        "custom_courses": custom_courses,
        "prefs": prefs,
        "pending_conflicts": pending_conflicts,
    }))
    .unwrap_or_default()
}

/// Why an import was refused — each carries the exact student-facing copy,
/// so the app layer can't drift from the honest wording.
#[derive(Debug, PartialEq)]
pub enum ImportError {
    NotJson,
    WrongFormat(String),
    NewerFormat,
    BadSnapshot(String),
    /// A real backup envelope with a whole section missing — a truncated or
    /// hand-edited file, named by the missing part.
    MissingPart(&'static str),
    /// The `format` field says this IS a planner backup, but the envelope
    /// around it wouldn't parse (a missing or mistyped `format_version`,
    /// after hand-editing). Calling that "not a backup" would deny what the
    /// file itself says — name the real problem instead.
    BadEnvelope,
}

impl ImportError {
    pub fn message(&self) -> String {
        match self {
            ImportError::NotJson => {
                "That file couldn't be read — it may be damaged, or it may not \
                 be a file this app made."
                    .to_string()
            }
            ImportError::WrongFormat(found) if found == "cmi-timetable-export" => {
                "That file holds a timetable — the courses and the changes to \
                 them — but not a whole planner. Use “Import my courses…” \
                 under Share to load it."
                    .to_string()
            }
            ImportError::WrongFormat(_) => {
                "That file isn't a planner backup — nothing in it says it \
                 was made by this app's “Export everything”."
                    .to_string()
            }
            ImportError::NewerFormat => {
                "That file was made by a newer version of this app than the \
                 one you're using — reload this page to get the newest \
                 version, then try again."
                    .to_string()
            }
            ImportError::BadSnapshot(why) => {
                format!("The timetable inside that file couldn't be used: {why}")
            }
            ImportError::MissingPart(part) => {
                format!(
                    "Part of that backup is missing — the {part}. The file \
                     may be damaged or cut short. Nothing was changed."
                )
            }
            ImportError::BadEnvelope => "That file says it's a planner backup, but it doesn't say \
                 which version of the app made it — it may be damaged, or \
                 edited by hand. Nothing was changed."
                .to_string(),
        }
    }
}

/// Parse and validate a `cmi-planner-backup` file. Returns the snapshot with
/// its original `fetched_at` (the DATA's age — importing doesn't make old
/// data young) and whatever `source` the exporter recorded; the caller
/// overwrites `source` with `SourceTier::Imported`, because the pill must
/// say how THIS copy arrived, not how the exporter's did. `now_ms` guards
/// against files claiming to be fetched in the future. The store values are
/// returned raw for the app to deserialize fail-closed.
pub fn parse_planner_backup(text: &str, now_ms: f64) -> Result<ParsedBackup, ImportError> {
    let envelope: PlannerBackup = match serde_json::from_str(text) {
        Ok(e) => e,
        Err(_) => {
            // Distinguish "not JSON at all / not our shape" from "ours, but
            // the wrong kind" — two different honest error messages.
            let value: serde_json::Value =
                serde_json::from_str(text).map_err(|_| ImportError::NotJson)?;
            let found = value
                .get("format")
                .and_then(|f| f.as_str())
                .unwrap_or_default();
            // The file's own format field claiming OUR name means the
            // envelope around it is broken, not that the file is foreign.
            if found == "cmi-planner-backup" {
                return Err(ImportError::BadEnvelope);
            }
            return Err(ImportError::WrongFormat(found.to_string()));
        }
    };
    if envelope.format != "cmi-planner-backup" {
        return Err(ImportError::WrongFormat(envelope.format));
    }
    let major = envelope
        .format_version
        .split('.')
        .next()
        .and_then(|n| n.parse::<u32>().ok());
    if major != Some(1) {
        return Err(ImportError::NewerFormat);
    }
    for (value, name) in [
        (&envelope.snapshot, "timetable"),
        (&envelope.selection, "course selection"),
        (&envelope.overrides, "changes you made"),
        (&envelope.custom_courses, "courses you added"),
        (&envelope.prefs, "settings"),
    ] {
        if value.is_null() {
            return Err(ImportError::MissingPart(name));
        }
    }
    let snapshot: Snapshot = serde_json::from_value(envelope.snapshot).map_err(|_| {
        ImportError::BadSnapshot("it doesn't look the way this app saves it".to_string())
    })?;

    // Sanity — the raw pages aren't in the file, so the real validation gate
    // can't re-run; this is the honest subset that catches a mangled file.
    if snapshot.courses.is_empty() {
        return Err(ImportError::BadSnapshot(
            "it lists no courses at all".to_string(),
        ));
    }
    if snapshot.slot_grid.is_empty() {
        return Err(ImportError::BadSnapshot(
            "it doesn't list any class times".to_string(),
        ));
    }
    let slot_ok = |s: &crate::model::Slot| s.start_min < s.end_min && s.end_min <= 1440;
    if !snapshot.slot_grid.iter().all(slot_ok)
        || !snapshot
            .courses
            .iter()
            .flat_map(|c| &c.meetings)
            .all(|m| slot_ok(&m.slot))
    {
        return Err(ImportError::BadSnapshot(
            "some of its class times don't make sense".to_string(),
        ));
    }
    if snapshot.semester_label.trim().is_empty() {
        return Err(ImportError::BadSnapshot(
            "it doesn't say which semester it is".to_string(),
        ));
    }
    if snapshot.fetched_at <= 0.0 || snapshot.fetched_at > now_ms + 86_400_000.0 {
        return Err(ImportError::BadSnapshot(
            "it doesn't say when it was downloaded from CMI, or the date it \
             gives is in the future"
                .to_string(),
        ));
    }
    Ok(ParsedBackup {
        snapshot,
        selection: envelope.selection,
        overrides: envelope.overrides,
        custom_courses: envelope.custom_courses,
        prefs: envelope.prefs,
        pending_conflicts: envelope.pending_conflicts,
    })
}
