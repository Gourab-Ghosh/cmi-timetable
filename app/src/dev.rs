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

use crate::state::{App, BannerKind, Density, DevTab, Prefs, Tab, ThemePref};
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

/// One collapsible group of tweak rows (R88 — collapse earned its keep at
/// 35 rows where 10 needed none). The heading IS the disclosure button
/// (aria-expanded, the app's own idiom — never `<details>`, which would
/// fight the group_any hiding). `open` is session-only, like the search: a
/// lens, not work product. While a query is live the collapse is overridden
/// — a group with a matching row renders EXPANDED and one without hides
/// entirely — so search always reveals; clearing the box restores the
/// reader's own toggles untouched.
fn tweak_group(
    any: Signal<bool>,
    searching: Memo<bool>,
    open: RwSignal<bool>,
    title: &'static str,
    lede: Option<&'static str>,
    rows: impl Fn() -> AnyView + Clone + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        {move || {
            let rows = rows.clone();
            any.get()
                .then(|| {
                    view! {
                        <div class="panel tweak-group">
                            <h3>
                                <button
                                    type="button"
                                    class="group-toggle"
                                    aria-expanded=move || {
                                        if searching.get() || open.get() { "true" } else { "false" }
                                    }
                                    on:click=move |_| {
                                        // Mid-search the group is already held
                                        // open, so a click that silently flips
                                        // the hidden state would surprise later.
                                        if !searching.get_untracked() {
                                            open.update(|o| *o = !*o);
                                        }
                                    }
                                >
                                    <span class="caret" aria-hidden="true">"▸"</span>
                                    {title}
                                </button>
                            </h3>
                            {move || {
                                let rows = rows.clone();
                                (searching.get() || open.get())
                                    .then(move || {
                                        view! {
                                            <div class="group-body">
                                                {lede
                                                    .map(|l| {
                                                        view! { <p class="muted small group-lede">{l}</p> }
                                                    })}
                                                {rows()}
                                            </div>
                                        }
                                    })
                            }}
                        </div>
                    }
                })
        }}
    }
}

/// One option of a segmented tweak (the Theme/Row-height shape, generalised):
/// `picked` reads the pref, `apply` writes it and says so. Every seg row is a
/// radiogroup — one Tab stop, arrows move the choice.
fn seg_choice(
    app: App,
    picked: impl Fn(&Prefs) -> bool + Copy + Send + Sync + 'static,
    apply: impl Fn(App) + Copy + Send + Sync + 'static,
    label: &'static str,
) -> impl IntoView {
    let is_on = move || app.prefs.with(picked);
    view! {
        <button
            role="radio"
            aria-checked=move || if is_on() { "true" } else { "false" }
            tabindex=move || if is_on() { "0" } else { "-1" }
            on:click=move |_| apply(app)
        >
            {label}
        </button>
    }
}

/// A number tweak: empty = "how the app ships" (the pref goes back to None),
/// anything typed is clamped to the honest range at THIS site, the same
/// clamp the read site applies — so what the box shows after a change is
/// what the app is actually doing. The wheel steps it, like every numeric
/// box in the app (and that stepping obeys its own tweak).
#[allow(clippy::too_many_arguments)]
fn tweak_number(
    app: App,
    visible: Signal<bool>,
    label: &'static str,
    hint: &'static str,
    min: u32,
    max: u32,
    placeholder: &'static str,
    get: fn(&Prefs) -> Option<u32>,
    set: fn(&mut Prefs, Option<u32>),
    toast_set: fn(u32) -> String,
    toast_clear: &'static str,
) -> impl IntoView {
    view! {
        {move || {
            visible
                .get()
                .then(|| {
                    view! {
                        <div class="tweak">
                            <div class="tweak-seg">
                                <span>{label}</span>
                                <input
                                    type="number"
                                    class="tweak-num"
                                    min=min
                                    max=max
                                    step="1"
                                    placeholder=placeholder
                                    aria-label=label
                                    prop:value=move || {
                                        app.prefs
                                            .with(get)
                                            .map(|v| v.to_string())
                                            .unwrap_or_default()
                                    }
                                    on:wheel=domx::step_on_wheel
                                    on:change=move |ev| {
                                        let raw = event_target_value(&ev);
                                        let parsed = raw.trim().parse::<u32>().ok();
                                        match parsed {
                                            Some(n) => {
                                                let n = n.clamp(min, max);
                                                app.prefs.update(|p| set(p, Some(n)));
                                                app.persist_prefs();
                                                app.toast(toast_set(n));
                                            }
                                            None => {
                                                app.prefs.update(|p| set(p, None));
                                                app.persist_prefs();
                                                app.toast(toast_clear);
                                            }
                                        }
                                    }
                                />
                            </div>
                            <p class="muted small">{hint}</p>
                        </div>
                    }
                })
        }}
    }
}

