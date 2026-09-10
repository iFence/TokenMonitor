//! Remembered window size for the desktop frontend.
//!
//! The main window reopens at the size it had when the app was last closed.
//! Saving goes through a small writer thread that waits for a resize to stop
//! before touching SQLite, so the UI thread never waits on a database write and
//! dragging a window edge costs a single settings row.
//!
//! Restoring happens *before* the window exists, through a read-only
//! connection: the collector (which owns the writable one) is only opened by
//! the app entity, which is created with the window.

use std::path::Path;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use gpui::{px, size, App, Bounds, Pixels, Window};

use crate::collector::Collector;
use crate::storage::repository::SettingsRepo;
use crate::storage::{default_db_path, sqlite};

/// Size the window opens with before anything was saved (the historical
/// default), in logical px.
pub const DEFAULT_SIZE: (f32, f32) = (1100.0, 800.0);

/// Smallest window the app will open with. Also handed to GPUI as
/// `window_min_size`, so a remembered size can never be below it.
pub const MIN_SIZE: (f32, f32) = (557.0, 671.0);

/// How long the writer waits for resizing to stop before saving. A drag emits a
/// bounds change per frame; only the size it settles on is worth a write.
const SETTLE_DELAY: Duration = Duration::from_millis(400);

/// Initial bounds for the main window: the size the app was last closed with,
/// centered on the primary display and fitted to it. Falls back to
/// [`DEFAULT_SIZE`] on a first run (or when nothing usable was saved).
pub fn initial_bounds(cx: &mut App) -> Bounds<Pixels> {
    let saved = default_db_path()
        .ok()
        .and_then(|path| restored_size(&path))
        .unwrap_or(DEFAULT_SIZE);
    let (width, height) = fit_to_display(saved, display_size(cx));
    Bounds::centered(None, size(px(width), px(height)), cx)
}

/// Start the writer thread and return the sender window sizes are fed into.
///
/// The thread lives for the process: it blocks on the channel, and writes
/// through the collector's connection so SQLite keeps a single writer.
pub fn start_writer(collector: Arc<Collector>) -> Sender<(f32, f32)> {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("tokenmonitor-window-size".into())
        .spawn(move || {
            while let Some((width, height)) = settled(&rx, SETTLE_DELAY) {
                if let Err(err) = collector.set_window_size(width, height) {
                    eprintln!("TokenMonitor: save window size: {err:#}");
                }
            }
        })
        .expect("spawn window size writer");
    tx
}

/// Watches the main window for size changes and feeds them to the writer.
pub struct SizeWatcher {
    tx: Sender<(f32, f32)>,
    last: Option<(f32, f32)>,
}

impl SizeWatcher {
    pub fn new(tx: Sender<(f32, f32)>) -> Self {
        SizeWatcher { tx, last: None }
    }

    /// Handle one window-bounds change. Moves (same size) and the degenerate
    /// rect a minimized window reports are ignored, so neither can overwrite the
    /// remembered size with something the user never chose.
    pub fn observe(&mut self, window: &Window) {
        let size = window.bounds().size;
        let current = (f32::from(size.width), f32::from(size.height));
        if !usable(current) || self.last == Some(current) {
            return;
        }
        self.last = Some(current);
        // The channel is unbounded and the writer drains it continuously, so
        // the UI thread never waits here.
        let _ = self.tx.send(current);
    }
}

/// The size the app was last closed with, if `db_path` holds a usable value.
fn restored_size(db_path: &Path) -> Option<(f32, f32)> {
    // Never create the database here: `Collector::open` migrates a legacy
    // database into a missing path, and materializing an empty file first would
    // make that migration skip.
    if !db_path.is_file() {
        return None;
    }
    let conn = sqlite::open_read(db_path).ok()?;
    let saved = SettingsRepo::new(&conn).window_size().ok().flatten()?;
    usable(saved).then_some(saved)
}

