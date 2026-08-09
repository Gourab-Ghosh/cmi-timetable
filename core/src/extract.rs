//! Native `<pre>` extraction with scraper — used by /sync, /app's build
//! script and this crate's tests. The wasm app extracts with the browser's
//! DOMParser instead (smaller binary, maximally tolerant of CMI's HTML) and
//! feeds the same `PreBlock` list to the same parsing functions.

use crate::model::SourceTier;
use crate::parse::PreBlock;
use crate::validate::{ParseOutcome, SnapshotMeta, parse_and_validate};
use scraper::Html;
use scraper::node::Node;

/// Every `<pre>` block's raw text in document order, each paired with the
/// nearest preceding heading (h1–h6) text. Whitespace inside `<pre>` is
/// preserved exactly; nested tags (CMI wraps grid rows in `<b>`, day groups
/// in `<div>`/`<a>`) contribute their text content in order.
pub fn extract_pre_blocks(html: &str) -> Vec<PreBlock> {
    let doc = Html::parse_document(html);
    let mut blocks = Vec::new();
    let mut last_heading = String::new();

    for node in doc.tree.root().descendants() {
        if let Node::Element(el) = node.value() {
            match el.name() {
                "pre" => {
                    let text: String = node
                        .descendants()
                        .filter_map(|n| match n.value() {
                            Node::Text(t) => Some(&**t),
                            _ => None,
                        })
                        .collect();
                    blocks.push(PreBlock::new(text, last_heading.clone()));
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let text: String = node
                        .descendants()
                        .filter_map(|n| match n.value() {
                            Node::Text(t) => Some(&**t),
                            _ => None,
                        })
                        .collect();
                    last_heading = text.trim().to_string();
                }
                _ => {}
            }
        }
    }
    blocks
}

/// Convenience: extract + parse + gate both pages from raw HTML.
pub fn parse_html_pages(
    timetable_html: &str,
    lecturehalls_html: &str,
    fetched_at: f64,
    source: SourceTier,
    store_raw: bool,
) -> ParseOutcome {
    let tt_blocks = extract_pre_blocks(timetable_html);
    let hall_blocks = extract_pre_blocks(lecturehalls_html);
    parse_and_validate(
        &tt_blocks,
        &hall_blocks,
        SnapshotMeta {
            fetched_at,
            source,
            raw_html: store_raw
                .then(|| (timetable_html.to_string(), lecturehalls_html.to_string())),
        },
    )
}
