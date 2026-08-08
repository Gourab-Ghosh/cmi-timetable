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

Covered flows (41 tests): Sync-now header + hidden developer mode (URL
endpoint only), `?c=` selection + clash badges/panel (any casing), unknown-
code warning, credits defaulting to 4 and per-course credit overwrites,
master-grid ⚠ would-clash markers and clash toast on add, the ⓘ details
button, drag & drop gated behind Edit layout, custom times surviving
deselection and reloads, the unified overwrites list (Your changes panel +
My data) with per-item and remove-all restore, filter dropdowns closing
each other / on outside click / on Esc, adding extra weekly meetings to any
course, undo/redo, "Give it a time" auto-selecting, the free-hall finder
requiring an explicit day + slot, hall-and-slot drag & drop in the Halls
view (including the chip relocating in the halls grid itself, surviving
reload, and drag-back-to-reset), filter menus keeping focus/scroll while
ticking boxes, the ✓ selected-course marker in the master grid, toasts
pausing while hovered, the first-run welcome prompt (failed sync → honest
banner; reachable mirror → auto-populates), filters in the undo history
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
share links carrying full definitions to a fresh browser, and delete as
one undoable step.

The app ships no timetable data, so the suite derives a snapshot from
`core/fixtures/` at startup (via core's `snapshot_json` example — cargo must
be on PATH) and seeds it into localStorage before each test. The browser
runs with all non-localhost DNS blackholed: the direct/proxy tiers fail
instantly, the first-run tests serve the same seed as a fake same-origin
mirror, and nothing ever touches the real network.
