use crate::core::model::Provider;

/// The dimension a group of usage stats is aggregated by.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AggKey {
    Provider(Provider),
    Project(String),
    Day(String),
    Model(String),
    Session(String),
}
