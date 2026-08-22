//! Report computation shared by the GPUI and TUI frontends: pure summary
//! statistics, contribution-grid geometry, and the 365-day data loader. This
//! layer has no rendering dependency, so both frontends feed on the same data.

pub mod data;
pub mod heatmap;
pub mod stats;

pub use stats::{report_stats, ReportStats};
