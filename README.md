# CMI Timetable Planner

**Live:** <https://gourab-ghosh.github.io/cmi-timetable/>

A **100% client-side** timetable planner for students of the Chennai
Mathematical Institute. It parses CMI's two public timetable pages, lets you
assemble your personal timetable (drag meetings around in edit mode, add or
remove individual meetings, resolve clashes, export to your calendar), and
keeps everything **saved in your browser** — no backend, no accounts, no
analytics. Meetings at unusual hours get their own clearly-marked column
rather than being squeezed into the nearest official slot. The header's **My data**
dialog shows exactly what is stored (including which CMI times your custom
times overwrite) with one-click removal for each piece.

The app **ships no timetable data at all**, and neither does the site it is
served from: there is no bundled snapshot and no hosted copy of CMI's pages.
Every timetable you see was fetched from cmi.ac.in by your own browser, so
it is CMI's current page and not somebody's saved version of it. On first
load the app shows a welcome screen asking for one sync; after that the
fetched timetable lives in your browser and everything works offline.
Nothing about CMI's current pages is hard-coded anywhere.

Data sources (the only two):

- <https://www.cmi.ac.in/practical/timetable.php>
- <https://www.cmi.ac.in/practical/lecturehalls.php>

Built with **Rust → WebAssembly** ([Leptos](https://leptos.dev) CSR +
[Trunk](https://trunkrs.dev)), deployable as a static site on GitHub Pages.

> **[FEATURES.md](FEATURES.md) — everything the app does, in plain English.**
> Start there if you want to use the planner; this file is about how it is
> built and how to work on it.

## Architecture

```text
/core   Data model, parsers for both CMI pages, validation gate, snapshot
        diff, three-way merge, .ics generation, URL-state codecs.
        No wasm-only deps; unit-tested against two saved pages in
        core/fixtures/ (test input only — never served, never bundled)
        and against synthetic pages the tests generate themselves.
/app    Leptos CSR UI (Trunk). Extracts <pre> blocks with the browser's
        DOMParser and feeds them to the same core parsing functions.
        Ships empty: the first load asks for a sync (build.rs only stamps
        build metadata — no data is baked in).
```

### How "Sync now" gets data (the CORS reality)

`cmi.ac.in` sends no `Access-Control-Allow-Origin` header, so a browser on a
GitHub Pages origin cannot fetch the pages directly. Sync walks a tiered
source chain, each tier labeled for provenance in the sync pill's tooltip and
the developer-mode fetch log (the pill's own text names the route only when
it is actionable — "old copy", "imported"). For speed, each tier fetches both pages **in
parallel**, and the proxy tier **races all relays at once** — the first
valid response wins:

1. **proxy** — public CORS relays raced in parallel (see `app/src/fetch.rs`)
2. **direct** — a cheap 4 s attempt at the CMI URLs, only if no relay answered

Both routes end at cmi.ac.in. There is deliberately no third tier serving a
copy of the pages from this site: a fallback like that works by showing you
something CMI published a while ago, without you knowing how long ago, and a
timetable you can't date is worse than an honest "couldn't reach CMI".

**Why the relays go first, though they are the less trustworthy route.** Most
people using this app are on CMI's own network, where `www.cmi.ac.in`
resolves to a *private* address. A page served from github.io asking for a
private address is exactly what the browser's local-network permission prompt
exists to catch — so pressing Sync asked the student whether this site may
"access devices on your local network", about the one fetch the whole app is
for. A relay is a public host and can never raise that prompt. The direct
route is kept because it is CMI's own bytes and the only route that works
when every relay is down; it runs last, announces itself, and explains the
prompt before it can appear. Two consequences worth knowing: the relays see
which CMI pages are being fetched (nothing else — no selection, no
identity), and because a relay's cache would otherwise decide how fresh a
timetable is, the URL handed to a relay carries a cache-buster.

### Offline (service worker)

A Trunk `post_build` hook (`app/hooks/gen-sw.sh`) writes `sw.js` into every
release build: a service worker that precaches that exact build (cache name =
hash of every file's name and bytes), so the app opens with no connection
after one normal visit. Navigations are network-first with a cached-shell
fallback; hashed assets are cache-first; **cross-origin requests (cmi.ac.in,
the relays) are never intercepted**, so the sync path is byte-identical with
or without the worker, and no copy of CMI's pages ever enters the worker's
cache (R32 holds). Debug builds (`trunk serve`) get a self-cleaning stub that
caches nothing. A new deploy replaces the cache on the next online reload; no
prompts, no reload loops.

### JSON exports and the whole-planner backup

`core/src/export.rs` owns the file formats (versioned, semver'd, natively
tested); `app/src/export.rs` builds them from the app's own course
resolution and stores. `cmi-timetable-export` (1.1.0) describes the
student's week in two halves: `courses` — stable keys, effective meetings,
credit provenance — and `my_changes`, which round-trips the parts a catalog
cannot supply (classes moved/added/struck out, credit corrections, the
student's own courses). Both are written in the format's own explicit style
rather than the app's storage shapes, with every list always present and
each value stated both ways (minutes beside "HH:MM", ISO weekday beside
"Mon", ISO 8601 beside epoch ms); reading defaults the decoration, so
another program can write one of these files from the load-bearing fields
alone. "Import my courses…" (Share → As a timetable file) reads it back and asks
join-or-replace through a dialog; `core/src/combine.rs` owns the merge
rules — identical changes collapse, a contested class keeps the reader's,
invented classes are additive, deletions never travel — and is where the
native tests for combining two students' weeks live. `cmi-planner-backup` carries the
WHOLE planner — the internal `Snapshot` serde JSON plus selection,
overrides, custom courses, prefs and postponed conflicts — and importing it
(Share → "As a full backup", or the welcome screen) replaces the saved
state and reloads — after a confirm, unless the planner is untouched
(`App::planner_is_untouched`: no selection, no overrides, no customs), in
which case there is nothing to confirm away: validated fail-closed in core (envelope,
snapshot sanity) and in app (each store), labelled `SourceTier::Imported`
in the pill, keeping the ORIGINAL fetch date.

Until the first sync succeeds the app stays on its welcome screen; a failed
first sync explains itself in a banner and every later page load retries
(the usual 12 h background throttle only applies once data exists).

The parser + validation gate are the **only** judges of content — no
hard-coded shape or wording check can reject a page they would accept, so a
CMI redesign surfaces honestly as "the app needs an update", never as fake
unreachability. (A loose marker check runs only on proxy responses, only
after a gate failure, to tell proxy error pages apart from real CMI drift.)

A fetched snapshot replaces the stored one **only after the validation gate
passes** (≥ 10 branch grids, ≥ 40 courses, ≥ 90 % legend resolution, sane
hall grid and slots, matching semester labels). Any failure leaves the stored
snapshot untouched and is explained in plain language. **Fail closed,
always.**

### Storage

Everything lives in `localStorage` under versioned `cmitt.v1.*` keys
(snapshot incl. compressed raw HTML, selection, overrides, prefs).

**Only `cmitt.v1.snapshot` is a cache** — it is CMI's data, and a sync can
fetch it again. Everything beside it (`selection`, `overrides`, `custom`,
`prefs`) is the user's own work, exists nowhere else, and is never called a
cache in this codebase: the word decides how carelessly code and copy treat
a key, and these are the keys nothing can rebuild.
Corrupt blobs are backed up under `cmitt.corrupt.<ts>` — never deleted.
On quota pressure the raw HTML copies are dropped first. The snapshot stores
`parser_version` + the raw pages, so shipping a parser fix re-parses the
stored HTML on next load without refetching (bump `PARSER_VERSION` in
`core/src/model.rs` whenever parsing behavior changes).

### URL state

`?c=TOC,QCOM,MFD` reproduces a selection anywhere (codes are matched
case-insensitively against the live catalog, so hand-typed `?c=toc` works);
`&s=<lz-string>` also carries meeting **and credit** overrides ("Copy link
with custom changes"). When both are present, `s` wins. The query stays
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
  courses. The same bar sits on **Catalog**, **Master grid** and **My
  courses** — on My courses it narrows the courses you have picked, so
  "which of mine meet on Thursday" is one click. It is one set of filters,
  so what you set on one page is still set on the next; the credit total
  keeps counting your whole timetable, and says so when the list below it
  is showing fewer. Every facet's options are drawn from **the courses that
  bar is actually filtering** (`FilterScope`), so no menu offers a value
  that could only ever match nothing — and **"Fits my schedule" is not
  rendered on My courses at all**, because `fits_schedule` returns true for
  anything already selected, which made it a checkbox that could not act.
- **One editor per course.** Clicking any course and pressing **Edit this
  course** opens the whole of it in one form — every weekly meeting (day,
  time, hall), its credits, and for a course of your own its name and code —
  saved in a single step you can undo in one go. Each row you change says
  which of CMI's meetings it replaces, with **Put it back**; meetings you
  struck out are listed underneath so you can put those back too.
- Deselecting a course **keeps** its custom times, so re-adding it (or
  spotting it in the master grid) doesn't silently revert a move. Remove
  custom times in the editor, or in **My data**.
- **Deleting** a course takes it out of your planner entirely: off the
  timetable, out of the catalog and the master grid. CMI's pages are never
  edited, so this is your copy of them — the deletion is listed under "Your
  changes" with a one-click **Restore** that brings the course back along
  with everything you had done to it, the catalog says how many are hidden,
  and a link naming a deleted course lifts the deletion. Anything that takes
  something away — delete, remove, clear, reset — is **red**.
- **Every custom change is visible in one place**: the "Your changes" panel
  on My timetable (and the same list inside **My data**) shows each one as
  *official → yours*, grouped by what kind of change it is — courses you
  added, courses you deleted, meetings moved, added or removed, credits you
  set — each with a one-click way back that says what pressing it leaves
  behind ("Put it back", "Back to CMI's time", "Back to CMI's room", "Back
  to CMI's credits"), plus "Undo my changes to CMI's courses" (which keeps
  your own courses). A "✎ N changes" pill sits in the grid
  toolbars whenever custom data is in play, counting exactly the rows in
  that list, and overridden meeting rows say inline which CMI time they
  overwrite.
- **The wheel adjusts a box that has a step.** Scroll over the credits box
  (the one behind "Other…"), a meeting's start or end time, an export date,
  or the calendar-reminder lead, and it moves one step — hovering is
  enough, no click first. Dropdowns count too — a meeting's Day, Time and
  Hall, the export scope, the free-hall finder's day and slot — because a
  list of options is a box with a step as much as a number is. While the
  wheel is over a box, the box takes the scroll and the dialog behind it
  stays put; the reminder lead nudges by single minutes on the wheel while
  its arrows jump by fives.
- **Enter does the obvious thing.** In the course editor it saves, in Export
  it downloads the file, and in a search box it puts the phone keyboard away.
- **A half-written form isn't thrown away by a stray key.** Nothing in the
  course editor is committed until you press Save, so Escape and a click on
  the dark area ask first — but only once you've actually changed something.
- **Any course can gain extra time slots**: "＋ Add a weekly meeting" in the
  editor appends another one. Every course, scheduled or not, has the same
  one door — **"Edit this course"** — and it opens on what the course
  actually has. A course CMI hasn't given a time opens with no meetings and
  no row filled in on your behalf, so you can change its credits, or its
  name, without it quietly acquiring a Monday morning class.
- In the master grid your selected courses are unmistakable — a **✓ mark
  plus an accent ring** (never color alone); an **ⓘ** button opens full
  course details, and unselected courses that would clash with your current
  timetable carry a **⚠** marker; adding a clashing course warns immediately
  (never blocks). Notifications pause their auto-dismiss while hovered or
  focused, so there's always time to read (and hit Undo).
- Credits: CMI states credits only exceptionally; unstated courses count as
  **4 credits** (marked "assumed") — unless the course is annotated with a
  shorter month span, in which case the assumption is **one credit per
  month** ("(Oct-Nov)" ⇒ 2 credits, "(Sep)" ⇒ 1; the tooltip explains).
  Stated credits are never second-guessed. My courses shows the total plus
  a per-value breakdown ("1 × 4 cr · 2 × 2 cr"). The editor lets you
  **overwrite credits** per course (totals, filters and the catalog follow
  suit), with a one-click "Use CMI's value".
- **Your own courses**: "Add your own course" (My courses, or the catalog —
  including straight from a failed search) creates anything CMI's pages
  don't list: seminars, reading groups, classes from other institutes.
  Name first, code auto-suggested, credits 0–20, any number of weekly
  meetings on official slots or fully custom times (evenings and weekends
  get their own grid columns/rows), with a live clash line while you type.
  They behave like real courses everywhere — clash detection, drag & drop,
  credit totals, .ics export — but their definition is yours: drags and
  edits change the course itself (no override bookkeeping), a violet
  **Added by you** badge marks them, they are listed as "Courses you added" among
  your changes, "Remove" parks them under "off the timetable" with the
  definition intact, and the full share link carries them whole to other
  browsers. Opening one gives you **Delete this course** right there (still
  undoable); if a later CMI sync introduces
  the same code, your version keeps winning and a note offers a one-click
  switch to CMI's.

### Developer mode (hidden endpoint)

Developer mode is not linked anywhere in the UI. Open it by navigating to
the **`#/developer`** endpoint directly, e.g.
`https://<host>/<repo>/#/developer` (or `http://127.0.0.1:8080/#/developer`
during development). It exposes the fetch log, parse reports, storage
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
load works offline") was **removed by explicit request**, and so, later, was
the same-origin data mirror that replaced it. Nothing about CMI's pages may
ship inside the app or be hosted alongside it. First load asks for a sync;
the two pages in `core/fixtures/` are test input and nothing else — they are
not copied into the build and no code path in the app can reach them.

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

# tests — parser tests 1–11 against the fixtures, the synthetic-site suite,
# merge decision table, .ics golden files, URL codecs
cargo test --workspace

# regenerate the .ics golden after an intentional format change
UPDATE_GOLDEN=1 cargo test -p cmi-timetable-core --test ics_tests

# end-to-end browser tests (Selenium + headless Chromium) — see e2e/README.md
python e2e/test_app.py
```

## Deploying

Everything runs on your machine. **This repo has no GitHub Actions
workflows at all** — nothing on GitHub's side builds, tests, or schedules
anything, so no CI job can fail, stall, or mail you about it.

Committing never deploys. Pushing does: a `pre-push` git hook runs
`deploy.sh` whenever `main` is pushed, so the code on GitHub and the live
site can't drift apart. One-time setup per clone:

```sh
git config core.hooksPath githooks   # activate the deploy-on-push hook
```

Push without deploying once: `CMITT_SKIP_DEPLOY=1 git push`.

```sh
./deploy.sh               # test + build + publish + verify it went live
./deploy.sh --push        # push your commits too (ship code + site)
./deploy.sh --skip-tests  # skip the test suite
./deploy.sh --republish   # re-trigger serving of what is already published
./deploy.sh --build-only  # rehearse a release: build + test, publish nothing
```

The script builds with Trunk inside a **temporary Docker container**
(`rust:1`; it falls back to a plain local build when Docker is absent),
runs the tests, and force-pushes the result as a **single orphan commit**
to the `gh-pages` branch. `main` never carries build artifacts and the
branch keeps no history, so the repository shows only source. It then
checks that the live URL really serves the new build and asks Pages to
rebuild if it doesn't (`--no-verify` skips the wait).

Configure Pages once: Settings → Pages → deploy from branch →
`gh-pages` / root (or `gh api -X PUT repos/<owner>/<repo>/pages -f
build_type=legacy -f "source[branch]=gh-pages"`). For a
**user/organization page** (`<user>.github.io` repo) run with
`PUBLIC_URL=/`, and adjust the `base` computation in
`app/public/404.html` (comment inside).

The one step still on GitHub's side is serving the branch — their managed
`pages-build-deployment`, which only copies static files. If they are
having an incident it can lag or fail; nothing is lost, because the
finished site is already on the branch, and `./deploy.sh --republish`
re-triggers serving without rebuilding.

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
