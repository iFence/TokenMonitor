//! Report content: a usage summary panel plus a native contribution-style
//! heatmap of the last 365 days. Rendered as a section of the dashboard page.

pub mod section;

mod heatmap;

pub use heatmap::ContributionHeatmap;
