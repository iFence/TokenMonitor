//! TUI application state: owns the collector + scheduler and the loaded report,
//! translates key events and collector events into state changes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{Datelike, Duration, NaiveDate, Utc};
use semver::Version;

use crate::collector::{Collector, CollectorEvent};
use crate::core::aggregation::SumStats;
use crate::core::model::{Period, Provider, TimeWindow};
use crate::core::time::east8_local;
use crate::core::update::UpdateState;
use crate::report::heatmap::{grid_start, week_count, ROWS};
use crate::tui::update::{self, UpdateEvent};

/// Result of handling one key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    None,
}

/// Selectable time range for the summary cards and the agent/model breakdown.
/// The heatmap stays a fixed 365-day overview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TuiRange {
    #[default]
    Year365,
    Today,
    Yesterday,
    ThisWeek,
    LastWeek,
    ThisMonth,
    ThisYear,
}

impl TuiRange {
    pub const ALL: [TuiRange; 7] = [
        Self::Year365,
        Self::Today,
        Self::Yesterday,
        Self::ThisWeek,
        Self::LastWeek,
        Self::ThisMonth,
        Self::ThisYear,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Year365 => "近365天",
            Self::Today => "今日",
            Self::Yesterday => "昨日",
            Self::ThisWeek => "本周",
            Self::LastWeek => "上周",
            Self::ThisMonth => "本月",
            Self::ThisYear => "本年",
        }
    }

    pub fn window(self, now: chrono::DateTime<Utc>) -> TimeWindow {
        match self {
            Self::Year365 => TimeWindow::last_n_days(365, now),
            Self::Today => TimeWindow::current(Period::Day, now),
            Self::Yesterday => TimeWindow::previous(Period::Day, now),
            Self::ThisWeek => TimeWindow::current(Period::Week, now),
            Self::LastWeek => TimeWindow::previous(Period::Week, now),
            Self::ThisMonth => TimeWindow::current(Period::Month, now),
            Self::ThisYear => TimeWindow::current_year(now),
        }
    }

    /// The next/previous range in cycling order (`dir < 0` goes backwards).
    pub fn step(self, dir: isize) -> Self {
        let ix = Self::ALL.iter().position(|r| *r == self).unwrap_or(0);
        let n = Self::ALL.len() as isize;
        Self::ALL[((ix as isize + dir).rem_euclid(n)) as usize]
    }
}

/// Selectable full-screen panel. `Tab` cycles between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TuiView {
    #[default]
    Overview,
    TodayHourly,
    Updates,
}

impl TuiView {
    pub const ALL: [TuiView; 3] = [Self::Overview, Self::TodayHourly, Self::Updates];

    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "总览",
            Self::TodayHourly => "当日分时",
            Self::Updates => "更新检查",
        }
    }

    /// The next/previous view in cycling order (`dir < 0` goes backwards).
    pub fn step(self, dir: isize) -> Self {
        let ix = Self::ALL.iter().position(|v| *v == self).unwrap_or(0);
        let n = Self::ALL.len() as isize;
        Self::ALL[((ix as isize + dir).rem_euclid(n)) as usize]
    }
}

/// Interactive TUI state: the loaded 365-day report plus grid selection.
pub struct TuiApp {
    db_path: PathBuf,
    collector: Arc<Collector>,
    /// East-8 calendar days with recorded usage, chronological ascending.
    days: Vec<(NaiveDate, SumStats)>,
    today: NaiveDate,
    /// First grid column (the Sunday on or before 365 days back).
    start: NaiveDate,
    /// Number of grid columns (weeks).
    weeks: usize,
    /// Selected grid cell `(week, row)`, row 0 = Sunday.
    selection: (usize, usize),
    /// Per-provider ("agent") totals over the report window, cost-descending.
    by_provider: Vec<(Provider, SumStats)>,
    /// Per-model totals over the report window, cost-descending.
    by_model: Vec<(String, SumStats)>,
    /// Selected time range for the summary cards and the breakdown sections.
    range: TuiRange,
    /// East-8 days inside the selected range, for the range summary stats.
    range_days: Vec<(NaiveDate, SumStats)>,
    /// Per-hour aggregates of today (index 0..24, East-8), for the per-hour
    /// chart and the "today hourly" panel.
    hour_tokens: [SumStats; 24],
    /// Full-screen panel currently shown.
    view: TuiView,
    /// One-line status shown in the header (scan progress / last result).
    pub status: String,
    /// Short summary of the last completed scan.
    pub last_scan: Option<String>,
    /// Auto-update state (check / download result machine).
    update: UpdateState,
    /// Update results are delivered over this channel from background threads.
    update_rx: async_channel::Receiver<UpdateEvent>,
    update_tx: async_channel::Sender<UpdateEvent>,
    /// Where the current update asset was / will be saved.
    update_dest: Option<PathBuf>,
    /// Version the user chose to skip, read from settings at startup.
    skipped_update_version: Option<String>,
    /// Whether to check for updates on startup (defaults to `true`).
    check_updates_on_startup: bool,
}

