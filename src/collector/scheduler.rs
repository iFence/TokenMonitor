use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::{Collector, CollectorEvent};

/// Floor for the rescan interval (seconds). A misconfigured or hand-edited
/// setting below this is ignored so the scheduler can never spin.
const MIN_INTERVAL_SECS: u64 = 5;

/// Start a background thread that periodically rescans all providers.
///
/// The thread runs until the process exits, matching app lifetime. Each run's
/// summaries are emitted as `CollectorEvent::ScanCompleted`. Each cycle the
/// thread waits up to the current interval for a wake signal; a wake (sent when
/// the interval changes in settings) re-reads the interval and starts a fresh
/// wait, so the new value takes effect immediately instead of after the old
/// interval elapses. When the wait times out, it runs a scan.
pub fn start_scheduler(
    collector: Arc<Collector>,
    interval_secs: Arc<AtomicU64>,
    wake: Receiver<()>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("tokenmonitor-scheduler".into())
        .spawn(move || loop {
            let secs = interval_secs.load(Ordering::Relaxed).max(MIN_INTERVAL_SECS);
            match wake.recv_timeout(Duration::from_secs(secs)) {
                // Interval changed: loop back to re-read it and start a fresh wait.
                Ok(()) => continue,
                // Timed out with no wake: run the periodic scan.
                Err(RecvTimeoutError::Timeout) => {}
                // The wake sender was dropped (app shutting down).
                Err(RecvTimeoutError::Disconnected) => break,
            }
            match collector.run_scan() {
                Ok(summaries) => {
                    for summary in summaries {
                        let _ = collector
                            .tx
                            .try_send(CollectorEvent::ScanCompleted { summary });
                    }
                }
                Err(e) => {
                    eprintln!("[TokenMonitor] scheduled scan failed: {e}");
                }
            }
        })
        .expect("spawn scheduler thread")
}
