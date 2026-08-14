//! Display formatting helpers (presentation conventions, not domain logic).

/// Compact token count: `>=1e8` → "3.5亿", `>=1e6` → "9.1M", `>=1e3` → "641K",
/// otherwise the raw number.
pub fn format_tokens_compact(v: u64) -> String {
    if v >= 100_000_000 {
        format!("{:.1}亿", v as f64 / 1e8)
    } else if v >= 1_000_000 {
        format!("{:.1}M", v as f64 / 1e6)
    } else if v >= 1_000 {
        format!("{:.0}K", v as f64 / 1e3)
    } else {
        v.to_string()
    }
}

/// Cost in USD micros → "$12.34".
pub fn format_cost_usd(micros: u64) -> String {
    format!("${:.2}", micros as f64 / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_formats_by_magnitude() {
        assert_eq!(format_tokens_compact(0), "0");
        assert_eq!(format_tokens_compact(999), "999");
        assert_eq!(format_tokens_compact(641_000), "641K");
        assert_eq!(format_tokens_compact(9_100_000), "9.1M");
        assert_eq!(format_tokens_compact(350_000_000), "3.5亿");
    }

    #[test]
    fn cost_formats_usd() {
        assert_eq!(format_cost_usd(0), "$0.00");
        assert_eq!(format_cost_usd(317_490_000), "$317.49");
    }
}