impl TuiApp {
    pub fn new(collector: Arc<Collector>) -> Self {
        let db_path = collector.db_path().to_path_buf();
        let skipped_update_version = collector.skipped_update_version();
        let check_updates_on_startup = collector.check_updates_on_startup();
        let (update_tx, update_rx) = async_channel::unbounded();
        let mut app = TuiApp {
            db_path,
            collector,
            days: Vec::new(),
            today: Utc::now().date_naive(),
            start: Utc::now().date_naive(),
            weeks: 0,
            selection: (0, 0),
            by_provider: Vec::new(),
            by_model: Vec::new(),
            range: TuiRange::default(),
            range_days: Vec::new(),
            hour_tokens: std::array::from_fn(|_| SumStats::default()),
            view: TuiView::default(),
            status: "等待扫描…".to_string(),
            last_scan: None,
            update: UpdateState::default(),
            update_rx,
            update_tx,
            update_dest: None,
            skipped_update_version,
            check_updates_on_startup,
        };
        let _ = app.reload();
        // Start the cursor on the most recent day (today) instead of the
        // grid's first cell.
        app.selection = (
            app.weeks.saturating_sub(1),
            app.today.weekday().num_days_from_sunday() as usize,
        );
        app
    }

    pub fn collector(&self) -> &Arc<Collector> {
        &self.collector
    }

    /// Re-read the 365-day heatmap series and the selected range's aggregates
    /// from the database (read-only WAL connection, so it never blocks a
    /// running scan).
    pub fn reload(&mut self) -> Result<()> {
        let conn = crate::storage::sqlite::open_read(&self.db_path)?;
        let now = Utc::now();
        let window = TimeWindow::last_n_days(365, now);
        self.days = crate::report::data::load_report_days(&conn, &window)?;
        self.today = east8_local(now).date_naive();
        self.start = grid_start(self.today);
        self.weeks = week_count(self.start, self.today);
        self.selection.0 = self.selection.0.min(self.weeks.saturating_sub(1));
        self.hour_tokens = crate::report::data::load_report_hours(&conn, self.today)?;
        self.reload_range(&conn)?;
        Ok(())
    }

    /// Re-read the summary + agent/model aggregates for the selected range.
    fn reload_range(&mut self, conn: &rusqlite::Connection) -> Result<()> {
        let window = self.range.window(Utc::now());
        self.by_provider = crate::report::data::load_report_by_provider(conn, &window)?;
        self.by_model = crate::report::data::load_report_by_model(conn, &window)?;
        self.range_days = crate::report::data::load_report_days(conn, &window)?;
        Ok(())
    }

    /// Switch to the next/previous time range and reload its aggregates.
    pub fn cycle_range(&mut self, dir: isize) -> Result<()> {
        self.range = self.range.step(dir);
        let conn = crate::storage::sqlite::open_read(&self.db_path)?;
        self.reload_range(&conn)
    }

    pub fn today(&self) -> NaiveDate {
        self.today
    }

    pub fn start(&self) -> NaiveDate {
        self.start
    }

    pub fn weeks(&self) -> usize {
        self.weeks
    }

    pub fn selection(&self) -> (usize, usize) {
        self.selection
    }

    /// The date under the current selection.
    pub fn selected_date(&self) -> NaiveDate {
        self.start + Duration::days(self.selection.0 as i64 * 7 + self.selection.1 as i64)
    }

