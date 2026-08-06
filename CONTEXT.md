# CONTEXT.md — living session context for LLM assistants

> **Maintenance rule (for the assistant):** update this file at the END of
> every user prompt round, before committing. Keep sections 1–6 *current
> state* (rewrite in place; no history), and APPEND one compact entry to
> section 7 (newest last). Optimize for a fresh LLM re-acquiring the project
> in one read: dense facts, exact paths, exact commands, no prose padding.

## 1. What this project is

100% client-side timetable planner for Chennai Mathematical Institute
students. Rust → WebAssembly (Leptos 0.8 CSR + Trunk), static-deployable to
GitHub Pages, hash routing (`#/` planner, `#/developer` hidden endpoint),
all state in localStorage (`cmitt.v1.*`). Parses CMI's two public pages
(`timetable.php`, `lecturehalls.php` — ASCII `<pre>` grids) behind a
fail-closed validation gate. **Ships zero timetable data**: first load shows
a welcome screen asking for one sync; the repo carries no bundled snapshot
and no committed mirror (fixtures exist only for tests/e2e seed).

## 2. Standing user rules (verbatim intent, do not violate)

- "Don't access anything outside this folder" (temp files live outside the
  repo and must never be committed).
- Package installs: pacman first, `cargo install` only on failure.
- Nothing about the CMI website may be hard-coded; process dynamically.
- Keep the dev server running in the background for manual testing.
- Ultracode is ON for this session (workflows allowed for substantive work).
- Write copy "in your own words" — plain, honest, student-facing English.

## 3. Layout & key files

```text
/core   parsers, model, validate (gate), merge (3-way), diff, ics, share,
        date; feature `html` = native scraper path (sync/tests/e2e seed).
        core/examples/snapshot_json.rs → fixtures → latest.json (e2e seed).
        PARSER_VERSION=2 in core/src/model.rs. Fixtures: core/fixtures/.
/app    Leptos UI. src/app.rs (boot/routing), state.rs (App handle, undo,
        filters), fetch.rs (tier chain direct→proxy→mirror, adopt/merge),
        ui.rs (header/tabs/facets/dialogs/chips), views.rs (5 tabs +
        welcome()), dnd.rs (pointer+keyboard drag), storage.rs, dev.rs,
        domx.rs; styles.css = whole design system (tokens, light+dark).
/sync   native mirror publisher (CI cron writes app/public/data/).
/e2e    test_app.py — 28 Selenium tests, self-seeding (see §5).
```

## 4. Invariants & hard-won gotchas (violating these re-breaks fixed bugs)

- **Build isolation:** `trunk serve` (bg task) races other builds via the
  shared target dir. ALL manual builds/tests use
  `CARGO_TARGET_DIR=~/.rust-target-e2e`, app builds to `--dist dist-e2e`.
- **Leptos reactivity trap:** a reactive `prop:checked`/`prop:value` closure
  run at build time subscribes the SURROUNDING dynamic-children closure →
  menu rebuilds each filter tick → focus/scroll loss. Pattern: NodeRef +
  isolated `Effect::new` poking the DOM node, `untrack` for the initial
  value, plain `prop:checked=initial`. Facet option lists must NEVER read
  the filters signal.
- Planner tab is memoized (`Memo<Tab>`); catalog `<For>` is keyed. Keep it.
- Empty snapshot (`courses.is_empty()`) ⇔ "never synced" (gate guarantees
  non-empty otherwise). `SourceTier::Bundled` is legacy: discard on load.
- First sync: `adopt()` canonicalizes verbatim URL codes, skips the
  "what changed" digest (`first_data`), background retry ignores the 12 h
  throttle while empty.
- Gate + parser are the ONLY content judges; `looks_like_cmi()` only picks
  error copy on proxy tiers AFTER a gate failure.
- Undo history entries = (selection, overrides, **filters**); search box
  coalesces by identical label via `act_filters(label, coalesce=true, …)`.
- Halls grid renders official bookings MINUS moved-away meetings PLUS
  "arrivals" (overridden/user-created meetings landing in a cell, matched
  via `hall_col_of`); re-drags reuse the override matched on its BASE.
- `--alarm` color is reserved EXCLUSIVELY for clashes.
- Toast auto-dismiss pauses while hovered/focused (`HOVERED_TOASTS`
  thread_local, deliberately NOT a signal).
- e2e Chrome flags: `--force-prefers-reduced-motion` (dialog animations) and
  `--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1` (no network).

## 5. Build & test commands (exact)

