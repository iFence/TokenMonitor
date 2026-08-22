//! Terminal frontend (headless / server use).
//!
//! Reuses the collector, storage, `report` computation and `format` helpers —
//! no GPUI. Build with `--no-default-features --features tui`.

pub mod app;
pub mod ui;

use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::collector::{scheduler, Collector, CollectorEvent};
use crate::storage::default_db_path;
use crate::tui::app::{Action, TuiApp};

/// Entry point for the terminal frontend: owns the collector + periodic
/// scheduler, and drives the ratatui event loop.
pub fn run() -> Result<()> {
    let db_path = default_db_path()?;
    let collector = Arc::new(Collector::open(&db_path)?);
    let interval = Arc::new(AtomicU64::new(collector.scan_interval_seconds()));
    // The scheduler wakes on this channel to re-read the interval; keeping the
    // sender alive keeps the thread running for the app's lifetime.
    let (_wake_tx, wake_rx) = mpsc::channel();
    let _scheduler = scheduler::start_scheduler(collector.clone(), interval.clone(), wake_rx);
    let events = collector.events();

    let mut app = TuiApp::new(collector.clone());
    collector.scan_async()?;

    let mut terminal = ratatui::try_init()?;
    let result = event_loop(&mut terminal, &mut app, &events);
    ratatui::try_restore()?;
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut TuiApp,
    events: &async_channel::Receiver<CollectorEvent>,
) -> Result<()> {
    use ratatui::crossterm::event::{self, Event};

    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;
        // Drain collector events first so scan results show up even when the
        // user is idle.
        while let Ok(event) = events.try_recv() {
            app.handle_collector_event(event)?;
        }
        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) => match app.handle_key(key)? {
                    Action::Quit => break,
                    Action::None => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
    Ok(())
}