    pub fn day_stats(&self) -> HashMap<NaiveDate, SumStats> {
        self.days.iter().copied().collect()
    }

    pub fn by_provider(&self) -> &[(Provider, SumStats)] {
        &self.by_provider
    }

    pub fn by_model(&self) -> &[(String, SumStats)] {
        &self.by_model
    }

    pub fn range(&self) -> TuiRange {
        self.range
    }

    /// East-8 days inside the selected range (for the range summary).
    pub fn range_days(&self) -> &[(NaiveDate, SumStats)] {
        &self.range_days
    }

    /// Per-hour aggregates of today (index 0..24, East-8).
    pub fn hour_tokens(&self) -> &[SumStats; 24] {
        &self.hour_tokens
    }

    pub fn view(&self) -> TuiView {
        self.view
    }

    /// Cycle to the next/previous full-screen panel.
    pub fn cycle_view(&mut self, dir: isize) {
        self.view = self.view.step(dir);
    }

    /// The auto-update state machine.
    pub fn update_state(&self) -> &UpdateState {
        &self.update
    }

    /// Where the last update download landed (if any).
    pub fn update_dest(&self) -> Option<&std::path::Path> {
        self.update_dest.as_deref()
    }

    /// Whether to run a silent update check on startup.
    pub fn check_updates_on_startup(&self) -> bool {
        self.check_updates_on_startup
    }

    /// Start a check on a background thread. `manual` controls whether a
    /// failure is surfaced (`Error`) or swallowed (`Idle`).
    pub fn check_updates(&mut self, manual: bool) -> Result<()> {
        if self.update.is_busy() {
            return Ok(());
        }
        self.update = UpdateState::Checking;
        let current =
            Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is valid semver");
        let portable = crate::platform::is_portable();
        let tx = self.update_tx.clone();
        std::thread::Builder::new()
            .name("tokenmonitor-update-check".into())
            .spawn(move || {
                let result = update::check_update(&current, portable);
                let _ = tx.send_blocking(UpdateEvent::Checked { manual, result });
            })?;
        Ok(())
    }

    /// Download the available asset next to the running exe (blocking on a
    /// background thread).
    pub fn download_update(&mut self) -> Result<()> {
        let (version, asset) = match &self.update {
            UpdateState::Available {
                latest_version,
                asset,
                ..
            } => (latest_version.clone(), asset.clone()),
            _ => return Ok(()),
        };
        self.update = UpdateState::Downloading {
            latest_version: version.clone(),
            downloaded_bytes: 0,
            total_bytes: (asset.size > 0).then_some(asset.size),
        };
        let dest = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
            .unwrap_or_else(std::env::temp_dir)
            .join(&asset.name);
        self.update_dest = Some(dest.clone());
        let tx = self.update_tx.clone();
        std::thread::Builder::new()
            .name("tokenmonitor-update-download".into())
            .spawn(move || {
                let result = update::download(&asset.url, &dest, asset.size).map(|()| dest);
                let _ = tx.send_blocking(UpdateEvent::Downloaded { version, result });
            })?;
        Ok(())
    }

    /// Dismiss the available update and remember the version in settings.
    pub fn skip_update(&mut self) -> Result<()> {
        if let UpdateState::Available { latest_version, .. } = &self.update {
            let version = latest_version.to_string();
            self.skipped_update_version = Some(version.clone());
            self.collector.set_skipped_update_version(Some(&version))?;
        }
        self.update = UpdateState::Idle;
        Ok(())
    }

    /// Poll the update-result channel without blocking.
    pub fn try_recv_update_event(&self) -> Option<UpdateEvent> {
        self.update_rx.try_recv().ok()
    }

