# End-to-end browser tests

`test_app.py` serves the built app and drives it with Selenium in headless
Chromium — real pointer events, real localStorage, real drag & drop.

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
`PORT`.

Covered flows (24 tests): Sync-now header + hidden developer mode (URL
endpoint only), `?c=` selection + clash badges/panel (any casing), unknown-
code warning, credits defaulting to 4 and per-course credit overwrites,
master-grid ⚠ would-clash markers and clash toast on add, the ⓘ details
button, drag & drop gated behind Edit layout, custom times surviving
deselection and reloads, the unified overwrites list (Your changes panel +
My data) with per-item and remove-all restore, filter dropdowns closing
each other / on outside click / on Esc, adding extra weekly meetings to any
course, undo/redo, "Give it a time" auto-selecting, the free-hall finder
requiring an explicit day + slot, hall-and-slot drag & drop in the Halls
view, filter menus keeping focus/scroll while ticking boxes, the ✓
selected-course marker in the master grid, and toasts pausing while
hovered.

Each test starts from a wiped localStorage with the background sync
suppressed, so everything runs deterministically against the bundled
snapshot — no network needed.