/// Group 1 — Marks (3 rows, all pre-R88).
fn rows_marks(app: App, v: [Signal<bool>; 3]) -> AnyView {
    view! {
        {tweak_toggle(
            app,
            v[0],
            "Mark clashes with ⚠ and a red border",
            "Untick to hide the marks, on screen and on paper. The Clashes \
             panel under My timetable still lists every overlap either way.",
            |p| !p.marks_clash_off,
            |p, on| p.marks_clash_off = !on,
            "Clash marks are back.",
            "Clash marks are hidden. The Clashes panel still lists every clash.",
        )}
        {tweak_toggle(
            app,
            v[1],
            "Mark what you changed with ✎",
            "Untick to hide the ✎ on times, rooms and credits you set \
             yourself. “Your changes” under My data keeps the full list.",
            |p| !p.marks_edits_off,
            |p, on| p.marks_edits_off = !on,
            "The ✎ marks are back.",
            "The ✎ marks are hidden. “Your changes” still lists everything.",
        )}
        {tweak_toggle(
            app,
            v[2],
            "Tick your courses with ✓ on the Master grid and Halls",
            "Untick to hide the ✓ that picks your courses out of everyone's. \
             The printed key mentions ✓ only while it is on.",
            |p| !p.marks_ticks_off,
            |p, on| p.marks_ticks_off = !on,
            "The ✓ ticks are back.",
            "The ✓ ticks are hidden, on screen and on paper.",
        )}
    }
    .into_any()
}

/// Group 2 — The week grid (8 rows; 4 joined in R88).
fn rows_week(app: App, v: [Signal<bool>; 8]) -> AnyView {
    view! {
        {tweak_toggle(
            app,
            v[0],
            "Highlight today's row",
            "The accent line on today's row in every week table. Paper never \
             marks today either way.",
            |p| !p.today_highlight_off,
            |p, on| p.today_highlight_off = !on,
            "Today's row is highlighted again.",
            "Today's row is no longer highlighted.",
        )}
        {tweak_toggle(
            app,
            v[1],
            "Dim days with no classes",
            "Days with nothing on them keep quiet, so full days stand out.",
            |p| !p.quiet_dim_off,
            |p, on| p.quiet_dim_off = !on,
            "Days with no classes are dimmed again.",
            "Days with no classes now look like every other day.",
        )}
        {tweak_toggle(
            app,
            v[2],
            "Show hall names on chips",
            "Where each class meets, written on every chip. Tight rows hide \
             them to save space either way.",
            |p| !p.chip_halls_off,
            |p, on| p.chip_halls_off = !on,
            "Hall names are back on the chips.",
            "Hall names are off the chips.",
        )}
        {tweak_toggle(
            app,
            v[3],
            "Course names on chips",
            "The course's name written under its code on every week grid, for \
             weeks when the codes haven't stuck yet. Tight rows leave names \
             off to save space, and paper keeps its legend either way.",
            |p| p.chip_names,
            |p, on| p.chip_names = on,
            "Course names are on the chips now.",
            "Chips show codes only again.",
        )}
        {move || {
            v[4].get()
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
                                    {row_height_choice(app, Some(Density::Comfortable), "Roomy")}
                                    {row_height_choice(app, Some(Density::Compact), "Tight")}
                                </div>
                            </div>
                            <p class="muted small">
                                // The hint names the true scope: the row
                                // below can widen it, and a hint describing
                                // yesterday's reach is a small lie.
                                {move || {
                                    if app.prefs.with(|p| p.density_everywhere) {
                                        "Every week table's rows — “Apply the row height \
                                         everywhere” below is on. Until you choose, they \
                                         follow the screen the app is opened on."
                                    } else {
                                        "The Master grid's rows. Until you choose, they \
                                         follow the screen the app is opened on."
                                    }
                                }}
                            </p>
                        </div>
                    }
                })
        }}
        {tweak_toggle(
            app,
            v[5],
            "Apply the row height everywhere",
            "Your Roomy or Tight choice reaches My timetable and Halls too, \
             not only the Master grid. Tight everywhere fits a whole week on \
             a laptop screen.",
            |p| p.density_everywhere,
            |p, on| p.density_everywhere = on,
            "Your row height now reaches My timetable and Halls too.",
            "The row height choice is back to the Master grid only.",
        )}
        {tweak_toggle(
            app,
            v[6],
            "Show Saturday and Sunday even when empty",
            "Every week grid draws all seven days, instead of growing a \
             weekend row only once something meets there. Handy for dragging \
             a course of your own onto a Saturday that doesn't exist yet.",
            |p| p.weekend_rows,
            |p, on| p.weekend_rows = on,
            "Every week grid now draws Saturday and Sunday.",
            "Weekend rows appear only when something meets there again.",
        )}
        {tweak_toggle(
            app,
            v[7],
            "Show the how-to hints under the grids",
            "The one-line instructions, like how ✎ Edit layout drags work. \
             Untick once you know the app — every control they describe \
             stays.",
            |p| !p.grid_hints_off,
            |p, on| p.grid_hints_off = !on,
            "The how-to hints are back.",
            "The how-to hints are hidden. Every control they describe stays.",
        )}
    }
    .into_any()
}

