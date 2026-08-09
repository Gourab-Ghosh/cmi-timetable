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

- **LOCAL COMMITS ONLY (from R13 on): never `git push`, never deploy or
  touch GitHub Pages, unless the user explicitly says to in that prompt.**
  Deploys happen through the user's own `git push` (pre-push hook) or their
  explicit ask. Committing must never trigger a deploy.
- "Don't access anything outside this folder" (temp files live outside the
  repo and must never be committed).
- Package installs: pacman first, `cargo install` only on failure.
- Nothing about the CMI website may be hard-coded; process dynamically.
- Keep the dev server running in the background for manual testing.
- Ultracode is ON for this session (workflows allowed for substantive work).
- Write copy "in your own words" — plain, honest, student-facing English.
- **RESTORE DEAD WORK (R19).** Whenever anything fails to finish — a
  subagent/workflow agent killed by a session or rate limit, a background
  command that died, an interrupted step — and the user then says
  "continue" (or anything resuming the work), FIRST check for unfinished
  work and restore it, before starting anything new. Do not assume a
  partial result was complete. How to check workflow casualties:
  `subagents/workflows/<run>/journal.jsonl` under the session dir records
  one `started` and one `result` line per agent — agents with a `started`
  and no `result` died, and their prompts survive in
  `agent-<id>.jsonl`; recover the findings/task from there and finish
  them by hand (or re-run). A workflow's returned value counts only the
  agents that lived, so a "clean" report can be hiding losses. Report
  honestly what died and what you did about it.

## 3. Layout & key files

```text
/core   parsers, model, validate (gate), merge (3-way), diff, ics, share,
        date; feature `html` = native scraper path (sync/tests/e2e seed).
        core/examples/snapshot_json.rs → fixtures → latest.json (e2e seed).
        PARSER_VERSION=3 in core/src/model.rs. Fixtures: core/fixtures/.
/app    Leptos UI. src/app.rs (boot/routing), state.rs (App handle, undo,
        filters), fetch.rs (tier chain direct→proxy→mirror, adopt/merge),
        ui.rs (header/tabs/facets/dialogs/chips), views.rs (5 tabs +
        welcome()), dnd.rs (pointer+keyboard drag), storage.rs, dev.rs,
        domx.rs; styles.css = whole design system (tokens, light+dark).
/sync   native mirror publisher (CI cron writes app/public/data/).
/e2e    test_app.py — 43 Selenium tests, self-seeding (see §5); shoot.py —
        design-review screenshots + print PDFs.
/githooks  pre-push — builds+publishes via deploy.sh when main is pushed
        (activate per clone: `git config core.hooksPath githooks`; skip
        once: CMITT_SKIP_DEPLOY=1; deploy.sh sets CMITT_IN_DEPLOY=1 so its
        own pushes never recurse).
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
- Planner tab is memoized (`Memo<Tab>`); catalog `<For>` is keyed. Keep it —
  BUT keyed `<For>` children run once per key in a non-tracked scope, so any
  selection/override-derived value inside a catalog row must be a
  Memo/closure (chip()'s selected/clash/aria and catalog_row's times/temp
  are), or it silently freezes until a remount (R14). Dialogs: DialogHost's
  closure tracks whatever a dialog body reads at build → stateless dialogs
  (details, my-data, share, what-changed) rebuild live, which is what they
  want. **Every FORM dialog must read UNTRACKED in its builder** — custom
  course, edit-meeting, export — or a background sync landing (or an Undo
  toast click) rebuilds the form mid-edit and silently throws the typed
  input away. Watch the helpers: `course_by_code`, `effective_meetings`,
  `is_custom` all read signals tracked, so wrap them in `untrack(…)`
  (e2e t43, t45 pin this).
- `snapshot.with(…)`, never `snapshot.get()`, in per-chip/per-check paths:
  the Snapshot carries the gzipped raw pages, and `.get()` deep-clones it.
- Empty snapshot (`courses.is_empty()`) ⇔ "never synced" (gate guarantees
  non-empty otherwise). `SourceTier::Bundled` is legacy: discard on load.
- First sync: `adopt()` canonicalizes verbatim URL codes, skips the
  "what changed" digest (`first_data`), background retry ignores the 12 h
  throttle while empty.
- Gate + parser are the ONLY content judges; `looks_like_cmi()` only picks
  error copy on proxy tiers AFTER a gate failure.
- The gate is FAIL-CLOSED but its count floors are garbage detectors
  (≥3 grids, ≥10 courses, ≥3 halls/days), NOT semester-size estimates — a
  small term must pass; error pages parse to zeros and still fail. Never
  "fix" a parse problem by weakening the scale-free rules (legend ≥90%,
  per-grid substance, slot sanity).
- Parser drift-tolerance is deliberate and test-pinned (PARSER_VERSION 3):
  day labels via `Day::from_label` (variants/case/decoration; rejects
  ranges and glued words), grid KIND by which rows carry the data (never
  by day-name spelling), times accept dots/am-pm/"to" (bare hours 1–6 =
  afternoon), pipe-stripped pages are RECOVERABLE (space-aligned fallback,
  t11b proves byte-identical recovery) — so "mangled" in fail-closed tests
  must destroy the times, not just the pipes. Month notes are validated as
  month WORDS (`month_from_word`) so "(Haskell)"/"(Maroon)" can't match.
- Credits: stated > user-override precedence is unchanged, but the ASSUMED
  default is duration-aware (`assumed_credits`: 1 credit/month for
  sub-4-month spans, else 4). Anything displaying credits must go through
  `course_credits`/`effective_credits`, never hardcode "assumed at 4".
- Undo history entries = (selection, overrides, **filters**); search box
  coalesces by identical label via `act_filters(label, coalesce=true, …)`.
- Halls grid renders official bookings MINUS moved-away meetings PLUS
  "arrivals" (overridden/user-created meetings landing in a cell, matched
  via `hall_col_of`); re-drags reuse the override matched on its BASE.
  **The page shows CMI's allocation AND the user's own placements — never
  only CMI's** (R21). Three helpers carry that: `user_placements()` (every
  overridden meeting + every meeting of a SELECTED custom course — customs
  carry no overrides, so the old "snapshot courses that have an override"
  loop never saw them at all), `App::user_halls()` (places CMI doesn't list
  → their own `tr.own-hall` rows, badged "yours", after CMI's), and
  `App::hall_slot_grid()` (official slots + synthetic `.extra` columns for
  hall bookings and user placements at out-of-grid times — the halls table
  needs its own version because `display_slot_grid` only covers the
  SELECTION). The free-hall finder must go through `hall_booking_state()`
  and `user_placements()` too, or it will call a hall free that the grid
  above shows as occupied (and vice versa after a meeting moves away).
  `perform_drop` resolves a dropped cell against BOTH grids — a cell that
  lights up as a drop target must accept the drop. Bookings match their
  column on the START only (`b.slot.start_min`), never full `Slot` equality:
  join.rs warns that CMI's two pages can disagree about where a slot ends,
  and equality would empty the entire table when they do. A booking with no
  matching meeting in the course (join.rs keeps and warns about these) is a
  `BookingCell::Reference` — a plain, undraggable chip; fabricating a base
  for it turned one drag into a brand-new weekly meeting. A code the user
  owns is `Gone` here: their own definition draws itself through
  `user_placements`. A bare `TMP*` cell has NO codes (parse.rs) — the room is
  still taken, and its badge stays; the badge is dropped only when the cell's
  own courses have all moved away. Keyboard move mode walks days × times, a
  shape the Halls table (rooms down the side) doesn't have, so M there says
  so instead of starting an invisible move.
- **Only CMI's edits may be described as CMI's.** `fetch::adopt` takes an
  `Adoption`: a `Reparsed` snapshot is the SAME cached pages read by a newer
  parser, so every difference is the app's own doing — no "what changed"
  digest, no "CMI changed times you customised" dialog (whose default throws
  the user's override away), no "CMI now matches your change" toast. The
  merge still runs so override ids stay attached (R23).
- **Anything acting on the SELECTION resolves through
  `App::selected_course`** — your own course, else CMI's, else a
  `removed_stub`. A code in the selection is on the timetable, and a feature
  that silently skips it is lying by omission: the .ics export did exactly
  that to courses CMI had dropped, whose meetings survive as overrides and
  render everywhere else (R23).
- Halls day selection: `App::halls_view()` — a stored choice always wins
  (`prefs.halls_view`, written only by a real click, so it survives
  reloads); with none stored the tab opens on TODAY, or on every day when
  today isn't a teaching day. `HallsView::All` renders one table per day.
- Keyboard move mode addresses the COLUMN a chip renders in (`column_for`),
  on the grid the user is looking at (`active_slot_grid` switches on the
  tab) — a cursor holding a raw start time highlights no cell and jumps on
  the first arrow key.
- `?c=` is percent-encoded everywhere it is written (address bar, share
  links, the .ics link): a course of the user's own can be called anything,
  and `+`/`&`/`#`/`,` in a code would come back mangled.
