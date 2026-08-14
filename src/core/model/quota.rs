use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::provider::Provider;

/// Quota reset period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Period {
    Day,
    Week,
    Month,
    All,
}

impl Period {
    /// Stable lowercase identifier, used as the DB `period` column.
    pub fn id(self) -> &'static str {
        match self {
            Period::Day => "day",
            Period::Week => "week",
            Period::Month => "month",
            Period::All => "all",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "day" => Some(Period::Day),
            "week" => Some(Period::Week),
            "month" => Some(Period::Month),
            "all" => Some(Period::All),
            _ => None,
        }
    }
}

/// A usage quota (token and/or cost limit) for a provider over a period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quota {
    pub provider: Provider,
    pub period: Period,
    /// Token limit; 0 = unlimited.
    pub limit_tokens: u64,
    pub used_tokens: u64,
    /// Cost limit in USD micros; 0 = unlimited.
    pub limit_cost_micros: u64,
    pub used_cost_micros: u64,
    pub updated_at: DateTime<Utc>,
}

impl Quota {
    pub fn remaining_tokens(&self) -> Option<u64> {
        if self.limit_tokens == 0 {
            None
        } else {
            Some(self.limit_tokens.saturating_sub(self.used_tokens))
        }
    }

    /// Percentage of the token limit used; `None` when unlimited.
    pub fn percent_used(&self) -> Option<f64> {
        if self.limit_tokens == 0 {
            None
        } else {
            Some(self.used_tokens as f64 / self.limit_tokens as f64 * 100.0)
        }
    }
}
