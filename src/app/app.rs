use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_channel::{unbounded, Receiver, Sender};
use chrono::{NaiveDate, Utc};
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, WeakEntity, Window,
};
use gpui_component::calendar::Date;
use gpui_component::date_picker::{DatePickerEvent, DatePickerState};
use gpui_component::select::{SelectEvent, SelectState};
use gpui_component::{v_flex, IndexPath};

use crate::collector::{scheduler, Collector, CollectorEvent};
use crate::core::aggregation::SumStats;
use crate::core::model::{Provider, ProviderSelection, TimeWindow};
use crate::storage::default_db_path;
use crate::storage::repository::UsageRepo;
use crate::storage::sqlite;
use crate::ui;

use super::state::{
    ActivePage, AppState, ChartApp, ChartMetric, ChartRange, ChartsSnapshot, ScanStatus,
    SettingsGroup, TimeTab, ViewSnapshot,
};
use super::update_check::UpdateCheckUiState;

/// Periodic auto-rescan interval, matching tokei's 30-second refresh.
const SCAN_INTERVAL: Duration = Duration::from_secs(30);

/// Root application entity: owns app state, the collector, and the window focus.
pub struct RTokenApp {
    pub state: AppState,
    pub collector: Arc<Collector>,
    pub focus_handle: FocusHandle,
    pub weak_self: WeakEntity<RTokenApp>,
    /// Keeps the periodic auto-rescan thread alive for the app's lifetime.
    _scheduler: std::thread::JoinHandle<()>,
    view_tx: Sender<ViewSnapshot>,
    view_rx: Receiver<ViewSnapshot>,
    view_seq: u64,
    /// Stateful dropdown / date-picker entities for the charts page controls.
    pub chart_metric_select: Entity<SelectState<Vec<ChartMetric>>>,
    pub chart_app_select: Entity<SelectState<Vec<ChartApp>>>,
    pub chart_range_picker: Entity<DatePickerState>,
    /// Auto-update state and preferences.
    pub update_check: UpdateCheckUiState,
    pub check_updates_on_startup: bool,
    pub skipped_update_version: Option<String>,
}

