//! CMI Timetable Planner — core: data model, parsers for the two CMI pages,
//! validation gate, snapshot diff, three-way merge, .ics generation and URL
//! state codecs. No wasm-only dependencies; unit-tested against committed
//! HTML fixtures.

pub mod date;
pub mod diff;
pub mod export;
#[cfg(feature = "html")]
pub mod extract;
pub mod ics;
pub mod join;
pub mod merge;
pub mod model;
pub mod parse;
pub mod rawhtml;
pub mod share;
pub mod textgrid;
pub mod validate;

pub use model::PARSER_VERSION;
