# CMI Timetable Planner

A **100% client-side** timetable planner for students of the Chennai
Mathematical Institute. It parses CMI's two public timetable pages, lets you
assemble your personal timetable (drag meetings around in edit mode, resolve
clashes, export to your calendar), and keeps everything **saved in your
browser** — no backend, no accounts, no analytics. The header's **My data**
dialog shows exactly what is stored (including which CMI times your custom
times overwrite) with one-click removal for each piece.

The app **ships no timetable data at all** — no bundled snapshot, no
checked-in mirror. On first load it shows a welcome screen asking for one
sync; after that the fetched timetable lives in the browser and everything
works offline. Nothing about CMI's current pages is hard-coded anywhere.

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
        Ships empty: the first load asks for a sync (build.rs only stamps
        build metadata — no data is baked in).
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
2. **proxy** — public CORS relays raced in parallel (see `app/src/fetch.rs`)
3. **mirror** — same-origin `data/latest.json` + raw HTML copies committed by
   the `sync.yml` cron (never committed by hand — the repo carries no data)

Until the first sync succeeds the app stays on its welcome screen; a failed
first sync explains itself in a banner and every later page load retries
(the usual 12 h background throttle only applies once data exists).

The parser + validation gate are the **only** judges of content — no
hard-coded shape or wording check can reject a page they would accept, so a
CMI redesign surfaces honestly as "the app needs an update", never as fake
unreachability. (A loose marker check runs only on proxy responses, only
after a gate failure, to tell proxy error pages apart from real CMI drift.)

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

`?c=TOC,QCOM,MFD` reproduces a selection anywhere (codes are matched
case-insensitively against the live catalog, so hand-typed `?c=toc` works);
`&s=<lz-string>` also carries meeting **and credit** overrides ("Copy incl.
my custom changes"). When both are present, `s` wins. The query stays
*before* the hash: `…/?c=TOC#/`.

### Everyday use

- **Edit layout** (My timetable / Master grid / Halls toolbars) turns on
  drag & drop — chips are deliberately not draggable outside edit mode so
  touch scrolling and clicks stay accident-free. Keyboard alternative while
  editing: focus a chip, press `M`, arrows, `Enter`. In the **Halls** view a
  drop targets a hall row *and* a time column, so one gesture moves a
  meeting into a different hall and slot — and the grid updates in place:
  moved meetings render in their new cell (dashed, ✎) and leave the official
  one; dropping back on the official cell resets the change.
- **Filters are undoable** like everything else (one step per change, one
  per burst of typing in the search box), and every filter dropdown has its
  own search field plus **All** / **None** shortcuts that act on whatever
  the search currently shows. A **Course** dropdown filters to hand-picked
  courses.
- Deselecting a course **keeps** its custom times, so re-adding it (or
  spotting it in the master grid) doesn't silently revert a move. Remove
  custom times explicitly per meeting, per course, or in **My data**.
- **Every custom change is visible in one place**: the "Your changes" panel
  on My timetable (and the same list inside **My data**) shows each custom
  meeting and credit change as *official → yours*, each with a one-click
  Remove, plus "Remove all changes". A "✎ N changes" pill sits in the grid
  toolbars whenever custom data is in play, and overridden meeting rows say
  inline exactly which CMI time they overwrite.
- **Any course can gain extra time slots**: "Add a meeting" (course card or
  details dialog) appends another weekly meeting; unscheduled courses get
  the same flow as "Give it a time".
- In the master grid your selected courses are unmistakable — a **✓ mark
  plus an accent ring** (never color alone); an **ⓘ** button opens full
  course details, and unselected courses that would clash with your current
  timetable carry a **⚠** marker; adding a clashing course warns immediately
  (never blocks). Notifications pause their auto-dismiss while hovered or
  focused, so there's always time to read (and hit Undo).
- Credits: CMI states credits only exceptionally; unstated courses count as
  **4 credits** (marked "assumed"). The details dialog lets you **overwrite
  credits** per course (totals, filters and the catalog follow suit), with a
  one-click reset to CMI's value.

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

A second deliberate deviation: the build spec's bundled snapshot ("first
load works offline") was **removed by explicit request** — nothing about
CMI's pages may ship inside the app. First load now asks for a sync; the
fixtures remain in the repo only for parser tests and the e2e seed.

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
  (branch list, course names, the unscheduled set, …). The fixtures feed
  only the parser tests and the e2e seed — the shipped app carries no data.

## Manual acceptance checklist

The full 15-point checklist from the build spec (fresh-browser share links,
offline first load, fail-closed updates, touch drag, conflict dialog,
keyboard-only operation, print to one landscape A4, Lighthouse a11y ≥ 95,
cross-browser) lives in the spec and should be run against a deployed build
before each semester rollover.
