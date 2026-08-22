//! TUI application state: owns the collector + scheduler and the loaded report,
//! translates key events and collector events into state changes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{Duration, NaiveDate, Utc};

use crate::collector::{Collector, CollectorEvent};
use crate::core::aggregation::SumStats;
use crate::core::model::TimeWindow;
use crate::core::time::east8_local;
use crate::report::heatmap::{grid_start, week_count, ROWS};

/// Result of handling one key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    None,
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
    /// One-line status shown in the header (scan progress / last result).
    pub status: String,
    /// Short summary of the last completed scan.
    pub last_scan: Option<String>,
}

impl TuiApp {
    pub fn new(collector: Arc<Collector>) -> Self {
        let db_path = collector.db_path().to_path_buf();
        let mut app = TuiApp {
            db_path,
            collector,
            days: Vec::new(),
            today: Utc::now().date_naive(),
            start: Utc::now().date_naive(),
            weeks: 0,
            selection: (0, 0),
            status: "等待扫描…".to_string(),
            last_scan: None,
        };
        let _ = app.reload();
        app
    }

    pub fn collector(&self) -> &Arc<Collector> {
        &self.collector
    }

    /// Re-read the 365-day report series from the database (read-only WAL
    /// connection, so it never blocks a running scan).
    pub fn reload(&mut self) -> Result<()> {
        let conn = crate::storage::sqlite::open_read(&self.db_path)?;
        let now = Utc::now();
        let window = TimeWindow::last_n_days(365, now);
        self.days = crate::report::data::load_report_days(&conn, &window)?;
        self.today = east8_local(now).date_naive();
        self.start = grid_start(self.today);
        self.weeks = week_count(self.start, self.today);
        self.selection.0 = self.selection.0.min(self.weeks.saturating_sub(1));
        Ok(())
    }

    pub fn days(&self) -> &[(NaiveDate, SumStats)] {
        &self.days
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
            KeyCode::Left => self.move_selection(-1, 0),
            KeyCode::Right => self.move_selection(1, 0),
            KeyCode::Up => self.move_selection(0, -1),
            KeyCode::Down => self.move_selection(0, 1),
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
