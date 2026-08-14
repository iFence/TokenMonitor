use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::core::aggregation::SumStats;
use crate::core::model::{Period, Provider, TimeWindow};

/// The active navigation page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivePage {
    #[default]
    Dashboard,
    Project,
    Settings,
}

/// Dashboard time-range tab (a UI concept; not persisted, decoupled from the
/// quota-domain `Period`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeTab {
    #[default]
    Today,
    Yesterday,
    ThisWeek,
    LastWeek,
    ThisMonth,
    ThisYear,
}

impl TimeTab {
    pub const ALL: [TimeTab; 6] = [
        Self::Today,
        Self::Yesterday,
        Self::ThisWeek,
        Self::LastWeek,
        Self::ThisMonth,
        Self::ThisYear,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "今日",
            Self::Yesterday => "昨日",
            Self::ThisWeek => "本周",
            Self::LastWeek => "上周",
            Self::ThisMonth => "本月",
            Self::ThisYear => "本年",
        }
    }

    pub fn window(self, now: DateTime<Utc>) -> TimeWindow {
        match self {
            Self::Today => TimeWindow::current(Period::Day, now),
            Self::Yesterday => TimeWindow::previous(Period::Day, now),
            Self::ThisWeek => TimeWindow::current(Period::Week, now),
            Self::LastWeek => TimeWindow::previous(Period::Week, now),
            Self::ThisMonth => TimeWindow::current(Period::Month, now),
            Self::ThisYear => TimeWindow::current_year(now),
        }
    }
}

/// Status of the latest scan.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ScanStatus {
    #[default]
    Idle,
    Scanning {
        completed: u32,
        total: u32,
    },
    Done {
        records: u64,
        at: DateTime<Utc>,
    },
    Failed {
        error: String,
    },
}

/// App-wide UI state.
#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub active_page: ActivePage,
    pub time_tab: TimeTab,
    pub expanded_provider: Option<Provider>,
    pub selected_project: Option<String>,
    pub scan_status: ScanStatus,
    pub last_error: Option<String>,
    pub summary: Option<SumStats>,
    pub by_provider: Vec<(Provider, SumStats)>,
    pub by_provider_model: BTreeMap<Provider, Vec<(String, SumStats)>>,
    pub by_project: Vec<(String, SumStats)>,
    pub by_day: Vec<(String, SumStats)>,
}