impl RTokenApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let db_path = default_db_path().expect("resolve app data dir");
        let collector = Arc::new(Collector::open(&db_path).expect("open collector"));
        let scheduler = scheduler::start_scheduler(collector.clone(), SCAN_INTERVAL);
        let (view_tx, view_rx) = unbounded();

        let selection = collector.selection();
        let check_updates_on_startup = collector.check_updates_on_startup();
        let skipped_update_version = collector.skipped_update_version();

        // Stateful dropdown / date-picker entities live for the app's lifetime;
        // recreating them each render would reset open state on every notify.
        let chart_metric_select = cx.new(|cx| {
            SelectState::new(
                ChartMetric::ALL.to_vec(),
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let mut chart_app_options = vec![ChartApp::All];
        chart_app_options.extend(selection.enabled().into_iter().map(ChartApp::One));
        let chart_app_select = cx
            .new(|cx| SelectState::new(chart_app_options, Some(IndexPath::default()), window, cx));
        let chart_range_picker = cx.new(|cx| DatePickerState::range(window, cx));

        let mut app = RTokenApp {
            state: AppState::default(),
            collector,
            focus_handle,
            weak_self: cx.weak_entity(),
            _scheduler: scheduler,
            view_tx,
            view_rx,
            view_seq: 0,
            chart_metric_select,
            chart_app_select,
            chart_range_picker,
            update_check: UpdateCheckUiState::default(),
            check_updates_on_startup,
            skipped_update_version,
        };
        app.state.provider_selection = selection;
        app.sync_chart_app_select(window, cx);

        // Dispatch dropdown / date-picker events back into app handlers.
        {
            let metric = app.chart_metric_select.clone();
            cx.subscribe_in(
                &metric,
                window,
                |this, _, ev: &SelectEvent<Vec<ChartMetric>>, _, cx| {
                    if let SelectEvent::Confirm(Some(m)) = ev {
                        this.select_chart_metric(*m, cx);
                    }
                },
            )
            .detach();
        }
        {
            let app_select = app.chart_app_select.clone();
            cx.subscribe_in(
                &app_select,
                window,
                |this, _, ev: &SelectEvent<Vec<ChartApp>>, _, cx| {
                    if let SelectEvent::Confirm(Some(app)) = ev {
                        this.select_chart_app(*app, cx);
                    }
                },
            )
            .detach();
        }
        {
            let picker = app.chart_range_picker.clone();
            cx.subscribe_in(
                &picker,
                window,
                |this, _, ev: &DatePickerEvent, _, cx| match ev {
                    DatePickerEvent::Change(Date::Range(Some(start), Some(end))) => {
                        this.select_chart_custom_range(*start, *end, cx);
                    }
                    DatePickerEvent::Change(Date::Range(_, _)) => {
                        // Cleared: fall back to the preset range.
                        this.state.charts.custom_range = None;
                        this.refresh_view(cx);
                        cx.notify();
                    }
                    _ => {}
                },
            )
            .detach();
        }

        app.spawn_event_loop(cx);
        app.trigger_scan(cx); // initial auto-scan so data shows without manual action
        app.refresh_view(cx); // async: returns immediately, fills state in background
        app
    }

    /// Kick off a background scan of every provider.
    pub fn trigger_scan(&mut self, cx: &mut Context<Self>) {
        match self.collector.scan_async() {
            Ok(()) => {
                self.state.scan_status = ScanStatus::Scanning {
                    completed: 0,
                    total: self.collector.sources().len() as u32,
                };
                self.state.last_error = None;
            }
            Err(e) => self.state.last_error = Some(format!("failed to start scan: {e}")),
        }
        cx.notify();
    }

    /// Switch the dashboard time-range tab and re-query the window.
    pub fn select_time_tab(&mut self, tab: TimeTab, cx: &mut Context<Self>) {
        self.state.time_tab = tab;
        self.state.expanded_provider = None; // collapse expansion; data window changed
        self.refresh_view(cx);
        cx.notify();
    }

    /// Switch the active page; entering the charts page triggers a load.
    pub fn select_page(&mut self, page: ActivePage, cx: &mut Context<Self>) {
        self.state.active_page = page;
        if page == ActivePage::Charts {
            self.refresh_view(cx);
        }
        cx.notify();
    }

    /// Switch the settings page group (left-hand navigation).
    pub fn select_settings_group(&mut self, group: SettingsGroup, cx: &mut Context<Self>) {
        self.state.settings_group = group;
        cx.notify();
    }

    /// Charts control handlers. Range/provider changes re-query; metric/kind
    /// changes are pure render-time transforms.
    pub fn select_chart_range(
        &mut self,
        range: ChartRange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.charts.range = range;
        self.state.charts.custom_range = None;
        self.chart_range_picker.update(cx, |picker, cx| {
            picker.set_date(Date::Range(None, None), window, cx);
        });
        self.refresh_view(cx);
        cx.notify();
    }

    /// Set a custom (East-8, inclusive) date range, overriding the preset.
    pub fn select_chart_custom_range(
        &mut self,
        start: NaiveDate,
        end: NaiveDate,
        cx: &mut Context<Self>,
    ) {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        self.state.charts.custom_range = Some((start, end));
        self.refresh_view(cx);
        cx.notify();
    }

    pub fn select_chart_metric(&mut self, metric: ChartMetric, cx: &mut Context<Self>) {
        self.state.charts.metric = metric;
        cx.notify();
    }

    pub fn select_chart_app(&mut self, app: ChartApp, cx: &mut Context<Self>) {
        self.state.charts.app = app;
        self.refresh_view(cx);
        cx.notify();
    }

    /// Expand/collapse a provider card's per-model breakdown.
    pub fn toggle_provider_expanded(&mut self, provider: Provider, cx: &mut Context<Self>) {
        self.state.expanded_provider = if self.state.expanded_provider == Some(provider) {
            None
        } else {
            Some(provider)
        };
        cx.notify();
    }

    /// Toggle whether a provider is tracked (the keep checkbox in settings).
    pub fn set_provider_enabled(
        &mut self,
        provider: Provider,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut selection = self.state.provider_selection.clone();
        selection.set_enabled(provider, enabled);
        self.apply_provider_selection(selection, window, cx);
    }

    /// Move a provider up (`dir < 0`) or down (`dir > 0`) in the app order.
    pub fn move_provider(
        &mut self,
        provider: Provider,
        dir: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut selection = self.state.provider_selection.clone();
        selection.move_entry(provider, dir);
        self.apply_provider_selection(selection, window, cx);
    }

    /// Persist a new selection, rebuild scan sources, and refresh the view.
    fn apply_provider_selection(
        &mut self,
        selection: ProviderSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.provider_selection = selection.clone();

        // Drop UI state that points at a now-disabled provider.
        if let Some(expanded) = self.state.expanded_provider {
            if !selection.is_enabled(expanded) {
                self.state.expanded_provider = None;
            }
        }
        // Drop the per-model app filter if it points at a now-disabled app.
        if let ChartApp::One(provider) = self.state.charts.app {
            if !selection.is_enabled(provider) {
                self.state.charts.app = ChartApp::All;
            }
        }

        if let Err(e) = self.collector.set_selection(selection) {
            self.state.last_error = Some(format!("save app selection: {e}"));
        }
        self.sync_chart_app_select(window, cx);
        self.trigger_scan(cx);
        self.refresh_view(cx);
        cx.notify();
    }

    /// Rebuild the charts app dropdown ("全部" + enabled apps) from the current
    /// selection and re-select the active app filter.
    fn sync_chart_app_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut options = vec![ChartApp::All];
        options.extend(
            self.state
                .provider_selection
                .enabled()
                .into_iter()
                .map(ChartApp::One),
        );
        let selected = self.state.charts.app;
        self.chart_app_select.update(cx, |select, cx| {
            select.set_items(options, window, cx);
            select.set_selected_value(&selected, window, cx);
        });
    }

    /// Forward collector events into app state, refreshing the view after each scan.
    fn spawn_event_loop(&self, cx: &mut Context<Self>) {
        let receiver = self.collector.events();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = receiver.recv().await {
                let _ = this.update(cx, |app, cx| app.handle_collector_event(event, cx));
            }
        })
        .detach();

        let view_rx = self.view_rx.clone();
        cx.spawn(async move |this, cx| {
            while let Ok(snapshot) = view_rx.recv().await {
                let _ = this.update(cx, |app, cx| app.apply_snapshot(snapshot, cx));
            }
        })
        .detach();
    }

    fn handle_collector_event(&mut self, event: CollectorEvent, cx: &mut Context<Self>) {
        match event {
            CollectorEvent::ScanStarted { .. } => {
                self.state.scan_status = ScanStatus::Scanning {
                    completed: 0,
                    total: self.collector.sources().len() as u32,
                };
            }
            CollectorEvent::ScanCompleted { summary } => {
                self.state.scan_status = ScanStatus::Done {
                    records: summary.records,
                    at: Utc::now(),
                };
                // Unchanged scans (fingerprint matched) have no new data; skip
                // the re-query + re-render so idle polls stay near-free.
                if !summary.unchanged {
                    self.refresh_view(cx);
                }
            }
            CollectorEvent::ScanFailed { provider, error } => {
                self.state.last_error = Some(format!("{}: {error}", provider.display_name()));
                self.state.scan_status = ScanStatus::Failed { error };
            }
            CollectorEvent::Watch(_) => {
                let _ = self.collector.scan_async();
            }
        }
        cx.notify();
    }

    /// Kick off a background aggregation. Runs on a dedicated read-only SQLite
    /// connection (WAL, so it never blocks the scan writer), then posts the
    /// result back through `view_tx` for the event loop to apply.
    fn refresh_view(&mut self, _cx: &mut Context<Self>) {
        let seq = self.view_seq.wrapping_add(1);
        self.view_seq = seq;

        let db_path = self.collector.db_path().to_path_buf();
        let now = Utc::now();
        let time_tab = self.state.time_tab;
        let window = time_tab.window(now);
        let charts = if self.state.active_page == ActivePage::Charts {
            Some((self.state.charts.window(now), self.state.charts.app))
        } else {
            None
        };
        let enabled = self.state.provider_selection.enabled();
        let tx = self.view_tx.clone();

        std::thread::Builder::new()
            .name("rtoken-aggregate".into())
            .spawn(move || {
                let snapshot =
                    compute_view_snapshot(seq, time_tab, &db_path, window, charts, &enabled);
                let _ = tx.send_blocking(snapshot);
            })
            .expect("spawn aggregate thread");
    }

    fn apply_snapshot(&mut self, snap: ViewSnapshot, cx: &mut Context<Self>) {
        if snap.seq != self.view_seq {
            return; // stale: a newer request superseded this one
        }
        self.state.summary = snap.summary;
        self.state.by_provider = snap.by_provider;
        self.state.by_provider_model = snap.by_provider_model;
        self.state.by_project = snap.by_project;
        self.state.by_day = snap.by_day;
        if let Some(charts) = snap.charts {
            self.state.charts.data = Some(charts);
        }
        if let Some(error) = snap.error {
            self.state.last_error = Some(error);
        }
        cx.notify();
    }
}

