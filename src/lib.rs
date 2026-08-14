//! rToken — token-usage tracking desktop app for AI coding tools.
//!
//! Reads each AI coding tool's local usage data, persists it to SQLite,
//! and presents aggregated token/cost statistics in a GPUI desktop UI.

pub mod app;
pub mod collector;
pub mod core;
pub mod platform;
pub mod providers;
pub mod storage;
pub mod ui;

pub use core::{aggregation, model, pricing, quota, usage};
