use serde::{Deserialize, Serialize};

use crate::core::model::{Provider, Usage};

/// A normalized usage record extracted from a provider's raw data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub provider: Provider,
    /// Display name of the project the usage belongs to.
    pub project: String,
    pub session_id: String,
    pub usage: Usage,
    /// Bytes of the raw source line this record was parsed from.
    pub raw_bytes: u64,
    /// Dedup key, e.g. "<relative path>:<line index>" — set by the provider adapter.
    pub fingerprint: String,
}

impl UsageRecord {
    pub fn new(
        provider: Provider,
        project: String,
        session_id: String,
        usage: Usage,
        raw_bytes: u64,
        fingerprint: String,
    ) -> Self {
        UsageRecord {
            provider,
            project,
            session_id,
            usage,
            raw_bytes,
            fingerprint,
        }
    }
}