/// Compute the aggregate view snapshot on the calling (background) thread.
fn compute_view_snapshot(
    seq: u64,
    time_tab: TimeTab,
    db_path: &std::path::Path,
    window: TimeWindow,
    charts: Option<(TimeWindow, ChartApp)>,
    enabled: &[Provider],
) -> ViewSnapshot {
    let mut snap = ViewSnapshot {
        seq,
        time_tab,
        ..ViewSnapshot::default()
    };
    let conn = match sqlite::open_read(db_path) {
        Ok(conn) => conn,
        Err(e) => {
            snap.error = Some(format!("open read db failed: {e}"));
            return snap;
        }
    };
    let repo = UsageRepo::new(&conn);

    // Keep the old partial-success semantics: a failing query sets the error
    // but does not discard the other aggregates.
    match repo.aggregate_window(&window) {
        Ok(s) => snap.summary = Some(s),
        Err(e) => snap.error = Some(format!("query failed: {e}")),
    }
    match repo.aggregate_by_provider(&window) {
        Ok(v) => snap.by_provider = v,
        Err(e) => snap.error = Some(format!("query failed: {e}")),
    }
    match repo.aggregate_by_provider_model(&window) {
        Ok(m) => snap.by_provider_model = m,
        Err(e) => snap.error = Some(format!("query failed: {e}")),
    }
    match repo.aggregate_by_project(&window) {
        Ok(v) => snap.by_project = v,
        Err(e) => snap.error = Some(format!("query failed: {e}")),
    }
    match repo.aggregate_by_day(&window) {
        Ok(v) => snap.by_day = v,
        Err(e) => snap.error = Some(format!("query failed: {e}")),
    }
    if let Some((chart_window, app)) = charts {
        snap.charts = Some(compute_chart_snapshot(&repo, chart_window, app, enabled));
    }
    snap
}