/// Group 3 — Colour and motion (5 rows; 2 joined in R88).
fn rows_colour(app: App, v: [Signal<bool>; 5]) -> AnyView {
    view! {
        {move || {
            v[0].get()
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
                                "Auto follows your device. The header's theme button cycles \
                                 the same choice."
                            </p>
                        </div>
                    }
                })
        }}
        {tweak_toggle(
            app,
            v[1],
            "Colour chips by programme",
            "Each programme keeps its own shade. Untick for one plain shade — \
             clearer when the colours read alike to you.",
            |p| !p.chips_plain,
            |p, on| p.chips_plain = !on,
            "Chips are coloured again.",
            "Chips wear one plain shade now.",
        )}
        {tweak_toggle(
            app,
            v[2],
            "Vivid chip colours",
            "Turns the programme shades up a notch, for screens or eyes that \
             wash them out. “Colour chips by programme” unticked still wins — \
             plain means plain.",
            |p| p.chips_vivid,
            |p, on| p.chips_vivid = on,
            "Chip colours turned up.",
            "Chip colours are back to normal.",
        )}
        {tweak_toggle(
            app,
            v[3],
            "Stronger lines and small print",
            "Darker grid lines and darker fine print, in both themes — for \
             screens where the hairlines fade, or eyes that wish they \
             wouldn't.",
            |p| p.strong_lines,
            |p, on| p.strong_lines = on,
            "Lines and small print darkened.",
            "Lines and small print are back to normal.",
        )}
        {tweak_toggle(
            app,
            v[4],
            "Animate panels and toasts",
            "Untick for the same stillness your device's reduce-motion \
             setting asks for, without needing it set.",
            |p| !p.reduce_motion,
            |p, on| p.reduce_motion = !on,
            "Animations are back.",
            "Animations are off.",
        )}
    }
    .into_any()
}

/// Group 4 — Opening the app (2 rows, R88).
fn rows_opening(app: App, v: [Signal<bool>; 2]) -> AnyView {
    view! {
        {move || {
            v[0].get()
                .then(|| {
                    view! {
                        <div class="tweak">
                            <div class="tweak-seg">
                                <span>"Open the app on"</span>
                                // A dropdown, not a seg: six options do not
                                // fit a pill row at 320px.
                                <select
                                    aria-label="Open the app on"
                                    on:wheel=domx::cycle_on_wheel
                                    prop:value=move || {
                                        app.prefs
                                            .with(|p| p.landing_tab)
                                            .map(|t| t.label().to_string())
                                            .unwrap_or_default()
                                    }
                                    on:change=move |ev| {
                                        let raw = event_target_value(&ev);
                                        let tab = Tab::ALL
                                            .iter()
                                            .copied()
                                            .find(|t| t.label() == raw);
                                        app.prefs.update(|p| p.landing_tab = tab);
                                        app.persist_prefs();
                                        app.toast(match tab {
                                            Some(t) => {
                                                format!("The app now opens on {}.", t.label())
                                            }
                                            None => {
                                                "The app opens wherever you last were again."
                                                    .to_string()
                                            }
                                        });
                                    }
                                >
                                    <option value="">
                                        "The section I left (how the app ships)"
                                    </option>
                                    {Tab::ALL
                                        .iter()
                                        .map(|t| {
                                            let l = t.label();
                                            view! { <option value=l>{l}</option> }
                                        })
                                        .collect_view()}
                                </select>
                            </div>
                            <p class="muted small">
                                "Which section a fresh visit lands on. The app ships \
                                 opening wherever you last were — a share link's courses \
                                 arrive either way."
                            </p>
                        </div>
                    }
                })
        }}
        {tweak_toggle(
            app,
            v[1],
            "Remember the day pickers between visits",
            "My timetable's day strip and the Halls day reopen where you left \
             them. Untick and every visit opens on today — a pick still holds \
             until you close the tab.",
            |p| !p.day_picks_forget,
            |p, on| p.day_picks_forget = !on,
            "The day pickers are remembered between visits again.",
            "Every visit's day pickers now open on today.",
        )}
    }
    .into_any()
}