- **A list of names is not a sentence.** Anything the app answers with a SET
  renders as a list you can scan, never as `join(", ")` inside a paragraph:
  the free-hall answer leads with the count (`.finder-count`) and lays the
  rooms out as `.hall-list` pills; clashes are one row per collision
  (`.clash-list`, code × code · when) on both My timetable and the details
  dialog. Prose is for explanation, not for data (R22).
- **Hall text is user input, so it is canonicalised on the way in and
  compared loosely on the way out.** `App::canonical_hall` (trim, and adopt
  CMI's spelling when it matches case-insensitively) runs on every save;
  `same_hall` (trim + `eq_ignore_ascii_case`) does every render-side match.
  Without both, " lecture hall 803 " sat in CMI's row for one comparison and
  spawned a separate, permanently empty "yours" row for another, and the chip
  disappeared entirely between them.
- `--alarm` color is reserved EXCLUSIVELY for clashes — and that includes
  VOLUME: `.btn.danger` is quiet (muted) at rest and turns alarm only on
  hover/focus; standing red exists solely in the My-data danger zone. One
  documented exception: `.diff-del` keeps red (universal diff convention,
  glyph-scale). Second accent `--accent2` (violet) + `--grad` carry the
  brand: header hairline, active nav, h2 kickers, primary buttons, toast
  edge, welcome hero, credit-summary total. Ambience lives on a fixed
  `body::before` (iOS ignores background-attachment). `.main` has
  min-width:0 and the app grid uses minmax(0,1fr) — WITHOUT these the
  720px grid table widens the whole page on phones instead of scrolling
  in its container. The mobile sync-hint stays VISIBLE (user requirement,
  R6) — reclaim header space by other means only. `.row` is a GLOBAL flex
  utility: it was once scoped `.card .row`, which silently left every other
  `.row` (details-dialog header, override lists, data rows) a BLOCK — chips
  stacked above their titles and inline `gap`/`align-items` did nothing;
  several call sites had patched around it with inline `display:flex`.
  `.grid-scroll` owns the gap under every grid (`.panel` has no margin-top,
  so without it the clashes/changes panels sit flush against the table).
  `.dialog .actions` is sticky (long forms) and `.dialog` sizes with `dvh`
  (phone keyboards don't shrink the layout viewport); because that bar
  floats over the content, `.dialog` also sets `scroll-padding-bottom` —
  without it the last control scrolls to a position UNDER the bar and
  can't be tapped. Below 560px, `.fieldrow` labels take their own line so
  every control starts at the same left edge (mixed wrapping read as
  ragged), while controls still flow in a row so paired time inputs stay
  side by side.
- **Never offer a choice through `<input list=…>` + `<datalist>`.** Browsers
  filter datalist suggestions against the text already in the box, and these
  boxes open pre-filled (a meeting's current hall) — so the list collapses to
  one entry, itself, and the control looks dead; several mobile browsers show
  no list at all (R20 bug report). Halls go through `hall_picker()` (ui.rs):
  a real `<select>` of `snapshot.halls` + "Hall to be announced" (empty) +
  "Other place…", which reveals a focused free-text box for rooms CMI never
  lists. It matches the change event against the hall list itself, never a
  sentinel string, so no hall name can be mistaken for the "Other" row. One
  helper, both call sites (edit-meeting dialog, every custom-course meeting
  row); e2e t44 pins it.
- Toast auto-dismiss pauses while hovered/focused (`HOVERED_TOASTS`
  thread_local, deliberately NOT a signal).
- **Custom (user-created) courses** reuse `Course` wholesale in a
  `CustomStore` (`cmitt.v1.custom`), so clashes/grids/credits/ics/share all
  work unchanged. Two rules carry the whole feature: (1) customs resolve
  BEFORE the snapshot everywhere (`App::course_by_code`, selected_courses,
  chip, details, export) — a later CMI sync that introduces the same code
  never replaces user data, it just lights up `custom_shadows_official`
  plus a one-click "Use CMI's version instead"; (2) a custom course NEVER
  carries overrides — its definition IS its schedule, so apply_override /
  select_and_override / add_meeting / remove_meeting / set_credit_override
  all branch to `edit_custom_meetings` (which re-sorts + re-derives
  status), and every writer purges overrides under a custom code
  (save/delete in state.rs, `purge_custom_overrides` on share import —
  a shared store is written wholesale and can aim at a code the recipient
  owns). Customs are undoable like everything else: `UndoEntry.customs` +
  `act_customs`. They never appear on CMI's pages (Catalog/Master/Halls).
  "Remove" parks them (`parked_customs`, still in the store, off the
  selection); only Delete destroys. Renames rewrite the selection entry
  and then dedupe it (a code CMI dropped can still hold a slot).
- The header's "Synced … ago" pill and its 48 h stale tint tick on their
  own: Header owns a `now: RwSignal<f64>` bumped by a 30 s
  `gloo_timers::callback::Interval` plus a `visibilitychange` listener
  (throttled background tabs catch up instantly on return). `domx::rel_time`
  takes `now` as a parameter so the text is reactive — never call it with a
  bare `now_ms()` from render code, that freezes the label until an
  unrelated re-render. Header mounts once (outside the route switch), so
  the forgotten handles are page-lifetime, not leaks-per-mount.
- e2e Chrome flags: `--force-prefers-reduced-motion` (dialog animations) and
  `--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1` (no network).

## 5. Build & test commands (exact)

```sh
# native tests (66; --features html for the fixture-driven parser tests)
CARGO_TARGET_DIR=~/.rust-target-e2e cargo test --workspace --features html
# app build for e2e (never plain dist while trunk serve runs)
cd app && CARGO_TARGET_DIR=~/.rust-target-e2e trunk build --release --dist dist-e2e
# e2e (49 tests; self-generates seed via core example, needs cargo on PATH)
cd e2e && DIST_DIR=../app/dist-e2e .venv/bin/python test_app.py
# ...or just a few, by name fragment
cd e2e && DIST_DIR=../app/dist-e2e .venv/bin/python test_app.py t44 t45
# screenshots + print PDFs for design review (writes e2e/shots/, gitignored)
cd e2e && .venv/bin/python shoot.py
# deploy the site (Docker build → force-push gh-pages; no Actions involved)
./deploy.sh            # or --skip-tests
```

Dev server: background task `trunk serve --release` at
`http://127.0.0.1:8080/` (auto-rebuilds ~30 s after source changes).
The e2e venv (`e2e/.venv`, selenium only) serves both scripts.
`UPDATE_GOLDEN=1 cargo test -p cmi-timetable-core --test ics_tests`
regenerates the .ics golden.

## 6. Current state

- Tests: 66 native + 49/49 e2e green. Meeting removals: `MeetingOverride.to`
  is `Option<Meeting>` (None = removed; legacy JSON/share payloads still
  load — present meeting ⇒ Some). Out-of-grid times: **all three tables grow
  synthetic `.extra` columns**, each from its own source, all built by the
  shared `push_extra_column`/`columns` pair in state.rs —
  `display_slot_grid()` (the selection), `master_slot_grid()` (every override
  destination, R22), `hall_slot_grid()` (hall bookings + placements with a
  hall). `column_for` prefers the tightest containing slot and still
  sublabels when the meeting's own time differs from its column.
  Removed meetings produce NO EffMeeting — halls view checks removals
  explicitly (hall_booking_chip) or the official chip would reappear. Print sheet (`@media print` block in
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
- `app/public/data/` mirror files are written ONLY by the sync binary
  (`./deploy.sh --sync`), never by hand.
- Published: `https://github.com/Gourab-Ghosh/cmi-timetable` (origin, ssh),
  live at `https://gourab-ghosh.github.io/cmi-timetable/`. Deploys are
  LOCAL-FIRST: `./deploy.sh` builds in a temporary Docker container (rust:1;
  falls back to a local build without Docker), runs tests, and force-pushes
  the site as a SINGLE orphan commit to `gh-pages` (no build files on main,
  no history on the branch). Pages source = branch `gh-pages` / root
  (`build_type=legacy`). Caches in `.build-cache/` (gitignored).
  **There are NO GitHub Actions workflows in this repo** (all four deleted at
  the user's request: nothing on GitHub may build/schedule/fail/mail). The
  data-mirror cron became `./deploy.sh --sync` (same binary, same gate, run
  locally; commits `app/public/data` as data, not build output). The ONLY
  GitHub-side step left is their managed `pages-build-deployment`, which
  copies the branch's static files — unavoidable for Pages.

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
- **R5 (no shipped data + fixes):** removed ALL shipped data (bundled snapshot + committed
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
- **R7 (privacy audit + pre-merge sweep):** removed the tracked
  `e2e/__pycache__/*.pyc` (bytecode embeds absolute local paths) and purged
  it from ALL git history (filter-branch; hashes changed); .gitignore covers
  pycache/venv/shots; shoot.py moved into the repo, machine-independent.
  Then a 14-scenario interactive sweep beyond the suite (share round-trip,
  ics export, merge-conflict UI, keyboard move, theme/density/mobile-day
  view, corrupt storage, custom-time validation, dev simulators, Esc chain,
  fits-filter, halls details). Two app fixes: corrupt-data banner no longer
  claims a "built-in timetable" exists, and `set_banner` never clobbers a
  sticky notice with a transient failure banner. Five scenarios promoted to
  the permanent suite (t29–t33: share-with-changes, merge-conflict flow
  incl. keep-mine rebase, keyboard move mode, corrupt-storage recovery,
  ics-honors-overrides); harness gained boot(selection/overrides/
  raw_snapshot), a mutable fake mirror, and a downloads dir. The conflict
  dialog defaulting to "Use CMI's" is by design.
- **R8 (publish):** created public repo `Gourab-Ghosh/cmi-timetable`, pushed
  main, enabled GitHub Pages with the Actions source; deploy.yml (already
  present) redeploys on every push. Added the live URL to README, fixed a
  stale "bundled snapshot" comment in sync.yml. Ran sync.yml once by hand:
  mirror committed (76 courses, Aug–Nov 2026) and served at
  `…/cmi-timetable/data/latest.json` (its Pages deploy job hit a transient
  GitHub "Service Unavailable" — `gh run rerun --failed` fixed it). Live
  first-run verified in a real browser: welcome → sync (proxy tier won) →
  data pill; `#/developer` reachable.
- **R9 (CI failure mail):** the failed runs were a confirmed GitHub Actions
  incident (githubstatus: Actions partial outage + Pages deployment lag) —
  "Service Unavailable" at Set up job, then "job not acquired by Runner";
  nothing in our workflows at fault. Added
  `.github/workflows/retry.yml`: on failure of deploy/sync, `gh run rerun
  --failed` ONCE (`run_attempt < 2` guard, so real breakage still fails and
  emails on attempt 2). Gotcha: deploy.yml's `concurrency: pages,
  cancel-in-progress` means a push while a deploy rerun is in flight cancels
  it — cancelled ≠ failure, so no retry fires for it.
- **R10 (mobile long-press DnD):** on phones a long-press fired the native
  context menu (~500 ms, after the 350 ms drag lift-off) → pointercancel
  killed the drag → the synthesized click toggled the chip and DESELECTED
  the course. Fix in dnd.rs: document-level contextmenu listener that
  preventDefault()s whenever `app.drag` is Some (desktop right-click
  unaffected — mouse right-button never creates drag state), and
  cancel_drag now sets the 250 ms click-suppression flag when the drag had
  started; plus `-webkit-touch-callout: none` on chips (iOS). e2e t34
  simulates the whole gesture with synthetic touch PointerEvents. An
  adversarial review workflow then confirmed 3 edge-case defects, all fixed:
  pen barrel-clicks created drag state (now `button != 0` returns for ALL
  pointer types), Esc-cancel with the button held >250 ms let the release
  click toggle the chip (CANCELLED_POINTER tombstone re-arms suppression at
  the matching pointerup), and pointercancel ignored pointer_id (unrelated
  palm/finger cancels killed the drag). Verified with REAL W3C touch
  pointer actions (Chrome's actual gesture recognizer): non-passive
  touchmove preventDefault stops the scroll takeover, drag lands, no
  deselect. Gotcha: ChromeDriver mobileEmulation misplaces synthesized
  touches (coordinate transform) — use plain-window touch actions instead.
- **R11 (local-first deploys):** GitHub's Actions+Pages major outage kept
  failing the workflow deploys (runner acquisition / action-download / HTTP
  timeouts — never our steps), so deploys no longer depend on GitHub-hosted
  runners at all: new `./deploy.sh` builds in a temporary Docker container
  (rust:1, caches in gitignored `.build-cache/`; local build fallback per
  the user's spec), tests, then force-pushes the site as a single orphan
  commit to `gh-pages`; Pages switched to `build_type=legacy` serving that
  branch. main carries zero build artifacts (user requirement). deploy.yml
  rewritten as manual-dispatch remote fallback (same build → gh-pages
  push); new ci.yml tests every push; sync.yml now copies fresh mirror data
  directly onto gh-pages (pure git, no rebuild) instead of workflow_call
  redeploy; retry.yml watches all three. Self-hosted runner was considered
  and rejected (security risk on a public repo; still depends on the Actions
  control plane). An adversarial review then found 8 real defects, all fixed:
  container git hit "dubious ownership" so build.rs stamped
  APP_GIT_COMMIT=unknown (`safe.directory /work`, plus build.rs now watches
  `.git/HEAD` + its ref file — without rerun-if-changed cargo reused a stale
  stamp); a failed build left root-owned files in the repo (chown via EXIT
  trap; rootless Docker detected and skipped); deploying behind origin/main
  silently rolled back the data mirror (now refused unless `--allow-stale`);
  sync's publish-data was gated on changed-vs-main so it could never heal
  that (now runs every cron, `concurrency: pages`); `git fetch` failure was
  read as "branch missing" (now `git ls-remote --exit-code`); trunk download
  was linux-gnu-only without `curl -f` (OS/arch mapped, cache key includes
  the target, reuses a PATH trunk); the throwaway staging repo ignored
  repo-local git config (publish now builds the orphan commit with
  `git commit-tree` in the real repo via a scratch index — working tree
  untouched); `git init -b` needed git ≥2.28 (gone with the above). Also
  `XDG_CACHE_HOME` into .build-cache so trunk stops re-downloading
  wasm-bindgen/wasm-opt each run. `./deploy.sh --push` = push + publish
  (a bare `git push` no longer updates the site — by design). Verified:
  three real deploys, commit stamp correct in the wasm, caches reused.
  Caveat to state honestly: branch-served Pages still runs GitHub's own
  managed `pages-build-deployment` (static copy only — no toolchain, no
  third-party actions), so during their outage the SITE can lag even though
  the build never fails; the artifact is already on gh-pages either way.
  That step DID fail during the outage (19 min, then errored), so deploy.sh
  now verifies publication: poll `pages/builds/latest`, curl the live URL for
  the built `*_bg.wasm` filename, and request a rebuild if absent (2 attempts,
  never fails the deploy — `--no-verify` skips). `--republish` re-points
  gh-pages at a fresh commit with the SAME tree (a push event is what
  triggers Pages, so it works even when the Pages API is unreachable) and
  re-verifies. Gotcha found in testing: bash regexes are POSIX ERE (no lazy
  quantifiers), so `([^/]+?)(\.git)?$` captured `repo.git` and every Pages
  API call 404'd — the slug now comes from `basename -s .git`. A third review
  round caught 4 more, all fixed: the poll broke on the PREVIOUS build's
  `built` record (right after a push "latest" is still the old build), so a
  healthy deploy could be declared unserved — the LIVE PAGE is now the only
  success signal (cache-busted query; API status used for messages only);
  `expect=$(git show … | grep -o …)` aborted `--republish` under `set -e`
  when index.html had no wasm (the `${expect:-<html>}` fallback was dead code
  AND would have falsely reported "live"); `--republish` ignored
  `--no-verify` (blocked ~6 min); `--help` truncated the header mid-sentence
  (now prints the whole comment block via awk).
- **R12 (no GitHub-side processes):** user aborted an in-progress switch to
  committing build output into `docs/` on main (uncommitted work reverted with
  `git checkout --`) and instead asked that nothing on GitHub be able to
  error, while build files stay out of the repo. Deleted ALL FOUR workflows
  (ci/deploy/retry/sync — recoverable from history if ever wanted) and folded
  the mirror cron into `deploy.sh --sync`. GitHub's own
  `pages-build-deployment` for the served branch cannot be removed (it is how
  branch Pages works) and was failing repeatedly during their major outage
  (15–19 min, then error), so publication is retried until the live URL
  serves the new build. Reminder for future rounds: a republish push CANCELS
  an in-flight Pages build, so never re-trigger while one is running; prefer
  `gh api -X POST repos/<slug>/pages/builds` (queues a build, no push, no
  cancel churn). Their failure reason is unambiguous — "The job was not
  acquired by Runner of type hosted even after multiple attempts" — so
  nothing about the artifact is at fault. The artifact itself was verified
  independently of GitHub by serving the exact `origin/gh-pages` tree at the
  same `/cmi-timetable/` subpath with DNS blackholed: boots, auto-syncs from
  its own mirror tier, real-touch long-press drag lands without deselecting,
  0 unexpected console errors (scratchpad/verify_artifact.py). GitHub's
  runners came back ~20:40 IST and the queued build published: the LIVE site
  now serves this build (index.html byte-identical to the local dist, all
  assets 200, data/latest.json 200, /nope → 404) and a browser pass on the
  real URL confirms welcome → sync (76 courses, proxy tier) → touch
  long-press drag lands without deselecting → Ctrl+Z reverts → hidden
  `#/developer` reachable → 0 unexpected console errors.
  Follow-up hardening (asked "can the next publish fail?"): the no-Docker
  fallback called `rustup` unconditionally, but this machine has NO rustup
  (Arch ships rust via pacman) — under `set -e` that aborted the whole
  fallback. It now uses rustup only when present and otherwise probes
  `rustc --print target-libdir --target wasm32-unknown-unknown`. New
  `--build-only` rehearses a release without publishing; both paths verified
  with it (local: 555f09fc…, docker: 778cad60…). Note each build has a unique
  wasm hash (APP_BUILD_TIME), which is what makes verify_published exact.
- **R13 (remove meetings + honest out-of-grid rendering + push-only
  deploys):** STANDING RULE ADDED to §2 — local commits only, push/deploy
  only on the user's explicit ask. Features: (1) every meeting row in the
  details dialog gets "Remove this meeting" (counterpart of Add a meeting):
  `MeetingOverride.to: Option<Meeting>` (None = removed), remove_meeting()
  folds into an existing override / deletes user-created ones, changes list
  says "removed CMI's …" with a Restore button, merge treats CMI-deleted as
  auto-agree and CMI-moved as a conflict ("Keep it removed" rebases). Legacy
  storage/share payloads keep loading; old apps opening NEW links fall back
  to `?c=`. (2) Meetings outside CMI's hours (e.g. 19:30) used to clamp into
  the last grid column (column_for's nearest-fallback); now
  display_slot_grid() adds tinted `.extra` columns with the real times (also
  fixes lunch-gap times). (3) githooks/pre-push runs deploy.sh on pushes of
  main only — commits never deploy (user requirement); recursion guarded by
  CMITT_IN_DEPLOY; skip with CMITT_SKIP_DEPLOY=1. e2e t35/t36; merge/share/
  legacy-compat native tests. An adversarial review then confirmed 5 more
  defects, all fixed: drops onto synthetic columns silently no-opped while
  highlighted (perform_drop/move_cursor now resolve via App::display_slot_grid
  — moved to state.rs; perform_drop returns bool so keyboard Enter can't
  announce a false "Dropped"); "Keep it removed" on a stale-base removal
  deleted the override and silently RESTORED the meeting (stale removals are
  now dropped in merge as inert — UNLESS their base still matches a current
  official meeting, which keeps suppressing it); master grid clamped custom
  times into the nearest column with no time shown (now sublabels the real
  time; the false "official meetings only" comment fixed); a fully-removed
  course was mislabeled "CMI hasn't put it on the timetable" (tray now
  requires officially-empty meetings; details dialog says "You've removed
  all of this course's meetings"). 52 native + 36/36 e2e after fixes. All
  committed LOCALLY, not pushed.
- **R14 (catalog updates live):** user report: clash marks in the Catalog
  only appeared after a refresh. Root cause: catalog rows live in a keyed
  `<For>` (key = the course's Debug repr) whose children run ONCE per key in
  a non-tracked scope — chip() froze `selected`/`clash`/aria-label at build
  time and catalog_row froze the meeting-times text + temp badge. Every
  other view (grids, halls, My courses, dialogs) builds chips inside
  reactive closures, which is why only the Catalog went stale. Fix: chip()
  holds `selected`/`clash`/`aria` as Memos (aria also dedupes clash partners
  now — two shared meetings used to read "clashes with ISS, ISS");
  catalog_row memoizes effective_meetings for its times text and temp badge.
  Covers every mutation path: Add/Remove buttons, master-grid toggles, drag
  or dialog time changes, meeting removals, and My data → Clear selection.
  Review extras fixed alongside: branch_chip titles are reactive (a sync can
  rename a branch without touching any course — retained rows kept the old
  tooltip); share_dialog + what_changed_dialog read state TRACKED so an
  undo while they're open rebuilds them (both stateless; export_dialog stays
  frozen deliberately — it has local form state and its download re-reads
  live state anyway); selected_courses() and chip() use snapshot.with()
  instead of .get() (the Snapshot carries gzipped raw pages — a full clone
  per clash check / per chip was real cost). e2e t37 (verified to FAIL on
  the pre-fix build). 52 native + 37/37 e2e. Committed locally, not pushed.
- **R15 (duration-aware credits + drift-proof parser + credit counts):**
  user: 2-month courses ("(Oct-Nov)", e.g. MATH in the fixture) must assume
  2 credits and 1-month 1 credit instead of the blanket 4; generalize every
  hardcoded structure assumption so parsing "never errors" and extracts as
  much as possible; show counts of 2-/4-credit selected courses under My
  courses. Implemented: `assumed_credits` = 1 credit/month for
  sub-4-month spans (stated credits and user overrides always win);
  `extract_name_notes` accepts single months and full names/`to`
  separators via `month_from_word` (exact word — "(Haskell)"/"(Maroon)"
  can never match; dangling "(Oct-)" rejected as incomplete, not 1 month);
  ics clamps single-month notes at both ends; My courses now reads "Total
  credits: 8 · 1 × 4 cr · 2 × 2 cr · …" with value-aware assumed notes;
  badges/dialog say "assumed from its Oct-Nov duration". Parser audit (18
  agents, 15 confirmed findings) implemented: Day::from_label +
  data-carrying-rows classification, dot/am-pm/"to"/bare-afternoon times
  (backwards ranges re-joined, "6:30-7:45" = evening), pipe-neutral
  separators, nudged slicing (label cut extends through straddling tokens
  — "Lecture Hall 803" keeps its number), pipe-less space-aligned fallback
  (t11b: fixture with EVERY pipe stripped parses byte-identically),
  garbage-detection gate floors (small term passes; NEW cross-page
  consistency rule fails a partially truncated timetable page — ≤25%
  ScheduledNoBranch — since no count floor catches that shape), semantic
  semester-label compare + dash normalization, hall matching by overlap,
  legend thresholds (decoration excluded; single-line legends need
  all-caps/digit codes so "Note: …" can't become a course),
  PARSER_VERSION 3 (cached raw HTML re-parses on load; mirror files
  regenerate on next --sync). Review pass fixed 10 more findings:
  map_sparse_row DELETED (moved aligned cells to wrong slots — nudged
  byte-slicing is correct), time-range inversion, "Mon, Wed" half-accept,
  prose guards in the pipe-less path, "1 credits" grammar, stale docs.
  Rejected as designed: overlap fallback re-matching one merged booking
  for two meetings (real double-slot bookings need it); "(May)" as a
  name-note false positive (context-free parser; badge self-explains;
  credit override available). 65 native + 38/38 e2e. Committed locally,
  NOT pushed (standing rule).
- **R16 (beauty + copy pass):** user: make it "extremely good, as
  beautiful and colorful as possible", texts professional, everything
  readable — "Total credits: 8 · 1 × 4 cr · 2 × 2 cr" called out as hard
  to read. Credits line → structured `.credit-summary` (big gradient
  total + "N courses at M credits" pills + full-sentence footnotes;
  t05/t17/t29/t38 assertions rewritten). Design system gained --accent2
  (violet) + --grad: see the §4 entry for every gradient moment, the
  quiet-danger rule and the mobile layout guards. Copy audit (27 agents,
  24 confirmed fixes, 3 contradictory pairs resolved by hand): "different
  from", "(tried N routes)", capitalized progress lines, "Synced" pill
  title, "Moved X to", "Copy link with custom changes", "This cannot be
  undone.", My-data lede restructured, keyboard-move announcements read
  "Tuesday, 09:10 to 10:25" aloud. Screenshot-driven design critique (19
  agents, 16 confirmed) fixed: alarm-red flood from danger buttons
  (major), mobile page blowout via min-width:auto (major), 200px sticky
  mobile header → static+packed (major, sync-hint KEPT per R6), fake
  per-cell thead "gradient" → honest flat tint, ragged row-action
  alignment (:first-of-type never matched — chips are buttons),
  invisible ghost-accent/ⓘ borders (→ color-mix ring + --ctl-ring
  tokens), dark-toast gradient edge, toast shrink-wrap, halls rowhead
  accent overload, compact touch targets, ambience on body::before.
  shoot.py fixed: welcome shots now served WITHOUT /data (same-origin
  mirror silently auto-populated before — 12/13/17 never showed the
  hero), compact shot targets the Master grid, new 07b light my-courses
  shot. 65 native + 38/38 e2e. Committed locally, NOT pushed.
- **R17 (live sync pill + hint copy):** user: the "Synced …" text only
  updated after a refresh — make it update on its own; and reword the
  header hint ("stay current" → suggested "stay updated", final wording
  my choice). Root cause: `rel_time` read the wall clock non-reactively,
  so nothing re-rendered as time passed. Fix: Header-owned
  `now: RwSignal` bumped by a 30 s `gloo_timers::callback::Interval` +
  a `visibilitychange` listener (instant catch-up after tab throttling);
  `rel_time(ms, now)` now takes the clock as a parameter; the 48 h
  `stale` class reads the same signal, so the tint flips live too.
  Copy: "sync every few days to stay up to date" across all three sites
  (header hint, My-data note, welcome note). New e2e t39 overrides
  `Date.now` (wasm-bindgen glue resolves it call-time) and dispatches a
  synthetic `visibilitychange`: pill → "7 min ago" → "2 days ago" +
  `.stale`, no reload. NOTE: e2e must run via `e2e/.venv/bin/python`
  (system python has no selenium). 65 native + 39/39 e2e. Committed
  locally, NOT pushed.
- **R18 (your own courses + spacing bugs):** user: "Add an option to add
  custom courses… be smart on how to design it… make sure it looks
  extremely good"; mid-round: no gap between the grid and the clash panel
  (and the same with the changes panel). Feature: `CustomStore`
  (`cmitt.v1.custom`) reusing `Course`, `Course::custom`, share field `x`,
  `Dialog::CustomCourse`, name-first form (auto-suggested code, credits
  0–4 + Other 0–20, repeating meeting rows with official slots or custom
  times, per-row live clash line, focus moves to new/next row), entry
  points in My courses (dashed tile + empty state), the catalog toolbar and
  its empty-search state ("Add “X” as your own course"). See the §4 entry
  for the two rules the design rests on. 3-lens UX panel (first-timer /
  power-user / consistency) reshaped it before coding: credits start at 0,
  code is derived from the name instead of demanded, Remove parks instead
  of deleting, the shadow note got an action, the hall field became a
  datalist input (also applied to the existing EditMeeting dialog — a bare
  `<select>` silently dropped free-text halls; **this was the wrong fix and
  R20 undid it**, see `hall_picker`), and the dialog got a sticky
  footer + dvh sizing for phones. SPACING BUGS (the user's report, then a
  sweep): `.grid-scroll` had no bottom margin and `.panel` has no
  margin-top → every panel under a grid sat flush; and `.row` was scoped
  `.card .row`, so `.row` everywhere else was a block (details-dialog chip
  stacked above the title — visible in the shipped app; several call sites
  had inline `display:flex` patches). 21-agent review (8 verifiers + both
  design critics died on session limits; those findings verified by hand):
  fixed a selection duplicate on rename, share-import overrides landing on
  custom codes (`purge_custom_overrides`), the collision banner being wiped
  by the load-time sync (now sticky), chip identity frozen in keyed
  catalog rows (now a Memo — t42 pins the live refresh), the
  keep_selected casing mismatch, and the credits editor citing a "CMI
  value" for a course CMI never had. 66 native + 42/42 e2e; shots 18–26 +
  24-mobile. Committed locally as 01d10f8, NOT pushed.
- **R18b (finishing the agents that died mid-review):** user asked whether
  the session-limit casualties' work was ever done. Audit of every
  workflow journal (started-vs-result per agent): R14/R15/R16 rounds were
  already consumed; the local-deploy review's survivors are all
  implemented in deploy.sh (staleness guard + `--allow-stale`, rootless
  chown trap, `set -euo` before `rustup target add`, per-OS/arch trunk
  cache, `commit-tree` publish — its CI findings are moot, there are no
  workflows); the privacy audit re-run by hand found no secrets, emails or
  machine paths in tracked files. Genuinely outstanding were 4 findings
  from this round's review and the whole design critique. Fixed: the
  custom-course dialog did TRACKED reads (`custom_course`,
  `custom_shadows_official`) inside DialogHost's closure, so a background
  sync or an Undo-toast click rebuilt the form and threw away everything
  typed — builder is now fully untracked, the shadow note lives in its own
  closure (t43 pins it); the two aria-live regions were re-created per
  change instead of updated (screen readers announce changes INSIDE a live
  region, not a new node); t41's "no override" assertion was vacuous (the
  form path structurally cannot create one) — it now moves the meeting
  through the per-meeting Edit dialog, i.e. `apply_override`'s custom
  branch, and checks the definition itself moved. Design critique done by
  hand from the shots: mobile `.fieldrow` wrapping was ragged (labels now
  take their own line under 560px), the sticky action bar could cover the
  last control and make it untappable (`scroll-padding-bottom`), it now
  casts a soft shadow so content reads as sliding under it, and the Name
  placeholder no longer overflows on phones. 66 native + 43/43 e2e.
  Committed locally, NOT pushed.
- **R19 (restore-dead-work rule + direct Delete):** user: "whenever you
  cannot complete a task for any reason and I say continue, always check
  if any task has died and restore it" (now §2, and mirrored in the
  assistant's cross-session memory so it fires before this file is read);
  plus "add the delete course button directly when I click on a custom
  added course, instead of clicking edit course first". The details
  dialog of one of your own courses now leads with a quiet-danger
  "Delete this course" (left of the row, spacer, then the rest), so
  deleting is one click from the course instead of a detour through the
  edit form — which keeps its own Delete. t41 now deletes through this
  path. 66 native + 43/43 e2e. Committed locally, NOT pushed.
- **R20 (the hall dropdown didn't work):** user: "the dropdown menu is not
  working when I try to edit lecture hall through edit button in meeting
  timings of a course", then "check for all such errors very carefully".
  Root cause was mine from R18: the hall `<select>` had become an
  `<input list="em-halls">` + `<datalist>`, and browsers filter datalist
  suggestions against the text already in the box — which opens pre-filled
  with the meeting's current hall, so the list collapsed to that one entry
  and the control looked dead (and shows nothing at all on several mobile
  browsers). Replaced by `hall_picker()`, one helper shared by the
  edit-meeting dialog and every custom-course meeting row: a real dropdown
  of CMI's halls, "Hall to be announced", and "Other place…" revealing a
  focused free-text box for rooms CMI never lists (see §4). The sweep for
  the same class of bug found two more: `edit_meeting_dialog` and
  `export_dialog` still read the snapshot TRACKED inside DialogHost's
  closure, so a sync landing mid-edit rebuilt the form and put the original
  day/time/hall (or the default export dates) back — both untracked now,
  `untrack(…)` around the tracked helpers in the title. e2e gained t44
  (dropdown lists every hall, opens on the meeting's own hall, switches it,
  "Other place…" focuses and stores a free-text place, same control in the
  create form) and t45 (edit form survives a sync); t40 now drives the new
  control; test_app.py takes name fragments on argv. 66 native + 45/45 e2e.
  Committed locally, NOT pushed.
- **R21 (your own halls and times on the Halls page):** user: a hall they
  typed ("1002") never appeared in the Halls section, then "same happens
  when I change time outside the current timetable time — it should add a
  new row like My timetable. Check these things for all the tables", plus
  "check for all such possible bugs, probably with multiple agents".
  Root causes were two: hall rows came only from `snapshot.halls`, and the
  arrivals loop iterated `snapshot.courses` that HAVE an override — so a
  custom course (which never has one) was invisible on that page entirely,
  official hall or not. Fixed via `user_halls()`, `hall_slot_grid()` and
  `user_placements()` (see §4); the hall chooser now offers places you
  invented under "Your own places", the Hall facet lists them (read
  UNTRACKED — an option list that subscribed to overrides would rebuild
  under the cursor), and the free-hall finder shares the grid's own
  `hall_booking_state` so the two can't contradict each other. The master
  grid deliberately keeps CMI's columns and sublabels the real time
  instead ("don't let the column lie", R13) — checked, not changed.
  Two read-only audit agents then swept the area; their real findings, all
  fixed: a bare `TMP*` booking (no codes) was reported as a free hall; a
  custom course shadowing a CMI code rendered twice and dragging the CMI
  chip silently appended a meeting to the user's own course; overrides on a
  course CMI had dropped drew an empty row, column and day tab but no chip;
  hall text differing only in case or spacing made the chip vanish from the
  page (now `canonical_hall` + `same_hall`); bookings matched their column
  by full `Slot` equality; a booking with no matching meeting handed drags a
  fabricated base; the "temporary booking" badge outlived its own chips; a
  halls chip never showed its real time when it differed from the column; a
  stored `halls_day` could name a day the strip no longer offers; the
  Unscheduled tray called the user's own course one of CMI's (now "No fixed
  slot yet", the name the toast already promised); searching the catalog for
  a course you already own offered to create it again and then rejected the
  duplicate code (now points at My courses); `grid_days` deep-cloned the
  snapshot on every call. Deliberately NOT done: keyboard move mode in the
  Halls tab needs a rooms axis it doesn't have — M there now says where
  moving works instead of starting an invisible move. e2e t46 (own place +
  own column + finder agreement) and t47 (the user's exact report);
  `boot()` takes `customs=`; shot 29. 66 native + 47/47 e2e. Committed
  locally, NOT pushed.
- **R22 (master grid columns + readable output):** user: "the out-of-timetable
  problem still stays in the master grid — it sets the course to the last
  column available. Apply the fixes already done in My timetable", then "make
  the free-hall finder output extremely beautiful and readable, search for all
  such unreadability". The master grid's clamp was a deliberate R13 choice
  (keep CMI's columns, sublabel the real time); the user overruled it, so it
  now grows synthetic `.extra` columns from `master_slot_grid()` (extras come
  from every override destination, since this grid draws CMI's whole catalog,
  not the selection). The three grid builders now share `push_extra_column` +
  `columns` in state.rs, and `perform_drop` resolves against all three.
  Readability: the free-hall answer became a count + `.hall-list` pills +
  a right-aligned when-line instead of one comma-separated sentence, and both
  clash lists (My timetable panel, details dialog) became one row per
  collision — see the §4 rule. e2e t48 pins the master-grid column; t15/t46/t47
  now read the finder's list rather than its old sentence; shots 30 and 31.
  66 native + 48/48 e2e. Committed locally, NOT pushed.
- **R23 (whole-app sweep + halls day picker + copy):** user asked for the
  master-grid fix (R22) plus "search for all such bugs in the whole app",
  then for an all-days option in Halls defaulting to today, then for a
  professional rewrite of today's copy. The sweep ran as an ultracode
  Workflow: 5 finder lenses (grids, ownership, round-trip, controls,
  reactivity) → 30 findings → an adversarial refuter per finding → 18
  survived, deduping to 11 distinct bugs, all fixed. Highest value: the
  .ics export dropped selected courses CMI had dropped (five lenses found
  it independently) → `App::selected_course`; a PARSER_VERSION bump
  re-parsed the cached pages and ran them through the CMI-vs-CMI merge, so
  the app's own parser change was announced as CMI's edit and the conflict
  dialog offered to delete the user's overrides → `Adoption::Reparsed`.
  Then: catalog rows printed the user's overridden times as CMI's listing
  (now an "✎ your times" badge); the details dialog said CMI lists a course
  the user invented; the Time-slot facet offered only CMI's slots; `?c=`
  was written unencoded; the phone's per-day list had draggable chips and
  no drop target; validation errors were inserted outside any live region;
  the credits editor kept form state inside the rebuilt details dialog
  (hoisted to `App::credit_edit`); the Hall facet compared halls exactly;
  keyboard move addressed raw start times on the wrong grid. Halls gained
  an "All" day button and `HallsView` (see §4). Copy: today's new strings
  rewritten (halls lede, "your own" hall badge, finder note, unscheduled
  tray, catalog empty state, keyboard-move message). e2e t49; shot 32.
  66 native + 49/49 e2e. Committed locally, NOT pushed.
