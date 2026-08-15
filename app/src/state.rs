//! Application state: one `App` handle of copyable signals provided through
//! context, plus every user action (all undoable ones go through `act`).

use crate::{domx, storage};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use ttcore::diff::SnapshotDiff;
use ttcore::merge::Conflict;
use ttcore::model::{
    Course, CustomStore, Day, Meeting, MeetingOverride, OverridesStore, ParseReport,
    ScheduleStatus, Slot, Snapshot, SourceTier,
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
    /// The Catalog and the Master grid share this set: both pages ask the
    /// same question ("what does CMI offer?"), so a filter set on one is
    /// meant to still be set on the other.
    pub filters: Filters,
    /// My courses has its OWN set. Filtering your own five courses down to
    /// Thursday must not quietly empty the catalog you look at next — the
    /// two bars answer different questions, so they stopped sharing state
    /// (R43). `#[serde(default)]` on Prefs means stored prefs from before
    /// the split load with this empty, which is the right start.
    pub my_filters: Filters,
    /// ms since epoch of the last automatic update attempt (12 h throttle).
    pub last_update_attempt: f64,
    pub tab: Tab,
    /// Legacy single-day preference. Kept so older stored prefs still load;
    /// `halls_view` is what the app reads now.
    /// Legacy: written by the Halls day buttons until R40 and read by
    /// nothing — `halls_view` is what the app reads. Kept, with its Default,
    /// so stored prefs from older builds still deserialize.
    pub halls_day: Day,
    /// Which day (or all of them) the Halls tab shows. `None` means the user
    /// has never chosen: the tab then opens on today, and only a real click
    /// writes a value here — so the choice, once made, survives every reload.
    #[serde(default)]
    pub halls_view: Option<HallsView>,
    /// Whether the "what changed" digest is narrowed to the reader's own
    /// courses. A stored preference rather than state that dies with the
    /// dialog: someone who only wants to hear about their own week wants
    /// that at every sync, and re-ticking a box each time is a tax on the
    /// one reader the digest is trying to help. Never applied when none of
    /// the changes are theirs — see `what_changed_dialog`.
    #[serde(default)]
    pub changes_mine_only: bool,
}

/// The Halls tab's day selection.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HallsView {
    /// One day at a time — the usual way to read a room timetable.
    Day(Day),
    /// Every day the hall data covers, one table each.
    All,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            theme: ThemePref::default(),
            density: Density::default(),
            filters: Filters::default(),
            my_filters: Filters::default(),
            last_update_attempt: 0.0,
            tab: Tab::default(),
            halls_day: Day::Mon,
            halls_view: None,
            changes_mine_only: false,
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
    Conflicts,
    Export {
        scope: Option<String>,
    },
    Share,
    WhatChanged,
    /// The last record of a course CMI dropped, opened by clicking its code
    /// in the What-changed digest. The whole record RIDES IN the variant: a
    /// sync may replace `what_changed` while this is open, and the popup
    /// must keep showing exactly what was clicked. Back swaps the one
    /// dialog slot to WhatChanged. Keeping it writes this record into the
    /// user's own courses — the one way anything about a dropped course
    /// becomes permanent.
    RemovedCourse(ttcore::model::Course),
    /// The one editor. `code: None` creates a course of the user's own;
    /// `Some(code)` edits that course, whether it is CMI's or theirs — every
    /// field of it, in one form, saved in one step. `prefill` seeds the name
    /// from a failed catalog search.
    ///
    /// It opens on what the course HAS and nothing more. A course with no
    /// time used to open with a meeting row already waiting, filled in with
    /// Monday and the first slot — so the one door to its credits also
    /// stood ready to invent a time nobody asked for. Adding a meeting is
    /// now always a thing the user does.
    EditCourse {
        code: Option<String>,
        prefill: Option<String>,
    },
    /// "Import from JSON" on Course selection found these codes in the file:
    /// `known` are in this catalog (or the student's own), `unknown` aren't
    /// and will be left out. The dialog asks replace-or-add; nothing changes
    /// until the user picks.
    ImportSelection {
        known: Vec<String>,
        unknown: Vec<String>,
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

/// What a drop-target cell needs from a live drag: the cell under the
/// pointer as `(day, column start, hall)` — the hall only in the Halls
/// view, whose cells carry `data-hall`. `None` while nothing is being
/// dragged, before a touch drag has lifted off, and whenever the pointer
/// is off the grid.
pub type DropTarget = Option<(Day, u16, Option<String>)>;

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
    /// Where the cursor started — the cell the chip already occupies.
    /// Enter without moving is the default gesture, and announcing
    /// "Dropped X." for it told a screen-reader user about a move that
    /// never happened.
    pub start: (Day, u16),
}