/// Group 5 — Notices and dialogs (2 rows, R88).
fn rows_notices(app: App, v: [Signal<bool>; 2]) -> AnyView {
    view! {
        {move || {
            v[0].get()
                .then(|| {
                    view! {
                        <div class="tweak">
                            <div class="tweak-seg">
                                <span>"Notices stay for"</span>
                                <div
                                    class="seg"
                                    role="radiogroup"
                                    aria-label="Notices stay for"
                                    on:keydown=domx::seg_radio_keydown
                                >
                                    {seg_choice(
                                        app,
                                        |p| p.toast_life_secs == Some(3),
                                        |app| {
                                            app.prefs.update(|p| p.toast_life_secs = Some(3));
                                            app.persist_prefs();
                                            app.toast("Notices now stay 3 seconds — like this one.");
                                        },
                                        "3 s",
                                    )}
                                    {seg_choice(
                                        app,
                                        |p| p.toast_life_secs.is_none(),
                                        |app| {
                                            app.prefs.update(|p| p.toast_life_secs = None);
                                            app.persist_prefs();
                                            app.toast("Notices stay 6 seconds again — how the app ships.");
                                        },
                                        "6 s",
                                    )}
                                    {seg_choice(
                                        app,
                                        |p| p.toast_life_secs == Some(12),
                                        |app| {
                                            app.prefs.update(|p| p.toast_life_secs = Some(12));
                                            app.persist_prefs();
                                            app.toast("Notices now stay 12 seconds — like this one.");
                                        },
                                        "12 s",
                                    )}
                                    {seg_choice(
                                        app,
                                        |p| p.toast_life_secs == Some(0),
                                        |app| {
                                            app.prefs.update(|p| p.toast_life_secs = Some(0));
                                            app.persist_prefs();
                                            app.toast(
                                                "Notices now stay until you close them — \
                                                 like this one, with its ✕.",
                                            );
                                        },
                                        "Until dismissed",
                                    )}
                                </div>
                            </div>
                            <p class="muted small">
                                "Every notice waits while you hover, focus or hold it, and \
                                 every one has its own ✕. 6 seconds is how the app ships."
                            </p>
                        </div>
                    }
                })
        }}
        {tweak_toggle(
            app,
            v[1],
            "Close a dialog by clicking the dark area",
            "Untick and only Close, Escape or the browser's own ways leave a \
             dialog — a stray click beside it does nothing. A half-written \
             form still asks before being thrown away.",
            |p| !p.scrim_close_off,
            |p, on| p.scrim_close_off = !on,
            "Clicking beside a dialog closes it again.",
            "Only Close or Escape leaves a dialog now.",
        )}
    }
    .into_any()
}

/// Group 6 — Wheel and swipe (2 rows, R88).
fn rows_gestures(app: App, v: [Signal<bool>; 2]) -> AnyView {
    view! {
        {tweak_toggle(
            app,
            v[0],
            "Step values with the wheel",
            "Scroll over credits, a time, a date or a dropdown and it moves \
             one step. Untick and the wheel only ever scrolls — typing and \
             the arrows still change every value.",
            |p| !p.wheel_step_off,
            |p, on| p.wheel_step_off = !on,
            "The wheel steps values again.",
            "The wheel only scrolls now. Typing and the arrow keys still \
             change every value.",
        )}
        {tweak_toggle(
            app,
            v[1],
            "Switch sections with the wheel or a swipe on the bar",
            "A wheel notch or a drag along the section bar steps one section. \
             Untick if it keeps happening while you scroll — taps and the \
             arrow keys still work.",
            |p| !p.rail_gestures_off,
            |p, on| p.rail_gestures_off = !on,
            "The wheel and swipes walk the sections again.",
            "The section bar answers taps and arrow keys only now.",
        )}
    }
    .into_any()
}

/// Group 7 — Editing and undo (2 rows, R88).
fn rows_editing(app: App, v: [Signal<bool>; 2]) -> AnyView {
    view! {
        {tweak_toggle(
            app,
            v[0],
            "Keep drags behind ✎ Edit layout",
            "Untick and a mouse or pen can drag a chip any time, no toggle \
             first. A finger still needs the toggle, so scrolling a phone \
             never moves a class — and every move stays undoable and listed \
             under Your changes.",
            |p| !p.drag_without_edit,
            |p, on| p.drag_without_edit = !on,
            "Drags need ✎ Edit layout again.",
            "A mouse can drag chips any time now. A finger still needs \
             ✎ Edit layout.",
        )}
        {tweak_number(
            app,
            v[1],
            "Undo history depth",
            "How many steps Ctrl+Z can walk back — 100 unless you say \
             otherwise. Each step keeps a copy of your selection in memory, \
             never in storage, so a very deep history costs RAM.",
            10,
            1000,
            "100",
            |p| p.undo_depth.map(u32::from),
            |p, n| p.undo_depth = n.map(|v| v as u16),
            |n| format!("Ctrl+Z now keeps {n} steps."),
            "Undo history is back to 100 steps.",
        )}
    }
    .into_any()
}

