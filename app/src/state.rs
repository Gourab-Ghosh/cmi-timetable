//! Application state: one `App` handle of copyable signals provided through
//! context, plus every user action (all undoable ones go through `act`).

use crate::{domx, storage};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use ttcore::diff::SnapshotDiff;
use ttcore::merge::Conflict;
use ttcore::model::{
    Course, CustomStore, Day, Meeting, OverridesStore, ParseReport, ScheduleStatus, Slot,
    Snapshot, SourceTier,
};

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_COMMIT: &str = env!("APP_GIT_COMMIT");
pub const BUILD_TIME: &str = env!("APP_BUILD_TIME");

// ---------------------------------------------------------------------------
// Preferences & filters (persisted)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemePref {
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Tab {
    #[default]
    MyTimetable,
    MyCourses,
    MasterGrid,
    Catalog,
    Halls,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::MyTimetable,
        Tab::MyCourses,
        Tab::MasterGrid,
        Tab::Catalog,
        Tab::Halls,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Tab::MyTimetable => "My timetable",
            Tab::MyCourses => "My courses",
            Tab::MasterGrid => "Master grid",
            Tab::Catalog => "Catalog",
            Tab::Halls => "Halls",
        }
    }
}

/// Facets are multi-select: OR within a facet, AND across facets.
#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Filters {
    pub branches: Vec<String>,
    pub instructors: Vec<String>,
    pub days: Vec<Day>,
    pub slot_starts: Vec<u16>,
    pub halls: Vec<String>,
    /// "2", "4", … or "?" for unknown.
    pub credits: Vec<String>,
    /// "optional", "unscheduled", "custom".
    pub flags: Vec<String>,
    /// Specific course codes picked in the Course dropdown.
    pub courses: Vec<String>,
    pub text: String,
    /// "Fits my schedule": hide anything overlapping the current selection.
    pub fits: bool,
}

impl Filters {
    pub fn active_count(&self) -> usize {
        self.branches.len()
            + self.instructors.len()
            + self.days.len()
            + self.slot_starts.len()
            + self.halls.len()
            + self.credits.len()
            + self.flags.len()
            + self.courses.len()
            + usize::from(!self.text.trim().is_empty())
            + usize::from(self.fits)
    }

    pub fn is_empty(&self) -> bool {
        self.active_count() == 0
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: ThemePref,
    pub density: Density,
    pub filters: Filters,
    /// ms since epoch of the last automatic update attempt (12 h throttle).
    pub last_update_attempt: f64,
    pub tab: Tab,
    pub halls_day: Day,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            theme: ThemePref::default(),
            density: Density::default(),
            filters: Filters::default(),
            last_update_attempt: 0.0,
            tab: Tab::default(),
            halls_day: Day::Mon,
        }
    }
}

// ---------------------------------------------------------------------------
// Transient state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Planner,
    Developer,
}

#[derive(Clone, PartialEq)]
pub struct SyncMeta {
    pub fetched_at: f64,
    pub source: SourceTier,
    pub updating: bool,
    /// Live per-tier progress line while updating ("trying proxy 1 of 2…").
    pub progress: String,
}

#[derive(Clone, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub undo: bool,
}

