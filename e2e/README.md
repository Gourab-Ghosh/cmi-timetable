# End-to-end browser tests

`test_app.py` serves the built app and drives it with Selenium in headless
Chromium — real pointer events, real localStorage, real drag & drop.

```sh
# one-time setup
python3 -m venv .venv && .venv/bin/pip install selenium   # driver auto-managed

# build the app, then run
(cd ../app && trunk build --release)
.venv/bin/python test_app.py
```

Environment knobs: `CHROME_BIN` (default `/usr/bin/chromium`), `DIST_DIR`,
`PORT`.

Covered flows (15 tests): Sync-now header + hidden developer mode (URL
endpoint only), `?c=` selection + clash badges/panel, unknown-code warning,
credits defaulting to 4, master-grid ⚠ would-clash markers and clash toast on
add, the ⓘ details button, drag & drop gated behind Edit layout, custom
times surviving deselection and reloads, the My data dialog listing/removing
overrides, undo/redo, "Give it a time" auto-selecting, and the free-hall
finder requiring an explicit day + slot.

Each test starts from a wiped localStorage with the background sync
suppressed, so everything runs deterministically against the bundled
snapshot — no network needed.