/// Group 8 — Syncing (4 rows, R88).
fn rows_syncing(app: App, v: [Signal<bool>; 4]) -> AnyView {
    view! {
        {move || {
            v[0].get()
                .then(|| {
                    view! {
                        <div class="tweak">
                            <div class="tweak-seg">
                                <span>"Check CMI on its own"</span>
                                <div
                                    class="seg"
                                    role="radiogroup"
                                    aria-label="Check CMI on its own"
                                    on:keydown=domx::seg_radio_keydown
                                >
                                    {seg_choice(
                                        app,
                                        |p| p.auto_sync.is_none(),
                                        |app| {
                                            app.prefs.update(|p| p.auto_sync = None);
                                            app.persist_prefs();
                                            app.toast(
                                                "The app checks CMI up to twice a day again — \
                                                 how it ships.",
                                            );
                                        },
                                        "Twice a day",
                                    )}
                                    {seg_choice(
                                        app,
                                        |p| p.auto_sync.as_deref() == Some("hourly"),
                                        |app| {
                                            app.prefs
                                                .update(|p| {
                                                    p.auto_sync = Some("hourly".to_string())
                                                });
                                            app.persist_prefs();
                                            app.toast("The app checks CMI every hour now.");
                                        },
                                        "Every hour",
                                    )}
                                    {seg_choice(
                                        app,
                                        |p| p.auto_sync.as_deref() == Some("manual"),
                                        |app| {
                                            app.prefs
                                                .update(|p| {
                                                    p.auto_sync = Some("manual".to_string())
                                                });
                                            app.persist_prefs();
                                            app.toast(
                                                "The app now fetches CMI only when you press \
                                                 Sync now.",
                                            );
                                        },
                                        "Only when I ask",
                                    )}
                                </div>
                            </div>
                            <p class="muted small">
                                "How often the app fetches CMI's pages without being asked. \
                                 The header keeps counting how old the timetable is \
                                 whichever you pick — and a browser that has never synced \
                                 still fetches its first timetable."
                            </p>
                        </div>
                    }
                })
        }}
        {tweak_toggle(
            app,
            v[1],
            "Ask the public helper sites when syncing",
            "Untick and no public relay is ever shown which CMI page you read \
             — only your own helper site (set in My data) and CMI itself are \
             asked. With “Ask CMI directly” also off and no helper site set, \
             pasting CMI's page is the way left, and a failed sync says so.",
            |p| !p.public_relays_off,
            |p, on| p.public_relays_off = !on,
            "Public helper sites are back in the sync.",
            "Syncs now ask only your own helper site and CMI itself.",
        )}
        {tweak_toggle(
            app,
            v[2],
            "Ask CMI directly when every helper site fails",
            "The last route of a sync is cmi.ac.in itself — on CMI's own \
             network that is what raises the browser's local-network \
             question. Untick and the app never contacts CMI's address from \
             your browser, and a failed sync says this route was switched off \
             here instead of pretending it was tried.",
            |p| !p.direct_route_off,
            |p, on| p.direct_route_off = !on,
            "cmi.ac.in is back as the sync's last resort.",
            "The app will never contact cmi.ac.in from this browser now.",
        )}
        {tweak_number(
            app,
            v[3],
            "Turn the synced pill amber after",
            "How many days old the timetable gets before the header's pill \
             wears its warning colour — 2 unless you say otherwise. The age \
             itself is always written out and keeps counting; this only moves \
             where the colour starts worrying.",
            1,
            14,
            "2",
            |p| p.stale_after_days.map(u32::from),
            |p, n| p.stale_after_days = n.map(|v| v as u8),
            |n| {
                if n == 1 {
                    "The pill turns amber after 1 day now.".to_string()
                } else {
                    format!("The pill turns amber after {n} days now.")
                }
            },
            "The pill turns amber after 2 days again.",
        )}
    }
    .into_any()
}

