//! Developer mode (`#/developer`). No auth — all data here is public.

use crate::state::{App, BannerKind};
use crate::{domx, fetch, storage};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

pub fn developer(app: App) -> impl IntoView {
    view! {
        <section aria-label="Developer mode">
            <div class="toolbar">
                <h2 style="margin:0">"Developer mode"</h2>
                <div class="grow"></div>
                <button class="btn" on:click=move |_| app.goto_planner()>
                    "← Back to the planner"
                </button>
            </div>
            {build_info(app)}
            {simulators(app)}
            {fetch_log(app)}
            {parse_reports(app)}
            {cache_inspector(app)}
            {raw_html_viewer(app)}
        </section>
    }
}

fn build_info(app: App) -> impl IntoView {
    view! {
        <div class="panel">
            <h3>"Build info"</h3>
            <dl class="kv mono small">
                <dt>"App version"</dt>
                <dd>{crate::state::APP_VERSION}</dd>
                <dt>"PARSER_VERSION"</dt>
                <dd>{ttcore::PARSER_VERSION.to_string()}</dd>
                <dt>"Git commit"</dt>
                <dd>{crate::state::GIT_COMMIT}</dd>
                <dt>"Built at"</dt>
                <dd>{crate::state::BUILD_TIME}</dd>
                <dt>"Bundled snapshot"</dt>
                <dd>{env!("APP_BUNDLED_SEMESTER")}</dd>
                <dt>"Current snapshot"</dt>
                <dd>
                    {move || {
                        app.snapshot
                            .with(|s| {
                                format!(
                                    "{} · fetched {} · parser v{} · {}",
                                    s.semester_label,
                                    domx::fmt_local(s.fetched_at),
                                    s.parser_version,
                                    s.source.label(),
                                )
                            })
                    }}
                </dd>
            </dl>
        </div>
    }
}

fn simulators(app: App) -> impl IntoView {
    view! {
        <div class="panel">
            <h3>"Simulators"</h3>
            <div class="row" style="display:flex;gap:0.5rem;flex-wrap:wrap;align-items:center">
                <label for="force-tier" class="muted small">"Force tier on next update"</label>
                <select
                    id="force-tier"
                    on:change=move |ev| {
                        let v = event_target_value(&ev);
                        app.force_tier.set((!v.is_empty()).then_some(v));
                    }
                >
                    <option value="">"(all tiers, normal order)"</option>
                    <option value="direct">"direct only"</option>
                    <option value="proxy">"proxy only"</option>
                    <option value="mirror">"mirror only"</option>
                </select>
                <button class="btn small" on:click=move |_| {
                    leptos::task::spawn_local(async move {
                        fetch::run_update(app, true).await;
                    });
                }>
                    "Run update"
                </button>
                <button class="btn small" on:click=move |_| fetch::load_bundled_fixture(app)>
                    "Load bundled fixture"
                </button>
                <button class="btn small" on:click=move |_| fetch::simulate_parse_failure(app)>
                    "Simulate parse failure"
                </button>
            </div>
            <p class="muted small">
                "“Simulate parse failure” runs mangled pages through the full pipeline to \
                 demonstrate that the validation gate keeps the cached data untouched."
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
                            "No parses yet this session (the bundled snapshot was parsed at build time)."
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

fn cache_inspector(app: App) -> impl IntoView {
    let bump = RwSignal::new(0u32);
    view! {
        <div class="panel">
            <h3>"Cache inspector"</h3>
            <p class="muted small">
                "Everything the app keeps in your browser. Import replaces a key and reloads."
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
                                    <span class="mono">{key.clone()}</span>
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
                                    <label class="btn small" for=input_id_for.clone()>
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
                                                {
                                                    if let Some(text) = text.as_string() {
                                                        let _ = storage::set_raw(&key, &text);
                                                        let _ = domx::window().location().reload();
                                                    }
                                                }
                                            });
                                        }
                                    />
                                    <button
                                        class="btn small danger"
                                        on:click=move |_| {
                                            let confirmed = domx::window()
                                                .confirm_with_message(
                                                    &format!("Clear {clear_key}? A backup is not kept."),
                                                )
                                                .unwrap_or(false);
                                            if confirmed {
                                                storage::remove(&clear_key);
                                                bump.update(|b| *b += 1);
                                                app.toast("Cleared.");
                                            }
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
    // startup. First two sentences are the exact §2.4 copy.
    app.set_banner_sticky(
        BannerKind::Warn,
        "Your saved data couldn't be read, so it was set aside and the built-in \
         timetable loaded. Nothing was deleted. The unreadable copy is kept under \
         a cmitt.corrupt.* key — see the cache inspector in developer mode.",
    );
}