/// Usable area of the primary display, excluding the taskbar / dock.
fn display_size(cx: &App) -> Option<(f32, f32)> {
    let bounds = cx.primary_display()?.visible_bounds();
    Some((f32::from(bounds.size.width), f32::from(bounds.size.height)))
}

/// Keep a remembered size between the app's minimum and the display it opens
/// on, so a window sized on a large monitor still fits a laptop screen.
fn fit_to_display(saved: (f32, f32), display: Option<(f32, f32)>) -> (f32, f32) {
    let Some((max_width, max_height)) = display else {
        return (saved.0.max(MIN_SIZE.0), saved.1.max(MIN_SIZE.1));
    };
    (
        clamp(saved.0, MIN_SIZE.0, max_width),
        clamp(saved.1, MIN_SIZE.1, max_height),
    )
}

/// `f32::clamp` panics when the range is inverted, which a display smaller than
/// the window's own minimum would produce.
fn clamp(value: f32, min: f32, max: f32) -> f32 {
    value.clamp(min, max.max(min))
}

/// Whether the size is one the window can actually have: finite and at least
/// the enforced minimum. A minimized window reports `0x0` here, which must not
/// be remembered.
fn usable(size: (f32, f32)) -> bool {
    size.0.is_finite() && size.1.is_finite() && size.0 >= MIN_SIZE.0 && size.1 >= MIN_SIZE.1
}

/// Block until the size stops changing, then return the newest one; `None` once
/// every sender is gone (shutdown).
fn settled(rx: &Receiver<(f32, f32)>, idle: Duration) -> Option<(f32, f32)> {
    let mut newest = rx.recv().ok()?;
    loop {
        match rx.recv_timeout(idle) {
            Ok(size) => newest = size,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return Some(newest),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_the_minimum_and_fits_the_display() {
        let display = Some((1920.0, 1040.0));
        assert_eq!(fit_to_display((1100.0, 800.0), display), (1100.0, 800.0));
        assert_eq!(fit_to_display((4000.0, 3000.0), display), (1920.0, 1040.0));
        assert_eq!(fit_to_display((300.0, 300.0), display), MIN_SIZE);
        // A display narrower/taller than the minimum must not invert the clamp
        // range: the display bounds what it can, the minimum holds the rest.
        assert_eq!(
            fit_to_display((900.0, 900.0), Some((640.0, 480.0))),
            (640.0, MIN_SIZE.1)
        );
        // Without a reported display the saved size is only floored.
        assert_eq!(fit_to_display((300.0, 900.0), None), (MIN_SIZE.0, 900.0));
    }

    #[test]
    fn usable_rejects_degenerate_sizes() {
        assert!(usable(MIN_SIZE));
        assert!(!usable((0.0, 0.0)));
        assert!(!usable((MIN_SIZE.0 - 1.0, MIN_SIZE.1)));
        assert!(!usable((f32::NAN, MIN_SIZE.1)));
    }

    #[test]
    fn restored_size_reads_back_what_was_saved() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("tokenmonitor.db");

        assert_eq!(restored_size(&db), None);
        assert!(
            !db.exists(),
            "restoring must not materialize a database the collector still has to create"
        );

        let conn = sqlite::open(&db).unwrap();
        SettingsRepo::new(&conn)
            .set_window_size(1234.0, 900.0)
            .unwrap();
        drop(conn);

        assert_eq!(restored_size(&db), Some((1234.0, 900.0)));
    }

    #[test]
    fn settled_waits_for_the_drag_to_end_and_keeps_the_newest_size() {
        let (tx, rx) = channel();
        for size in [(1.0, 1.0), (2.0, 2.0), (3.0, 3.0)] {
            tx.send(size).unwrap();
        }
        assert_eq!(
            settled(&rx, Duration::from_millis(20)),
            Some((3.0, 3.0)),
            "a drag collapses into its final size"
        );

        drop(tx);
        assert_eq!(settled(&rx, Duration::from_millis(20)), None);
    }
}
