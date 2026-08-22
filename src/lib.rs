//! TokenMonitor — token-usage tracking app for AI coding tools.
//!
//! Reads each AI coding tool's local usage data, persists it to SQLite,
//! and presents aggregated token/cost statistics. Two frontends:
//! - `app` + `ui` (feature `ui`, default): the GPUI desktop app.
//! - `tui` (feature `tui`): a ratatui terminal frontend for servers and
//!   machines without a display (`--no-default-features --features tui`).

// mimalloc returns freed memory to the OS (Windows system malloc tends to keep
// it resident), so RSS drops after scans and view refreshes instead of parking
// at the allocator's high-water mark.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "ui")]
pub mod app;
pub mod cli;
pub mod collector;
pub mod core;
pub mod format;
pub mod platform;
pub mod providers;
pub mod report;
pub mod storage;
#[cfg(feature = "tui")]
pub mod tui;
#[cfg(feature = "ui")]
pub mod ui;

pub use core::{aggregation, model, pricing, quota, usage};
