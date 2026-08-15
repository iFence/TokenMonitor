use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::core::aggregation::SumStats;
use crate::core::model::{Period, Provider, ProviderSelection, TimeWindow};

/// The active navigation page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivePage {
    #[default]
    Dashboard,
    Project,
    Settings,
    Charts,
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
    pub provider_selection: ProviderSelection,
    pub scan_status: ScanStatus,
    pub last_error: Option<String>,
    pub summary: Option<SumStats>,
    pub by_provider: Vec<(Provider, SumStats)>,
    pub by_provider_model: BTreeMap<Provider, Vec<(String, SumStats)>>,
    pub by_project: Vec<(String, SumStats)>,
    pub by_day: Vec<(String, SumStats)>,
    pub charts: ChartsState,
}

/// Charts page time range (a UI concept, not persisted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartRange {
    #[default]
    Last7,
    Last30,
    Last90,
    ThisYear,
}

impl ChartRange {
    pub const ALL: [ChartRange; 4] = [Self::Last7, Self::Last30, Self::Last90, Self::ThisYear];

    pub fn label(self) -> &'static str {
        match self {
            Self::Last7 => "近7天",
            Self::Last30 => "近30天",
            Self::Last90 => "近90天",
            Self::ThisYear => "本年",
        }
    }

    /// Window for this range, measured in East-8 calendar days.
    pub fn window(self, now: DateTime<Utc>) -> TimeWindow {
        match self {
            Self::Last7 => TimeWindow::last_n_days(7, now),
            Self::Last30 => TimeWindow::last_n_days(30, now),
            Self::Last90 => TimeWindow::last_n_days(90, now),
            Self::ThisYear => TimeWindow::current_year(now),
        }
    }
}

/// Metric shown on the charts page (mapped to `f64` at render time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartMetric {
    #[default]
    TotalTokens,
    OutputTokens,
    Cost,
}

impl ChartMetric {
    pub const ALL: [ChartMetric; 3] = [Self::TotalTokens, Self::OutputTokens, Self::Cost];

    pub fn label(self) -> &'static str {
        match self {
            Self::TotalTokens => "总Token",
            Self::OutputTokens => "输出Token",
            Self::Cost => "花费",
        }
    }
}

/// Chart rendering mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartKind {
    #[default]
    Line,
    Bar,
}

impl ChartKind {
    pub const ALL: [ChartKind; 2] = [Self::Line, Self::Bar];

    pub fn label(self) -> &'static str {
        match self {
            Self::Line => "折线图",
            Self::Bar => "柱状图",
        }
    }
}

/// Raw per-day series for the charts page. Kept as `SumStats` so metric/kind
/// changes are render-time transforms rather than DB re-queries.
#[derive(Debug, Clone, Default)]
pub struct ChartsSnapshot {
    /// Per-provider daily series, in selection order.
    pub provider_series: Vec<(Provider, Vec<(String, SumStats)>)>,
    /// Per-model daily series for the selected provider.
    pub model_series: BTreeMap<String, Vec<(String, SumStats)>>,
}

/// Charts page control + loaded data state.
#[derive(Debug, Clone, Default)]
pub struct ChartsState {
    pub range: ChartRange,
    pub metric: ChartMetric,
    pub kind: ChartKind,
    /// Provider shown in the per-model chart.
    pub provider: Provider,
    pub data: Option<ChartsSnapshot>,
}

/// A complete set of aggregate view data, computed on a background thread and
/// applied to app state on the main thread. All fields are owned and `Send`.
#[derive(Debug, Default)]
pub struct ViewSnapshot {
    /// Monotonic request sequence; stale results are dropped on apply.
    pub seq: u64,
    pub time_tab: TimeTab,
    pub summary: Option<SumStats>,
    pub by_provider: Vec<(Provider, SumStats)>,
    pub by_provider_model: BTreeMap<Provider, Vec<(String, SumStats)>>,
    pub by_project: Vec<(String, SumStats)>,
    pub by_day: Vec<(String, SumStats)>,
    pub charts: Option<ChartsSnapshot>,
    pub error: Option<String>,
}