/// Group 9 — Printing (3 rows, R88).
fn rows_printing(app: App, v: [Signal<bool>; 3]) -> AnyView {
    view! {
        {tweak_toggle(
            app,
            v[0],
            "Print in colour",
            "Untick for plain-ink sheets: white header bands, grey-bordered \
             chips, black rules — kinder to toner and photocopiers. ⚠, ✎ and \
             ✓ already say everything in black and white, and clash red \
             stays: it is a warning, not decoration.",
            |p| !p.print_plain,
            |p, on| p.print_plain = !on,
            "Sheets print in colour again.",
            "Sheets print in plain ink now. Clash red stays.",
        )}
        {move || {
            v[1].get()
                .then(|| {
                    view! {
                        <div class="tweak">
                            <div class="tweak-seg">
                                <span>"Page shape"</span>
                                <div
                                    class="seg"
                                    role="radiogroup"
                                    aria-label="Page shape"
                                    on:keydown=domx::seg_radio_keydown
                                >
                                    {seg_choice(
                                        app,
                                        |p| p.print_page.is_none(),
                                        |app| {
                                            app.prefs.update(|p| p.print_page = None);
                                            app.persist_prefs();
                                            app.toast("Sheets print wide again — how they ship.");
                                        },
                                        "Wide",
                                    )}
                                    {seg_choice(
                                        app,
                                        |p| p.print_page.as_deref() == Some("portrait"),
                                        |app| {
                                            app.prefs
                                                .update(|p| {
                                                    p.print_page = Some("portrait".to_string())
                                                });
                                            app.persist_prefs();
                                            app.toast("Sheets print tall now.");
                                        },
                                        "Tall",
                                    )}
                                    {seg_choice(
                                        app,
                                        |p| p.print_page.as_deref() == Some("ask"),
                                        |app| {
                                            app.prefs
                                                .update(|p| p.print_page = Some("ask".to_string()));
                                            app.persist_prefs();
                                            app.toast("The print dialog chooses the page shape now.");
                                        },
                                        "Let the browser ask",
                                    )}
                                </div>
                            </div>
                            <p class="muted small">
                                "The sheets are designed wide, like the week is. Tall suits \
                                 binders and clipboards; “Let the browser ask” puts the \
                                 choice back in the print dialog. Only the app's own Print \
                                 buttons obey — the browser's Ctrl+P keeps the wide design."
                            </p>
                        </div>
                    }
                })
        }}
        {tweak_toggle(
            app,
            v[2],
            "Sign each sheet “made with the CMI Timetable Planner”",
            "The credit at the end of every printed sheet's stats line. \
             Untick for an unsigned sheet — the semester, the sync date and \
             the check-against-CMI line stay, because those are facts about \
             the timetable, not the app.",
            |p| !p.print_credit_off,
            |p, on| p.print_credit_off = !on,
            "The printed credit is back.",
            "Sheets print unsigned now.",
        )}
    }
    .into_any()
}

/// Group 10 — Calendar files (2 rows, R88). Named for what the files are —
/// never "Exports", which would collide with My data's export buttons.
fn rows_calendar(app: App, v: [Signal<bool>; 2]) -> AnyView {
    view! {
        {tweak_toggle(
            app,
            v[0],
            "Put a link back to this planner in every calendar event",
            "Each event's notes end with a link that reopens this timetable. \
             Untick to keep the file to class facts — the link also spells \
             out which courses you take, which matters if you send the file \
             to someone.",
            |p| !p.ics_link_off,
            |p, on| p.ics_link_off = !on,
            "Calendar events carry the planner link again.",
            "Calendar events keep to class facts now.",
        )}
        {tweak_toggle(
            app,
            v[1],
            "Describe the course inside each calendar event",
            "The instructor and branch lines in every event's notes. Untick \
             to keep events to title, room and time — some calendar apps read \
             the notes aloud on every reminder.",
            |p| !p.ics_desc_off,
            |p, on| p.ics_desc_off = !on,
            "Instructor and branch lines are back in calendar events.",
            "Calendar events keep to title, room and time now.",
        )}
    }
    .into_any()
}

/// Group 11 — Developer mode (2 rows, R88).
fn rows_devmode(app: App, v: [Signal<bool>; 2]) -> AnyView {
    view! {
        {tweak_toggle(
            app,
            v[0],
            "Show a Developer button in the header",
            "A one-press door to this mode, beside the theme button. The door \
             in My data stays either way.",
            |p| p.dev_button_on,
            |p, on| p.dev_button_on = on,
            "The Developer button is in the header now.",
            "The header Developer button is gone. The door in My data stays.",
        )}
        {tweak_toggle(
            app,
            v[1],
            "Echo every fetch to the browser console",
            "Each sync request also prints one line in your browser's \
             DevTools — route, status, milliseconds, bytes — so a bug report \
             can carry the console. The Sync page's log shows the same rows \
             either way.",
            |p| p.console_fetch_log_on,
            |p, on| p.console_fetch_log_on = on,
            "Every sync request now also prints one line in the browser \
             console.",
            "The console echo is off. The Sync page keeps its log.",
        )}
    }
    .into_any()
}

