//! Developer mode (`#/developer[/<category>]`). No auth — all data here is
//! public.
//!
//! Until R87 this was one flat page reached only by typing its URL. It is
//! now a real MODE: the tab rail swaps to its categories (Overview, Tweaks,
//! Sync, Storage — see `DevTab`), the way in is My data → "Under the hood",
//! and the ways out are the rail's ← Back, the toolbar button, Escape and
//! browser Back. The six original panels kept their names — seven e2e tests
//! scrape them by heading — and only their shelves changed: Overview holds
//! Build info; Sync holds Simulators, the Fetch log and Parse reports (the
//! levers beside the instruments they feed); Storage holds the Storage
//! inspector and the Raw HTML viewer (both about stored bytes). Tweaks is
//! new: every small choice about how the timetable looks, searchable with
//! the same three switches every search box in the app has.

use crate::state::{App, BannerKind, Density, DevTab, Prefs, ThemePref};
use crate::{domx, fetch, storage};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

pub fn developer(app: App, tab: DevTab) -> impl IntoView {
    view! {
        // The R84 landmark pattern, same as the planner's: `role="tabpanel"`
        // on a WRAPPER so the section inside stays a region landmark — an
        // element has one role. `tabindex="-1"` so the entry point can land
        // focus here (the control that was pressed unmounts with the mode
        // switch, and focus must go somewhere deliberate).
        <div role="tabpanel" id=tab.panel_id() aria-label=tab.label() tabindex="-1">
            <section aria-label="Developer mode">
                <div class="toolbar noprint">
                    <h2 style="margin:0">"Developer mode"</h2>
                    <div class="grow"></div>
                    <button
                        class="btn"
                        on:click=move |_| {
                            app.goto_planner();
                            // This button unmounts with the mode; focus
                            // follows to the planner rail (or the header's
                            // My data button before the first sync, when
                            // there is no rail at all).
                            domx::focus_soon(&[
                                "nav.tabs button.tab[tabindex='0']",
                                "[data-mydata]",
                            ]);
                        }
                    >
                        "← Back to the planner"
                    </button>
                </div>
                {match tab {
                    DevTab::Overview => overview_page(app).into_any(),
                    DevTab::Tweaks => tweaks_page(app).into_any(),
                    DevTab::Sync => sync_page(app).into_any(),
                    DevTab::Storage => storage_page(app).into_any(),
                }}
            </section>
        </div>
    }
}

/// Overview: which build, which snapshot, what this browser holds. The
/// landing category — bare `#/developer` opens here, so the URL that has
/// been typed and tested since the beginning still shows Build info and its
/// `[data-update-check]` button (t114 boots it verbatim).
fn overview_page(app: App) -> impl IntoView {
    view! {
        {build_info(app)}
        {this_browser(app)}
    }
}

/// Sync: the levers first, then the instruments they feed — press "Run
/// sync" or "Simulate parse failure" and the Fetch log and Parse reports
/// under it show what happened, without the scroll across unrelated panels
/// the old flat page demanded.
fn sync_page(app: App) -> impl IntoView {
    view! {
        {simulators(app)}
        {fetch_log(app)}
        {parse_reports(app)}
    }
}

/// Storage: what this browser holds, byte by byte — every `cmitt.*` key,
/// and the two CMI pages stored inside the biggest of them.
fn storage_page(app: App) -> impl IntoView {
    view! {
        {storage_inspector(app)}
        {raw_html_viewer(app)}
    }
}

fn build_info(app: App) -> impl IntoView {
    view! {
        <div class="panel">
            <h3>"Build info"</h3>
            <dl class="kv mono small">
                <dt>"App version"</dt>
                <dd>{crate::state::APP_VERSION}</dd>
                <dt>"Parser version"</dt>
                <dd>{ttcore::PARSER_VERSION.to_string()}</dd>
                <dt>"Git commit"</dt>
                <dd>{crate::state::GIT_COMMIT}</dd>
                <dt>"Built at"</dt>
                <dd>{crate::state::BUILD_TIME}</dd>
                <dt>"Current snapshot"</dt>
                <dd>
                    {move || {
                        app.snapshot
                            .with(|s| {
                                if !s.has_data() {
                                    "none — nothing synced yet".to_string()
                                } else {
                                    format!(
                                        "{} · fetched {} · parser v{} · {}",
                                        s.semester_label,
                                        domx::fmt_local(s.fetched_at),
                                        s.parser_version,
                                        s.source.label(),
                                    )
                                }
                            })
                    }}
                </dd>
                <dt>"This build's id"</dt>
                <dd>{crate::update::own_id_for_display()}</dd>
            </dl>
            // The daily self-update check, on demand. The app asks the server
            // it was loaded from which build it is serving and, if it differs,
            // ASKS the reader — nothing installs without a press. My data has
            // the same button for everyone; this one is here because the panel
            // beside it shows the build id being compared.
            <div class="row" style="display:flex;gap:0.5rem;flex-wrap:wrap;margin-top:0.6rem">
                <button
                    class="btn small"
                    data-update-check
                    title="Ask the server whether a newer build of this app is published"
                    on:click=move |_| crate::update::check_now(app)
                >
                    "Check for an update now"
                </button>
            </div>
        </div>
    }
}