thread_local! {
    /// Toasts currently under the pointer or holding keyboard focus —
    /// auto-dismiss pauses for them so the reader sets the pace, not the
    /// timer. Deliberately NOT a signal: hover must not re-render toasts.
    static HOVERED_TOASTS: std::cell::RefCell<std::collections::HashSet<u64>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BannerKind {
    Info,
    Warn,
}

#[derive(Clone, PartialEq)]
pub struct Banner {
    pub kind: BannerKind,
    pub text: String,
    /// Sticky banners (e.g. the corrupt-data notice) survive the start of
    /// the next update attempt; ordinary failure banners are cleared there.
    pub sticky: bool,
}

#[derive(Clone, PartialEq)]
pub enum Dialog {
    /// Course details popover (any compact rendering opens this).
    Details(String),
    /// "My data": everything saved in the browser, with removal options.
    MyData,
    /// Create/edit a meeting override. `create` = "Give it a time".
    EditMeeting {
        course: String,
        ov_id: Option<u64>,
        base: Option<Meeting>,
        init: Meeting,
        create: bool,
    },
    Conflicts,
    Export { scope: Option<String> },
    Share,
    WhatChanged,
    /// Create (`edit: None`) or edit one of the user's own courses.
    /// `prefill` seeds the name field from a failed catalog search.
    CustomCourse {
        edit: Option<String>,
        prefill: Option<String>,
    },
}

/// An in-flight pointer drag.
#[derive(Clone, PartialEq)]
pub struct DragState {
    pub spec: DragSpec,
    pub pointer_id: i32,
    /// Threshold/long-press passed; ghost is showing.
    pub started: bool,
    pub start_x: f64,
    pub start_y: f64,
    pub x: f64,
    pub y: f64,
    pub over: Option<(Day, u16)>,
    /// Set when hovering a Halls-view cell: dropping there also moves the
    /// meeting into that row's hall.
    pub over_hall: Option<String>,
    /// Touch drags lift only after a 350 ms long-press.
    pub awaiting_longpress: bool,
}

#[derive(Clone, PartialEq)]
pub struct DragSpec {
    pub code: String,
    /// Existing override being moved (its id), if any.
    pub ov_id: Option<u64>,
    /// The official meeting being moved (None ⇒ unscheduled tray / created).
    pub base: Option<Meeting>,
    /// Hall carried over into the drop (editable afterwards).
    pub hall: Option<String>,
    pub from_master: bool,
    pub label: String,
}

/// Keyboard move mode: focus a chip → M → arrows → Enter.
#[derive(Clone, PartialEq)]
pub struct MoveMode {
    pub spec: DragSpec,
    pub cursor: (Day, u16),
}

#[derive(Clone)]
pub struct UndoEntry {
    pub label: String,
    pub selection: Vec<String>,
    pub overrides: OverridesStore,
    /// Filter changes are undoable too, so every entry carries the filter
    /// state alongside selection + overrides.
    pub filters: Filters,
    /// The user's own courses ride the history too, so deleting or editing
    /// one is as undoable as any other change.
    pub customs: CustomStore,
}

#[derive(Clone, Default)]
pub struct UndoStack {
    pub undo: Vec<UndoEntry>,
    pub redo: Vec<UndoEntry>,
}

const UNDO_MAX: usize = 100;

#[derive(Clone, PartialEq)]
pub struct FetchLogEntry {
    pub at: f64,
    pub tier: String,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: f64,
    pub bytes: usize,
    pub error: Option<String>,
}

#[derive(Clone, PartialEq)]
pub struct StoredReport {
    pub at: f64,
    pub source: String,
    pub report: ParseReport,
}

/// One effective (post-override) meeting of a course.
#[derive(Clone, PartialEq)]
pub struct EffMeeting {
    pub meeting: Meeting,
    pub overridden: bool,
    pub ov_id: Option<u64>,
    pub base: Option<Meeting>,
    pub user_created: bool,
}

#[derive(Clone, PartialEq)]
pub struct ClashPair {
    pub a: String,
    pub b: String,
    pub day: Day,
    pub a_slot: Slot,
    pub b_slot: Slot,
}

// ---------------------------------------------------------------------------
// The App handle
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct App {
    pub snapshot: RwSignal<Snapshot>,
    pub sync: RwSignal<SyncMeta>,
    pub selection: RwSignal<Vec<String>>,
    pub overrides: RwSignal<OverridesStore>,
    /// Courses the user created themselves (always selected while they exist).
    pub customs: RwSignal<CustomStore>,
    pub prefs: RwSignal<Prefs>,
    pub undo_stack: RwSignal<UndoStack>,
    pub toasts: RwSignal<Vec<Toast>>,
    pub toast_seq: RwSignal<u64>,
    pub banner: RwSignal<Option<Banner>>,
    pub conflicts: RwSignal<Vec<Conflict>>,
    pub what_changed: RwSignal<Option<SnapshotDiff>>,
    /// Selected codes that vanished upstream ("No longer on CMI's timetable").
    pub removed_upstream: RwSignal<Vec<String>>,
    /// Unknown codes from a shared URL (dismissible warning chips).
    pub unknown_codes: RwSignal<Vec<String>>,
    pub fetch_log: RwSignal<Vec<FetchLogEntry>>,
    pub reports: RwSignal<Vec<StoredReport>>,
    pub route: RwSignal<Route>,
    pub dialog: RwSignal<Option<Dialog>>,
    pub drag: RwSignal<Option<DragState>>,
    pub move_mode: RwSignal<Option<MoveMode>>,
    /// Developer-mode simulator: force a specific tier on the next update.
    pub force_tier: RwSignal<Option<String>>,
    /// aria-live announcements (keyboard move mode, etc.).
    pub announce: RwSignal<String>,
    /// Drag & drop (pointer and keyboard move mode) only works while edit
    /// mode is on — toggled per session from the grid toolbars.
    pub edit_mode: RwSignal<bool>,
}

impl App {
    pub fn use_ctx() -> App {
        expect_context::<App>()
    }

    // -- feedback ----------------------------------------------------------

    pub fn toast(&self, text: impl Into<String>) {
        self.push_toast(text.into(), false);
    }

    pub fn toast_undo(&self, text: impl Into<String>) {
        self.push_toast(text.into(), true);
    }

    fn push_toast(&self, text: String, undo: bool) {
        let id = self.toast_seq.get_untracked() + 1;
        self.toast_seq.set(id);
        self.toasts.update(|t| t.push(Toast { id, text, undo }));
        let toasts = self.toasts;
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(6000).await;
            // A hovered (or focused) toast stays until the reader lets go.
            while HOVERED_TOASTS.with(|h| h.borrow().contains(&id)) {
                gloo_timers::future::TimeoutFuture::new(700).await;
            }
            toasts.update(|t| t.retain(|x| x.id != id));
        });
    }

    pub fn dismiss_toast(&self, id: u64) {
        HOVERED_TOASTS.with(|h| {
            h.borrow_mut().remove(&id);
        });
        self.toasts.update(|t| t.retain(|x| x.id != id));
    }

    pub fn set_toast_hovered(&self, id: u64, hovered: bool) {
        HOVERED_TOASTS.with(|h| {
            if hovered {
                h.borrow_mut().insert(id);
            } else {
                h.borrow_mut().remove(&id);
            }
        });
    }

    pub fn set_banner(&self, kind: BannerKind, text: impl Into<String>) {
        // A transient banner (e.g. "couldn't sync") must never clobber a
        // sticky notice (e.g. "your data was set aside — nothing deleted"):
        // the sticky one carries information the user can't recover.
        if self.banner.with_untracked(|b| b.as_ref().is_some_and(|b| b.sticky)) {
            return;
        }
        self.banner.set(Some(Banner {
            kind,
            text: text.into(),
            sticky: false,
        }));
    }

    pub fn set_banner_sticky(&self, kind: BannerKind, text: impl Into<String>) {
        self.banner.set(Some(Banner {
            kind,
            text: text.into(),
            sticky: true,
        }));
    }

    /// Clear any non-sticky banner (called when a new update attempt starts).
    pub fn clear_transient_banner(&self) {
        self.banner.update(|b| {
            if b.as_ref().is_some_and(|b| !b.sticky) {
                *b = None;
            }
        });
    }

    pub fn say(&self, text: impl Into<String>) {
        self.announce.set(text.into());
    }

    // -- persistence + URL -------------------------------------------------

    pub fn persist_selection(&self) {
        let _ = storage::save(storage::KEY_SELECTION, &self.selection.get_untracked());
        self.sync_url();
    }

    pub fn persist_overrides(&self) {
        let _ = storage::save(storage::KEY_OVERRIDES, &self.overrides.get_untracked());
    }

    pub fn persist_customs(&self) {
        let _ = storage::save(storage::KEY_CUSTOM, &self.customs.get_untracked());
    }

    pub fn persist_prefs(&self) {
        let _ = storage::save(storage::KEY_PREFS, &self.prefs.get_untracked());
    }

    /// Keep `?c=` canonical on every selection change (replaceState). Any
    /// `s=` payload is consumed at load time and dropped here.
    pub fn sync_url(&self) {
        let selection = self.selection.get_untracked();
        if selection.is_empty() {
            domx::replace_query("");
        } else {
            domx::replace_query(&format!(
                "?c={}",
                ttcore::share::selection_to_c_param(&selection)
            ));
        }
    }

    // -- undo / redo ---------------------------------------------------------

    fn push_undo(&self, label: &str) {
        let entry = UndoEntry {
            label: label.to_string(),
            selection: self.selection.get_untracked(),
            overrides: self.overrides.get_untracked(),
            filters: self.prefs.with_untracked(|p| p.filters.clone()),
            customs: self.customs.get_untracked(),
        };
        self.undo_stack.update(|s| {
            s.undo.push(entry);
            if s.undo.len() > UNDO_MAX {
                s.undo.remove(0);
            }
            s.redo.clear();
        });
    }

    /// The state captured right now, for moving between the undo/redo stacks.
    fn current_entry(&self, label: &str) -> UndoEntry {
        UndoEntry {
            label: label.to_string(),
            selection: self.selection.get_untracked(),
            overrides: self.overrides.get_untracked(),
            filters: self.prefs.with_untracked(|p| p.filters.clone()),
            customs: self.customs.get_untracked(),
        }
    }

    /// Restore one history entry (shared by undo and redo).
    fn apply_entry(&self, entry: &UndoEntry) {
        self.selection.set(entry.selection.clone());
        self.overrides.set(entry.overrides.clone());
        self.prefs.update(|p| p.filters = entry.filters.clone());
        self.customs.set(entry.customs.clone());
        self.persist_selection();
        self.persist_overrides();
        self.persist_prefs();
        self.persist_customs();
    }

    pub fn can_undo(&self) -> bool {
        self.undo_stack.with(|s| !s.undo.is_empty())
    }

    pub fn can_redo(&self) -> bool {
        self.undo_stack.with(|s| !s.redo.is_empty())
    }

    /// Every undoable action funnels through here: one entry on the stack,
    /// mutate, persist, sync URL.
    pub fn act(&self, label: &str, f: impl FnOnce(&mut Vec<String>, &mut OverridesStore)) {
        self.push_undo(label);
        let mut selection = self.selection.get_untracked();
        let mut overrides = self.overrides.get_untracked();
        f(&mut selection, &mut overrides);
        self.selection.set(selection);
        self.overrides.set(overrides);
        self.persist_selection();
        self.persist_overrides();
    }

    /// `act` for mutations that also touch the user's own courses.
    pub fn act_customs(
        &self,
        label: &str,
        f: impl FnOnce(&mut CustomStore, &mut Vec<String>, &mut OverridesStore),
    ) {
        self.push_undo(label);
        let mut customs = self.customs.get_untracked();
        let mut selection = self.selection.get_untracked();
        let mut overrides = self.overrides.get_untracked();
        f(&mut customs, &mut selection, &mut overrides);
        self.customs.set(customs);
        self.selection.set(selection);
        self.overrides.set(overrides);
        self.persist_customs();
        self.persist_selection();
        self.persist_overrides();
    }

    pub fn undo(&self) {
        let entry = self.undo_stack.try_update(|s| s.undo.pop()).flatten();
        if let Some(entry) = entry {
            let current = self.current_entry(&entry.label);
            self.undo_stack.update(|s| s.redo.push(current));
            self.apply_entry(&entry);
            self.toast(format!("Undid: {}", entry.label));
        }
    }

    pub fn redo(&self) {
        let entry = self.undo_stack.try_update(|s| s.redo.pop()).flatten();
        if let Some(entry) = entry {
            let current = self.current_entry(&entry.label);
            self.undo_stack.update(|s| s.undo.push(current));
            self.apply_entry(&entry);
            self.toast(format!("Redid: {}", entry.label));
        }
    }

    // -- selection -----------------------------------------------------------

    pub fn is_selected(&self, code: &str) -> bool {
        self.selection.with(|s| s.iter().any(|c| c == code))
    }

    pub fn add_course(&self, code: &str) {
        if self.is_selected(code) {
            return;
        }
        let code = code.to_string();
        self.act(&format!("add {code}"), |sel, _| {
            if !sel.contains(&code) {
                sel.push(code.clone());
            }
        });
        // Warn immediately (never block) when the new course clashes.
        let clashing = self.clashing_partners(&code);
        if clashing.is_empty() {
            self.toast_undo(format!("Added {code}"));
        } else {
            self.toast_undo(format!(
                "Added {code} — ⚠ clashes with {}",
                clashing.join(", ")
            ));
        }
    }

    /// Distinct courses this (selected) course clashes with, with day/time.
    pub fn clashing_partners(&self, code: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for c in self.clashes() {
            let other = if c.a == code {
                &c.b
            } else if c.b == code {
                &c.a
            } else {
                continue;
            };
            let entry = format!("{other} ({} {})", c.day.short(), c.a_slot.label());
            if !out.contains(&entry) {
                out.push(entry);
            }
        }
        out
    }

    /// Deselecting keeps the course's custom times: re-adding it (or looking
    /// at it in the master grid) must not silently revert a move. Custom
    /// times are removed explicitly via "My data" or per-meeting resets.
    pub fn remove_course(&self, code: &str) {
        if !self.is_selected(code) {
            return;
        }
        let code = code.to_string();
        self.act(&format!("remove {code}"), |sel, _| {
            sel.retain(|c| c != &code);
        });
        self.removed_upstream.update(|r| r.retain(|c| c != &code));
        self.toast_undo(format!("Removed {code}"));
    }

    pub fn toggle_select(&self, code: &str) {
        if self.is_selected(code) {
            self.remove_course(code);
        } else {
            self.add_course(code);
        }
    }

    // -- custom courses --------------------------------------------------------

    /// The user's own definition for a code, if they created one.
    pub fn custom_course(&self, code: &str) -> Option<Course> {
        self.customs.with(|cs| cs.get(code).cloned())
    }

    pub fn is_custom(&self, code: &str) -> bool {
        self.customs.with(|cs| cs.get(code).is_some())
    }

    /// Resolve a code to its course: the user's own courses first (their
    /// data must never be shadowed by a later CMI sync), then the catalog.
    pub fn course_by_code(&self, code: &str) -> Option<Course> {
        self.custom_course(code)
            .or_else(|| self.snapshot.with(|s| s.course_ci(code).cloned()))
    }

    /// A custom course whose code CMI's catalog ALSO lists now (it appeared
    /// in a sync after the course was created) — surfaced as a quiet note,
    /// the user's own definition keeps winning.
    pub fn custom_shadows_official(&self, code: &str) -> bool {
        self.is_custom(code) && self.snapshot.with(|s| s.course_ci(code).is_some())
    }

    /// Create a course of the user's own, or save edits to one.
    /// `original_code` is the code it had before an edit (a rename follows
    /// through into the selection). A custom course's definition is the
    /// single source of truth for its times — it never carries overrides
    /// (drags and edits write the definition directly), so saving purges
    /// any foreign override that targets its code. Creating selects the
    /// course; editing leaves its selected/parked state alone.
    pub fn save_custom_course(&self, original_code: Option<&str>, course: Course) {
        let creating = original_code.is_none();
        let new_code = course.code.clone();
        let old_code = original_code.unwrap_or(&new_code).to_string();
        let label = if creating {
            format!("add your course {new_code}")
        } else {
            format!("edit your course {old_code}")
        };
        self.act_customs(&label, |customs, sel, ovs| {
            if !old_code.eq_ignore_ascii_case(&new_code) {
                customs.remove(&old_code);
                for entry in sel.iter_mut() {
                    if entry.eq_ignore_ascii_case(&old_code) {
                        *entry = new_code.clone();
                    }
                }
                // The new code may already sit in the selection — a course
                // CMI dropped keeps its slot there, and the form can't
                // reject a code the catalog no longer has. Keep the first.
                let mut seen: Vec<String> = Vec::new();
                sel.retain(|c| {
                    let dup = seen.iter().any(|s: &String| s.eq_ignore_ascii_case(c));
                    if !dup {
                        seen.push(c.clone());
                    }
                    !dup
                });
            }
            for code in [&old_code, &new_code] {
                ovs.items.retain(|o| !o.course.eq_ignore_ascii_case(code));
                ovs.credits.retain(|c| !c.course.eq_ignore_ascii_case(code));
            }
            customs.upsert(course);
            if creating && !sel.iter().any(|c| c.eq_ignore_ascii_case(&new_code)) {
                sel.push(new_code.clone());
            }
        });
    }

    /// Delete one of the user's own courses: store entry, selection slot and
    /// any overrides under its code go together. Fully undoable (the history
    /// carries the custom store). When the code ALSO exists in CMI's catalog
    /// (it appeared in a sync after the course was created), `keep_selected`
    /// turns deletion into "use CMI's version instead": the code stays
    /// selected and now resolves to the official course.
    pub fn delete_custom_course(&self, code: &str, keep_selected: bool) {
        if !self.is_custom(code) {
            return;
        }
        let code = code.to_string();
        let label = if keep_selected {
            format!("switch {code} to CMI's version")
        } else {
            format!("delete your course {code}")
        };
        // Switching to CMI's version: the selection entry carries the
        // custom's (always uppercased) code, but every snapshot lookup is
        // exact-match — hand it the catalog's own casing or the course
        // would read as "no longer on CMI's timetable".
        let official = self
            .snapshot
            .with_untracked(|s| s.course_ci(&code).map(|c| c.code.clone()));
        self.act_customs(&label, |customs, sel, ovs| {
            customs.remove(&code);
            if keep_selected {
                if let Some(official) = &official {
                    for entry in sel.iter_mut() {
                        if entry.eq_ignore_ascii_case(&code) {
                            *entry = official.clone();
                        }
                    }
                }
            } else {
                sel.retain(|c| !c.eq_ignore_ascii_case(&code));
            }
            ovs.items.retain(|o| !o.course.eq_ignore_ascii_case(&code));
            ovs.credits.retain(|c| !c.course.eq_ignore_ascii_case(&code));
        });
        self.removed_upstream.update(|r| r.retain(|c| c != &code));
        if keep_selected {
            self.toast_undo(format!("{code} now uses CMI's version"));
        } else {
            self.toast_undo(format!("Deleted {code}"));
        }
    }

    /// Mutate one custom course's meeting list in place (the definition IS
    /// the schedule for customs — no override layer). Keeps the meeting
    /// order and schedule status consistent afterwards.
    fn edit_custom_meetings(&self, code: &str, label: &str, f: impl FnOnce(&mut Vec<Meeting>)) {
        let code = code.to_string();
        self.act_customs(label, |customs, _, _| {
            if let Some(course) = customs
                .courses
                .iter_mut()
                .find(|c| c.code.eq_ignore_ascii_case(&code))
            {
                f(&mut course.meetings);
                course
                    .meetings
                    .sort_by_key(|m| (m.day.index(), m.slot.start_min, m.slot.end_min));
                course.status = if course.meetings.is_empty() {
                    ScheduleStatus::UnscheduledListed
                } else {
                    ScheduleStatus::Scheduled
                };
            }
        });
    }

    // -- overrides -----------------------------------------------------------

    /// Create or update an override. `ov_id` targets an existing override;
    /// otherwise one matching (course, base) is updated in place.
    pub fn apply_override(
        &self,
        course: &str,
        ov_id: Option<u64>,
        base: Option<Meeting>,
        to: Meeting,
        label: &str,
        toast: Option<String>,
    ) {
        // A custom course owns its schedule outright: a move edits the
        // definition itself, no override bookkeeping.
        if self.is_custom(course) {
            self.edit_custom_meetings(course, label, |meetings| {
                match base.as_ref().and_then(|b| meetings.iter().position(|m| m == b)) {
                    Some(i) => meetings[i] = to.clone(),
                    None => meetings.push(to.clone()),
                }
            });
            if let Some(text) = toast {
                self.toast_undo(text);
            }
            return;
        }
        let course = course.to_string();
        let now = domx::now_ms();
        self.act(label, |_, ovs| {
            let existing = ovs.items.iter_mut().find(|o| match ov_id {
                Some(id) => o.id == id,
                None => o.course == course && o.base == base,
            });
            match existing {
                Some(o) => o.to = Some(to.clone()),
                None => {
                    ovs.add(&course, base.clone(), Some(to.clone()), now);
                }
            }
        });
        if let Some(text) = toast {
            self.toast_undo(text);
        }
    }

    /// Drag a not-yet-selected course in the master grid: select it *and*
    /// apply the override as one action (one undo step).
    pub fn select_and_override(
        &self,
        course: &str,
        base: Option<Meeting>,
        to: Meeting,
        toast: String,
    ) {
        if self.is_custom(course) {
            let code = course.to_string();
            self.act_customs(&format!("add & move {code}"), |customs, sel, _| {
                if let Some(c) = customs
                    .courses
                    .iter_mut()
                    .find(|c| c.code.eq_ignore_ascii_case(&code))
                {
                    match base.as_ref().and_then(|b| c.meetings.iter().position(|m| m == b)) {
                        Some(i) => c.meetings[i] = to.clone(),
                        None => c.meetings.push(to.clone()),
                    }
                    c.meetings
                        .sort_by_key(|m| (m.day.index(), m.slot.start_min, m.slot.end_min));
                    c.status = ScheduleStatus::Scheduled;
                }
                if !sel.iter().any(|s| s.eq_ignore_ascii_case(&code)) {
                    sel.push(code.clone());
                }
            });
            self.toast_undo(toast);
            return;
        }
        let course = course.to_string();
        let now = domx::now_ms();
        self.act(&format!("add & move {course}"), |sel, ovs| {
            if !sel.iter().any(|c| c == &course) {
                sel.push(course.clone());
            }
            ovs.add(&course, base.clone(), Some(to.clone()), now);
        });
        self.toast_undo(toast);
    }

    /// Add an extra weekly meeting to a course (base = None ⇒ user-created).
    /// Unlike `apply_override`, this ALWAYS creates a new entry, so a course
    /// can gain any number of additional time slots.
    pub fn add_meeting(&self, course: &str, to: Meeting, toast: String) {
        if self.is_custom(course) {
            self.edit_custom_meetings(
                course,
                &format!("add a meeting to {course}"),
                |meetings| meetings.push(to.clone()),
            );
            self.toast_undo(toast);
            return;
        }
        let course = course.to_string();
        let now = domx::now_ms();
        self.act(&format!("add a meeting to {course}"), |_, ovs| {
            ovs.add(&course, None, Some(to.clone()), now);
        });
        self.toast_undo(toast);
    }

    /// Remove one meeting from the user's timetable. Removing a meeting the
    /// user created (or already moved) folds into its existing override;
    /// removing an official meeting records a removal override, restorable
    /// from Your changes / My data like any other change.
    pub fn remove_meeting(&self, course: &str, ov_id: Option<u64>, base: Option<Meeting>) {
        let when = base
            .as_ref()
            .map(|b| format!(" ({})", b.describe()))
            .unwrap_or_default();
        // A custom course's meeting is deleted from the definition itself —
        // there is no official version underneath to hide.
        if self.is_custom(course) {
            let course_name = course.to_string();
            self.edit_custom_meetings(
                course,
                &format!("remove a meeting from {course_name}"),
                |meetings| {
                    if let Some(b) = &base {
                        if let Some(i) = meetings.iter().position(|m| m == b) {
                            meetings.remove(i);
                        }
                    }
                },
            );
            self.toast_undo(format!("Removed a {course_name} meeting{when}"));
            return;
        }
        let course = course.to_string();
        let now = domx::now_ms();
        self.act(&format!("remove a meeting from {course}"), |_, ovs| {
            match (ov_id, &base) {
                // A meeting the user created out of thin air: removing it
                // just deletes the override — nothing of CMI's is hidden.
                (Some(id), None) => ovs.remove(id),
                // A meeting the user had already moved: the same override
                // now records the removal (base identity preserved).
                (Some(id), Some(_)) => {
                    if let Some(o) = ovs.items.iter_mut().find(|o| o.id == id) {
                        o.to = None;
                    }
                }
                // An untouched official meeting.
                (None, Some(_)) => {
                    ovs.add(&course, base.clone(), None, now);
                }
                (None, None) => {}
            }
        });
        self.toast_undo(format!("Removed a {course} meeting{when}"));
    }

    /// Starting point for newly created meetings: the grid's first day and
    /// slot — derived from the parsed data, never from CMI's current scheme.
    /// (The validation gate guarantees a non-empty slot grid; the fallback
    /// is a generic 09:00–10:00 hour.)
    pub fn default_meeting(&self) -> Meeting {
        let slot = self
            .snapshot
            .with_untracked(|s| s.slot_grid.first().copied())
            .unwrap_or(Slot::new(9 * 60, 10 * 60));
        let day = self.grid_days().first().copied().unwrap_or(Day::Mon);
        Meeting {
            day,
            slot,
            hall: None,
            temp_booking: false,
        }
    }

    pub fn reset_override(&self, id: u64, toast: Option<String>) {
        self.act("reset to CMI's time", |_, ovs| ovs.remove(id));
        if let Some(text) = toast {
            self.toast_undo(text);
        }
    }

    pub fn reset_course_overrides(&self, code: &str) {
        let code = code.to_string();
        self.act(&format!("reset {code} to CMI's times"), |_, ovs| {
            ovs.items.retain(|o| o.course != code);
        });
        self.toast_undo(format!("{code} back on CMI's times"));
    }

    // -- credits -------------------------------------------------------------

    /// Credits used everywhere: your override, else CMI's stated value,
    /// else the duration-aware assumption (1 credit per month for
    /// sub-semester spans, otherwise the campus default of 4).
    pub fn course_credits(&self, course: &Course) -> u8 {
        self.overrides
            .with(|o| o.credits_for(&course.code))
            .unwrap_or_else(|| course.effective_credits())
    }

    /// The user's custom credit value for a course, if any.
    pub fn credits_custom(&self, code: &str) -> Option<u8> {
        self.overrides.with(|o| o.credits_for(code))
    }

    pub fn set_credit_override(&self, code: &str, credits: u8) {
        // For the user's own course there is no "official" value to keep
        // around: the definition itself is edited.
        if self.is_custom(code) {
            let code_own = code.to_string();
            self.act_customs(
                &format!("set {code_own} to {credits} credits"),
                |customs, _, _| {
                    if let Some(c) = customs
                        .courses
                        .iter_mut()
                        .find(|c| c.code.eq_ignore_ascii_case(&code_own))
                    {
                        c.credits = Some(credits);
                    }
                },
            );
            self.toast_undo(format!(
                "{code_own} now counts as {credits} credit{}",
                if credits == 1 { "" } else { "s" },
            ));
            return;
        }
        let code = code.to_string();
        let now = domx::now_ms();
        self.act(&format!("set {code} to {credits} credits"), |_, ovs| {
            ovs.set_credits(&code, credits, now);
        });
        self.toast_undo(format!(
            "{code} now counts as {credits} credit{}",
            if credits == 1 { "" } else { "s" },
        ));
    }

    pub fn remove_credit_override(&self, code: &str) {
        let code = code.to_string();
        self.act(&format!("reset {code} credits"), |_, ovs| {
            ovs.remove_credits(&code);
        });
        self.toast_undo(format!("{code} back on official credits"));
    }

    /// Total number of custom changes (meeting moves + credit overrides).
    pub fn custom_change_count(&self) -> usize {
        self.overrides.with(|o| o.items.len() + o.credits.len())
    }

    /// Resolve all queued conflicts in one undoable step.
    /// `choices[i] = (conflict, keep_mine)`.
    pub fn resolve_conflicts(&self, choices: Vec<(Conflict, bool)>) {
        self.act("resolve timetable conflicts", |_, ovs| {
            for (conflict, keep_mine) in &choices {
                ttcore::merge::resolve_conflict(ovs, conflict, *keep_mine);
            }
        });
        self.conflicts.set(Vec::new());
        self.toast_undo("Conflicts resolved");
    }

    // -- derived data --------------------------------------------------------

    /// Official meetings with the user's overrides layered on top.
    pub fn effective_meetings(&self, course: &Course) -> Vec<EffMeeting> {
        let overrides = self.overrides.get();
        effective_meetings(course, &overrides)
    }

    /// A selected course no longer present upstream ("No longer on CMI's
    /// timetable"). Derived from the snapshot so it survives reloads. The
    /// user's own courses were never upstream, so they can't be removed
    /// from there.
    pub fn is_removed_upstream(&self, code: &str) -> bool {
        self.is_selected(code)
            && !self.is_custom(code)
            && self.snapshot.with(|s| s.course(code).is_none())
    }

    pub fn selected_courses(&self) -> Vec<Course> {
        // `with`, not `get`: this runs per clash/fit check, and cloning the
        // whole snapshot (raw gzipped pages included) each time is real cost.
        // Custom courses resolve FIRST: if a later sync brings an official
        // course with the same code, the user's own definition keeps winning
        // instead of being silently replaced.
        self.snapshot.with(|snapshot| {
            self.customs.with(|customs| {
                self.selection.with(|selection| {
                    selection
                        .iter()
                        .map(|code| {
                            customs
                                .get(code)
                                .or_else(|| snapshot.course(code))
                                .cloned()
                                .unwrap_or_else(|| {
                                    // Removed upstream but still selected:
                                    // synthesize a stub so it stays visible
                                    // with its badge.
                                    Course {
                                        code: code.clone(),
                                        name: code.clone(),
                                        instructors: vec![],
                                        branches: vec![],
                                        credits: None,
                                        starts: None,
                                        part_of_semester: None,
                                        optional_flag: false,
                                        status: ScheduleStatus::UnscheduledListed,
                                        meetings: vec![],
                                    }
                                })
                        })
                        .collect()
                })
            })
        })
    }

    /// The user's own courses that are NOT currently on the timetable —
    /// parked (say, over exam weeks). Their definitions stay intact and one
    /// click puts them back.
    pub fn parked_customs(&self) -> Vec<Course> {
        self.customs.with(|cs| {
            self.selection.with(|sel| {
                cs.courses
                    .iter()
                    .filter(|c| !sel.iter().any(|s| s.eq_ignore_ascii_case(&c.code)))
                    .cloned()
                    .collect()
            })
        })
    }

    /// Interval-overlap clash detection across all selected meetings, after
    /// overrides. Clashes are warnings, never blockers.
    pub fn clashes(&self) -> Vec<ClashPair> {
        let courses = self.selected_courses();
        let overrides = self.overrides.get();
        let mut all: Vec<(String, Meeting)> = Vec::new();
        for c in &courses {
            for eff in effective_meetings(c, &overrides) {
                all.push((c.code.clone(), eff.meeting));
            }
        }
        let mut out = Vec::new();
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                let (a, ma) = &all[i];
                let (b, mb) = &all[j];
                if a != b && ma.day == mb.day && ma.slot.overlaps(&mb.slot) {
                    out.push(ClashPair {
                        a: a.clone(),
                        b: b.clone(),
                        day: ma.day,
                        a_slot: ma.slot,
                        b_slot: mb.slot,
                    });
                }
            }
        }
        out
    }

    pub fn course_has_clash(&self, code: &str) -> bool {
        self.clashes().iter().any(|c| c.a == code || c.b == code)
    }

    pub fn meeting_has_clash(&self, code: &str, meeting: &Meeting) -> bool {
        self.clashes().iter().any(|c| {
            (c.a == code && c.day == meeting.day && c.a_slot == meeting.slot)
                || (c.b == code && c.day == meeting.day && c.b_slot == meeting.slot)
        })
    }

    /// Would this course fit the current selection without any overlap?
    pub fn fits_schedule(&self, course: &Course) -> bool {
        if self.is_selected(&course.code) {
            return true;
        }
        let overrides = self.overrides.get();
        let mine: Vec<(Day, Slot)> = self
            .selected_courses()
            .iter()
            .flat_map(|c| {
                effective_meetings(c, &overrides)
                    .into_iter()
                    .map(|e| (e.meeting.day, e.meeting.slot))
            })
            .collect();
        effective_meetings(course, &overrides).iter().all(|e| {
            !mine
                .iter()
                .any(|(d, s)| *d == e.meeting.day && s.overlaps(&e.meeting.slot))
        })
    }

    /// The personal grid's columns: CMI's slot grid PLUS a synthetic column
    /// for every selected meeting that fits inside no official slot —
    /// evening courses, times in the lunch gap, anything custom. Synthetic
    /// columns carry the meeting's real times, so nothing is silently
    /// squeezed into the nearest official slot. Also the slot resolver for
    /// drag & drop and keyboard move (dnd.rs), so a column that exists on
    /// screen is always a real drop target. Returns (slot, is_outside_grid).
    pub fn display_slot_grid(&self) -> Vec<(Slot, bool)> {
        let official = self.snapshot.with(|s| s.slot_grid.clone());
        let mut extra: Vec<Slot> = Vec::new();
        for course in self.selected_courses() {
            for e in self.effective_meetings(&course) {
                let start = e.meeting.slot.start_min;
                let covered = official
                    .iter()
                    .any(|s| s.start_min == start || (start >= s.start_min && start < s.end_min));
                if covered {
                    continue;
                }
                match extra.iter_mut().find(|s| s.start_min == start) {
                    // Same start, different lengths: one column, widest
                    // range; chips whose times differ sublabel themselves.
                    Some(s) => s.end_min = s.end_min.max(e.meeting.slot.end_min),
                    None => extra.push(e.meeting.slot),
                }
            }
        }
        let mut all: Vec<(Slot, bool)> = official.into_iter().map(|s| (s, false)).collect();
        all.extend(extra.into_iter().map(|s| (s, true)));
        all.sort_by_key(|(s, _)| s.start_min);
        all
    }

    /// The day rows shown in grids: Mon–Fri always, Sat/Sun only when data
    /// mentions them — CMI's pages, an override, or one of the user's own
    /// courses (a Saturday class must get its row, or it would be saved yet
    /// invisible). One list for every grid AND for drag/keyboard-move
    /// targets, so a row that exists on screen is always reachable.
    pub fn grid_days(&self) -> Vec<Day> {
        let snapshot = self.snapshot.get();
        let overrides = self.overrides.get();
        let mut days = vec![Day::Mon, Day::Tue, Day::Wed, Day::Thu, Day::Fri];
        for c in &snapshot.courses {
            for e in effective_meetings(c, &overrides) {
                if !days.contains(&e.meeting.day) {
                    days.push(e.meeting.day);
                }
            }
        }
        for c in self.selected_courses() {
            for e in effective_meetings(&c, &overrides) {
                if !days.contains(&e.meeting.day) {
                    days.push(e.meeting.day);
                }
            }
        }
        days.sort_by_key(|d| d.index());
        days
    }

    /// Days for the Halls tab and the free-hall finder: grid days UNION any
    /// day that appears only in hall bookings (e.g. a Saturday seminar) —
    /// parsed hall data must never be silently unviewable.
    pub fn hall_days(&self) -> Vec<Day> {
        let mut days = self.grid_days();
        self.snapshot.with(|s| {
            for booking in &s.hall_bookings {
                if !days.contains(&booking.day) {
                    days.push(booking.day);
                }
            }
        });
        days.sort_by_key(|d| d.index());
        days
    }

    // -- navigation ----------------------------------------------------------

    pub fn set_tab(&self, tab: Tab) {
        self.prefs.update(|p| p.tab = tab);
        self.persist_prefs();
    }

    pub fn goto_developer(&self) {
        domx::set_hash("#/developer");
    }

    pub fn goto_planner(&self) {
        domx::set_hash("#/");
    }

    /// Change the filters as one undoable step, like any other action. With
    /// `coalesce`, a run of consecutive same-label edits shares a single
    /// history entry — the search box makes one entry per burst of typing,
    /// not one per keystroke.
    pub fn act_filters(&self, label: &str, coalesce: bool, f: impl FnOnce(&mut Filters)) {
        let amend_top = coalesce
            && self
                .undo_stack
                .with_untracked(|s| s.undo.last().is_some_and(|e| e.label == label));
        if amend_top {
            // The previous entry already holds the pre-burst state; a new
            // action still invalidates anything on the redo side.
            self.undo_stack.update(|s| s.redo.clear());
        } else {
            self.push_undo(label);
        }
        self.prefs.update(|p| f(&mut p.filters));
        self.persist_prefs();
    }

    pub fn filters(&self) -> Filters {
        self.prefs.with(|p| p.filters.clone())
    }

    /// False until the first gate-passed sync: the app ships no timetable
    /// data, so an empty course list means "never synced".
    pub fn has_data(&self) -> bool {
        self.snapshot.with(|s| s.has_data())
    }
}

