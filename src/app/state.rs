use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};

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

/// Settings page group (left-hand navigation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsGroup {
    #[default]
    Applications,
    About,
}

impl SettingsGroup {
    pub const ALL: [SettingsGroup; 2] = [Self::Applications, Self::About];

    pub fn label(self) -> &'static str {
        match self {
            Self::Applications => "应用",
            Self::About => "关于",
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
    pub settings_group: SettingsGroup,
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

/// Charts page time range (a UI concept, not persisted). `Custom` is the
/// sentinel that opens the date-range picker; the chosen dates are stored in
/// [`ChartsState::custom_range`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartRange {
    #[default]
    Last7,
    Last30,
    ThisYear,
    Custom,
}

impl ChartRange {
    pub const ALL: [ChartRange; 4] = [Self::Last7, Self::Last30, Self::ThisYear, Self::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Self::Last7 => "近7天",
            Self::Last30 => "近30天",
            Self::ThisYear => "本年",
            Self::Custom => "自定义",
        }
    }

    /// Window for this range, measured in East-8 calendar days. `Custom` has
    /// no fixed window of its own; it is overridden by `custom_range`.
    pub fn window(self, now: DateTime<Utc>) -> TimeWindow {
        match self {
            Self::Last7 => TimeWindow::last_n_days(7, now),
            Self::Last30 => TimeWindow::last_n_days(30, now),
            Self::ThisYear => TimeWindow::current_year(now),
            Self::Custom => TimeWindow::last_n_days(7, now),
        }
    }
}

/// A time-range dropdown item: a stable [`ChartRange`] value paired with a
/// display title that can reflect the selected custom dates.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartRangeItem {
    pub range: ChartRange,
    pub title: String,
}

impl ChartRangeItem {
    /// The four range options with their default (preset) titles.
    pub fn all() -> Vec<Self> {
        ChartRange::ALL
            .iter()
            .map(|&range| Self {
                range,
                title: range.label().to_string(),
            })
            .collect()
    }
}

/// Metric shown on the charts page (mapped to `f64` at render time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartMetric {
    #[default]
    TotalTokens,
    OutputTokens,
    InputTokens,
    CacheRead,
    CacheHitRate,
    Cost,
}

impl ChartMetric {
    pub const ALL: [ChartMetric; 6] = [
        Self::TotalTokens,
        Self::OutputTokens,
        Self::InputTokens,
        Self::CacheRead,
        Self::CacheHitRate,
        Self::Cost,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::TotalTokens => "总Token",
            Self::OutputTokens => "输出Token",
            Self::InputTokens => "输入Token",
            Self::CacheRead => "缓存读",
            Self::CacheHitRate => "缓存命中率",
            Self::Cost => "花费",
        }
    }
}

/// App filter for the per-model chart: every enabled app, or a single app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartApp {
    #[default]
    All,
    One(Provider),
}

impl ChartApp {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::One(provider) => provider.display_name(),
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
    /// App filter for the per-model chart (all apps, or a single app).
    pub app: ChartApp,
    /// Custom date range (East-8, both endpoints inclusive) overriding `range`.
    pub custom_range: Option<(NaiveDate, NaiveDate)>,
    pub data: Option<ChartsSnapshot>,
}

impl ChartsState {
    /// Time window for the current chart selection; `custom_range` wins over
    /// the preset `range` when set.
    pub fn window(&self, now: DateTime<Utc>) -> TimeWindow {
        if let Some((start, end)) = self.custom_range {
            TimeWindow::custom(start, end)
        } else {
            self.range.window(now)
        }
    }

    /// Dropdown options for the time-range selector. The `Custom` item's title
    /// shows the selected dates once a custom range is chosen, so the trigger
    /// reflects the active window.
    pub fn range_options(&self) -> Vec<ChartRangeItem> {
        ChartRange::ALL
            .iter()
            .map(|&range| ChartRangeItem {
                range,
                title: if range == ChartRange::Custom {
                    match self.custom_range {
                        Some((start, end)) => format!("{} ~ {}", start, end),
                        None => range.label().to_string(),
                    }
                } else {
                    range.label().to_string()
                },
            })
            .collect()
    }
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
