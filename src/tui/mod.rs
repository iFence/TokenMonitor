//! Terminal frontend (headless / server use).
//!
//! Reuses the collector, storage, `report` computation and `format` helpers —
//! no GPUI. Build with `--no-default-features --features tui`.

pub mod app;
pub mod ui;
pub mod update;

use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};

use crate::collector::{scheduler, Collector, CollectorEvent};
use crate::storage::default_db_path;
use crate::tui::app::{Action, TuiApp};

/// `--check-update`: one-shot update check that prints the result and exits.
/// Exit codes: 0 = up to date, 2 = newer release available, 1 = check failed.
pub fn check_update_cli() -> Result<()> {
    let current =
        semver::Version::parse(env!("CARGO_PKG_VERSION")).context("parse current version")?;
    match update::check_update(&current, crate::platform::is_portable()) {
        Ok(Some(info)) => {
            println!("当前版本: v{current}");
            println!("最新版本: v{}", info.latest_version);
            println!("新版本可用。");
            println!("下载: {} ({} bytes)", info.asset.name, info.asset.size);
            if !info.release_notes.trim().is_empty() {
                println!();
                println!("更新说明:");
                println!("{}", info.release_notes.trim());
            }
            std::process::exit(2);
        }
        Ok(None) => {
            println!("当前版本: v{current}");
            println!("已是最新版本。");
            Ok(())
        }
        Err(error) => {
            eprintln!("检查更新失败: {error:#}");
            std::process::exit(1);
        }
    }
}

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
    if app.check_updates_on_startup() {
        let _ = app.check_updates(false);
    }

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
    use ratatui::crossterm::event::{self, Event, KeyEventKind};

    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;
        // Drain collector events first so scan results show up even when the
        // user is idle.
        while let Ok(event) = events.try_recv() {
            app.handle_collector_event(event)?;
        }
        // Apply any finished update check / download.
        while let Some(event) = app.try_recv_update_event() {
            app.handle_update_event(event);
        }
        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                // A single physical press can produce Press + Release; act on
                // the press only, otherwise every key would trigger twice.
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    match app.handle_key(key)? {
                        Action::Quit => break,
                        Action::None => {}
                    }
                }
                Event::Key(_) => {}
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
    Ok(())
}
