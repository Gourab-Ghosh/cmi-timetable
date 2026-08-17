//! CMI Timetable Planner — core: data model, parsers for the two CMI pages,
//! validation gate, snapshot diff, three-way merge, combining one student's
//! timetable file with another's, .ics generation and URL state codecs. No
//! wasm-only dependencies; unit-tested against committed HTML fixtures.

pub mod combine;
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
pub mod search;
pub mod share;
pub mod shorten;
pub mod textgrid;
pub mod update;
pub mod validate;

pub use model::PARSER_VERSION;
