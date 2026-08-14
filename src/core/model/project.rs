use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::provider::Provider;

/// A code project that consumed tokens, across one or more providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub path: PathBuf,
    pub providers: Vec<Provider>,
}