/// Every small choice about how the app looks and acts, in one searchable
/// place. Eleven groups, collapsible; a search box with the same three
/// switches every search box in this app has; one reset. The search state is
/// session-only on purpose — a lens, not work product — so it lives in plain
/// signals, never in `Filters`, never in the undo history.
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
    let ga = move |a: usize, b: usize| {
        Signal::derive(move || visible.with(|v| v[a..b].iter().any(|x| *x)))
    };
    let none_match = Memo::new(move |_| visible.with(|v| v.iter().all(|b| !*b)));
    let has_text = Memo::new(move |_| !query.with(String::is_empty));
    let box_ref = NodeRef::<leptos::html::Input>::new();
    // Each group's disclosure, session-only (the search-switch precedent —
    // no Prefs field, so "Reset all tweaks" has nothing to know about it).
    // The shipped page's three groups open, the eight R88 groups closed:
    // a first visit shows the familiar page, depth one press away.
    let open: [RwSignal<bool>; 11] = std::array::from_fn(|i| RwSignal::new(i < 3));

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

        {tweak_group(
            ga(0, 3),
            has_text,
            open[0],
            "Marks",
            Some(
                "Hiding a mark hides the sign, never the fact — the Clashes \
                 list and “Your changes” keep saying everything.",
            ),
            move || rows_marks(app, [vis(0), vis(1), vis(2)]),
        )}
        {tweak_group(
            ga(3, 11),
            has_text,
            open[1],
            "The week grid",
            None,
            move || {
                rows_week(
                    app,
                    [vis(3), vis(4), vis(5), vis(6), vis(7), vis(8), vis(9), vis(10)],
                )
            },
        )}
        {tweak_group(
            ga(11, 16),
            has_text,
            open[2],
            "Colour and motion",
            None,
            move || rows_colour(app, [vis(11), vis(12), vis(13), vis(14), vis(15)]),
        )}
        {tweak_group(
            ga(16, 18),
            has_text,
            open[3],
            "Opening the app",
            None,
            move || rows_opening(app, [vis(16), vis(17)]),
        )}
        {tweak_group(
            ga(18, 20),
            has_text,
            open[4],
            "Notices and dialogs",
            None,
            move || rows_notices(app, [vis(18), vis(19)]),
        )}
        {tweak_group(
            ga(20, 22),
            has_text,
            open[5],
            "Wheel and swipe",
            None,
            move || rows_gestures(app, [vis(20), vis(21)]),
        )}
        {tweak_group(
            ga(22, 24),
            has_text,
            open[6],
            "Editing and undo",
            None,
            move || rows_editing(app, [vis(22), vis(23)]),
        )}
        {tweak_group(
            ga(24, 28),
            has_text,
            open[7],
            "Syncing",
            Some(
                "However you tune it, the header never stops saying how old \
                 the timetable is.",
            ),
            move || rows_syncing(app, [vis(24), vis(25), vis(26), vis(27)]),
        )}
        {tweak_group(
            ga(28, 31),
            has_text,
            open[8],
            "Printing",
            Some(
                "Paper hides signs, never facts — the clash strip and the \
                 caveat line print always.",
            ),
            move || rows_printing(app, [vis(28), vis(29), vis(30)]),
        )}
        {tweak_group(
            ga(31, 33),
            has_text,
            open[9],
            "Calendar files",
            None,
            move || rows_calendar(app, [vis(31), vis(32)]),
        )}
        {tweak_group(
            ga(33, 35),
            has_text,
            open[10],
            "Developer mode",
            None,
            move || rows_devmode(app, [vis(33), vis(34)]),
        )}

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
const TWEAK_HAYSTACKS: [(&str, &str, &str); 35] = [
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
        "Course names on chips",
        "The course's name written under its code on every week grid, for weeks when \
         the codes haven't stuck yet. Tight rows leave names off to save space, and \
         paper keeps its legend either way.",
    ),
    (
        "The week grid",
        "Row height",
        "Follow this device Roomy Tight The Master grid's rows. Until you choose, they \
         follow the screen the app is opened on.",
    ),
    (
        "The week grid",
        "Apply the row height everywhere",
        "Your Roomy or Tight choice reaches My timetable and Halls too, not only the \
         Master grid. Tight everywhere fits a whole week on a laptop screen.",
    ),
    (
        "The week grid",
        "Show Saturday and Sunday even when empty",
        "Every week grid draws all seven days, instead of growing a weekend row only \
         once something meets there. Handy for dragging a course of your own onto a \
         Saturday that doesn't exist yet.",
    ),
    (
        "The week grid",
        "Show the how-to hints under the grids",
        "The one-line instructions, like how ✎ Edit layout drags work. Untick once you \
         know the app — every control they describe stays.",
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
        "Vivid chip colours",
        "Turns the programme shades up a notch, for screens or eyes that wash them out. \
         Colour chips by programme unticked still wins — plain means plain.",
    ),
    (
        "Colour and motion",
        "Stronger lines and small print",
        "Darker grid lines and darker fine print, in both themes — for screens where \
         the hairlines fade, or eyes that wish they wouldn't.",
    ),
    (
        "Colour and motion",
        "Animate panels and toasts",
        "Untick for the same stillness your device's reduce-motion setting asks for, \
         without needing it set.",
    ),
    (
        "Opening the app",
        "Open the app on",
        "The section I left My timetable My courses Master grid Catalog Halls Which \
         section a fresh visit lands on. The app ships opening wherever you last were \
         — a share link's courses arrive either way.",
    ),
    (
        "Opening the app",
        "Remember the day pickers between visits",
        "My timetable's day strip and the Halls day reopen where you left them. Untick \
         and every visit opens on today — a pick still holds until you close the tab.",
    ),
    (
        "Notices and dialogs",
        "Notices stay for",
        "3 6 12 seconds Until dismissed Every notice waits while you hover, focus or \
         hold it, and every one has its own ✕. 6 seconds is how the app ships. toast",
    ),
    (
        "Notices and dialogs",
        "Close a dialog by clicking the dark area",
        "Untick and only Close, Escape or the browser's own ways leave a dialog — a \
         stray click beside it does nothing. A half-written form still asks before \
         being thrown away.",
    ),
    (
        "Wheel and swipe",
        "Step values with the wheel",
        "Scroll over credits, a time, a date or a dropdown and it moves one step. \
         Untick and the wheel only ever scrolls — typing and the arrows still change \
         every value.",
    ),
    (
        "Wheel and swipe",
        "Switch sections with the wheel or a swipe on the bar",
        "A wheel notch or a drag along the section bar steps one section. Untick if it \
         keeps happening while you scroll — taps and the arrow keys still work.",
    ),
    (
        "Editing and undo",
        "Keep drags behind ✎ Edit layout",
        "Untick and a mouse or pen can drag a chip any time, no toggle first. A finger \
         still needs the toggle, so scrolling a phone never moves a class — and every \
         move stays undoable and listed under Your changes.",
    ),
    (
        "Editing and undo",
        "Undo history depth",
        "How many steps Ctrl+Z can walk back — 100 unless you say otherwise. Each step \
         keeps a copy of your selection in memory, never in storage, so a very deep \
         history costs RAM.",
    ),
    (
        "Syncing",
        "Check CMI on its own",
        "Twice a day Every hour Only when I ask How often the app fetches CMI's pages \
         without being asked. The header keeps counting how old the timetable is \
         whichever you pick — and a browser that has never synced still fetches its \
         first timetable.",
    ),
    (
        "Syncing",
        "Ask the public helper sites when syncing",
        "Untick and no public relay is ever shown which CMI page you read — only your \
         own helper site (set in My data) and CMI itself are asked. With Ask CMI \
         directly also off and no helper site set, pasting CMI's page is the way left, \
         and a failed sync says so. privacy",
    ),
    (
        "Syncing",
        "Ask CMI directly when every helper site fails",
        "The last route of a sync is cmi.ac.in itself — on CMI's own network that is \
         what raises the browser's local-network question. Untick and the app never \
         contacts CMI's address from your browser, and a failed sync says this route \
         was switched off here instead of pretending it was tried.",
    ),
    (
        "Syncing",
        "Turn the synced pill amber after",
        "days How many days old the timetable gets before the header's pill wears its \
         warning colour — 2 unless you say otherwise. The age itself is always written \
         out and keeps counting; this only moves where the colour starts worrying. \
         stale",
    ),
    (
        "Printing",
        "Print in colour",
        "Untick for plain-ink sheets: white header bands, grey-bordered chips, black \
         rules — kinder to toner and photocopiers. ⚠, ✎ and ✓ already say everything \
         in black and white, and clash red stays: it is a warning, not decoration.",
    ),
    (
        "Printing",
        "Page shape",
        "Wide Tall Let the browser ask The sheets are designed wide, like the week is. \
         Tall suits binders and clipboards; Let the browser ask puts the choice back \
         in the print dialog. Only the app's own Print buttons obey — the browser's \
         Ctrl+P keeps the wide design. portrait landscape",
    ),
    (
        "Printing",
        "Sign each sheet made with the CMI Timetable Planner",
        "The credit at the end of every printed sheet's stats line. Untick for an \
         unsigned sheet — the semester, the sync date and the check-against-CMI line \
         stay, because those are facts about the timetable, not the app.",
    ),
    (
        "Calendar files",
        "Put a link back to this planner in every calendar event",
        "Each event's notes end with a link that reopens this timetable. Untick to \
         keep the file to class facts — the link also spells out which courses you \
         take, which matters if you send the file to someone. ics privacy",
    ),
    (
        "Calendar files",
        "Describe the course inside each calendar event",
        "The instructor and branch lines in every event's notes. Untick to keep events \
         to title, room and time — some calendar apps read the notes aloud on every \
         reminder. ics",
    ),
    (
        "Developer mode",
        "Show a Developer button in the header",
        "A one-press door to this mode, beside the theme button. The door in My data \
         stays either way.",
    ),
    (
        "Developer mode",
        "Echo every fetch to the browser console",
        "Each sync request also prints one line in your browser's DevTools — route, \
         status, milliseconds, bytes — so a bug report can carry the console. The \
         Sync page's log shows the same rows either way. debug",
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
