//! Data model shared by the parser, the wasm app and the sync binary.

use serde::{Deserialize, Serialize};

/// Bump whenever parsing logic changes in a way that should trigger a
/// re-parse of the raw HTML stored inside a cached snapshot.
/// v2: looser semester-label detection + last-colon halls-legend split.
pub const PARSER_VERSION: u32 = 2;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Slot {
    pub start_min: u16,
    pub end_min: u16,
}

impl Slot {
    pub fn new(start_min: u16, end_min: u16) -> Slot {
        Slot { start_min, end_min }
    }

    pub fn overlaps(&self, other: &Slot) -> bool {
        self.start_min < other.end_min && other.start_min < self.end_min
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

    /// "Wed 14:00–15:15 · Lecture Hall 6" (hall part omitted when unknown).
    pub fn describe(&self) -> String {
        match &self.hall {
            Some(h) => format!("{} {} · {}", self.day.short(), self.slot.label(), h),
            None => format!("{} {} · Hall TBA", self.day.short(), self.slot.label()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// should use [`Course::effective_credits`], which assumes the campus
    /// default of 4.
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

    /// The stated credits, or the campus default of 4.
    pub fn effective_credits(&self) -> u8 {
        self.credits.unwrap_or(Self::DEFAULT_CREDITS)
    }

    /// True when the credits are the assumed default rather than stated.
    pub fn credits_assumed(&self) -> bool {
        self.credits.is_none()
    }
}

/// Where a snapshot's data came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceTier {
    Direct,
    Proxy(String),
    Mirror,
    Bundled,
}

impl SourceTier {
    pub fn label(&self) -> String {
        match self {
            SourceTier::Direct => "directly from cmi.ac.in".to_string(),
            SourceTier::Proxy(name) => format!("via proxy ({name})"),
            SourceTier::Mirror => "via mirror".to_string(),
            SourceTier::Bundled => "bundled with the app".to_string(),
        }
    }

    pub fn short_label(&self) -> String {
        match self {
            SourceTier::Direct => "direct".to_string(),
            SourceTier::Proxy(_) => "proxy".to_string(),
            SourceTier::Mirror => "mirror".to_string(),
            SourceTier::Bundled => "bundled".to_string(),
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
    /// Sorted by code.
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
    /// What the user wants.
    pub to: Meeting,
    pub created_at: f64,
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OverridesStore {
    pub next_id: u64,
    pub items: Vec<MeetingOverride>,
    /// At most one per course.
    #[serde(default)]
    pub credits: Vec<CreditOverride>,
}

impl OverridesStore {
    pub fn add(&mut self, course: &str, base: Option<Meeting>, to: Meeting, now: f64) -> u64 {
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

    pub fn for_course<'a>(&'a self, code: &'a str) -> impl Iterator<Item = &'a MeetingOverride> {
        self.items.iter().filter(move |o| o.course == code)
    }

    pub fn set_credits(&mut self, course: &str, credits: u8, now: f64) {
        match self.credits.iter_mut().find(|c| c.course == course) {
            Some(c) => c.credits = credits,
            None => self.credits.push(CreditOverride {
                course: course.to_string(),
                credits,
                created_at: now,
            }),
        }
    }

    pub fn remove_credits(&mut self, course: &str) {
        self.credits.retain(|c| c.course != course);
    }

    pub fn credits_for(&self, course: &str) -> Option<u8> {
        self.credits
            .iter()
            .find(|c| c.course == course)
            .map(|c| c.credits)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.credits.is_empty()
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
