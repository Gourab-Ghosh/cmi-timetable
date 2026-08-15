# End-to-end browser tests

`test_app.py` serves the built app and drives it with Selenium in headless
Chromium — real pointer events, real localStorage, real drag & drop.
`shoot.py` (same setup) captures design-review screenshots of every view
plus the print PDFs into `shots/` (gitignored).

```sh
# one-time setup
python3 -m venv .venv && .venv/bin/pip install selenium   # driver auto-managed

# build the app, then run
(cd ../app && trunk build --release)
.venv/bin/python test_app.py

# if `trunk serve` is running, build to a separate dist and target dir so the
# watcher can't race the test build (truncated wasm / mismatched hashes):
(cd ../app && CARGO_TARGET_DIR=~/.rust-target-e2e trunk build --release --dist dist-e2e)
DIST_DIR=../app/dist-e2e .venv/bin/python test_app.py
```

Environment knobs: `CHROME_BIN` (default `/usr/bin/chromium`), `DIST_DIR`,
`PORT`, `CARGO_TARGET_DIR` (for the seed generator; defaults to
`~/.rust-target-e2e`).

Covered flows (88 tests): Sync-now header + hidden developer mode (URL
endpoint only), `?c=` selection + clash badges/panel (any casing), unknown-
code warning, credits defaulting to 4 and per-course credit overwrites,
master-grid ⚠ would-clash markers and clash toast on add, the ⓘ details
button, drag & drop gated behind Edit layout, custom times surviving
deselection and reloads, the unified overwrites list (Your changes panel +
My data) with per-item and remove-all restore, filter dropdowns closing
each other / on outside click / on Esc, adding extra weekly meetings to any
course, undo/redo, an unscheduled course opening the same editor as any
other (and its credits saving without it gaining a time), the filter
bar on My courses (its own state since R43; menus offer only what your own
courses have, and no "Fits my schedule" box it could not act on), the
wheel stepping every box and dropdown that has a step, the course editor asking before a stray Escape throws a half-written
form away, the free-hall finder requiring an explicit day + slot, hall-and-slot drag & drop in the Halls
view (including the chip relocating in the halls grid itself, surviving
reload, and drag-back-to-reset), filter menus keeping focus/scroll while
ticking boxes, the ✓ selected-course marker in the master grid, toasts
pausing while hovered, the first-run welcome prompt (failed sync → honest
banner; reachable CMI → auto-populates), filters in the undo history
(with search-typing coalesced into one step), the per-dropdown
search + All/None shortcuts incl. the Course facet, share links carrying
custom changes onto a fresh browser, the full three-way-merge conflict
flow (keep-mine rebases, removed-course badge, What-changed digest),
keyboard-only move mode, corrupt-storage recovery (backup + sticky
banner), .ics export honoring overrides, mobile long-press drag (the
native context menu is suppressed mid-gesture, a browser-cancelled drag
can't deselect the course, touch drags land, plain taps still toggle),
removing a meeting (chip leaves the grid, listed as a restorable change,
survives reloads), and out-of-grid times getting their own clearly-marked
column instead of being squeezed into the last official slot, and the
catalog updating in place — clash marks, meeting times and selection state
change live as courses are added/removed/edited or the whole selection is
cleared, with no reload or tab switch — duration-based credit
assumptions (a "(Oct-Nov)" course counts 2 credits, stated credits win,
My courses breaks the selection down by credit value), the header's
"Synced … ago" pill ticking with the wall clock on its own (30 s interval
plus an instant visibilitychange catch-up, crossing into the stale tint
at 48 h without any reload), and the user's own courses: the name-first
create form (auto-suggested code, segmented credits, official-slot and
custom-time meeting rows, live clash line), grid chips including a
synthetic out-of-grid column, editing the definition in place with no
override bookkeeping, parking off the timetable instead of deleting,
share links carrying full definitions to a fresh browser, delete as one
undoable step, a course whose code a later CMI sync also introduces (your
version keeps winning, the catalog chip refreshes live when you switch to
CMI's), and the create form surviving a sync that lands mid-typing — plus the
one course editor: every course, CMI's and your own, is changed in a single
form (times, hall, credits, and for your own its name and code), saved as
ONE undoable step, with each changed row saying which of CMI's meetings it
replaces and a list of the ones you struck out so they can be put back;
deleting one of CMI's courses (it leaves the timetable, the catalog and the
master grid, is listed under Your changes with a Restore, and a link naming
it lifts the deletion); courses you added and deleted appearing in that same
list; and every button that takes something away wearing the same red —
and, guarding the two places where the app used to lie about CMI's data:
developer mode's parse-failure simulator (it must actually fail the gate,
keep the cached timetable, and not take the app down with it) and a hall
booking published at a time that starts inside an official column (it
belongs in the column that contains it, and the free-hall finder must not
call that room empty) — plus the two ways a save or a sync could quietly
take something away: a sync landing with a conflict while the course editor
is open (there is one dialog slot, and the conflict must wait in its banner
rather than throw the form away) and a meeting added exactly where a moved
CMI meeting used to be (two rows want the same official meeting; the added
one used to vanish on save) — and, holding the app to "a control that cannot
act is not offered" in the four places it still broke it: the master grid
counting courses it has no cell to draw (it now counts what it draws, says
where the others are, and stops offering the Unscheduled filter that could
only ever empty it), a keyboard move on the phone's per-day list (a visible
cursor, and the day strip following it across days), the ✓ Halls promises
appearing on a booking with no meeting behind it, and the export dialog
asking which courses to put in the file when only one course is on the
timetable — plus the invariant that keeps the "what changed" dialog from
ever opening with nothing to say; the R43 round — the app booting entirely
from its service worker's cache with the server dead (offline note included,
on its own port so its worker can't leak into the suite), My courses' filter
state being separate from the Catalog+Master grid's shared state (neither
leaks into the other; undo restores the right set), a share link opened in a
never-synced browser raising ZERO conflicts on its first sync while a real
conflict deferred with "Decide later" survives a reload, the What-changed
digest carrying what a dropped course WAS — one line per course, the record
opening as its own popup from the code, Back returning to the digest, and
that record being keepable as a course of your own (permanent through a
reload, one undoable step, credits still a guess, and the times YOU placed
winning over CMI's old ones) (and that course haunting nothing else), seventy active-filter chips collapsing behind "+N more" with every
chip still removable when expanded, the two JSON exports parsing and the
whole-planner backup restoring EVERYTHING through a wiped browser — the
selection, the custom move and the full catalog, with the pill saying
"imported" (R46 reshaped this: the backup file replaced the old
snapshot-only export, and "Import my courses" on Course selection asks
replace-or-add through a dialog, names the codes it must leave out, and
skips the question when the timetable is empty; R47's pre-deploy audit
added pins for the honest edges: an import that adds nothing spends no
undo step, a whitespace-variant duplicate code is named once, a code with
a comma or % is refused in the form with the share-link reason, the wheel
never reverses direction against a clamp, never fills an empty box, and
gathers trackpad deltas into whole notches, and a server answering 503 —
GitHub Pages during an outage — loses to the worker's cached copy; R48
closed the seven long-documented §8 bugs, each pinned: the conflicts
dialog answers nothing for you and applies only the rows you answered
with the rest still waiting and a Dismiss that hides without answering, Restore
returns a deleted course to the timetable it was deleted from, an
untouched save of a dropped course invents no credit change, editing an
unselected course asks with a ticked box before adding it, a course
hidden by filters is named with "Clear filters to show it" instead of a
duplicate-minting create offer, the day pickers and credits row are
arrow-key radio groups, and every badge/credits explanation is visible
text or a button to the details dialog rather than a hover title), and a
creditless seminar counting 0 with the note saying why; and the order
of the source chain, which
exists for a reason a browser enforces: on CMI's own network cmi.ac.in is a
LOCAL address, so a direct fetch makes the browser ask whether the page may
reach devices on the local network. The relays are public hosts, go first,
and while one answers nothing touches cmi.ac.in itself; when they are all
dead the direct route runs last and says what it is about to do.

`design-check-url.txt` holds the link to open when checking how the app
LOOKS rather than what it does: eleven courses, several customised, so the
grid is dense, the clash panel has something in it and "Your changes" shows
most of its groups at once. `shoot.py` reads that file and captures it in
light, dark and mobile (`00-*-design-link`). Use it — a two-course planner
hides almost every spacing problem.

The app ships no timetable data, so the suite derives a snapshot from
`core/fixtures/` at startup (via core's `snapshot_json` example — cargo must
be on PATH) and seeds it into localStorage before each test.

The browser runs with every non-localhost hostname blackholed, so a sync
fails instantly and nothing touches the real network. The tests that need a
sync to *succeed* call `serve_cmi()`, which stands a TLS server up on
localhost holding the fixture pages while Chromium resolves www.cmi.ac.in to
it (`--ignore-certificate-errors`, since the cert is self-signed). Those
tests therefore run the app's real DIRECT tier rather than a test-only path.
It answers 503 until a test asks for it, and the runner switches it back off
after every test, so "CMI is unreachable" stays the default.

Because the fake CMI serves the fixtures, a test that needs CMI to *differ*
from the stored snapshot arranges it from the other side:
`cache_from_before_cmi_moved_toc()` seeds a snapshot in which TOC's first
class sits on Friday, so syncing against
the real fixtures looks exactly like CMI moving it back to Tuesday.