    /// Apply an update check / download result.
    pub fn handle_update_event(&mut self, event: UpdateEvent) {
        match event {
            UpdateEvent::Checked { manual, result } => {
                self.update = match result {
                    Ok(Some(info)) => {
                        let skipped = self
                            .skipped_update_version
                            .as_deref()
                            .map(|v| v.trim_start_matches('v'))
                            .and_then(|v| Version::parse(v).ok());
                        if skipped.as_ref() == Some(&info.latest_version) {
                            if manual {
                                UpdateState::UpToDate
                            } else {
                                UpdateState::Idle
                            }
                        } else {
                            UpdateState::Available {
                                latest_version: info.latest_version,
                                release_notes: info.release_notes,
                                asset: info.asset,
                            }
                        }
                    }
                    Ok(None) => UpdateState::UpToDate,
                    Err(err) => {
                        if manual {
                            UpdateState::Error(format!("{err:#}"))
                        } else {
                            UpdateState::Idle
                        }
                    }
                };
            }
            UpdateEvent::Downloaded { version, result } => {
                self.update = match result {
                    Ok(_) => {
                        let file_name = self
                            .update_dest
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        UpdateState::Downloaded {
                            latest_version: version,
                            file_name,
                        }
                    }
                    Err(err) => UpdateState::Error(format!("{err:#}")),
                };
            }
        }
    }

    /// Apply a collector event, re-loading the report when a scan changed data.
    pub fn handle_collector_event(&mut self, event: CollectorEvent) -> Result<()> {
        match event {
            CollectorEvent::ScanStarted { provider } => {
                self.status = format!("正在扫描 {}", provider.display_name());
            }
            CollectorEvent::ScanCompleted { summary } => {
                self.last_scan = Some(format!(
                    "{} 条记录，新增 {}，重复 {}",
                    summary.records, summary.inserted, summary.skipped_duplicates
                ));
                if !summary.unchanged {
                    self.reload()?;
                }
                self.status = format!("扫描完成：{} 条记录", summary.records);
            }
            CollectorEvent::ScanFailed { provider, error } => {
                self.status = format!("扫描失败 {}：{error}", provider.display_name());
            }
            CollectorEvent::Watch(_) => {
                let _ = self.collector.scan_async();
            }
        }
        Ok(())
    }

    /// Handle a terminal key event. `q`/`Ctrl+C` quit, `r` rescans.
    pub fn handle_key(&mut self, key: ratatui::crossterm::event::KeyEvent) -> Result<Action> {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') if key.modifiers == KeyModifiers::NONE => {
                return Ok(Action::Quit);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(Action::Quit);
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.collector.scan_async()?;
                self.status = "手动扫描已触发".to_string();
            }
            // `t` / `T` cycle the time range for the summary + breakdown.
            KeyCode::Char('t') if key.modifiers == KeyModifiers::NONE => self.cycle_range(1)?,
            KeyCode::Char('T') if key.modifiers == KeyModifiers::NONE => self.cycle_range(-1)?,
            // `Tab` / `Shift+Tab` cycle the full-screen panel.
            KeyCode::Tab => self.cycle_view(1),
            KeyCode::BackTab => self.cycle_view(-1),
            // `u` opens the updates panel and starts a manual check.
            KeyCode::Char('u') if key.modifiers == KeyModifiers::NONE => {
                self.view = TuiView::Updates;
                self.check_updates(true)?;
            }
            // With an update available: `d` downloads, `s` skips it.
            KeyCode::Char('d') if key.modifiers == KeyModifiers::NONE => self.download_update()?,
            KeyCode::Char('s') if key.modifiers == KeyModifiers::NONE => self.skip_update()?,
            KeyCode::Left => self.move_selection(-1, 0),
            KeyCode::Right => self.move_selection(1, 0),
            KeyCode::Up => self.move_selection(0, -1),
            KeyCode::Down => self.move_selection(0, 1),
            KeyCode::Char('h') if key.modifiers == KeyModifiers::NONE => self.move_selection(-1, 0),
            KeyCode::Char('l') if key.modifiers == KeyModifiers::NONE => self.move_selection(1, 0),
            KeyCode::Char('k') if key.modifiers == KeyModifiers::NONE => self.move_selection(0, -1),
            KeyCode::Char('j') if key.modifiers == KeyModifiers::NONE => self.move_selection(0, 1),
            KeyCode::Home => self.selection.0 = 0,
            KeyCode::End => self.selection.0 = self.weeks.saturating_sub(1),
            _ => {}
        }
        Ok(Action::None)
    }

    fn move_selection(&mut self, dw: isize, dr: isize) {
        let (w, r) = self.selection;
        self.selection = (
            ((w as isize + dw).max(0) as usize).min(self.weeks.saturating_sub(1)),
            ((r as isize + dr).max(0) as usize).min((ROWS - 1) as usize),
        );
    }
}