fn simulators(app: App) -> impl IntoView {
    view! {
        <div class="panel">
            <h3>"Simulators"</h3>
            <div class="row" style="display:flex;gap:0.5rem;flex-wrap:wrap;align-items:center">
                <label for="force-tier" class="muted small">"Force tier on next sync"</label>
                <select
                    id="force-tier"
                    on:change=move |ev| {
                        let v = event_target_value(&ev);
                        app.force_tier.set((!v.is_empty()).then_some(v));
                    }
                >
                    <option value="">"(all tiers: relays, then CMI itself)"</option>
                    <option value="proxy">"relays only"</option>
                    <option value="direct">"CMI itself only (may prompt for local network)"</option>
                </select>
                <button class="btn small" on:click=move |_| {
                    leptos::task::spawn_local(async move {
                        fetch::run_update(app, true).await;
                    });
                }>
                    "Run sync"
                </button>
                <button class="btn small" on:click=move |_| fetch::simulate_parse_failure(app)>
                    "Simulate parse failure"
                </button>
            </div>
            <p class="muted small">
                "“Simulate parse failure” runs mangled pages through the full pipeline to \
                 demonstrate that the validation gate keeps the cached snapshot \
                 untouched."
            </p>
        </div>
    }
}

fn fetch_log(app: App) -> impl IntoView {
    view! {
        <div class="panel">
            <h3>"Fetch log"</h3>
            {move || {
                let log = app.fetch_log.get();
                if log.is_empty() {
                    view! { <p class="muted small">"No fetches yet this session."</p> }.into_any()
                } else {
                    view! {
                        <div style="overflow:auto">
                            <table class="devlog">
                                <thead>
                                    <tr>
                                        <th>"time"</th>
                                        <th>"tier"</th>
                                        <th>"url"</th>
                                        <th>"status"</th>
                                        <th>"ms"</th>
                                        <th>"bytes"</th>
                                        <th>"error"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {log.iter()
                                        .rev()
                                        .map(|e| {
                                            view! {
                                                <tr>
                                                    <td>{domx::fmt_local(e.at)}</td>
                                                    <td>{e.tier.clone()}</td>
                                                    <td style="max-width:26rem;overflow-wrap:anywhere">
                                                        {e.url.clone()}
                                                    </td>
                                                    <td class=if e.error.is_none() { "ok" } else { "fail" }>
                                                        {e.status.map(|s| s.to_string()).unwrap_or_else(|| "—".to_string())}
                                                    </td>
                                                    <td>{format!("{:.0}", e.duration_ms)}</td>
                                                    <td>{e.bytes.to_string()}</td>
                                                    <td class="fail">{e.error.clone().unwrap_or_default()}</td>
                                                </tr>
                                            }
                                        })
                                        .collect_view()}
                                </tbody>
                            </table>
                        </div>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

fn parse_reports(app: App) -> impl IntoView {
    view! {
        <div class="panel">
            <h3>"Parse reports"</h3>
            {move || {
                let reports = app.reports.get();
                if reports.is_empty() {
                    view! {
                        <p class="muted small">
                            "No parses yet this session — run a sync to see one."
                        </p>
                    }
                        .into_any()
                } else {
                    reports
                        .iter()
                        .rev()
                        .map(|r| {
                            let stats = &r.report.stats;
                            view! {
                                <details style="margin-bottom:0.5rem">
                                    <summary>
                                        <span class="mono">{r.source.clone()}</span>
                                        {format!(
                                            " · {} · {} · {} branch grids, {} courses, {} halls, {} warnings, {} errors",
                                            domx::fmt_local(r.at),
                                            if r.report.gate_passed() { "gate PASSED" } else { "gate FAILED" },
                                            stats.branch_grids,
                                            stats.unique_courses,
                                            stats.halls,
                                            r.report.warnings.len(),
                                            r.report.errors.len(),
                                        )}
                                    </summary>
                                    <table class="devlog">
                                        <thead>
                                            <tr>
                                                <th>"gate rule"</th>
                                                <th>"outcome"</th>
                                                <th>"detail"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {r.report
                                                .gate
                                                .iter()
                                                .map(|g| {
                                                    view! {
                                                        <tr>
                                                            <td>{g.rule.clone()}</td>
                                                            <td class=if g.passed { "ok" } else { "fail" }>
                                                                {if g.passed { "pass" } else { "FAIL" }}
                                                            </td>
                                                            <td>{g.detail.clone()}</td>
                                                        </tr>
                                                    }
                                                })
                                                .collect_view()}
                                        </tbody>
                                    </table>
                                    {(!r.report.warnings.is_empty())
                                        .then(|| {
                                            view! {
                                                <details>
                                                    <summary>{format!("{} warnings", r.report.warnings.len())}</summary>
                                                    <ul class="small">
                                                        {r.report
                                                            .warnings
                                                            .iter()
                                                            .map(|w| view! { <li>{w.clone()}</li> })
                                                            .collect_view()}
                                                    </ul>
                                                </details>
                                            }
                                        })}
                                    {(!r.report.errors.is_empty())
                                        .then(|| {
                                            view! {
                                                <ul class="small">
                                                    {r.report
                                                        .errors
                                                        .iter()
                                                        .map(|e| {
                                                            view! { <li class="fail">{e.clone()}</li> }
                                                        })
                                                        .collect_view()}
                                                </ul>
                                            }
                                        })}
                                    {(!r.report.branch_stats.is_empty())
                                        .then(|| {
                                            view! {
                                                <details>
                                                    <summary>
                                                        {format!(
                                                            "{} branch grids (per-branch stats)",
                                                            r.report.branch_stats.len(),
                                                        )}
                                                    </summary>
                                                    <table class="devlog">
                                                        <thead>
                                                            <tr>
                                                                <th>"branch"</th>
                                                                <th>"title"</th>
                                                                <th>"day rows"</th>
                                                                <th>"slots"</th>
                                                                <th>"cells"</th>
                                                                <th>"legend"</th>
                                                            </tr>
                                                        </thead>
                                                        <tbody>
                                                            {r.report
                                                                .branch_stats
                                                                .iter()
                                                                .map(|b| {
                                                                    view! {
                                                                        <tr>
                                                                            <td>{b.code.clone()}</td>
                                                                            <td>{b.title.clone()}</td>
                                                                            <td>{b.day_rows.to_string()}</td>
                                                                            <td>{b.slots.to_string()}</td>
                                                                            <td>{b.occurrences.to_string()}</td>
                                                                            <td>{b.legend_entries.to_string()}</td>
                                                                        </tr>
                                                                    }
                                                                })
                                                                .collect_view()}
                                                        </tbody>
                                                    </table>
                                                </details>
                                            }
                                        })}
                                    <details>
                                        <summary>
                                            {format!(
                                                "{} <pre> blocks classified",
                                                r.report.classifications.len(),
                                            )}
                                        </summary>
                                        <table class="devlog">
                                            <thead>
                                                <tr>
                                                    <th>"page"</th>
                                                    <th>"#"</th>
                                                    <th>"kind"</th>
                                                    <th>"lines"</th>
                                                    <th>"first line"</th>
                                                </tr>
                                            </thead>
                                            <tbody>
                                                {r.report
                                                    .classifications
                                                    .iter()
                                                    .map(|c| {
                                                        view! {
                                                            <tr>
                                                                <td>{c.page.clone()}</td>
                                                                <td>{c.index.to_string()}</td>
                                                                <td>{c.kind.clone()}</td>
                                                                <td>{c.line_count.to_string()}</td>
                                                                <td>{c.first_line.clone()}</td>
                                                            </tr>
                                                        }
                                                    })
                                                    .collect_view()}
                                            </tbody>
                                        </table>
                                    </details>
                                </details>
                            }
                        })
                        .collect_view()
                        .into_any()
                }
            }}
        </div>
    }
}

fn pretty_json(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .and_then(|v| serde_json::to_string_pretty(&v))
        .unwrap_or_else(|_| raw.to_string())
}

/// Every `cmitt.*` key in localStorage, with Copy / Export / Import / Clear.
///
/// Called the "cache inspector" until now, which was wrong in the one place
/// it matters: this panel lists `cmitt.v1.custom`, `…selection`,
/// `…overrides` and `…prefs` alongside the snapshot, and offers to Clear any
/// of them. Only the snapshot is a cache — it can be fetched from CMI again.
/// The rest is the user's own work and exists nowhere else, so a heading
/// calling the lot "cache" invited exactly the deletion the app spends the
/// rest of its code preventing.
fn storage_inspector(app: App) -> impl IntoView {
    let bump = RwSignal::new(0u32);
    view! {
        <div class="panel">
            <h3>"Storage inspector"</h3>
            <p class="muted small">
                "Everything the app keeps in your browser. Only cmitt.v1.snapshot is a \
                 cache — CMI can be asked for it again; the rest is your own work and \
                 exists nowhere else. Import replaces a key and reloads."
            </p>
            {move || {
                bump.get();
                let entries = storage::all_entries();
                if entries.is_empty() {
                    return view! { <p class="muted small">"No cmitt.* keys found."</p> }.into_any();
                }
                entries
                    .into_iter()
                    .map(|(key, value)| {
                        let size = value.len();
                        let copy_value = value.clone();
                        let export_key = key.clone();
                        let export_value = value.clone();
                        let clear_key = key.clone();
                        let import_key = key.clone();
                        let input_id = format!("import-{key}");
                        let input_id_for = input_id.clone();
                        view! {
                            <details style="margin-bottom:0.5rem">
                                <summary>
                                    <span class="mono">{key}</span>
                                    <span class="muted small">{format!(" · {size} bytes")}</span>
                                </summary>
                                <div class="row" style="display:flex;gap:0.4rem;flex-wrap:wrap;margin:0.4rem 0">
                                    <button
                                        class="btn small"
                                        on:click=move |_| {
                                            domx::copy_to_clipboard(copy_value.clone(), |_| {});
                                            app.toast("Copied.");
                                        }
                                    >
                                        "Copy"
                                    </button>
                                    <button
                                        class="btn small"
                                        on:click=move |_| {
                                            domx::download_text(
                                                &format!("{export_key}.json"),
                                                "application/json",
                                                &export_value,
                                            );
                                        }
                                    >
                                        "Export to file"
                                    </button>
                                    <label class="btn small" for=input_id_for>
                                        "Import from file"
                                    </label>
                                    <input
                                        type="file"
                                        id=input_id
                                        accept="application/json"
                                        style="display:none"
                                        on:change=move |ev| {
                                            let key = import_key.clone();
                                            let Some(input) = ev
                                                .target()
                                                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                            else {
                                                return;
                                            };
                                            let Some(file) = input.files().and_then(|f| f.item(0)) else {
                                                return;
                                            };
                                            leptos::task::spawn_local(async move {
                                                if let Ok(text) = wasm_bindgen_futures::JsFuture::from(file.text())
                                                    .await
                                                    && let Some(text) = text.as_string()
                                                {
                                                    let _ = storage::set_raw(&key, &text);
                                                    // Without the query, for
                                                    // the same reason Clear
                                                    // needs it: an imported
                                                    // `cmitt.v1.selection` is
                                                    // otherwise overwritten
                                                    // by the `?c=` still in
                                                    // the address bar (R83).
                                                    domx::reload_without_query();
                                                }
                                            });
                                        }
                                    />
                                    <button
                                        class="btn small danger"
                                        on:click=move |_| {
                                            app.ask(crate::state::ConfirmAsk {
                                                title: "Clear this key?".into(),
                                                lede: format!(
                                                    "Everything stored under {clear_key} is \
                                                     removed and the page reloads.",
                                                ),
                                                points: vec![
                                                    "No backup is kept.".into(),
                                                ],
                                                confirm_label: "Clear it".into(),
                                                danger: true,
                                                irreversible: true,
                                                action: crate::state::ConfirmAction::ClearStorageKey(
                                                    clear_key.clone(),
                                                ),
                                            });
                                        }
                                    >
                                        "Clear"
                                    </button>
                                </div>
                                <pre class="devpre">{pretty_json(&value)}</pre>
                            </details>
                        }
                    })
                    .collect_view()
                    .into_any()
            }}
        </div>
    }
}

fn raw_html_viewer(app: App) -> impl IntoView {
    view! {
        <div class="panel">
            <h3>"Raw HTML viewer"</h3>
            {move || {
                let snapshot = app.snapshot.get();
                match &snapshot.raw_html_gz {
                    None => view! {
                        <p class="muted small">
                            "The current snapshot has no stored raw pages (they may have been \
                             dropped to fit the storage quota)."
                        </p>
                    }
                        .into_any(),
                    Some(raw) => {
                        let tt = ttcore::rawhtml::decompress_from_b64(&raw.timetable_b64)
                            .unwrap_or_else(|| "(could not decompress)".to_string());
                        let halls = ttcore::rawhtml::decompress_from_b64(&raw.lecturehalls_b64)
                            .unwrap_or_else(|| "(could not decompress)".to_string());
                        view! {
                            <div class="row" style="margin-bottom:0.5rem">
                                <button class="btn small" on:click=move |_| fetch::reparse_stored(app, true)>
                                    "Re-parse now"
                                </button>
                            </div>
                            <details>
                                <summary>"timetable.php"</summary>
                                <pre class="devpre">{tt}</pre>
                            </details>
                            <details>
                                <summary>"lecturehalls.php"</summary>
                                <pre class="devpre">{halls}</pre>
                            </details>
                        }
                            .into_any()
                    }
                }
            }}
            <p class="muted small">
                {move || {
                    let _ = app.banner.get();
                    format!(
                        "Shipped parser is v{}; the snapshot was parsed with v{}.",
                        ttcore::PARSER_VERSION,
                        app.snapshot.with(|s| s.parser_version),
                    )
                }}
            </p>
        </div>
    }
}

/// Surfacing storage problems found at startup (called from init).
pub fn corrupt_data_banner(app: App) {
    // Sticky: must survive the background update that runs right after
    // startup. The backup key is in the console log, not here — a student
    // can't act on a storage key, but "check your courses" they can do.
    app.set_banner_sticky(
        BannerKind::Warn,
        "Some of what the app had saved in your browser couldn't be read, so \
         those parts were reset to defaults. Nothing was deleted — the unreadable \
         copy is still in your browser, but the app can't restore anything from \
         it. Check your courses and settings, and put back anything that's \
         missing.",
    );
}

/// "This browser": the storage total, the cross-tab story, and a
/// diagnostics block a bug report can carry whole.
fn this_browser(app: App) -> impl IntoView {
    // Built at mount, like the storage inspector's list: this page is
    // reopened far more often than storage changes underneath it.
    let entries = storage::all_entries();
    let keys = entries.len();
    let bytes: usize = entries.iter().map(|(k, v)| k.len() + v.len()).sum();
    view! {
        <div class="panel">
            <h3>"This browser"</h3>
            <dl class="kv mono small">
                <dt>"Stored here"</dt>
                <dd>{format!("{keys} keys · {} KB", bytes.div_ceil(1024))}</dd>
            </dl>
            <p class="muted small">
                "Other tabs share this storage. A change made in one tab lands in \
                 the others as soon as they are idle — mid-edit, a note asks the \
                 busy tab to finish first."
            </p>
            <div class="row" style="display:flex;gap:0.5rem;flex-wrap:wrap;margin-top:0.6rem">
                <button
                    class="btn small"
                    title="Everything a bug report needs — versions, sync attempts, \
                           sizes. No course data."
                    on:click=move |_| {
                        domx::copy_to_clipboard(diagnostics_text(app), |_| {});
                        app.toast("Copied.");
                    }
                >
                    "Copy diagnostics"
                </button>
            </div>
        </div>
    }
}

/// The diagnostics block, as plain text. Versions, the snapshot line, sizes
/// and the last few fetches — and deliberately NO course data: a bug report
/// gets pasted into chats and issue trackers, and someone's timetable is
/// nobody's business.
fn diagnostics_text(app: App) -> String {
    let snapshot = app.snapshot.with_untracked(|s| {
        if s.has_data() {
            format!(
                "{} · fetched {} · parser v{} · {}",
                s.semester_label,
                domx::fmt_local(s.fetched_at),
                s.parser_version,
                s.source.label(),
            )
        } else {
            "none — nothing synced yet".to_string()
        }
    });
    let entries = storage::all_entries();
    let bytes: usize = entries.iter().map(|(k, v)| k.len() + v.len()).sum();
    let tweaks = app.prefs.with_untracked(|p| {
        let mut t: Vec<&str> = Vec::new();
        if p.marks_clash_off {
            t.push("clash marks hidden");
        }
        if p.marks_edits_off {
            t.push("✎ marks hidden");
        }
        if p.marks_ticks_off {
            t.push("✓ ticks hidden");
        }
        if p.today_highlight_off {
            t.push("today unhighlighted");
        }
        if p.quiet_dim_off {
            t.push("quiet days undimmed");
        }
        if p.chip_halls_off {
            t.push("hall names off chips");
        }
        if p.chips_plain {
            t.push("plain chips");
        }
        if p.reduce_motion {
            t.push("reduced motion");
        }
        if p.theme != ThemePref::Auto {
            t.push("theme chosen");
        }
        if p.density.is_some() {
            t.push("row height chosen");
        }
        if t.is_empty() {
            "none".to_string()
        } else {
            t.join(", ")
        }
    });
    let mut out = format!(
        "CMI Timetable Planner diagnostics\n\
         app {} · parser v{} · commit {} · built {}\n\
         build id: {}\n\
         snapshot: {snapshot}\n\
         stored: {} keys · {} KB\n\
         tweaks off defaults: {tweaks}\n",
        crate::state::APP_VERSION,
        ttcore::PARSER_VERSION,
        crate::state::GIT_COMMIT,
        crate::state::BUILD_TIME,
        crate::update::own_id_for_display(),
        entries.len(),
        bytes.div_ceil(1024),
    );
    app.fetch_log.with_untracked(|log| {
        for e in log.iter().rev().take(5) {
            out.push_str(&format!(
                "fetch: {} · {} · {} · {:.0}ms · {}B{}\n",
                e.tier,
                e.url,
                e.status
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "—".to_string()),
                e.duration_ms,
                e.bytes,
                e.error
                    .as_deref()
                    .map(|err| format!(" · {err}"))
                    .unwrap_or_default(),
            ));
        }
    });
    out
}

// ---------------------------------------------------------------------------
// The Tweaks page (R87)
// ---------------------------------------------------------------------------

/// One boolean tweak: a `label.opt` checkbox — the app's only toggle idiom —
/// phrased feature-positive (checked = the feature is ON, matching the "Look
/// for a new version once a day" precedent), over a pref stored the off way
/// round. The hint under it carries the honesty and the search words: every
/// mark tweak's hint contains "hide", because that is the verb someone types.
#[allow(clippy::too_many_arguments)]
fn tweak_toggle(
    app: App,
    visible: Signal<bool>,
    label: &'static str,
    hint: &'static str,
    on_now: fn(&Prefs) -> bool,
    set_on: fn(&mut Prefs, bool),
    toast_on: &'static str,
    toast_off: &'static str,
) -> impl IntoView {
    view! {
        {move || {
            visible
                .get()
                .then(|| {
                    view! {
                        <div class="tweak">
                            <label class="opt">
                                <input
                                    type="checkbox"
                                    prop:checked=move || app.prefs.with(on_now)
                                    on:change=move |ev| {
                                        let on = event_target_checked(&ev);
                                        app.set_tweak(set_on, on);
                                        // Toast because the effect lands on tabs you
                                        // cannot currently see — the rule every
                                        // invisible flip in this app follows.
                                        app.toast(if on { toast_on } else { toast_off });
                                    }
                                />
                                <span>{label}</span>
                            </label>
                            <p class="muted small">{hint}</p>
                        </div>
                    }
                })
        }}
    }
}

/// Every small choice about how the timetable looks, in one searchable
/// place. Three groups; a search box with the same three switches every
/// search box in this app has; one reset. The search state is session-only
/// on purpose — a lens, not work product — so it lives in plain signals,
/// never in `Filters`, never in the undo history.
fn tweaks_page(app: App) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let match_case = RwSignal::new(false);
    let whole_word = RwSignal::new(false);
    let use_regex = RwSignal::new(false);

    // What each row is findable BY: its group, its label, its hint — the
    // words on the screen, nothing hidden. Index order = render order.
    let hay: Vec<String> = TWEAK_HAYSTACKS
        .iter()
        .map(|(group, label, hint)| format!("{group} {label} {hint}"))
        .collect();
    // ONE Matcher per pass, never one per row (core/src/search.rs's own
    // rule). A broken pattern matches NOTHING — the error line under the
    // box is the explanation, and rows staying put would contradict it.
    let visible = Memo::new(move |_| {
        let text = query.get();
        let m = ttcore::search::Matcher::new(&ttcore::search::Query {
            text: &text,
            match_case: match_case.get(),
            whole_word: whole_word.get(),
            use_regex: use_regex.get(),
        });
        hay.iter().map(|h| m.matches(h)).collect::<Vec<bool>>()
    });
    let bad = Memo::new(move |_| {
        let text = query.get();
        // Nothing is compiled unless the switch is on and there is something
        // to compile — the filter bar's own economy.
        if !use_regex.get() || text.trim().is_empty() {
            return None;
        }
        ttcore::search::Matcher::new(&ttcore::search::Query {
            text: &text,
            match_case: match_case.get(),
            whole_word: whole_word.get(),
            use_regex: true,
        })
        .error()
        .map(str::to_string)
    });
    let vis = move |i: usize| Signal::derive(move || visible.with(|v| v[i]));
    let group_any = move |r: std::ops::Range<usize>| visible.with(|v| v[r].iter().any(|b| *b));
    let none_match = Memo::new(move |_| visible.with(|v| v.iter().all(|b| !*b)));
    let has_text = Memo::new(move |_| !query.with(String::is_empty));
    let box_ref = NodeRef::<leptos::html::Input>::new();

    view! {
        <div class="filterbar noprint" role="group" aria-label="Find a tweak">
            <div class="search-group">
                <div
                    class="searchbox"
                    class:bad=move || bad.with(Option::is_some)
                    class:filled=move || has_text.get()
                >
                    <input
                        node_ref=box_ref
                        type="search"
                        placeholder=move || {
                            if use_regex.get() { "Pattern: marks|rows" } else { "Find a tweak" }
                        }
                        aria-label="Search the tweaks"
                        aria-invalid=move || {
                            if bad.with(Option::is_some) { "true" } else { "false" }
                        }
                        aria-describedby=move || {
                            if bad.with(Option::is_some) { "search-pattern-error" } else { "" }
                        }
                        prop:value=move || query.get()
                        on:input=move |ev| query.set(event_target_value(&ev))
                        on:keydown=domx::blur_on_enter
                    />
                    <div class="searchbox-switches">
                        {move || {
                            has_text
                                .get()
                                .then(|| {
                                    view! {
                                        <button
                                            type="button"
                                            class="search-switch search-clear"
                                            aria-label="Clear search"
                                            title="Clear the search box"
                                            on:click=move |_| {
                                                query.set(String::new());
                                                if let Some(el) = box_ref.get() {
                                                    let _ = el.focus();
                                                }
                                            }
                                        >
                                            "✕"
                                        </button>
                                    }
                                })
                        }}
                        {tweak_search_switch(
                            "Aa",
                            "Match case",
                            "Tell capitals apart: Aa matches Aa, not aa",
                            match_case,
                        )}
                        {tweak_search_switch(
                            "ab",
                            "Whole word",
                            "Only whole words: mark stops matching marks",
                            whole_word,
                        )}
                        {tweak_search_switch(
                            ".*",
                            "Regular expression",
                            "Read the box as a pattern: ^mark, marks|rows, t(i|o)ck",
                            use_regex,
                        )}
                    </div>
                </div>
            </div>
            {move || {
                bad.get()
                    .map(|why| {
                        view! {
                            <p id="search-pattern-error" class="searchbox-bad" role="status">
                                <span aria-hidden="true">"⚠ "</span>
                                {format!("Not a pattern yet — {why}.")}
                            </p>
                        }
                    })
            }}
        </div>

        {move || {
            group_any(0..3)
                .then(|| {
                    view! {
                        <div class="panel">
                            <h3>"Marks"</h3>
                            <p class="muted small">
                                "Hiding a mark hides the sign, never the fact — the \
                                 Clashes list and “Your changes” keep saying everything."
                            </p>
                            {tweak_toggle(
                                app,
                                vis(0),
                                "Mark clashes with ⚠ and a red border",
                                "Untick to hide the marks, on screen and on paper. The \
                                 Clashes panel under My timetable still lists every \
                                 overlap either way.",
                                |p| !p.marks_clash_off,
                                |p, on| p.marks_clash_off = !on,
                                "Clash marks are back.",
                                "Clash marks are hidden. The Clashes panel still lists \
                                 every clash.",
                            )}
                            {tweak_toggle(
                                app,
                                vis(1),
                                "Mark what you changed with ✎",
                                "Untick to hide the ✎ on times, rooms and credits you set \
                                 yourself. “Your changes” under My data keeps the full \
                                 list.",
                                |p| !p.marks_edits_off,
                                |p, on| p.marks_edits_off = !on,
                                "The ✎ marks are back.",
                                "The ✎ marks are hidden. “Your changes” still lists \
                                 everything.",
                            )}
                            {tweak_toggle(
                                app,
                                vis(2),
                                "Tick your courses with ✓ on the Master grid and Halls",
                                "Untick to hide the ✓ that picks your courses out of \
                                 everyone's. The printed key mentions ✓ only while it is \
                                 on.",
                                |p| !p.marks_ticks_off,
                                |p, on| p.marks_ticks_off = !on,
                                "The ✓ ticks are back.",
                                "The ✓ ticks are hidden, on screen and on paper.",
                            )}
                        </div>
                    }
                })
        }}

        {move || {
            group_any(3..7)
                .then(|| {
                    view! {
                        <div class="panel">
                            <h3>"The week grid"</h3>
                            {tweak_toggle(
                                app,
                                vis(3),
                                "Highlight today's row",
                                "The accent line on today's row in every week table. \
                                 Paper never marks today either way.",
                                |p| !p.today_highlight_off,
                                |p, on| p.today_highlight_off = !on,
                                "Today's row is highlighted again.",
                                "Today's row is no longer highlighted.",
                            )}
                            {tweak_toggle(
                                app,
                                vis(4),
                                "Dim days with no classes",
                                "Days with nothing on them keep quiet, so full days \
                                 stand out.",
                                |p| !p.quiet_dim_off,
                                |p, on| p.quiet_dim_off = !on,
                                "Days with no classes are dimmed again.",
                                "Days with no classes now look like every other day.",
                            )}
                            {tweak_toggle(
                                app,
                                vis(5),
                                "Show hall names on chips",
                                "Where each class meets, written on every chip. Tight \
                                 rows hide them to save space either way.",
                                |p| !p.chip_halls_off,
                                |p, on| p.chip_halls_off = !on,
                                "Hall names are back on the chips.",
                                "Hall names are off the chips.",
                            )}
                            {move || {
                                vis(6)
                                    .get()
                                    .then(|| {
                                        view! {
                                            <div class="tweak">
                                                <div class="tweak-seg">
                                                    <span>"Row height"</span>
                                                    <div
                                                        class="seg"
                                                        role="radiogroup"
                                                        aria-label="Row height"
                                                        on:keydown=domx::seg_radio_keydown
                                                    >
                                                        {row_height_choice(app, None, "Follow this device")}
                                                        {row_height_choice(
                                                            app,
                                                            Some(Density::Comfortable),
                                                            "Roomy",
                                                        )}
                                                        {row_height_choice(app, Some(Density::Compact), "Tight")}
                                                    </div>
                                                </div>
                                                <p class="muted small">
                                                    "The Master grid's rows. Until you choose, they follow \
                                                     the screen the app is opened on."
                                                </p>
                                            </div>
                                        }
                                    })
                            }}
                        </div>
                    }
                })
        }}

        {move || {
            group_any(7..10)
                .then(|| {
                    view! {
                        <div class="panel">
                            <h3>"Colour and motion"</h3>
                            {move || {
                                vis(7)
                                    .get()
                                    .then(|| {
                                        view! {
                                            <div class="tweak">
                                                <div class="tweak-seg">
                                                    <span>"Theme"</span>
                                                    <div
                                                        class="seg"
                                                        role="radiogroup"
                                                        aria-label="Theme"
                                                        on:keydown=domx::seg_radio_keydown
                                                    >
                                                        {theme_choice(app, ThemePref::Auto, "Auto")}
                                                        {theme_choice(app, ThemePref::Light, "Light")}
                                                        {theme_choice(app, ThemePref::Dark, "Dark")}
                                                    </div>
                                                </div>
                                                <p class="muted small">
                                                    "Auto follows your device. The header's theme button \
                                                     cycles the same choice."
                                                </p>
                                            </div>
                                        }
                                    })
                            }}
                            {tweak_toggle(
                                app,
                                vis(8),
                                "Colour chips by programme",
                                "Each programme keeps its own shade. Untick for one plain \
                                 shade — clearer when the colours read alike to you.",
                                |p| !p.chips_plain,
                                |p, on| p.chips_plain = !on,
                                "Chips are coloured again.",
                                "Chips wear one plain shade now.",
                            )}
                            {tweak_toggle(
                                app,
                                vis(9),
                                "Animate panels and toasts",
                                "Untick for the same stillness your device's \
                                 reduce-motion setting asks for, without needing it set.",
                                |p| !p.reduce_motion,
                                |p, on| p.reduce_motion = !on,
                                "Animations are back.",
                                "Animations are off.",
                            )}
                        </div>
                    }
                })
        }}

        {move || {
            (none_match.get() && bad.with(Option::is_none))
                .then(|| {
                    view! { <div class="empty panel">"No tweaks match what you typed."</div> }
                })
        }}

        // Never hidden, even mid-search: when the search fails, this line IS
        // the answer — the knob being hunted has a home of its own.
        <p class="muted small">
            "Update checking, your own helper site, and everything the app \
             stores live in My data."
        </p>
        <div class="row noprint" style="margin-top:0.4rem">
            <button
                class="btn small danger"
                title="Everything on this page, back to how the app ships"
                on:click=move |_| {
                    app.reset_tweaks();
                    crate::apply_theme(app);
                    app.toast("All tweaks are back to how the app ships.");
                }
            >
                "Reset all tweaks"
            </button>
        </div>
    }
}

/// The searchable words of each tweak row, in render order: group, label,
/// hint. Kept beside the page that renders them; a row and its haystack
/// drifting apart makes the search quietly lie.
const TWEAK_HAYSTACKS: [(&str, &str, &str); 10] = [
    (
        "Marks",
        "Mark clashes with ⚠ and a red border",
        "Untick to hide the marks, on screen and on paper. The Clashes panel under My \
         timetable still lists every overlap either way.",
    ),
    (
        "Marks",
        "Mark what you changed with ✎",
        "Untick to hide the ✎ on times, rooms and credits you set yourself. Your changes \
         under My data keeps the full list.",
    ),
    (
        "Marks",
        "Tick your courses with ✓ on the Master grid and Halls",
        "Untick to hide the ✓ that picks your courses out of everyone's. The printed key \
         mentions ✓ only while it is on.",
    ),
    (
        "The week grid",
        "Highlight today's row",
        "The accent line on today's row in every week table. Paper never marks today \
         either way.",
    ),
    (
        "The week grid",
        "Dim days with no classes",
        "Days with nothing on them keep quiet, so full days stand out.",
    ),
    (
        "The week grid",
        "Show hall names on chips",
        "Where each class meets, written on every chip. Tight rows hide them to save \
         space either way.",
    ),
    (
        "The week grid",
        "Row height",
        "Follow this device Roomy Tight The Master grid's rows. Until you choose, they \
         follow the screen the app is opened on.",
    ),
    (
        "Colour and motion",
        "Theme",
        "Auto Light Dark follows your device. The header's theme button cycles the same \
         choice.",
    ),
    (
        "Colour and motion",
        "Colour chips by programme",
        "Each programme keeps its own shade. Untick for one plain shade — clearer when \
         the colours read alike to you.",
    ),
    (
        "Colour and motion",
        "Animate panels and toasts",
        "Untick for the same stillness your device's reduce-motion setting asks for, \
         without needing it set.",
    ),
];

/// A search switch over a plain session-local signal — the filter bar's
/// `search_switch` shape (same glyphs, same aria, same CSS) minus its undo
/// and persistence, which belong to course filtering and not to a lens.
fn tweak_search_switch(
    glyph: &'static str,
    label: &'static str,
    hint: &'static str,
    on: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="search-switch"
            class:on=move || on.get()
            aria-pressed=move || if on.get() { "true" } else { "false" }
            aria-label=label
            title=format!("{label} — {hint}")
            on:click=move |_| on.update(|v| *v = !*v)
        >
            {glyph}
        </button>
    }
}

/// One Row-height radio. "Follow this device" writes `None` — the state the
/// Master grid's own toolbar button cannot reach (it only toggles between
/// the two chosen values), so this seg is the one path back to the default
/// short of resetting everything.
fn row_height_choice(app: App, value: Option<Density>, label: &'static str) -> impl IntoView {
    let picked = move || app.prefs.with(|p| p.density) == value;
    view! {
        <button
            role="radio"
            aria-checked=move || if picked() { "true" } else { "false" }
            tabindex=move || if picked() { "0" } else { "-1" }
            on:click=move |_| {
                app.prefs.update(|p| p.density = value);
                app.persist_prefs();
                app.toast(match value {
                    None => "Master grid rows follow this device again.",
                    Some(Density::Comfortable) => "Master grid rows: roomy.",
                    Some(Density::Compact) => "Master grid rows: tight.",
                });
            }
        >
            {label}
        </button>
    }
}

/// One Theme radio. Same pref as the header's cycler, so the two always
/// agree; no toast, because the page repaints before your eyes.
fn theme_choice(app: App, value: ThemePref, label: &'static str) -> impl IntoView {
    let picked = move || app.prefs.with(|p| p.theme) == value;
    view! {
        <button
            role="radio"
            aria-checked=move || if picked() { "true" } else { "false" }
            tabindex=move || if picked() { "0" } else { "-1" }
            on:click=move |_| {
                app.prefs.update(|p| p.theme = value);
                app.persist_prefs();
                crate::apply_theme(app);
            }
        >
            {label}
        </button>
    }
}
