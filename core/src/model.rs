//! Data model shared by the parser, the wasm app and the sync binary.

use serde::{Deserialize, Serialize};

/// Bump whenever parsing logic changes in a way that should trigger a
/// re-parse of the raw HTML stored inside a cached snapshot.
/// v2: looser semester-label detection + last-colon halls-legend split.
/// v3: drift-tolerant parsing — day-name variants (Tues/Thurs/case/
/// decoration), composition-based grid classification, dot/am-pm/"to" time
/// formats, pipe-less space-aligned grids, ragged-row nudging, month-span
/// notes in more forms (single month, full names), duration-aware assumed
/// credits, semantic semester-label comparison, garbage-detection gate
/// floors, hall matching by overlap.
///
/// v4: the five quiet misreadings of §8 — hall-grid day lines read even when
/// reworded ("Thursday - 6 Nov") and refused rather than merged when they
/// cannot be, a legend never credited to the branch above the grid it
/// belongs to, rows sliced at their own separator so a hall keeps its name,
/// course codes matched case-insensitively across the two pages, and course
/// notes taken from both legends instead of only the one whose name is
/// shown. A cached snapshot parsed by v3 is re-read on the next load.
pub const PARSER_VERSION: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Day {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Day {
    pub const ALL: [Day; 7] = [
        Day::Mon,
        Day::Tue,
        Day::Wed,
        Day::Thu,
        Day::Fri,
        Day::Sat,
        Day::Sun,
    ];

    pub fn from_short(token: &str) -> Option<Day> {
        match token {
            "Mon" => Some(Day::Mon),
            "Tue" => Some(Day::Tue),
            "Wed" => Some(Day::Wed),
            "Thu" => Some(Day::Thu),
            "Fri" => Some(Day::Fri),
            "Sat" => Some(Day::Sat),
            "Sun" => Some(Day::Sun),
            _ => None,
        }
    }

    pub fn from_full(token: &str) -> Option<Day> {
        match token {
            "Monday" => Some(Day::Mon),
            "Tuesday" => Some(Day::Tue),
            "Wednesday" => Some(Day::Wed),
            "Thursday" => Some(Day::Thu),
            "Friday" => Some(Day::Fri),
            "Saturday" => Some(Day::Sat),
            "Sunday" => Some(Day::Sun),
            _ => None,
        }
    }

    /// Tolerant matcher for grid ROW LABELS (from_short/from_full stay exact
    /// for canonical tokens): case-insensitive, accepts the full name, the
    /// 3-letter form and the common English variants (Tues/Weds/Thur/Thurs),
    /// and allows only decoration after the day word — "Monday (10 Aug)" and
    /// "Mon." match, "Monitor Room" and a "Mon-Fri" RANGE never do.
    pub fn from_label(label: &str) -> Option<Day> {
        let trimmed = label.trim();
        let word: String = trimmed
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        let day = match word.to_ascii_lowercase().as_str() {
            "mon" | "monday" => Day::Mon,
            "tue" | "tues" | "tuesday" => Day::Tue,
            "wed" | "weds" | "wednesday" => Day::Wed,
            "thu" | "thur" | "thurs" | "thursday" => Day::Thu,
            "fri" | "friday" => Day::Fri,
            "sat" | "saturday" => Day::Sat,
            "sun" | "sunday" => Day::Sun,
            _ => return None,
        };
        // Only decoration may follow the day word: punctuation, digits, a
        // parenthetical. A joiner means a RANGE of days ("Mon-Fri"), a
        // following word means this isn't a day row at all, and a SECOND
        // day word anywhere ("Mon, Wed") means a list — claiming such a
        // row for one day would silently drop the other's classes.
        let rest = trimmed[word.len()..].trim_start();
        match rest.chars().next() {
            Some('-' | '\u{2013}' | '\u{2014}' | '/' | '&' | '+') => return None,
            Some(c) if c.is_ascii_alphabetic() => return None,
            _ => {}
        }
        let names_second_day = rest.split(|c: char| !c.is_ascii_alphabetic()).any(|w| {
            matches!(
                w.to_ascii_lowercase().as_str(),
                "mon"
                    | "monday"
                    | "tue"
                    | "tues"
                    | "tuesday"
                    | "wed"
                    | "weds"
                    | "wednesday"
                    | "thu"
                    | "thur"
                    | "thurs"
                    | "thursday"
                    | "fri"
                    | "friday"
                    | "sat"
                    | "saturday"
                    | "sun"
                    | "sunday"
            )
        });
        if names_second_day {
            return None;
        }
        Some(day)
    }

    /// The day named by a line that is a SECTION HEADER rather than a row of
    /// data — specifically the hall grid's day lines, which carry no cells.
    ///
    /// `from_label` is deliberately strict, and must stay that way: it reads
    /// rows that CARRY CLASSES, so "Mon-Fri" or "Mon, Wed" has to be refused
    /// rather than claimed for one day. A line with nothing on it cannot
    /// move anyone's class by being misread, so the only question it raises
    /// is which single day it names — which lets this accept the wordings
    /// `from_label` cannot, such as "Thursday - 6 Nov".
    ///
    /// Still refuses every range and list: the day word must come first, and
    /// no OTHER day may be named anywhere in the line.
    pub fn from_section_header(label: &str) -> Option<Day> {
        let words: Vec<&str> = label
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        // A header is short. Anything long is prose, and prose that happens
        // to open with a day name is not a header.
        if words.is_empty() || words.len() > 5 {
            return None;
        }
        let day = Day::from_label(words[0])?;
        if words[1..]
            .iter()
            .filter_map(|w| Day::from_label(w))
            .any(|d| d != day)
        {
            return None;
        }
        Some(day)
    }

    pub fn short(&self) -> &'static str {
        match self {
            Day::Mon => "Mon",
            Day::Tue => "Tue",
            Day::Wed => "Wed",
            Day::Thu => "Thu",
            Day::Fri => "Fri",
            Day::Sat => "Sat",
            Day::Sun => "Sun",
        }
    }

    pub fn full(&self) -> &'static str {
        match self {
            Day::Mon => "Monday",
            Day::Tue => "Tuesday",
            Day::Wed => "Wednesday",
            Day::Thu => "Thursday",
            Day::Fri => "Friday",
            Day::Sat => "Saturday",
            Day::Sun => "Sunday",
        }
    }

    /// Mon = 0 … Sun = 6.
    pub fn index(&self) -> usize {
        *self as usize
    }
}

