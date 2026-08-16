use std::sync::atomic::{AtomicU64, Ordering};
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
/// summaries are emitted as `CollectorEvent::ScanCompleted`. The interval is
/// read fresh from `interval_secs` each cycle, so changing it in settings takes
/// effect on the next scan without restarting the app.
pub fn start_scheduler(
    collector: Arc<Collector>,
    interval_secs: Arc<AtomicU64>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("rtoken-scheduler".into())
        .spawn(move || loop {
            let secs = interval_secs.load(Ordering::Relaxed).max(MIN_INTERVAL_SECS);
            thread::sleep(Duration::from_secs(secs));
            match collector.run_scan() {
                Ok(summaries) => {
                    for summary in summaries {
                        let _ = collector
                            .tx
                            .try_send(CollectorEvent::ScanCompleted { summary });
                    }
                }
                Err(e) => {
                    eprintln!("[rtoken] scheduled scan failed: {e}");
                }
            }
        })
        .expect("spawn scheduler thread")
}
