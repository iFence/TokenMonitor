//! Report page: a usage summary panel plus a native contribution-style
//! heatmap of the last 365 days.

pub mod page;

mod heatmap;
mod stats;

pub use heatmap::ContributionHeatmap;
pub use stats::{report_stats, ReportStats};