/// A lecture slot expressed in minutes from midnight, e.g. 09:10–10:25 is
/// `Slot { start_min: 550, end_min: 625 }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Slot {
    pub start_min: u16,
    pub end_min: u16,
}

impl Slot {
    pub fn new(start_min: u16, end_min: u16) -> Slot {
        Slot { start_min, end_min }
    }

    /// How many minutes of `other` fall inside this slot. Half-open, exactly
    /// like [`Slot::overlaps`], so a class ending at 14:00 shares nothing with
    /// a column starting at 14:00.
    pub fn overlap_minutes(&self, other: &Slot) -> u16 {
        let start = self.start_min.max(other.start_min);
        let end = self.end_min.min(other.end_min);
        end.saturating_sub(start)
    }

    pub fn overlaps(&self, other: &Slot) -> bool {
        self.start_min < other.end_min && other.start_min < self.end_min
    }

    /// A slot the app can honestly draw, state and export: it starts before
    /// it ends, and it does not run past midnight.
    ///
    /// The one definition of the rule. It was written out by hand in six
    /// places (`validate.rs`'s gate Rule 6 three times, `export.rs`'s backup
    /// `slot_ok`, `MeetingJson::to_meeting`, and the course editor) and was
    /// MISSING at the three doors that accept a stranger's bytes: a share
    /// link, a backup's own override/custom stores, and the localStorage
    /// blobs those two write. `start_min: 65535` from one hand-crafted `?s=`
    /// link therefore became a `1092:15-1092:15` column on My timetable, the
    /// Master grid and Halls, a `DTSTART;TZID=Asia/Kolkata:20260804T10921500`
    /// in the .ics (RFC 5545 3.3.5 allows six digits), and a
    /// `"start_min":65535` that survived every reload (R93 S1).
    ///
    /// **Drop-and-say, never clamp.** Clamping moves somebody's class to an
    /// hour nobody chose and then prints it as a fact; every caller here
    /// drops the entry and says so.
    pub fn is_sane(&self) -> bool {
        self.start_min < self.end_min && self.end_min <= 1440
    }

    fn fmt_min(min: u16) -> String {
        format!("{:02}:{:02}", min / 60, min % 60)
    }

    /// "09:10–10:25" (en dash, for display).
    pub fn label(&self) -> String {
        format!(
            "{}\u{2013}{}",
            Slot::fmt_min(self.start_min),
            Slot::fmt_min(self.end_min)
        )
    }

    pub fn start_label(&self) -> String {
        Slot::fmt_min(self.start_min)
    }

    pub fn end_label(&self) -> String {
        Slot::fmt_min(self.end_min)
    }
}

/// One weekly meeting of a course.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Meeting {
    pub day: Day,
    pub slot: Slot,
    pub hall: Option<String>,
    #[serde(default)]
    pub temp_booking: bool,
}

impl Meeting {
    /// Identity used by the three-way merge: day + slot + hall (a hall change
    /// upstream counts as a change), ignoring the TMP* decoration.
    pub fn same_place_time(&self, other: &Meeting) -> bool {
        self.day == other.day && self.slot == other.slot && self.hall == other.hall
    }

    /// A meeting whose time the app can draw — see [`Slot::is_sane`].
    pub fn is_sane(&self) -> bool {
        self.slot.is_sane()
    }

