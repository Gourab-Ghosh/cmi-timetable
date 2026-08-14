//! CMI Timetable Planner — 100% client-side Leptos (CSR) app.

mod app;
mod dev;
mod dnd;
mod domx;
mod export;
mod fetch;
mod hues;
mod state;
mod storage;
mod ui;
mod views;

pub use app::apply_theme;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::Root);
}