#[derive(Clone)]
pub struct UndoEntry {
    pub label: String,
    pub selection: Vec<String>,
    pub overrides: OverridesStore,
    /// Filter changes are undoable too, so every entry carries BOTH filter
    /// sets alongside selection + overrides — undoing a catalog filter must
    /// not silently reset My courses' filters, and vice versa.
    pub filters: Filters,
    pub my_filters: Filters,
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

/// One row of the course editor as it was saved: where it came from, and
/// what it says now. `from` is what makes the difference between moving a
/// meeting and inventing one — the same row of controls does both.
#[derive(Clone)]
pub struct EditedMeeting {
    /// The effective meeting this row started as — `None` for a row the user
    /// added in the form.
    pub from: Option<EffMeeting>,
    pub to: Meeting,
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

/// Where each catalog course sits in `Snapshot::courses`, by the code
/// exactly as the catalog spells it — the same match `Snapshot::course`
/// makes (exact, case-sensitive, first wins), so a lookup through this
/// answers precisely what that walk would answer. `Arc`, not `Rc`: signal
/// storage has to be `Send + Sync`.
pub type CodeIndex = Arc<HashMap<String, usize>>;

#[derive(Clone, Copy)]
pub struct App {
    pub snapshot: RwSignal<Snapshot>,
    /// The catalog's positions, rebuilt only when a sync lands (built at the
    /// root, app.rs). Read it BEFORE borrowing `snapshot`: it is a memo OVER
    /// that signal, so a read inside `snapshot.with(…)` reaches the same
    /// signal twice the moment the memo is stale (§4, rule 1).
    pub course_index: Memo<CodeIndex>,
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
    /// The conflicts banner, waved away for THIS session only. Never
    /// persisted, and reset by every `set_conflicts`: hiding a question is
    /// not answering it, so the queue survives and the banner returns with
    /// the next sync, partial apply, or reload.
    pub conflicts_dismissed: RwSignal<bool>,
    pub what_changed: RwSignal<Option<SnapshotDiff>>,
    /// Unknown codes from a shared URL (dismissible warning chips).
    pub unknown_codes: RwSignal<Vec<String>>,
    pub fetch_log: RwSignal<Vec<FetchLogEntry>>,
    pub reports: RwSignal<Vec<StoredReport>>,
    pub route: RwSignal<Route>,
    pub dialog: RwSignal<Option<Dialog>>,
    /// Set by the course editor once anything in it has been typed or
    /// picked. A dialog is normally cheap to close — Esc, or a click on the
    /// dark area — but that form holds work no undo can bring back, because
    /// nothing is committed until Save. While this is true, those two
    /// dismissals ask first. Cleared whenever the dialog changes.
    pub dialog_dirty: RwSignal<bool>,
    pub drag: RwSignal<Option<DragState>>,
    /// The cell under the pointer, for the drop-target highlight. Derived
    /// from `drag` at the root (see `app.rs`) and deliberately NOT read off
    /// `drag` directly: `drag` changes on every single pointermove, because
    /// the ghost chip follows the pointer, while this changes only when the
    /// pointer crosses into another cell. The Halls table hangs a `drop-ok`
    /// closure on several hundred cells; a Memo's PartialEq dedupe is what
    /// stops all of them re-running sixty times a second for a value that
    /// changed a few dozen times in the whole gesture.
    pub drop_target: Memo<DropTarget>,
    pub move_mode: RwSignal<Option<MoveMode>>,
    /// Developer-mode simulator: force a specific tier on the next update.
    pub force_tier: RwSignal<Option<String>>,
    /// aria-live announcements (keyboard move mode, etc.).
    pub announce: RwSignal<String>,
    /// Drag & drop (pointer and keyboard move mode) only works while edit
    /// mode is on — toggled per session from the grid toolbars.
    pub edit_mode: RwSignal<bool>,
    /// The grid's day rows, worked out ONCE per change for the whole
    /// session. Read it through `grid_days()`, never directly: the method is
    /// the name everything else uses, and this is only how it is paid for.
    /// Built in `app.rs::init_app` under the root owner (same reason as
    /// `CourseIndex`, so it outlives every view that reads it) — and a FIELD
    /// rather than a context, because `App` is copied into the document-level
    /// pointer and key handlers in `dnd.rs`, which the browser calls with no
    /// reactive owner: a context lookup there would find nothing and panic.
    pub grid_days_memo: Memo<Vec<Day>>,
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
        if self
            .banner
            .with_untracked(|b| b.as_ref().is_some_and(|b| b.sticky))
        {
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

    /// Close a dialog the way the two *accidental* dismissals do — Escape,
    /// and a click on the dark area beside the form.
    ///
    /// Cancel and Save close it outright: those are answers. These two are
    /// often slips — Escape is also how a browser's autocomplete popup is
    /// dismissed, and the dark area is a big target beside a tall form — and
    /// the course editor commits nothing until Save, so a slip there is the
    /// one loss in this app that Undo cannot reach. So it asks, but only
    /// when there is something to lose.
    pub fn dismiss_dialog(&self) {
        if self.dialog_dirty.get_untracked()
            && !crate::domx::window()
                .confirm_with_message(
                    "Close this form and lose your changes? Nothing here has been saved yet.",
                )
                .unwrap_or(true)
        {
            return;
        }
        self.dialog.set(None);
    }

    // -- persistence + URL -------------------------------------------------

    /// Saving the user's OWN data — the one thing in this app that cannot be
    /// fetched again. A failure here used to be discarded (`let _ =`) while
    /// the sync flow went on saying "Your courses and changes are safe", so
    /// a full localStorage lost the lot at the next reload without a word.
    /// It stays on screen (sticky) because it is about data the user can
    /// still rescue — the session in front of them is still correct.
    fn persisted(&self, what: &str, result: Result<(), String>) {
        if result.is_err() {
            self.set_banner_sticky(
                BannerKind::Warn,
                format!(
                    "Your browser wouldn't let the app save your {what}. Everything is \
                     still here for now, but it may not come back next time you open \
                     the app. Freeing some browser space — or clearing the downloaded \
                     timetable under My data — usually fixes it.",
                ),
            );
        }
    }

    pub fn persist_selection(&self) {
        let r = storage::save(storage::KEY_SELECTION, &self.selection.get_untracked());
        self.persisted("courses", r);
        self.sync_url();
    }

    pub fn persist_overrides(&self) {
        let r = storage::save(storage::KEY_OVERRIDES, &self.overrides.get_untracked());
        self.persisted("changes", r);
    }

    pub fn persist_customs(&self) {
        let r = storage::save(storage::KEY_CUSTOM, &self.customs.get_untracked());
        self.persisted("own courses", r);
    }

    pub fn persist_prefs(&self) {
        // Preferences are re-derivable and re-chosen in a second; a failure
        // here is not worth a banner that would then hide a real one.
        //
        // Borrowed with `with_untracked`, never taken with `get_untracked`:
        // this runs on EVERY keystroke in a filter search box, and taking
        // the value out cloned the whole of `Prefs` — both filter sets, so
        // sixteen `Vec`s plus every ticked branch, instructor, hall and
        // course code, deep-cloned and dropped again — purely to hand serde
        // a reference. Borrowing writes the same bytes with no allocation.
        //
        // The write stays synchronous and immediate, and must: t89 reads
        // `cmitt.v1.prefs` straight out of localStorage with no navigation
        // and no delay after ticking the digest's box, and t49 refreshes the
        // page a moment after a Halls day click. There is also no safe place
        // to flush a deferred write from — the backup import, "Delete
        // everything" and the storage inspector all rewrite or clear this
        // key and then reload, so anything still pending would land on top
        // of them with the stale in-memory value.
        let _ = self
            .prefs
            .with_untracked(|p| storage::save(storage::KEY_PREFS, p));
    }

    /// Keep `?c=` canonical on every selection change (replaceState). Any
    /// `s=` payload is consumed at load time and dropped here.
    pub fn sync_url(&self) {
        let selection = self.selection.get_untracked();
        if selection.is_empty() {
            domx::replace_query("");
        } else {
            // Plain commas between codes, each code percent-encoded — see
            // `domx::c_param`.
            domx::replace_query(&format!("?c={}", domx::c_param(&selection)));
        }
    }

    // -- undo / redo ---------------------------------------------------------

    fn push_undo(&self, label: &str) {
        let entry = UndoEntry {
            label: label.to_string(),
            selection: self.selection.get_untracked(),
            overrides: self.overrides.get_untracked(),
            filters: self.prefs.with_untracked(|p| p.filters.clone()),
            my_filters: self.prefs.with_untracked(|p| p.my_filters.clone()),
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
            my_filters: self.prefs.with_untracked(|p| p.my_filters.clone()),
            customs: self.customs.get_untracked(),
        }
    }

    /// Restore one history entry (shared by undo and redo).
    fn apply_entry(&self, entry: &UndoEntry) {
        self.selection.set(entry.selection.clone());
        self.overrides.set(entry.overrides.clone());
        self.prefs.update(|p| {
            p.filters = entry.filters.clone();
            p.my_filters = entry.my_filters.clone();
        });
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
        self.act(&format!("add {code}"), |sel, ovs| {
            if !sel.contains(&code) {
                sel.push(code.clone());
            }
            // A course cannot be on your timetable AND deleted: putting it
            // back is the same decision as restoring it.
            ovs.unhide(&code);
        });
        // Warn immediately (never block) when the new course clashes.
        let clashing = self.clashing_partners(&code);
        if clashing.is_empty() {
            self.toast_undo(format!("Added {code}"));
        } else {
            self.toast_undo(format!(
                "Added {code}. ⚠ It clashes with {}. It's on your timetable \
                 either way.",
                clashing.join(", ")
            ));
        }
    }

    /// The import dialog's two answers, one undoable step either way:
    /// `replace` makes the file's courses the whole selection; otherwise
    /// they join what's already there. Restoring a deleted course on the
    /// way in mirrors `add_course` — on your timetable and deleted at once
    /// is not a state.
    pub fn import_selection(&self, codes: &[String], replace: bool) {
        let n = codes.len();
        let already: usize = self
            .selection
            .with_untracked(|sel| codes.iter().filter(|c| sel.contains(c)).count());
        let label = if replace {
            "replace my courses with the ones from a file"
        } else {
            "add a file's courses"
        };
        // Nothing to add means nothing on the undo stack: an act here would
        // wipe the redo history and hand Ctrl+Z a step that restores an
        // identical state, while the toast says "nothing changed". (Unhide
        // can't be the difference — a selected course is never hidden.)
        if !replace && already == n {
            self.toast(
                "Every course in that file was already on your timetable — \
                 nothing changed.",
            );
            return;
        }
        let codes = codes.to_vec();
        self.act(label, move |sel, ovs| {
            if replace {
                sel.clear();
            }
            for code in &codes {
                if !sel.contains(code) {
                    sel.push(code.clone());
                }
                ovs.unhide(code);
            }
        });
        let plural = |n: usize| if n == 1 { "course" } else { "courses" };
        if replace {
            self.toast_undo(format!(
                "Your timetable now has exactly the {n} {} from that file.",
                plural(n)
            ));
        } else {
            let added = n - already;
            self.toast_undo(format!(
                "Added {added} {} from the file{}.",
                plural(added),
                if already > 0 {
                    " — the rest were already on your timetable"
                } else {
                    ""
                },
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
        // Say that the times are kept: the course leaves the timetable but
        // the work done on it does not, and finding that out by accident
        // weeks later (from a "✎ N changes" count that never went down) is
        // worse than three extra words here.
        self.toast_undo(format!(
            "Removed {code} from your timetable. Any times you set for it are \
             kept, so adding it back brings them with it."
        ));
    }

    pub fn toggle_select(&self, code: &str) {
        if self.is_selected(code) {
            self.remove_course(code);
        } else {
            self.add_course(code);
        }
    }

    // -- deleting a course ------------------------------------------------------

    /// Courses the user deleted, in the order they went.
    pub fn hidden_courses(&self) -> Vec<String> {
        self.overrides
            .with(|o| o.hidden.iter().map(|h| h.course.clone()).collect())
    }

    pub fn is_hidden(&self, code: &str) -> bool {
        self.overrides.with(|o| o.is_hidden(code))
    }

    /// Delete a course outright. CMI's pages are never edited, so deleting
    /// one of theirs means striking it from YOUR planner: off the timetable,
    /// out of the catalog and the master grid, and recorded in "Your
    /// changes" as a thing of theirs you overwrote — with nothing.
    ///
    /// Anything else you'd customised about it goes WITH it rather than
    /// under it: the overrides stay in the store but drop out of the changes
    /// list and its count (a moved meeting on a course you deleted is a
    /// change to nothing anyone can see), so Restore gives back the course
    /// AND everything you had done to it. One undoable step throughout.
    ///
    /// Your own course has no CMI version underneath: deleting it deletes
    /// the definition, which is what `delete_custom_course` already does.
    ///
    /// The Halls tab is deliberately untouched by this — see `hall_row`.
    pub fn delete_course(&self, code: &str) {
        if self.is_custom(code) {
            self.delete_custom_course(code, false);
            return;
        }
        if self.is_hidden(code) {
            return;
        }
        let code = code.to_string();
        let now = domx::now_ms();
        self.act(&format!("delete {code}"), |sel, ovs| {
            // Remember whether the deletion is also taking the selection,
            // so Restore can give back everything it took.
            let was = sel.iter().any(|c| c.eq_ignore_ascii_case(&code));
            sel.retain(|c| !c.eq_ignore_ascii_case(&code));
            ovs.hide(&code, was, now);
        });
        self.toast_undo(format!("Deleted {code} — restore it from Your changes"));
    }

    /// Put a deleted course back, with every change you had made to it —
    /// and its place on your timetable, if it was there when you deleted
    /// it. Deleting took the course AND your selection of it; Restore
    /// returns both.
    pub fn restore_course(&self, code: &str) {
        let was = self
            .overrides
            .with_untracked(|o| o.hidden_was_selected(code));
        let code = code.to_string();
        let toast_code = code.clone();
        self.act(&format!("restore {code}"), move |sel, ovs| {
            ovs.unhide(&code);
            if was && !sel.iter().any(|c| c.eq_ignore_ascii_case(&code)) {
                sel.push(code.clone());
            }
        });
        self.toast_undo(if was {
            format!(
                "{toast_code} is back — in the catalog, the master grid, and \
                 on your timetable, where it was when you deleted it."
            )
        } else {
            format!(
                "{toast_code} is back in the catalog and the master grid. It \
                 isn't on your timetable — add it when you want it."
            )
        });
    }

    pub fn restore_all_courses(&self) {
        let (n, reselect): (usize, Vec<String>) = self.overrides.with_untracked(|o| {
            (
                o.hidden.len(),
                o.hidden
                    .iter()
                    .filter(|h| h.was_selected)
                    .map(|h| h.course.clone())
                    .collect(),
            )
        });
        if n == 0 {
            return;
        }
        let back = reselect.len();
        self.act("restore deleted courses", move |sel, ovs| {
            ovs.hidden.clear();
            for code in &reselect {
                if !sel.iter().any(|c| c.eq_ignore_ascii_case(code)) {
                    sel.push(code.clone());
                }
            }
        });
        let plural = |n: usize| if n == 1 { "course" } else { "courses" };
        self.toast_undo(match back {
            0 => format!(
                "{n} {} you deleted {} back in the catalog and the master \
                 grid — none were on your timetable when you deleted them, so \
                 add the ones you want.",
                plural(n),
                if n == 1 { "is" } else { "are" },
            ),
            b if b == n => format!(
                "{n} {} you deleted {} back — in the catalog, the master \
                 grid, and on your timetable, where they were when you deleted \
                 them.",
                plural(n),
                if n == 1 { "is" } else { "are" },
            ),
            b => format!(
                "{n} {} you deleted are back in the catalog and the master \
                 grid — {b} also returned to your timetable, where they were \
                 when you deleted them.",
                plural(n),
            ),
        });
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
    /// course and un-deletes the code, for the same reason `add_course`
    /// does; editing leaves its selected/parked state alone.
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
            if creating {
                if !sel.iter().any(|c| c.eq_ignore_ascii_case(&new_code)) {
                    sel.push(new_code.clone());
                }
                // A course cannot be on your timetable AND deleted — the
                // same rule `add_course` keeps. Reachable for a code CMI has
                // since dropped: the catalog no longer holds it, so nothing
                // stops a course being created under a code you deleted.
                ovs.unhide(&new_code);
            }
        });
    }

    /// Keep a course CMI dropped. The "What changed" record is the last copy
    /// in existence — the new snapshot has never heard of the course, the old
    /// one was overwritten by the sync, and the digest is never saved — so
    /// keeping it means writing it into the user's own courses, where it
    /// survives a reload, a share link and the message being dismissed.
    ///
    /// Every field comes from the record verbatim; nothing is invented.
    /// Credits above all: a course CMI never put a number on stays a course
    /// without one, so the app goes on calling that number a guess instead of
    /// promoting 4 into a stated fact and moving the student's total.
    ///
    /// The times are the deliberate exception. A dropped course holds its
    /// place on the timetable through overrides — its stub has no meetings of
    /// its own — and `save_custom_course` purges those. So whatever is on the
    /// timetable NOW is folded into the definition first: keeping a course
    /// never moves a class the student placed themselves.
    pub fn keep_removed_course(&self, record: &Course) {
        let code = record.code.clone();
        if self.is_custom(&code) {
            return;
        }
        // A sync can land while the popup is open. If CMI lists the course
        // again there is nothing to keep, and writing one now would shadow
        // their live version from its first second.
        if self
            .snapshot
            .with_untracked(|s| s.course_ci(&code).is_some())
        {
            self.toast(format!(
                "CMI's timetable lists {code} again, so there was nothing to keep. \
                 What's on your timetable is CMI's own version."
            ));
            return;
        }
        // Read BEFORE the save: it purges both the meeting overrides and the
        // credit override under this code. Untracked — a click handler must
        // not subscribe anything to these reads.
        let (was_selected, placed, own_credits) = untrack(|| {
            (
                self.is_selected(&code),
                self.effective_meetings(&self.selected_course(&code))
                    .into_iter()
                    .map(|e| e.meeting)
                    .collect::<Vec<Meeting>>(),
                self.credits_custom(&code),
            )
        });

        let mut course = record.clone();
        // What's on the week wins; CMI's last-known times fill in only when
        // the student placed none. Merging the two would put the same class
        // on the week twice — the reason you move a dropped course's class is
        // that the class moved.
        course.meetings = if placed.is_empty() {
            record.meetings.clone()
        } else {
            placed
        };
        // A temporary-booking mark is a claim about CMI's live hall list, and
        // CMI no longer publishes this course at all.
        for m in &mut course.meetings {
            m.temp_booking = false;
        }
        course.meetings.sort_by_key(|m| {
            (
                m.day.index(),
                m.slot.start_min,
                m.slot.end_min,
                m.hall.clone(),
            )
        });
        course.status = if course.meetings.is_empty() {
            ScheduleStatus::UnscheduledListed
        } else {
            ScheduleStatus::Scheduled
        };
        // The student's own credit number outlives the override that carried
        // it: the save is about to purge that override.
        if let Some(n) = own_credits {
            course.credits = Some(n);
        }
        let no_times = course.meetings.is_empty();
        self.save_custom_course(None, course);
        self.toast_undo(if no_times {
            format!(
                "{code} is your own course now. It has no times yet, so it's waiting \
                 in “No fixed slot yet” on My timetable."
            )
        } else if was_selected {
            format!(
                "{code} is your own course now — its name, instructor and times stay \
                 on your timetable when the update message goes."
            )
        } else {
            format!(
                "Added {code} to your timetable as your own course — its name, \
                 instructor and times are saved for good."
            )
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
            ovs.credits
                .retain(|c| !c.course.eq_ignore_ascii_case(&code));
        });
        if keep_selected {
            self.toast_undo(format!(
                "{code} now uses CMI's version. Your own version is deleted — Undo \
             brings it back."
            ));
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
                match base
                    .as_ref()
                    .and_then(|b| meetings.iter().position(|m| m == b))
                {
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
            self.act_customs(&format!("add and move {code}"), |customs, sel, _| {
                if let Some(c) = customs
                    .courses
                    .iter_mut()
                    .find(|c| c.code.eq_ignore_ascii_case(&code))
                {
                    match base
                        .as_ref()
                        .and_then(|b| c.meetings.iter().position(|m| m == b))
                    {
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
        self.act(&format!("add and move {course}"), |sel, ovs| {
            if !sel.iter().any(|c| c.eq_ignore_ascii_case(&course)) {
                sel.push(course.clone());
                ovs.unhide(&course);
            }
            // Update in place when this CMI meeting already carries a change,
            // exactly as `apply_override` does. A course can be dragged in the
            // master grid while unselected AFTER it was already customised —
            // adding a second override for the same base makes the one meeting
            // render twice, on the timetable and in Your changes.
            match ovs
                .items
                .iter_mut()
                .find(|o| o.course.eq_ignore_ascii_case(&course) && o.base == base)
            {
                Some(o) => o.to = Some(to.clone()),
                None => {
                    ovs.add(&course, base.clone(), Some(to.clone()), now);
                }
            }
        });
        self.toast_undo(toast);
    }

    /// Save a whole course edit at once: every meeting row as it now stands,
    /// plus the credits. This is the ONE path that writes a CMI course's
    /// changes, so the entire edit — three moves, a new meeting, a removal
    /// and a credit change — is a single undo step.
    ///
    /// The overrides for the course are rebuilt from the rows rather than
    /// patched: an override exists exactly when a row differs from CMI's
    /// meeting, and a CMI meeting no row claims is a removal. That is what
    /// makes "put it back where CMI has it" fall out for free — the row
    /// matches its base again, so nothing is stored — and what lets the form
    /// restore a meeting the user had struck out earlier.
    ///
    /// Identity is preserved where it exists: an override that comes back
    /// unchanged keeps its id and the day it was made (the sync merge
    /// compares that against CMI's edits), and one that only changed target
    /// keeps them too — it is the same decision, revised.
    ///
    /// `credits: None` means the editor had no official value to compare
    /// against (a course CMI has dropped) — the save then says nothing
    /// about credits: it neither stores a "change" the student never made
    /// nor deletes one they made while CMI still listed the course.
    ///
    /// `add_to_timetable` is the editor's "Also add {code} to my timetable"
    /// box: when the course isn't selected, saving adds it only if the box
    /// stayed ticked — the add is asked, never assumed.
    pub fn save_course_edit(
        &self,
        code: &str,
        official: Vec<Meeting>,
        rows: Vec<EditedMeeting>,
        credits: Option<u8>,
        add_to_timetable: bool,
    ) {
        // (base, to) for every override the saved form implies.
        //
        // Two passes, and the order matters. A row that CAME from one of
        // CMI's meetings names it explicitly, so it has the first claim on
        // it; only afterwards may a row the user wrote themselves claim a
        // leftover meeting it happens to coincide with. The other way round
        // loses data: a meeting the user added at exactly the time of a CMI
        // meeting they had MOVED away would claim that meeting and store
        // nothing, while the move — finding its base already spoken for —
        // stored itself as a stale-base override. The added meeting then
        // existed nowhere, and a save that changed nothing made it vanish.
        let mut desired: Vec<(Option<Meeting>, Option<Meeting>)> = Vec::new();
        let mut claimed: Vec<usize> = Vec::new();
        for row in &rows {
            let Some(base) = row.from.as_ref().and_then(|f| f.base.clone()) else {
                continue;
            };
            let stands_for = official
                .iter()
                .enumerate()
                .find(|(i, m)| m.same_place_time(&base) && !claimed.contains(i))
                .map(|(i, _)| i);
            if let Some(i) = stands_for {
                claimed.push(i);
            }
            // Back on CMI's own meeting: no override at all. Day, time and
            // hall are what the form can set, so they are what "the same"
            // means — CMI's TMP* decoration isn't the user's to reproduce.
            //
            // Only when the row really stands for a meeting CMI has NOW,
            // though. A base CMI has since moved (an unresolved conflict, or
            // a share link imported against fresher data) stands for
            // nothing: storing nothing for it would delete a meeting that is
            // on the user's timetable and on this form — the one case where
            // saving could lose what it was showing.
            if stands_for.is_some() && base.same_place_time(&row.to) {
                continue;
            }
            desired.push((Some(base.clone()), Some(row.to.clone())));
        }
        // A row the user wrote that says exactly what CMI says IS CMI's
        // meeting, however it got there — so it speaks for it too, and
        // nothing is stored. Without this, striking a meeting out and adding
        // the same one back leaves a removal and an addition that cancel on
        // screen and read as two changes in the list.
        for row in &rows {
            if row.from.as_ref().and_then(|f| f.base.as_ref()).is_some() {
                continue;
            }
            match official
                .iter()
                .enumerate()
                .find(|(i, m)| m.same_place_time(&row.to) && !claimed.contains(i))
            {
                Some((i, _)) => claimed.push(i),
                None => desired.push((None, Some(row.to.clone()))),
            }
        }
        // Whatever CMI has that no row stands for, the user struck out.
        for (i, m) in official.iter().enumerate() {
            if !claimed.contains(&i) {
                desired.push((Some(m.clone()), None));
            }
        }

        // A credits override is a statement of difference from CMI's
        // official value. Without an official value (CMI dropped the course
        // — possibly mid-edit, so this is checked here and not only in the
        // form) there is nothing to differ from: say nothing about credits.
        let official_credits = self
            .snapshot
            .with_untracked(|s| s.course_ci(code).map(|c| c.effective_credits()));
        let credits_now: Option<Option<u8>> = match official_credits {
            None => None,
            Some(off) => credits.map(|n| (n != off).then_some(n)),
        };
        let was_selected = self.is_selected(code);
        // Adding is asked, not assumed: when the course isn't on the
        // timetable the editor shows a ticked "Also add … to my timetable"
        // box, and this flag is its answer — the step was visible before it
        // happened, instead of a "Save changes" that quietly changed the
        // clash picture and the credit total.
        let select_now = !was_selected && add_to_timetable;
        let code = code.to_string();
        let now = domx::now_ms();
        self.act(&format!("edit {code}"), |sel, ovs| {
            let mut pool: Vec<MeetingOverride> = Vec::new();
            ovs.items.retain(|o| {
                let mine = o.course.eq_ignore_ascii_case(&code);
                if mine {
                    pool.push(o.clone());
                }
                !mine
            });
            let mut kept: Vec<Option<MeetingOverride>> = vec![None; desired.len()];
            // Unchanged overrides claim their old selves first, so an edited
            // one can't walk off with an identical override's id.
            for (i, (base, to)) in desired.iter().enumerate() {
                if let Some(p) = pool.iter().position(|o| &o.base == base && &o.to == to) {
                    kept[i] = Some(pool.remove(p));
                }
            }
            for (i, (base, to)) in desired.iter().enumerate() {
                if kept[i].is_none()
                    && let Some(p) = pool.iter().position(|o| &o.base == base)
                {
                    let mut o = pool.remove(p);
                    o.to = to.clone();
                    kept[i] = Some(o);
                }
            }
            for (slot, (base, to)) in kept.into_iter().zip(desired.iter()) {
                match slot {
                    Some(o) => ovs.items.push(o),
                    None => {
                        ovs.add(&code, base.clone(), to.clone(), now);
                    }
                }
            }
            match credits_now {
                Some(Some(n)) => ovs.set_credits(&code, n, now),
                Some(None) => ovs.remove_credits(&code),
                None => {}
            }
            if select_now && !sel.iter().any(|c| c.eq_ignore_ascii_case(&code)) {
                sel.push(code.clone());
                ovs.unhide(&code);
            }
        });
        if select_now {
            self.toast_undo(format!("Added {code} to your timetable"));
        } else {
            self.toast_undo(format!("Saved your changes to {code}"));
        }
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

    pub fn remove_credit_override(&self, code: &str) {
        let code = code.to_string();
        self.act(&format!("reset {code} credits"), |_, ovs| {
            ovs.remove_credits(&code);
        });
        self.toast_undo(format!("Removed your credit change to {code}"));
    }

    /// Everything the user's own data adds up to: meetings moved, added or
    /// struck out, credits set, courses deleted, courses of their own. It is
    /// the count of the rows in "Your changes", because a number that
    /// doesn't match the list it opens is worse than no number at all.
    pub fn custom_change_count(&self) -> usize {
        self.overrides.with(|o| {
            o.items.iter().filter(|i| !o.is_hidden(&i.course)).count()
                + o.credits.iter().filter(|c| !o.is_hidden(&c.course)).count()
                + o.hidden.len()
        }) + self.customs.with(|c| c.courses.len())
    }

    /// Replace the queued conflicts AND their stored copy in one move.
    /// Every writer goes through here: a question the user deferred with
    /// "Decide later" has to survive a reload, so the signal and
    /// localStorage must never disagree.
    pub fn set_conflicts(&self, conflicts: Vec<Conflict>) {
        if conflicts.is_empty() {
            crate::storage::remove(crate::storage::KEY_CONFLICTS);
        } else if let Err(e) = crate::storage::save(crate::storage::KEY_CONFLICTS, &conflicts) {
            leptos::logging::warn!("cmitt: couldn't store pending conflicts: {e}");
        }
        // A new queue is a new question — a Dismiss given to the old banner
        // doesn't carry over.
        self.conflicts_dismissed.set(false);
        self.conflicts.set(conflicts);
    }

    /// Resolve the ANSWERED conflicts in one undoable step; the unanswered
    /// `remaining` go back to the queue exactly as they were, so opening
    /// the dialog to look never costs an answer.
    /// `choices[i] = (conflict, keep_mine)`.
    pub fn resolve_conflicts(&self, choices: Vec<(Conflict, bool)>, remaining: Vec<Conflict>) {
        self.act("resolve timetable conflicts", |_, ovs| {
            for (conflict, keep_mine) in &choices {
                ttcore::merge::resolve_conflict(ovs, conflict, *keep_mine);
            }
        });
        let left = remaining.len();
        self.set_conflicts(remaining);
        self.toast_undo(if left == 0 {
            "Your timetable now uses the times you picked.".to_string()
        } else {
            format!(
                "Your timetable now uses the times you picked. The {} you left \
                 unanswered {} still waiting — press Review in the message at \
                 the top of the page to finish.",
                if left == 1 { "row" } else { "rows" },
                if left == 1 { "is" } else { "are" },
            )
        });
    }

    // -- derived data --------------------------------------------------------

    /// Official meetings with the user's overrides layered on top.
    /// This is asked once per chip, and once per course code in every cell
    /// of the halls week — `.with`, so the whole override store isn't cloned
    /// each time just to be read.
    pub fn effective_meetings(&self, course: &Course) -> Vec<EffMeeting> {
        self.overrides
            .with(|overrides| effective_meetings(course, overrides))
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
        // The catalog's index FIRST, before the snapshot is borrowed below:
        // it is a memo over that same signal, so reading it inside the `with`
        // would reach the snapshot twice the moment a sync had left it stale
        // (§4, rule 1). Taking it costs one `Arc` clone.
        let by_code = self.course_index.get();
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
                                .or_else(|| {
                                    // What `Snapshot::course` would find,
                                    // without the walk: exact, case-sensitive,
                                    // first wins. The position is checked
                                    // against the course it points at, so a
                                    // stale index can never put somebody
                                    // else's course on the timetable — a miss
                                    // only draws the stub an unknown code
                                    // already draws.
                                    by_code
                                        .get(code.as_str())
                                        .and_then(|i| snapshot.courses.get(*i))
                                        .filter(|c| c.code == *code)
                                })
                                .cloned()
                                .unwrap_or_else(|| removed_stub(code))
                        })
                        .collect()
                })
            })
        })
    }

    /// The course behind ONE selected code, resolved the same way
    /// `selected_courses` resolves the whole list: the user's own first, then
    /// CMI's catalog, then a stub for a course CMI has dropped since it was
    /// added.
    ///
    /// Anything acting on the selection must go through this. Exporting used
    /// to resolve customs-then-snapshot and quietly skip whatever was left,
    /// so a course CMI stopped listing — visible on every screen, with the
    /// user's own meetings on it — was missing from the .ics with no word
    /// said (R23).
    pub fn selected_course(&self, code: &str) -> Course {
        self.custom_course(code)
            .or_else(|| self.snapshot.with(|s| s.course(code).cloned()))
            .unwrap_or_else(|| removed_stub(code))
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
        let mut all: Vec<(String, Meeting)> = Vec::new();
        self.overrides.with(|overrides| {
            for c in &courses {
                for eff in effective_meetings(c, overrides) {
                    all.push((c.code.clone(), eff.meeting));
                }
            }
        });
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

    /// Does anything else on the timetable overlap this course?
    ///
    /// Asked once per chip, and every grid draws dozens of them, so it stops
    /// at the first overlap instead of building the full list of pairs the
    /// way `clashes()` has to for the panel.
    pub fn course_has_clash(&self, code: &str) -> bool {
        self.overlaps_selection(code, None)
    }

    /// The same question about ONE meeting of a course.
    pub fn meeting_has_clash(&self, code: &str, meeting: &Meeting) -> bool {
        self.overlaps_selection(code, Some(meeting))
    }

    /// Shared engine for both: does `code` (all of it, or just `only`) run
    /// into any other selected course? Pair selection matches `clashes()`
    /// exactly — a course never clashes with itself.
    fn overlaps_selection(&self, code: &str, only: Option<&Meeting>) -> bool {
        let courses = self.selected_courses();
        let Some(mine) = courses.iter().find(|c| c.code == code) else {
            return false;
        };
        self.overrides.with(|overrides| {
            let mut my_slots: Vec<(Day, Slot)> = effective_meetings(mine, overrides)
                .into_iter()
                .map(|e| (e.meeting.day, e.meeting.slot))
                .collect();
            if let Some(m) = only {
                my_slots.retain(|(d, s)| *d == m.day && *s == m.slot);
            }
            if my_slots.is_empty() {
                return false;
            }
            courses
                .iter()
                .filter(|c| c.code != code)
                .flat_map(|c| effective_meetings(c, overrides))
                .any(|e| {
                    my_slots
                        .iter()
                        .any(|(d, s)| *d == e.meeting.day && s.overlaps(&e.meeting.slot))
                })
        })
    }

    /// Would this course fit the current selection without any overlap?
    pub fn fits_schedule(&self, course: &Course) -> bool {
        self.is_selected(&course.code) || self.would_clash_with(course).is_empty()
    }

    /// Which of the user's courses this one would run into, and when — the
    /// answer to the ⚠ the grid draws on a course they haven't picked.
    /// `fits_schedule` is this walk with the names thrown away, so they are
    /// one function: a badge that warns and a dialog that explains can never
    /// disagree about what overlaps.
    ///
    /// One entry per collision, in reading order (day, then start time).
    pub fn would_clash_with(&self, course: &Course) -> Vec<(String, Day, Slot)> {
        let selected = self.selected_courses();
        let mut hits = self.overrides.with(|overrides| {
            let mine: Vec<(String, Day, Slot)> = selected
                .iter()
                .filter(|c| !c.code.eq_ignore_ascii_case(&course.code))
                .flat_map(|c| {
                    effective_meetings(c, overrides)
                        .into_iter()
                        .map(|e| (c.code.clone(), e.meeting.day, e.meeting.slot))
                })
                .collect();
            effective_meetings(course, overrides)
                .iter()
                .flat_map(|e| {
                    mine.iter()
                        .filter(|(_, d, s)| *d == e.meeting.day && s.overlaps(&e.meeting.slot))
                        .map(|(other, d, s)| (other.clone(), *d, *s))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        });
        hits.sort_by_key(|(other, d, s)| (d.index(), s.start_min, other.clone()));
        hits.dedup();
        hits
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
                push_extra_column(&official, &mut extra, e.meeting.slot);
            }
        }
        columns(official, extra)
    }

    /// Columns for the Master grid. CMI's slots, plus a synthetic column for
    /// every out-of-grid time the user has moved something to.
    ///
    /// This grid draws CMI's whole catalog, so its extras come from the
    /// override store rather than the selection: any course on the page can
    /// carry one, selected or not. It used to keep CMI's columns alone and
    /// clamp a 19:00 meeting into the 17:00 one with a sublabel — the column
    /// header then said something that wasn't true, which is the same lie
    /// `display_slot_grid` was built to stop telling (R22).
    pub fn master_slot_grid(&self) -> Vec<(Slot, bool)> {
        let official = self.snapshot.with(|s| s.slot_grid.clone());
        let mut extra: Vec<Slot> = Vec::new();
        self.overrides.with(|ovs| {
            for o in &ovs.items {
                if let Some(m) = o.to.as_ref() {
                    push_extra_column(&official, &mut extra, m.slot);
                }
            }
        });
        columns(official, extra)
    }

    /// The day rows shown in grids: Mon–Fri always, Sat/Sun only when data
    /// mentions them — CMI's pages, an override, or one of the user's own
    /// courses (a Saturday class must get its row, or it would be saved yet
    /// invisible). One list for every grid AND for drag/keyboard-move
    /// targets, so a row that exists on screen is always reachable.
    pub fn grid_days(&self) -> Vec<Day> {
        self.grid_days_memo.get()
    }

    /// The body behind `grid_days`, run ONLY by the memo built in
    /// `app.rs::init_app`. It walks all ~200 catalog courses and allocates a
    /// `Vec<EffMeeting>` per course (two `Meeting` clones apiece) to collect
    /// at most seven `Day`s, so every reader has to share one answer.
    pub(crate) fn compute_grid_days(&self) -> Vec<Day> {
        let mut days = vec![Day::Mon, Day::Tue, Day::Wed, Day::Thu, Day::Fri];
        // `with`, not `get`, for BOTH stores: this runs for every grid body,
        // day strip and facet, and `get` would deep-clone the whole Snapshot
        // (gzipped raw pages included) and every override each time.
        let selected = self.selected_courses();
        self.overrides.with(|overrides| {
            self.snapshot.with(|snapshot| {
                for c in &snapshot.courses {
                    for e in effective_meetings(c, overrides) {
                        if !days.contains(&e.meeting.day) {
                            days.push(e.meeting.day);
                        }
                    }
                }
            });
            for c in &selected {
                for e in effective_meetings(c, overrides) {
                    if !days.contains(&e.meeting.day) {
                        days.push(e.meeting.day);
                    }
                }
            }
        });
        days.sort_by_key(|d| d.index());
        days
    }

    /// Columns for the Halls tab: CMI's official slots, plus a synthetic
    /// column for every time that doesn't fit one — a booking CMI published
    /// at an unusual hour, or a meeting the user moved outside the grid.
    ///
    /// Same idea as `display_slot_grid`, different source data: this table
    /// shows rooms rather than the user's selection, so it must cover
    /// everything that can appear in it. Without this a 19:00 meeting has no
    /// column and vanishes from the page instead of moving house.
    pub fn hall_slot_grid(&self) -> Vec<(Slot, bool)> {
        let official = self.snapshot.with(|s| s.slot_grid.clone());
        let mut extra: Vec<Slot> = Vec::new();
        {
            let mut add = |slot: Slot| push_extra_column(&official, &mut extra, slot);
            self.snapshot
                .with(|s| s.hall_bookings.iter().for_each(|b| add(b.slot)));
            // Only placements WITH a hall can land in this table.
            self.overrides.with(|ovs| {
                for o in &ovs.items {
                    if let Some(m) = o.to.as_ref().filter(|m| m.hall.is_some()) {
                        add(m.slot);
                    }
                }
            });
            for course in self.selected_courses() {
                if !self.is_custom(&course.code) {
                    continue;
                }
                for m in course.meetings.iter().filter(|m| m.hall.is_some()) {
                    add(m.slot);
                }
            }
        }
        columns(official, extra)
    }

    /// The hall as it should be STORED: trimmed, and spelled the way CMI (or
    /// an earlier meeting of the user's own) spells it. Everything that
    /// matches halls does so case-insensitively, so a stray " lecture hall
    /// 803 " would otherwise sit in CMI's row while showing up as a separate,
    /// permanently empty "yours" row in the Halls tab. `None` = to be
    /// announced.
    pub fn canonical_hall(&self, raw: &str) -> Option<String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let known = self
            .snapshot
            .with_untracked(|s| s.halls.clone())
            .into_iter()
            .chain(self.user_halls())
            .find(|h| h.eq_ignore_ascii_case(raw));
        Some(known.unwrap_or_else(|| raw.to_string()))
    }

    /// Places the user put a course themselves that CMI's hall list doesn't
    /// contain: typed by hand ("Seminar room", "1002"), or a hall CMI has
    /// since dropped from its allocation page.
    ///
    /// Same rule as everywhere else — the user's own data must never be
    /// invisible. The Halls tab gives these their own rows, the hall
    /// chooser offers them again (a place you invented once is a click away
    /// the next time), and the hall facet can filter by them.
    ///
    /// The set matches what can actually render: every override's
    /// destination, plus the meetings of the user's own courses that are ON
    /// the timetable (a parked course occupies nothing).
    pub fn user_halls(&self) -> Vec<String> {
        let official = self.snapshot.with(|s| s.halls.clone());
        let mut out: Vec<String> = Vec::new();
        let mut add = |hall: &str| {
            let hall = hall.trim();
            if hall.is_empty()
                || official.iter().any(|h| h.eq_ignore_ascii_case(hall))
                || out.iter().any(|h: &String| h.eq_ignore_ascii_case(hall))
            {
                return;
            }
            out.push(hall.to_string());
        };
        self.overrides.with(|ovs| {
            for o in &ovs.items {
                if let Some(hall) = o.to.as_ref().and_then(|m| m.hall.as_deref()) {
                    add(hall);
                }
            }
        });
        for course in self.selected_courses() {
            if !self.is_custom(&course.code) {
                continue;
            }
            for m in &course.meetings {
                if let Some(hall) = m.hall.as_deref() {
                    add(hall);
                }
            }
        }
        out.sort();
        out
    }

    /// What the Halls tab should show right now.
    ///
    /// A stored choice always wins, so the tab stays where the user left it
    /// across reloads. With no stored choice the tab opens on TODAY, which
    /// is what someone looking for a free room almost always wants. On a
    /// weekend with nothing timetabled it opens on every day instead, rather
    /// than on an empty Saturday.
    pub fn halls_view(&self) -> HallsView {
        if let Some(stored) = self.prefs.with(|p| p.halls_view) {
            if let HallsView::Day(d) = stored {
                // A stored day CMI no longer publishes would title a table
                // the day strip has no button for.
                if !self.hall_days().contains(&d) {
                    return HallsView::All;
                }
            }
            return stored;
        }
        let today = crate::domx::today_local().weekday();
        if self.hall_days().contains(&today) {
            HallsView::Day(today)
        } else {
            HallsView::All
        }
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
        // A move in progress belongs to the grid that draws its cursor. It
        // used to survive a tab change, leaving the global arrow and Enter
        // handlers live on a page with nothing highlighted.
        self.move_mode.set(None);
        self.prefs.update(|p| p.tab = tab);
        self.persist_prefs();
    }

    /// Narrow the "what changed" digest to the reader's own courses, or
    /// widen it back. Deliberately NOT an undo step: it changes what the
    /// dialog shows, never what the timetable holds, and an "Undid…" toast
    /// for reading a list would be noise beside the real ones.
    pub fn set_changes_mine_only(&self, on: bool) {
        self.prefs.update(|p| p.changes_mine_only = on);
        self.persist_prefs();
    }

    pub fn goto_developer(&self) {
        domx::set_hash("#/developer");
    }

    pub fn goto_planner(&self) {
        domx::set_hash("#/");
    }

    /// Change one of the two filter sets as one undoable step, like any
    /// other action. `mine` picks the set: `true` is My courses' own set,
    /// `false` the one the Catalog and the Master grid share. With
    /// `coalesce`, a run of consecutive same-label edits shares a single
    /// history entry — the search box makes one entry per burst of typing,
    /// not one per keystroke. (Labels carry the page name, so a burst of
    /// typing on My courses can never amend a catalog entry.)
    pub fn act_filters_in(
        &self,
        mine: bool,
        label: &str,
        coalesce: bool,
        f: impl FnOnce(&mut Filters),
    ) {
        // A change that changes nothing is not an action. "All" over a menu
        // whose options are all ticked, or "None" over one with none ticked,
        // used to push an undo entry and wipe the redo stack for it — so the
        // Redo button went dead because of a click that did nothing at all.
        let before = self.prefs.with_untracked(|p| {
            if mine {
                p.my_filters.clone()
            } else {
                p.filters.clone()
            }
        });
        let mut after = before.clone();
        f(&mut after);
        if after == before {
            return;
        }
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
        self.prefs.update(|p| {
            if mine {
                p.my_filters = after;
            } else {
                p.filters = after;
            }
        });
        self.persist_prefs();
    }

    /// The shared (Catalog + Master grid) set — see `act_filters_in`.
    pub fn act_filters(&self, label: &str, coalesce: bool, f: impl FnOnce(&mut Filters)) {
        self.act_filters_in(false, label, coalesce, f);
    }

    /// One of the two filter sets, BORROWED — picked the same way
    /// `act_filters_in` picks the one it edits. Tracked, exactly as
    /// `filters_in` is, so a reader still re-runs when the set changes.
    ///
    /// Almost every reader wants a length, a flag or one field, and paying
    /// a deep copy of eight Vecs and a String for that is real cost where
    /// it lands: each of the ~300 rows of the Course facet carries an
    /// Effect that asks whether ITS key is ticked, and every one of them
    /// was cloning the whole set — the ticked course list included — on
    /// every keystroke in the search box.
    ///
    /// The closure runs while `prefs` is borrowed, so it must not read
    /// `prefs` AGAIN (§4's "never nest two reads of the SAME signal"):
    /// that rules out `filters`, `filters_in`, `with_filters`,
    /// `with_filters_in`, and any bare `app.prefs.with(…)` for
    /// tab/theme/density. Reads of OTHER signals are allowed by the same
    /// rule, but hoist them above the call anyway — keep the borrow short
    /// and obviously pure.
    pub fn with_filters_in<R>(&self, mine: bool, f: impl FnOnce(&Filters) -> R) -> R {
        self.prefs.with(|p| {
            if mine {
                f(&p.my_filters)
            } else {
                f(&p.filters)
            }
        })
    }

    /// The set the Catalog and the Master grid share, borrowed —
    /// `with_filters_in(false, …)`.
    pub fn with_filters<R>(&self, f: impl FnOnce(&Filters) -> R) -> R {
        self.with_filters_in(false, f)
    }

    /// The filter set the Catalog and the Master grid share, owned. For the
    /// callers that genuinely need the value to outlive the read — see
    /// `with_filters` for everything else.
    pub fn filters(&self) -> Filters {
        self.with_filters(Clone::clone)
    }

    /// One of the two filter sets, owned, picked the same way
    /// `act_filters_in` picks the one it edits. See `with_filters_in`.
    pub fn filters_in(&self, mine: bool) -> Filters {
        self.with_filters_in(mine, Clone::clone)
    }

    /// False until the first gate-passed sync: the app ships no timetable
    /// data, so an empty course list means "never synced".
    pub fn has_data(&self) -> bool {
        self.snapshot.with(|s| s.has_data())
    }
}

/// A course that is still selected but no longer in CMI's catalog: a stub
/// carrying just its code, so it stays visible with its "No longer on CMI's
/// timetable" badge and keeps whatever meetings the user placed on it.
fn removed_stub(code: &str) -> Course {
    Course {
        code: code.to_string(),
        name: code.to_string(),
        instructors: vec![],
        branches: vec![],
        credits: None,
        starts: None,
        part_of_semester: None,
        optional_flag: false,
        status: ScheduleStatus::UnscheduledListed,
        meetings: vec![],
    }
}

/// Halls compare on trimmed, case-insensitive text everywhere they are
/// matched: the user types theirs by hand, so "lecture hall 803" and a
/// pasted " LH9 " have to land in CMI's row rather than nowhere at all.
pub fn same_hall(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.trim().eq_ignore_ascii_case(b.trim()),
        (None, None) => true,
        _ => false,
    }
}

/// Note a time that CMI's slot grid has no room for, so the table can grow a
/// column of its own for it. Times already covered — exactly, or by falling
/// inside an official slot — are left alone.
fn push_extra_column(official: &[Slot], extra: &mut Vec<Slot>, slot: Slot) {
    let start = slot.start_min;
    if official
        .iter()
        .any(|s| s.start_min == start || (start >= s.start_min && start < s.end_min))
    {
        return;
    }
    match extra.iter_mut().find(|s| s.start_min == start) {
        // Same start, different lengths: one column spanning the widest
        // range; chips whose own times differ sublabel themselves.
        Some(s) => s.end_min = s.end_min.max(slot.end_min),
        None => extra.push(slot),
    }
}

/// CMI's columns first, then the synthetic ones, in time order. The flag is
/// "this column is not part of CMI's grid" — every table tints those.
fn columns(official: Vec<Slot>, extra: Vec<Slot>) -> Vec<(Slot, bool)> {
    let mut all: Vec<(Slot, bool)> = official.into_iter().map(|s| (s, false)).collect();
    all.extend(extra.into_iter().map(|s| (s, true)));
    all.sort_by_key(|(s, _)| s.start_min);
    all
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
/// `overrides` is passed in, not read here: this runs once per course in a
/// filter pass, and cloning the whole override store per course made the
/// search box quadratic in what the user had customised. `app` is still
/// needed for the two things the override store does not hold — the user's
/// own courses (`is_custom`) and the "fits my schedule" walk, which reaches
/// the snapshot and so must not run inside a read of it (§4).
pub fn course_matches(app: &App, course: &Course, f: &Filters, overrides: &OverridesStore) -> bool {
    // A course the user deleted is out of the catalog and the master grid
    // entirely. It comes first because no filter should be able to bring
    // one back — restoring it is a decision, made in "Your changes".
    if overrides.is_hidden(&course.code) {
        return false;
    }
    if !f.branches.is_empty() && !course.branches.iter().any(|b| f.branches.contains(b)) {
        return false;
    }
    if !f.instructors.is_empty() && !course.instructors.iter().any(|i| f.instructors.contains(i)) {
        return false;
    }
    let eff = effective_meetings(course, overrides);
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
    // Halls compare trimmed and case-insensitively here as everywhere else:
    // the filter can be picked from a place the user typed themselves, and
    // exact bytes would quietly match nothing.
    if !f.halls.is_empty()
        && !eff.iter().any(|e| {
            e.meeting.hall.as_deref().is_some_and(|h| {
                f.halls
                    .iter()
                    .any(|pick| pick.trim().eq_ignore_ascii_case(h.trim()))
            })
        })
    {
        return false;
    }
    if !f.credits.is_empty() {
        // Facet matches what the user sees — custom credit values included.
        // Read out of the store in hand, not back through the signal:
        // this is `App::course_credits` inlined.
        let cr = overrides
            .credits_for(&course.code)
            .unwrap_or_else(|| course.effective_credits())
            .to_string();
        if !f.credits.contains(&cr) {
            return false;
        }
    }
    if !f.flags.is_empty() {
        let has_custom = !eff.is_empty() && eff.iter().any(|e| e.overridden);
        let matches_flag = f.flags.iter().any(|flag| match flag.as_str() {
            "optional" => course.optional_flag,
            "unscheduled" => course.status == ScheduleStatus::UnscheduledListed,
            // A course of the user's own IS custom, times and all. It can
            // never carry an override (its definition is the schedule), so
            // testing only for overrides hid exactly the courses the flag
            // most obviously describes.
            "custom" => has_custom || app.is_custom(&course.code),
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