    /// "Wed 14:00–15:15 · Lecture Hall 6" (hall part omitted when unknown).
    pub fn describe(&self) -> String {
        match &self.hall {
            Some(h) => format!("{} {} · {}", self.day.short(), self.slot.label(), h),
            None => format!("{} {} · Hall TBA", self.day.short(), self.slot.label()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScheduleStatus {
    /// Appears in at least one branch grid.
    Scheduled,
    /// Appears only in a legend — a real offering without a fixed slot.
    UnscheduledListed,
    /// Appears in the hall-allocation grid but in no branch grid.
    ScheduledNoBranch,
}

pub type BranchCode = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub code: BranchCode,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Course {
    /// "MFD"
    pub code: String,
    /// Verbatim name, including parenthetical fragments.
    pub name: String,
    /// Split on '/'.
    pub instructors: Vec<String>,
    /// May be empty for `ScheduledNoBranch`.
    pub branches: Vec<BranchCode>,
    /// From "(2 credits)"; `None` when CMI doesn't state it — display code
    /// should use [`Course::effective_credits`], which assumes one credit
    /// per month for month-span courses and the campus default of 4
    /// otherwise.
    pub credits: Option<u8>,
    /// From "(starts 12 Aug)" → `(12, "Aug")`.
    pub starts: Option<(u8, String)>,
    /// From "(Oct-Nov)" → `"Oct-Nov"`.
    pub part_of_semester: Option<String>,
    /// '+' marker in a grid cell.
    pub optional_flag: bool,
    pub status: ScheduleStatus,
    /// Official (post-join) meetings, sorted by day then start time.
    pub meetings: Vec<Meeting>,
}

impl Course {
    /// CMI states credits only exceptionally (e.g. "(2 credits)"); regular
    /// courses default to 4.
    pub const DEFAULT_CREDITS: u8 = 4;

    /// How many calendar months a "(Oct-Nov)" / "(Sep)" note spans,
    /// inclusive; wraps across December ("Nov-Jan" → 3). `None` without a
    /// parseable note.
    pub fn months_span(&self) -> Option<u8> {
        let part = self.part_of_semester.as_deref()?;
        let mut months = part.split(['-', '\u{2013}']);
        let first = crate::date::month_from_token(months.next()?)?;
        match months.next() {
            None => Some(1),
            Some(tok) => {
                let second = crate::date::month_from_token(tok)?;
                Some((i16::from(second) - i16::from(first)).rem_euclid(12) as u8 + 1)
            }
        }
    }

    /// The name as displays should show it: CMI sometimes writes the credit
    /// count into the name itself — "Visualization(2 credits)" — and the
    /// parser reads that number into `credits`, so a card that shows credits
    /// beside the name would state it twice. Data stays verbatim: exports,
    /// the editor's name field and the what-changed diff all keep `name`.
    pub fn display_name(&self) -> String {
        crate::join::strip_credits_note(&self.name)
    }

    /// Does CMI's name for this course say it is a seminar? Seminars at CMI
    /// are attended, not credited, so when CMI states no credits the honest
    /// guess for one is 0 — not the campus default of 4. Matched on the
    /// word anywhere in the name ("Number Theory Seminar", "Seminar on X"),
    /// case-insensitively; a stated credit value always wins over this.
    pub fn is_seminar(&self) -> bool {
        self.name
            .to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphabetic())
            .any(|w| w == "seminar" || w == "seminars")
    }

    /// The credit value assumed when CMI states none, and why:
    /// - a seminar counts 0 — seminars are attended, not credited;
    /// - a course annotated with a month span shorter than a full semester
    ///   counts one credit per month ("(Oct-Nov)" → 2, "(Sep)" → 1) —
    ///   CMI's credits track contact time;
    /// - anything else gets the campus default of 4.
    pub fn assumed_credits(&self) -> u8 {
        match self.credit_assumption() {
            CreditAssumption::Seminar => 0,
            CreditAssumption::Months(n) => n.max(1),
            CreditAssumption::Default => Self::DEFAULT_CREDITS,
        }
    }

    /// Which rule `assumed_credits` used — so the UI can say WHY the number
    /// isn't simply 4, in words ("it's a seminar", "it runs Oct–Nov only").
    pub fn credit_assumption(&self) -> CreditAssumption {
        if self.is_seminar() {
            return CreditAssumption::Seminar;
        }
        match self.months_span() {
            Some(n) if n < Self::DEFAULT_CREDITS => CreditAssumption::Months(n),
            _ => CreditAssumption::Default,
        }
    }

    /// The stated credits, or the assumption above.
    pub fn effective_credits(&self) -> u8 {
        self.credits.unwrap_or_else(|| self.assumed_credits())
    }

    /// True when the credits are the assumed value rather than stated.
    pub fn credits_assumed(&self) -> bool {
        self.credits.is_none()
    }

    /// The month-span note ("Oct-Nov") when it is what determined the
    /// assumed credits — for UI copy explaining WHY the assumption isn't 4.
    pub fn duration_note(&self) -> Option<&str> {
        (self.credits.is_none() && matches!(self.credit_assumption(), CreditAssumption::Months(_)))
            .then_some(self.part_of_semester.as_deref())
            .flatten()
    }

    /// Build a user-created course. Meetings get the same ordering parsed
    /// courses have (day, then start time), and the schedule status derives
    /// from whether any meeting exists — a time-less course lands in the
    /// "No fixed slot yet" tray exactly like an unscheduled CMI offering.
    pub fn custom(
        code: String,
        name: String,
        instructors: Vec<String>,
        credits: u8,
        mut meetings: Vec<Meeting>,
    ) -> Course {
        meetings.sort_by_key(|m| (m.day.index(), m.slot.start_min, m.slot.end_min));
        Course {
            status: if meetings.is_empty() {
                ScheduleStatus::UnscheduledListed
            } else {
                ScheduleStatus::Scheduled
            },
            code,
            name,
            instructors,
            branches: Vec::new(),
            credits: Some(credits),
            starts: None,
            part_of_semester: None,
            optional_flag: false,
            meetings,
        }
    }
}

/// Which rule filled in a credit value CMI didn't state — so copy can say
/// why, not just how many. See [`Course::assumed_credits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditAssumption {
    /// The name says seminar: attended, not credited → 0.
    Seminar,
    /// A month-span note shorter than the semester → one credit per month.
    Months(u8),
    /// Nothing to go on → the campus default of 4.
    Default,
}

/// Where a snapshot's data came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceTier {
    Direct,
    Proxy(String),
    /// Legacy: the site used to host a copy of CMI's pages and serve it as a
    /// third tier. It doesn't any more — everything is fetched from cmi.ac.in
    /// itself. Kept ONLY so a snapshot written by an older build still
    /// deserializes; nothing produces it. The data in such a snapshot is real
    /// CMI data (it went through the same parser and gate), so unlike
    /// `Bundled` it is kept and simply re-synced on the next update.
    Mirror,
    /// Legacy: snapshots baked into pre-1.x app builds. Kept only so those
    /// stored snapshots still deserialize; the app discards them at load time.
    Bundled,
    /// A snapshot loaded from a file another student (or you) exported. The
    /// data inside is CMI's, parsed by this app's own parser when it was
    /// fetched — but THIS browser did not fetch it, and the pill says so.
    /// `fetched_at` stays the ORIGINAL fetch time: importing a file does
    /// not make old data young, and the staleness tint measures the data.
    Imported,
    /// CMI's own page, opened by the reader in their browser and handed to
    /// the app — pasted as source, or picked as a saved file. The bytes are
    /// CMI's and went through this app's own parser and gate like any other
    /// route; what is different is that no server in between had to agree to
    /// serve them. It is the one route that cannot stop working: a browser
    /// can always open a page (R84).
    Pasted,
    /// The empty placeholder before the first successful sync.
    None,
}

impl SourceTier {
    pub fn label(&self) -> String {
        match self {
            SourceTier::Direct => "directly from cmi.ac.in".to_string(),
            SourceTier::Proxy(name) => format!("through the helper site {name}"),
            SourceTier::Mirror => "from this site's old copy".to_string(),
            SourceTier::Bundled => "bundled with the app".to_string(),
            SourceTier::Imported => "imported from a file".to_string(),
            SourceTier::Pasted => "from CMI's page, loaded by you".to_string(),
            SourceTier::None => "nothing synced yet".to_string(),
        }
    }