/// Overrides layered onto official meetings (usable outside a reactive
/// context, e.g. in the merge/adopt flow).
pub fn effective_meetings(course: &Course, overrides: &OverridesStore) -> Vec<EffMeeting> {
    let mut out: Vec<EffMeeting> = Vec::new();
    let course_ovs: Vec<&ttcore::model::MeetingOverride> =
        overrides.for_course(&course.code).collect();
    let mut replaced_ids: Vec<u64> = Vec::new();

    for official in &course.meetings {
        match course_ovs.iter().find(|o| {
            o.base.as_ref().is_some_and(|b| b.same_place_time(official))
                && !replaced_ids.contains(&o.id)
        }) {
            Some(o) => {
                replaced_ids.push(o.id);
                // A removal (`to == None`) claims its official meeting and
                // renders nothing in its place.
                if let Some(to) = &o.to {
                    out.push(EffMeeting {
                        meeting: to.clone(),
                        overridden: true,
                        ov_id: Some(o.id),
                        base: o.base.clone(),
                        user_created: false,
                    });
                }
            }
            None => out.push(EffMeeting {
                meeting: official.clone(),
                overridden: false,
                ov_id: None,
                base: Some(official.clone()),
                user_created: false,
            }),
        }
    }

    // User-created meetings (base = None) and stale overrides whose base no
    // longer matches an official meeting still show up as custom meetings.
    // Stale removals have nothing to show — the meeting is gone either way.
    for o in course_ovs {
        if replaced_ids.contains(&o.id) {
            continue;
        }
        let Some(to) = &o.to else { continue };
        out.push(EffMeeting {
            meeting: to.clone(),
            overridden: true,
            ov_id: Some(o.id),
            base: o.base.clone(),
            user_created: o.base.is_none(),
        });
    }

    out.sort_by_key(|e| (e.meeting.day.index(), e.meeting.slot.start_min));
    out
}

