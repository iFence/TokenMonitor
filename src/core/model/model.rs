use serde::{Deserialize, Serialize};

use super::provider::Provider;

/// A model identifier known to a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub provider: Provider,
    pub id: String,
    pub display_name: String,
}