    pub fn short_label(&self) -> String {
        match self {
            SourceTier::Direct => "direct".to_string(),
            SourceTier::Proxy(_) => "proxy".to_string(),
            SourceTier::Mirror => "old copy".to_string(),
            SourceTier::Bundled => "built-in copy".to_string(),
            SourceTier::Imported => "imported".to_string(),
            SourceTier::Pasted => "loaded by you".to_string(),
            SourceTier::None => "not synced".to_string(),
        }
    }
}

/// Raw page HTML, DEFLATE-compressed and base64-encoded so a shipped parser
/// fix can re-parse without refetching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawHtml {
    pub timetable_b64: String,
    pub lecturehalls_b64: String,
}

/// One cell of the hall-allocation grid, kept verbatim in the snapshot so
/// the Halls view and the free-hall finder reflect CMI's official
/// allocation — including `TMP*` bookings with no course code and bookings
/// that match no branch grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HallBooking {
    pub hall: String,
    pub day: Day,
    pub slot: Slot,
    pub codes: Vec<String>,
    #[serde(default)]
    pub temp: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Verbatim, e.g. "August--November 2026".
    pub semester_label: String,
    /// Milliseconds since the Unix epoch.
    pub fetched_at: f64,
    pub source: SourceTier,
    pub parser_version: u32,
    pub branches: Vec<Branch>,
    /// The catalog. NOT in `Course::code` order, and in no guaranteed order
    /// at all: the parser builds it from a map keyed on the UPPERCASED code
    /// (`join::fold`) while each course keeps CMI's own casing, and a
    /// snapshot restored from an imported backup file is deserialized in
    /// whatever order that file had. Look codes up with [`Snapshot::course`],
    /// or build a map — never binary search.
    pub courses: Vec<Course>,
    /// In hall-grid order.
    pub halls: Vec<String>,
    /// Canonical slot columns, in header order.
    pub slot_grid: Vec<Slot>,
    /// The hall-allocation grid, verbatim.
    #[serde(default)]
    pub hall_bookings: Vec<HallBooking>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_html_gz: Option<RawHtml>,
}

