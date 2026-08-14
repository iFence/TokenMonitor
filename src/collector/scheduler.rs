use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::{Collector, CollectorEvent};

/// Start a background thread that periodically rescans all providers.
///
/// The thread runs until the process exits, matching app lifetime. Each run's
/// summaries are emitted as `CollectorEvent::ScanCompleted`.
pub fn start_scheduler(collector: Arc<Collector>, interval: Duration) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("rtoken-scheduler".into())
        .spawn(move || loop {
            thread::sleep(interval);
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
