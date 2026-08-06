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
/e2e    test_app.py — 34 Selenium tests, self-seeding (see §5); shoot.py —
        design-review screenshots + print PDFs.
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
# e2e (34 tests; self-generates seed via core example, needs cargo on PATH)
cd e2e && DIST_DIR=../app/dist-e2e .venv/bin/python test_app.py
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

- Tests: 47 native + 34/34 e2e green. Print sheet (`@media print` block in
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
  0 unexpected console errors (scratchpad/verify_artifact.py).