impl Snapshot {
    /// The empty state the app boots into before its first successful sync.
    /// The validation gate rejects anything without courses, so an empty
    /// course list is unambiguous: `!has_data()` ⇔ never synced.
    pub fn placeholder() -> Snapshot {
        Snapshot {
            semester_label: String::new(),
            fetched_at: 0.0,
            source: SourceTier::None,
            parser_version: crate::PARSER_VERSION,
            branches: Vec::new(),
            courses: Vec::new(),
            halls: Vec::new(),
            slot_grid: Vec::new(),
            hall_bookings: Vec::new(),
            raw_html_gz: None,
        }
    }

    pub fn has_data(&self) -> bool {
        !self.courses.is_empty()
    }

    /// Exact, case-sensitive match on the code as stored — nothing trimmed,
    /// nothing folded — answering with the FIRST course that carries it.
    /// Linear, and deliberately so: [`Snapshot::courses`] has no order to
    /// exploit. Anything resolving a whole LIST of codes should build a map
    /// once and reuse it rather than call this per code.
    pub fn course(&self, code: &str) -> Option<&Course> {
        self.courses.iter().find(|c| c.code == code)
    }

    /// Case-insensitive lookup for codes typed by people (share URLs). The
    /// catalog's own casing — whatever CMI uses — is canonical.
    pub fn course_ci(&self, code: &str) -> Option<&Course> {
        self.courses
            .iter()
            .find(|c| c.code.eq_ignore_ascii_case(code))
    }

    pub fn branch(&self, code: &str) -> Option<&Branch> {
        self.branches.iter().find(|b| b.code == code)
    }

    /// "August–November 2026" — `--` normalized to an en dash for display.
    pub fn semester_label_display(&self) -> String {
        display_semester_label(&self.semester_label)
    }
}

pub fn display_semester_label(raw: &str) -> String {
    raw.replace("--", "\u{2013}")
}

/// One user edit to one official meeting (or a created meeting for an
/// unscheduled course).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeetingOverride {
    pub id: u64,
    pub course: String,
    /// The official meeting AT THE TIME the override was made
    /// (`None` ⇒ user-created meeting for an unscheduled course).
    pub base: Option<Meeting>,
    /// What the user wants: a different time/place, or `None` to remove the
    /// meeting from their timetable entirely. Stored data and share links
    /// from before removals existed always carry a meeting here, so they
    /// deserialize unchanged.
    pub to: Option<Meeting>,
    pub created_at: f64,
}

/// Where a meeting is DRAWN on a week grid, and which other columns it only
/// reaches into. One rule, in one place, because the two answers have to agree:
/// a column is either the chip's home or a band, never both and never neither.
///
/// # The rule
/// **The class is drawn where most of it happens.** A class of 16:40–18:00 has
/// five minutes in the 15:30–16:45 column and a full hour in 17:00–18:15, so it
/// belongs to the later one and merely reaches back into the earlier — which is
/// how a reader looking for it would describe it. Before R97 the home column was
/// wherever the class STARTED, so those five minutes decided it: the chip sat in
/// a column it had almost left, and the column holding the actual class showed a
/// continuation band as if the class were somewhere else.
///
/// An exact start match still wins outright, ahead of the count. Every meeting
/// CMI publishes starts on a column boundary, so this keeps the whole official
/// timetable exactly where it has always been drawn and confines the new rule to
/// the free-form times a reader types or drags — which is where the problem was.
///
/// Ties go to the earlier column: a class split evenly across two reads as
/// starting in the first.
///
/// If nothing overlaps at all — a class in a gap between columns, or one typed
/// outside teaching hours before the grid grew a column for it — the nearest
/// column by start time answers, and `covered` is then empty: a band is only
/// ever drawn where the class genuinely runs.
pub fn place_meeting(slot_grid: &[Slot], meeting: &Slot) -> Placement {
    let home = home_column(slot_grid, meeting);
    let covered = slot_grid
        .iter()
        .filter(|s| s.overlaps(meeting))
        .map(|s| s.start_min)
        .filter(|c| Some(*c) != home)
        .collect();
    Placement { home, covered }
}