```sh
# native tests (47)
CARGO_TARGET_DIR=~/.rust-target-e2e cargo test --workspace
# app build for e2e (never plain dist while trunk serve runs)
cd app && CARGO_TARGET_DIR=~/.rust-target-e2e trunk build --release --dist dist-e2e
# e2e (28 tests; self-generates seed via core example, needs cargo on PATH)
cd e2e && DIST_DIR=../app/dist-e2e .venv/bin/python test_app.py
# screenshots + print PDFs for design review (writes e2e/shots/, gitignored)
cd e2e && .venv/bin/python shoot.py
```

Dev server: background task `trunk serve --release` at
`http://127.0.0.1:8080/` (auto-rebuilds ~30 s after source changes).
The e2e venv (`e2e/.venv`, selenium only) serves both scripts.
`UPDATE_GOLDEN=1 cargo test -p cmi-timetable-core --test ics_tests`
regenerates the .ics golden.

## 6. Current state

- Tests: 47 native + 28/28 e2e green. Print sheet (`@media print` block in
  styles.css + `.print-masthead`/`.print-legend` DOM in views.rs
  my_timetable) is a designed poster: accent-rule masthead, dark time band,
  branch-colored chips filling cells, colorized legend. Header carries a
  permanent "sync every few days" hint next to Sync now (own row ≤899px).
  Note: headless Chrome clamps launch `--window-size` width to 500 — use
  `set_window_size()` for true phone-width screenshots.
- Fixture facts used by tests: TOC Tue+Thu 09:10–10:25 slot 550 LH803,
  credits unstated→4; RDBM 2 credits; SVA unscheduled; MFD Wed/Fri 840 LH6;
  RFLR Mon/Wed 630 LH5; QCOM Tue/Thu 930 LH803; slots
  550/630/710/840/930/1020; 75 courses, 18 branches.
- `app/public/data/` holds only a README — the CI cron (sync.yml) is the
  only writer of mirror data.

## 7. Prompt log (append one entry per user round; newest last)

- **R1 (credits/overwrites):** credit overrides + unified "Your changes"
  list + facet dropdowns close each other. e2e t17–t18.
- **R2 (no hardcoding):** 18-agent audit; dynamic semester/halls parsing
  (PARSER_VERSION 2), extra weekly meetings per course, grid-derived
  defaults.
- **R3 (beauty pass):** design-system rewrite of styles.css, structured
  "What changed" dialog, poster-style print stylesheet, copy pass.
- **R4 (halls DnD + filter scroll + ✓):** halls drag & drop, dropdown
  focus/scroll root-cause fix (reactivity trap, §4), ✓+ring selected
  marker, hover-paused toasts. e2e t21–t24.
- **R5 (this round):** removed ALL shipped data (bundled snapshot + committed
  mirror) → welcome/first-sync flow (views::welcome, SourceTier::None,
  app.no-data grid fix); fixed halls DnD not re-rendering (arrivals, §4);
  filters into undo/redo (`act_filters`, coalesced search); Course facet,
  per-dropdown search, All/None; My data dialog rebuilt as sectioned cards;
  spacing fixes (mobile chip ellipsis, tighter phone header); "sync every
  few days" copy in welcome + My data; e2e reworked to self-seed from
  fixtures + network blackhole, t25–t28; CONTEXT.md introduced.
- **R6 (visible sync reminder + print beauty):** standing "CMI keeps editing
  the timetable — sync every few days" hint in the header next to Sync now
  (full-width row on phones); print stylesheet redesigned (masthead with
  accent rule + semester right, dark slate header band, chips fill cells in
  branch pastels with corner ✎, colored code chips in legend, hairline
  footnote); `domx::fmt_local_date` now hand-rolled "6 Aug 2026" (was
  locale-numeric) — also improves failure banners.
- **R6b (print clashes):** clashing chips share their cell side by side,
  carry an alarm-red border + ⚠ corner glyph, and a red `.print-clashes`
  strip under the grid lists every overlap; footnote copy matches.
- **R6c (dense-print fix):** user's 12-course selection clipped chip text
  and pushed the legend to page 2. Print cells now `flex-wrap: wrap` with
  chips `flex: 1 1 44%; min-width: 0` (2 side-by-side, 3+ wrap; halls
  ellipsize, never chopped); legend rebuilt as a two-column `.print-courses`
  item list (`.pc-item`, column-count: 2) replacing the table; vertical
  rhythm tightened (td 54px) so 12 courses fit ONE page. Stress PDFs in
  shoot.py: `print-12.pdf` (user's selection) and `print-clash.pdf`
  (TOC+ISS+NLP triple-booked cell, 6 clash lines).
