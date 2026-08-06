# CMI Timetable Planner

A **100% client-side** timetable planner for students of the Chennai
Mathematical Institute. It parses CMI's two public timetable pages, lets you
assemble your personal timetable (drag meetings around in edit mode, resolve
clashes, export to your calendar), and keeps everything **saved in your
browser** — no backend, no accounts, no analytics. The header's **My data**
dialog shows exactly what is stored (including which CMI times your custom
times overwrite) with one-click removal for each piece.

Data sources (the only two):

- <https://www.cmi.ac.in/practical/timetable.php>
- <https://www.cmi.ac.in/practical/lecturehalls.php>

Built with **Rust → WebAssembly** ([Leptos](https://leptos.dev) CSR +
[Trunk](https://trunkrs.dev)), deployable as a static site on GitHub Pages.

## Architecture

```text
/core   Data model, parsers for both CMI pages, validation gate, snapshot
        diff, three-way merge, .ics generation, URL-state codecs.
        No wasm-only deps; unit-tested against committed HTML fixtures
        (core/fixtures/, fetched 5 Aug 2026).
/app    Leptos CSR UI (Trunk). Extracts <pre> blocks with the browser's
        DOMParser and feeds them to the same core parsing functions.
        build.rs bakes a bundled snapshot from the fixtures, so the very
        first load works even fully offline from CMI.
/sync   Small native binary (reqwest + core). The optional GitHub Actions
        cron uses it to publish a validated data mirror under
        app/public/data/. One parser everywhere: one source of truth.
```

### How "Sync now" gets data (the CORS reality)

`cmi.ac.in` sends no `Access-Control-Allow-Origin` header, so a browser on a
GitHub Pages origin cannot fetch the pages directly. Sync walks a tiered
source chain, each tier labeled for provenance in the sync pill and the
developer-mode fetch log. For speed, each tier fetches both pages **in
parallel**, and the proxy tier **races all relays at once** — the first
valid response wins:

1. **direct** — a cheap 4 s attempt at the CMI URLs (in case CORS ever opens up)
2. **proxy** — public CORS relays raced in parallel (see `app/src/fetch.rs`),
   every response sanity-checked before being trusted
3. **mirror** — same-origin `data/latest.json` + raw HTML copies committed by
   the `sync.yml` cron
4. **bundled** — a snapshot compiled in at build time from the fixtures

A fetched snapshot replaces the cache **only after the validation gate
passes** (≥ 10 branch grids, ≥ 40 courses, ≥ 90 % legend resolution, sane
hall grid and slots, matching semester labels). Any failure leaves the cache
untouched and is explained in plain language. **Fail closed, always.**

### Storage

Everything lives in `localStorage` under versioned `cmitt.v1.*` keys
(snapshot incl. compressed raw HTML, selection, overrides, prefs).
Corrupt blobs are backed up under `cmitt.corrupt.<ts>` — never deleted.
On quota pressure the raw HTML copies are dropped first. The snapshot stores
`parser_version` + the raw pages, so shipping a parser fix re-parses the
stored HTML on next load without refetching (bump `PARSER_VERSION` in
`core/src/model.rs` whenever parsing behavior changes).

### URL state

`?c=TOC,QCOM,MFD` reproduces a selection anywhere; `&s=<lz-string>` also
carries meeting overrides ("Share including my custom times"). When both are
present, `s` wins. The query stays *before* the hash: `…/?c=TOC#/`.

### Everyday use

- **Edit layout** (My timetable / Master grid toolbars) turns on drag & drop
  — chips are deliberately not draggable outside edit mode so touch
  scrolling and clicks stay accident-free. Keyboard alternative while
  editing: focus a chip, press `M`, arrows, `Enter`.
- Deselecting a course **keeps** its custom times, so re-adding it (or
  spotting it in the master grid) doesn't silently revert a move. Remove
  custom times explicitly per meeting, per course, or in **My data**.
- In the master grid an **ⓘ** button opens full course details, and
  unselected courses that would clash with your current timetable carry a
  **⚠** marker; adding a clashing course warns immediately (never blocks).
- Credits: CMI states credits only exceptionally; unstated courses count as
  **4 credits** (marked "assumed" in the details view).

### Developer mode (hidden endpoint)

Developer mode is not linked anywhere in the UI. Open it by navigating to
the **`#/developer`** endpoint directly, e.g.
`https://<host>/<repo>/#/developer` (or `http://127.0.0.1:8080/#/developer`
during development). It exposes the fetch log, parse reports, cache
inspector, raw-HTML viewer and fail-closed simulators.

### A note on routing (deliberate deviation)

The build spec asks for `leptos_router` *and* hash routing. leptos_router
0.8 hard-codes a pathname-based `BrowserUrl` location provider — it cannot
route on `location.hash` (verified against its source). Hash routing is the
load-bearing requirement for GitHub Pages (no server rewrites), so the app
uses a minimal hand-rolled hash router instead (two routes: `#/` and
`#/developer`, `hashchange`-driven — see `app/src/app.rs`). `404.html`
bounces unknown paths back to `index.html` preserving the query string.

Two more reality-driven deviations, both verified against the live pages on
5 Aug 2026 and documented in the code:

- Grid rows are split on their **own** `|` separators (with the header's
  character indices only as a fallback): the OCS1–3/OPDS1 headers and the
  hall grid's header are misaligned with their data rows by 1–2 characters,
  so slicing everything at the header's indices would misparse real data.
- Gate rule 1 treats a missing semester label on `lecturehalls.php` as a
  warning (the live page has none); a *conflicting* label still fails.

## Development

```sh
# prerequisites
rustup target add wasm32-unknown-unknown   # or your distro's rust-wasm pkg
cargo install trunk --locked               # or download a release binary

# dev loop (http://127.0.0.1:8080)
cd app && trunk serve

# tests — parser tests 1–11 against the fixtures, merge decision table,
# .ics golden files, URL codecs
cargo test --workspace

# regenerate the .ics golden after an intentional format change
UPDATE_GOLDEN=1 cargo test -p cmi-timetable-core --test ics_tests

# end-to-end browser tests (Selenium + headless Chromium) — see e2e/README.md
python e2e/test_app.py

# run the mirror publisher locally (writes app/public/data/)
cargo run -p cmi-timetable-sync
```

## Deploying

Push to `main`: `.github/workflows/deploy.yml` runs the tests, builds with
`--public-url "/<repo-name>/"` and deploys to GitHub Pages (enable Pages →
"GitHub Actions" in the repo settings). For a **user/organization page**
(`<user>.github.io` repo) change the public URL to `/` in `deploy.yml`, and
adjust the `base` computation in `app/public/404.html` (comment inside).

`.github/workflows/sync.yml` (optional; the app works without it) runs the
`/sync` binary every 6 hours, commits the validated mirror to
`app/public/data/` and re-deploys via `workflow_call` (GITHUB_TOKEN pushes
don't retrigger `push` workflows). A red sync run means the gate failed and
the last good mirror stayed.

## Maintenance recipes

- **Add/replace a CORS proxy**: edit the `PROXIES` array in
  `app/src/fetch.rs` — one entry per relay, plus a documented slot for a
  self-hosted Cloudflare Worker (the most reliable option).
- **Bump `PARSER_VERSION`**: change the constant in `core/src/model.rs`
  whenever parsing output changes shape or meaning. Cached snapshots with an
  older version re-parse their stored raw HTML automatically at startup.
- **Refresh fixtures next semester**: save both pages verbatim over
  `core/fixtures/*.html`, run `cargo test -p cmi-timetable-core`, and update
  the semester-specific expectations in `core/tests/parser_tests.rs`
  (branch list, course names, the unscheduled set, …). The bundled snapshot
  regenerates from the fixtures automatically via `app/build.rs`.

## Manual acceptance checklist

The full 15-point checklist from the build spec (fresh-browser share links,
offline first load, fail-closed updates, touch drag, conflict dialog,
keyboard-only operation, print to one landscape A4, Lighthouse a11y ≥ 95,
cross-browser) lives in the spec and should be run against a deployed build
before each semester rollover.