/// Which column a meeting is drawn in — see [`place_meeting`].
pub fn home_column(slot_grid: &[Slot], meeting: &Slot) -> Option<u16> {
    if let Some(s) = slot_grid.iter().find(|s| s.start_min == meeting.start_min) {
        return Some(s.start_min);
    }
    let best = slot_grid
        .iter()
        .map(|s| (s.overlap_minutes(meeting), s.start_min))
        .filter(|(overlap, _)| *overlap > 0)
        // Most minutes wins; a tie goes to the EARLIER column, which is what
        // `Reverse` on the start is doing inside a `max_by_key`.
        .max_by_key(|(overlap, start)| (*overlap, std::cmp::Reverse(*start)));
    if let Some((_, start)) = best {
        return Some(start);
    }
    slot_grid
        .iter()
        .min_by_key(|s| s.start_min.abs_diff(meeting.start_min))
        .map(|s| s.start_min)
}

/// Where one meeting sits on a grid: the column it is drawn in, and every other
/// column it genuinely runs through.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Placement {
    /// The column the chip is drawn in. `None` only for an empty grid.
    pub home: Option<u16>,
    /// Columns that show a band instead — unsorted, and never containing
    /// `home`. A column BEFORE `home` is possible and correct: it means the
    /// class begins there and finishes elsewhere.
    pub covered: Vec<u16>,
}

impl MeetingOverride {
    /// A removal: the user struck the base meeting from their timetable.
    pub fn is_removal(&self) -> bool {
        self.to.is_none()
    }

    /// An entry the app can hold without lying or crashing.
    ///
    /// Three rules, all about values no honest write path can produce:
    /// its times are drawable ([`Slot::is_sane`]); its id has a number after
    /// it (`id: u64::MAX` makes `OverridesStore::add`'s `next_id += 1`
    /// overflow — wrapping to 0 in release and PANICKING in any debug build —
    /// and forces `next_id` to collide with the live id, so `remove`'s
    /// `retain(|o| o.id != id)` deletes BOTH entries; R93 S8); and it names
    /// at least one class, the rule the FILE door already has
    /// (`MyChanges::into_stores`: "an entry that says nothing at all").
    pub fn is_sane(&self) -> bool {
        self.id != u64::MAX
            && (self.base.is_some() || self.to.is_some())
            && self.base.iter().chain(self.to.iter()).all(Meeting::is_sane)
    }
}

/// A user-set credit value for one course. The official value (stated by
/// CMI, or the assumed default) stays available for "official → yours"
/// displays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditOverride {
    pub course: String,
    pub credits: u8,
    pub created_at: f64,
}

impl CreditOverride {
    /// The largest number the app will ever describe as the reader's own
    /// credit figure. It is the range the editor offers and enforces
    /// (`ui.rs`: `min="0" max="20"`, and "Credits: enter a whole number from
    /// 0 to 20."), so it is the range every other door must enforce too —
    /// the editor was the ONLY door that did, and a friend's link or file
    /// then put "459 credits in total", "1 course at 255 credits" and "You
    /// set the credits on 2 courses yourself." on a reader's page for a
    /// number they never typed (R93 S5).
    pub const MAX: u8 = 20;

    pub fn is_sane(&self) -> bool {
        self.credits <= Self::MAX
    }
}

/// A course of CMI's that the user deleted from their planner. CMI's data is
/// never edited, so a deletion is stored the same way every other change is:
/// as a thing of theirs the user has overwritten — here, overwritten with
/// nothing. It hides the course everywhere the app *lists* courses, and is
/// undone by restoring it from "Your changes".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HiddenCourse {
    pub course: String,
    pub created_at: f64,
    /// Whether the course was on the user's timetable when it was deleted.
    /// Deleting took the selection too, so Restore gives both back. Absent
    /// (false) in data written before this was recorded — those restore the
    /// old way, to the catalog only. Everywhere selection is already
    /// explicit (share links, imports, add), the entry dies with its flag.
    #[serde(default)]
    pub was_selected: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OverridesStore {
    pub next_id: u64,
    pub items: Vec<MeetingOverride>,
    /// At most one per course.
    #[serde(default)]
    pub credits: Vec<CreditOverride>,
    /// Courses the user deleted. Absent in data written before deleting a
    /// CMI course was possible, and left out of what we write when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<HiddenCourse>,
}

