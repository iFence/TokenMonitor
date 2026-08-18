//! TokenMonitor — token-usage tracking desktop app for AI coding tools.
//!
//! Reads each AI coding tool's local usage data, persists it to SQLite,
//! and presents aggregated token/cost statistics in a GPUI desktop UI.

// mimalloc returns freed memory to the OS (Windows system malloc tends to keep
// it resident), so RSS drops after scans and view refreshes instead of parking
// at the allocator's high-water mark.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub mod app;
pub mod collector;
pub mod core;
pub mod platform;
pub mod providers;
pub mod storage;
pub mod ui;

pub use core::{aggregation, model, pricing, quota, usage};