/// Facet matching: OR within a facet, AND across facets.
pub fn course_matches(app: &App, course: &Course, f: &Filters) -> bool {
    if !f.branches.is_empty() && !course.branches.iter().any(|b| f.branches.contains(b)) {
        return false;
    }
    if !f.instructors.is_empty()
        && !course.instructors.iter().any(|i| f.instructors.contains(i))
    {
        return false;
    }
    let overrides = app.overrides.get();
    let eff = effective_meetings(course, &overrides);
    if !f.days.is_empty() && !eff.iter().any(|e| f.days.contains(&e.meeting.day)) {
        return false;
    }
    if !f.slot_starts.is_empty()
        && !eff
            .iter()
            .any(|e| f.slot_starts.contains(&e.meeting.slot.start_min))
    {
        return false;
    }
    if !f.halls.is_empty()
        && !eff
            .iter()
            .any(|e| e.meeting.hall.as_ref().is_some_and(|h| f.halls.contains(h)))
    {
        return false;
    }
    if !f.credits.is_empty() {
        // Facet matches what the user sees — custom credit values included.
        let cr = app.course_credits(course).to_string();
        if !f.credits.contains(&cr) {
            return false;
        }
    }
    if !f.flags.is_empty() {
        let has_custom = !eff.is_empty() && eff.iter().any(|e| e.overridden);
        let matches_flag = f.flags.iter().any(|flag| match flag.as_str() {
            "optional" => course.optional_flag,
            "unscheduled" => course.status == ScheduleStatus::UnscheduledListed,
            "custom" => has_custom,
            _ => false,
        });
        if !matches_flag {
            return false;
        }
    }
    if !f.courses.is_empty() && !f.courses.contains(&course.code) {
        return false;
    }
    let text = f.text.trim().to_ascii_lowercase();
    if !text.is_empty() {
        let hay = format!(
            "{} {} {}",
            course.code,
            course.name,
            course.instructors.join(" ")
        )
        .to_ascii_lowercase();
        if !hay.contains(&text) {
            return false;
        }
    }
    if f.fits && !app.fits_schedule(course) {
        return false;
    }
    true
}