impl OverridesStore {
    pub fn add(
        &mut self,
        course: &str,
        base: Option<Meeting>,
        to: Option<Meeting>,
        now: f64,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(MeetingOverride {
            id,
            course: course.to_string(),
            base,
            to,
            created_at: now,
        });
        id
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|o| o.id != id);
    }

    /// Drop every entry the app cannot state as a fact, and say how many
    /// went. The clamp law's other half: the rules the editor enforces on
    /// the way in have to be enforced at every OTHER way in — a share link,
    /// a backup file, and the localStorage blob those two write (R93 S1, S5,
    /// S8). Dropping, never clamping; every caller says so.
    ///
    /// `hidden` needs no rule — a deletion carries no time and no number.
    pub fn retain_sane(&mut self) -> usize {
        let before = self.items.len() + self.credits.len();
        self.items.retain(MeetingOverride::is_sane);
        self.credits.retain(CreditOverride::is_sane);
        self.bump_next_id();
        before - (self.items.len() + self.credits.len())
    }

    /// Nothing in here needs setting aside — the same question, asked
    /// without changing anything.
    pub fn is_sane(&self) -> bool {
        self.items.iter().all(MeetingOverride::is_sane)
            && self.credits.iter().all(CreditOverride::is_sane)
    }

    /// Move the COUNTER past every id in the store — the counter, never an
    /// id. This is the whole of S8's repair, and it is deliberately NOT a
    /// renumbering: `cmitt.v1.conflicts` (and a backup file's own
    /// `pending_conflicts`) hold `merge::Conflict::override_id` pointing INTO
    /// these items, and `merge::resolve_conflict` acts on them by id — so
    /// renumbering would silently re-aim a deferred "Use CMI's new time" at a
    /// different change. `checked_add`, so it is safe even before
    /// `retain_sane` has run.
    pub fn bump_next_id(&mut self) {
        if let Some(after) = self
            .items
            .iter()
            .map(|o| o.id)
            .max()
            .and_then(|m| m.checked_add(1))
            && self.next_id < after
        {
            self.next_id = after;
        }
    }

    pub fn for_course<'a>(&'a self, code: &'a str) -> impl Iterator<Item = &'a MeetingOverride> {
        self.items
            .iter()
            .filter(move |o| o.course.eq_ignore_ascii_case(code))
    }

    pub fn set_credits(&mut self, course: &str, credits: u8, now: f64) {
        match self
            .credits
            .iter_mut()
            .find(|c| c.course.eq_ignore_ascii_case(course))
        {
            Some(c) => c.credits = credits,
            None => self.credits.push(CreditOverride {
                course: course.to_string(),
                credits,
                created_at: now,
            }),
        }
    }

    pub fn remove_credits(&mut self, course: &str) {
        self.credits
            .retain(|c| !c.course.eq_ignore_ascii_case(course));
    }

    pub fn credits_for(&self, course: &str) -> Option<u8> {
        self.credits
            .iter()
            .find(|c| c.course.eq_ignore_ascii_case(course))
            .map(|c| c.credits)
    }

    /// Codes are compared case-insensitively throughout the store: a
    /// deletion can be made from a URL the user typed by hand, CMI's own
    /// casing is not something anyone should have to reproduce, and a
    /// student whose course is re-typed in another case upstream must not
    /// keep their deletion while quietly losing their credit correction and
    /// their moved classes. Two catalog codes differing only in case are one
    /// course typed twice, which is how [`Snapshot::course_ci`] reads them.
    pub fn is_hidden(&self, course: &str) -> bool {
        self.hidden
            .iter()
            .any(|h| h.course.eq_ignore_ascii_case(course))
    }

    pub fn hide(&mut self, course: &str, was_selected: bool, now: f64) {
        if !self.is_hidden(course) {
            self.hidden.push(HiddenCourse {
                course: course.to_string(),
                created_at: now,
                was_selected,
            });
        }
    }

    /// Was the course on the timetable when it was deleted? False when it
    /// isn't hidden at all, or was hidden by data from before the flag.
    pub fn hidden_was_selected(&self, course: &str) -> bool {
        self.hidden
            .iter()
            .find(|h| h.course.eq_ignore_ascii_case(course))
            .is_some_and(|h| h.was_selected)
    }

    /// `true` if the course was hidden and is now back.
    pub fn unhide(&mut self, course: &str) -> bool {
        let before = self.hidden.len();
        self.hidden
            .retain(|h| !h.course.eq_ignore_ascii_case(course));
        self.hidden.len() != before
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.credits.is_empty() && self.hidden.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Custom courses (user-created)
// ---------------------------------------------------------------------------

/// Courses the user created themselves — a seminar, a reading group, a
/// class from another institute. They reuse [`Course`] wholesale so every
/// downstream feature (clash detection, grids, credits, export, share)
/// treats them exactly like catalog courses; only their origin differs.
/// Lookups elsewhere resolve customs BEFORE the snapshot, so a later CMI
/// sync introducing the same code never silently replaces the user's data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CustomStore {
    pub courses: Vec<Course>,
}

impl CustomStore {
    pub fn get(&self, code: &str) -> Option<&Course> {
        self.courses
            .iter()
            .find(|c| c.code.eq_ignore_ascii_case(code))
    }