/// Compute the charts page's raw per-day series.
fn compute_chart_snapshot(
    repo: &UsageRepo<'_>,
    window: TimeWindow,
    app: ChartApp,
    enabled: &[Provider],
) -> ChartsSnapshot {
    let mut snap = ChartsSnapshot::default();
    for &p in enabled {
        if let Ok(series) = repo.daily_series_by_provider(p, &window) {
            snap.provider_series.push((p, series));
        }
    }
    snap.model_series = match app {
        ChartApp::All => {
            let mut merged: BTreeMap<String, Vec<(String, SumStats)>> = BTreeMap::new();
            for &p in enabled {
                if let Ok(models) = repo.daily_series_by_provider_model(p, &window) {
                    merge_model_series(&mut merged, models);
                }
            }
            merged
        }
        ChartApp::One(provider) => repo
            .daily_series_by_provider_model(provider, &window)
            .unwrap_or_default(),
    };
    snap
}

/// Merge per-model daily series from one provider into the cross-app map,
/// summing `SumStats` for matching (model, day) keys.
fn merge_model_series(
    dst: &mut BTreeMap<String, Vec<(String, SumStats)>>,
    src: BTreeMap<String, Vec<(String, SumStats)>>,
) {
    for (model, series) in src {
        let entry = dst.entry(model).or_default();
        for (day, stats) in series {
            match entry.iter_mut().find(|(d, _)| *d == day) {
                Some((_, acc)) => acc.add(&stats),
                None => entry.push((day, stats)),
            }
        }
    }
}

impl Focusable for RTokenApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RTokenApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_component::{Theme, ThemeMode};
        if Theme::global(cx).mode != ThemeMode::Dark {
            Theme::change(ThemeMode::Dark, None, cx);
        }

        let p = crate::ui::palette(cx);
        v_flex()
            .id("rtoken-root")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(p.background)
            .text_color(p.foreground)
            .child(ui::topbar::render_topbar(self, window, cx))
            .child(ui::router(self, window, cx))
    }
}
