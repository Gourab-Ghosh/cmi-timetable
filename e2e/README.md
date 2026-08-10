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

Covered flows (65 tests): Sync-now header + hidden developer mode (URL
endpoint only), `?c=` selection + clash badges/panel (any casing), unknown-
code warning, credits defaulting to 4 and per-course credit overwrites,
master-grid ⚠ would-clash markers and clash toast on add, the ⓘ details
button, drag & drop gated behind Edit layout, custom times surviving
deselection and reloads, the unified overwrites list (Your changes panel +
My data) with per-item and remove-all restore, filter dropdowns closing
each other / on outside click / on Esc, adding extra weekly meetings to any
course, undo/redo, an unscheduled course opening the same editor as any
other (and its credits saving without it gaining a time), the shared filter
bar on My courses, the wheel stepping every box and dropdown that has a
step, the course editor asking before a stray Escape throws a half-written
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
one used to vanish on save).

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
