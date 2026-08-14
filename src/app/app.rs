use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use gpui::{
    App, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, WeakEntity, Window,
};
use gpui_component::v_flex;

use crate::collector::{scheduler, Collector, CollectorEvent};
use crate::core::aggregation;
use crate::core::model::Provider;
use crate::storage::default_db_path;
use crate::storage::repository::UsageRepo;
use crate::ui;

use super::state::{AppState, ScanStatus, TimeTab};

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
}

impl RTokenApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let db_path = default_db_path().expect("resolve app data dir");
        let collector = Arc::new(Collector::open(&db_path).expect("open collector"));
        let scheduler = scheduler::start_scheduler(collector.clone(), SCAN_INTERVAL);
        let mut app = RTokenApp {
            state: AppState::default(),
            collector,
            focus_handle,
            weak_self: cx.weak_entity(),
            _scheduler: scheduler,
        };
        app.spawn_event_loop(cx);
        app.trigger_scan(cx); // initial auto-scan so data shows without manual action
        app.refresh_view(cx);
        app
    }

    /// Kick off a background scan of every provider.
    pub fn trigger_scan(&mut self, cx: &mut Context<Self>) {
        match self.collector.scan_async() {
            Ok(()) => {
                self.state.scan_status = ScanStatus::Scanning {
                    completed: 0,
                    total: Provider::ALL.len() as u32,
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

    /// Expand/collapse a provider card's per-model breakdown.
    pub fn toggle_provider_expanded(&mut self, provider: Provider, cx: &mut Context<Self>) {
        self.state.expanded_provider = if self.state.expanded_provider == Some(provider) {
            None
        } else {
            Some(provider)
        };
        cx.notify();
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
    }

    fn handle_collector_event(&mut self, event: CollectorEvent, cx: &mut Context<Self>) {
        match event {
            CollectorEvent::ScanStarted { .. } => {
                self.state.scan_status = ScanStatus::Scanning {
                    completed: 0,
                    total: Provider::ALL.len() as u32,
                };
            }
            CollectorEvent::ScanCompleted { summary } => {
                self.state.scan_status = ScanStatus::Done {
                    records: summary.records,
                    at: Utc::now(),
                };
                self.refresh_view(cx);
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

    /// Recompute the aggregate view state from persisted usage records.
    fn refresh_view(&mut self, _cx: &mut Context<Self>) {
        let db = self.collector.db();
        let conn = match db.lock() {
            Ok(conn) => conn,
            Err(_) => return,
        };
        let repo = UsageRepo::new(&conn);
        let window = self.state.time_tab.window(Utc::now());
        match repo.query_by_window(&window) {
            Ok(all) => {
                self.state.summary = Some(aggregation::total(&all));
                self.state.by_provider = aggregation::by_provider(&all);
                self.state.by_provider_model = aggregation::by_provider_model(&all);
                self.state.by_project = aggregation::by_project(&all);
                self.state.by_day = aggregation::by_day(&all);
            }
            Err(e) => self.state.last_error = Some(format!("query failed: {e}")),
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