    /// Insert, or replace the course carrying the same code
    /// (case-insensitive — codes are user-typed).
    pub fn upsert(&mut self, course: Course) {
        match self
            .courses
            .iter_mut()
            .find(|c| c.code.eq_ignore_ascii_case(&course.code))
        {
            Some(existing) => *existing = course,
            None => self.courses.push(course),
        }
    }

    /// Drop every course carrying a class the app cannot draw, and say how
    /// many went — the `OverridesStore::retain_sane` rule, for the courses a
    /// share link or a backup file brings with it. A course loses its WHOLE
    /// entry rather than the one bad class: a course quietly missing a
    /// meeting is lying by omission (4, R83), and its code then resolves to
    /// nothing and becomes the same dismissible "unknown code" chip a link
    /// naming a retired course already produces.
    pub fn retain_sane(&mut self) -> usize {
        let before = self.courses.len();
        self.courses
            .retain(|c| c.meetings.iter().all(Meeting::is_sane));
        before - self.courses.len()
    }

    pub fn is_sane(&self) -> bool {
        self.courses
            .iter()
            .all(|c| c.meetings.iter().all(Meeting::is_sane))
    }

    pub fn remove(&mut self, code: &str) -> bool {
        let before = self.courses.len();
        self.courses.retain(|c| !c.code.eq_ignore_ascii_case(code));
        self.courses.len() != before
    }

    pub fn is_empty(&self) -> bool {
        self.courses.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Parse reporting
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParseStats {
    pub branch_grids: usize,
    pub branch_legends: usize,
    pub unique_courses: usize,
    pub halls: usize,
    pub hall_days: usize,
    /// Distinct course codes seen in any grid cell.
    pub grid_codes: usize,
    /// Of those, how many resolve to a legend entry.
    pub grid_codes_resolved: usize,
    pub meetings_total: usize,
    pub meetings_without_hall: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateCheck {
    pub rule: String,
    pub passed: bool,
    pub detail: String,
}

/// Per-branch parse statistics — shown in developer mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchStat {
    pub code: String,
    pub title: String,
    pub day_rows: usize,
    pub slots: usize,
    pub occurrences: usize,
    pub legend_entries: usize,
}

/// How each `<pre>` block was classified — shown in developer mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreClassification {
    pub page: String,
    pub index: usize,
    pub kind: String,
    pub first_line: String,
    pub line_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParseReport {
    pub stats: ParseStats,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub gate: Vec<GateCheck>,
    pub classifications: Vec<PreClassification>,
    #[serde(default)]
    pub branch_stats: Vec<BranchStat>,
}

impl ParseReport {
    pub fn gate_passed(&self) -> bool {
        !self.gate.is_empty() && self.gate.iter().all(|g| g.passed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything the user's overrides store holds is keyed by course code,
    /// and every one of those keys is matched the same way — otherwise a
    /// code CMI re-types in another case keeps the student's deletion while
    /// dropping their credit correction and their moved classes.
    #[test]
    fn the_store_reads_codes_the_same_way_throughout() {
        let meeting = Meeting {
            day: Day::Tue,
            slot: Slot::new(550, 625),
            hall: Some("Lecture Hall 803".to_string()),
            temp_booking: false,
        };
        let mut store = OverridesStore::default();
        store.add("TOC", Some(meeting), None, 0.0);
        store.set_credits("TOC", 2, 0.0);
        store.hide("QCOM", true, 0.0);

        for spelling in ["TOC", "toc", "Toc"] {
            assert_eq!(store.for_course(spelling).count(), 1, "{spelling}");
            assert_eq!(store.credits_for(spelling), Some(2), "{spelling}");
        }
        for spelling in ["QCOM", "qcom", "QCom"] {
            assert!(store.is_hidden(spelling), "{spelling}");
            assert!(store.hidden_was_selected(spelling), "{spelling}");
        }

        // Stored data from before was_selected existed still loads — the
        // field defaults to false, so such entries restore to the catalog
        // only, exactly as they did when they were written.
        let old_json = r#"{"next_id":1,"items":[],"credits":[],
            "hidden":[{"course":"OLD1","created_at":0.0}]}"#;
        let old: OverridesStore = serde_json::from_str(old_json).unwrap();
        assert!(old.is_hidden("OLD1"));
        assert!(!old.hidden_was_selected("OLD1"));

        // Setting again in another case corrects the entry rather than
        // adding a second one that shadows it.
        store.set_credits("toc", 3, 1.0);
        assert_eq!(store.credits.len(), 1);
        assert_eq!(store.credits_for("TOC"), Some(3));

        store.remove_credits("Toc");
        assert!(store.credits.is_empty());
        assert!(store.unhide("qcom"));
        assert!(!store.unhide("qcom"));
        assert!(!store.is_empty(), "the meeting override is still there");
    }
}
