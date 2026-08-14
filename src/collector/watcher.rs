use std::path::PathBuf;

use async_channel::Sender;
use notify::{recommended_watcher, Event, EventKind, RecursiveMode, Watcher};

/// A file-system change under a provider data dir.
#[derive(Debug, Clone)]
pub struct WatchEvent {
    pub path: PathBuf,
    pub kind: WatchKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchKind {
    Create,
    Modify,
    Remove,
    Rename,
    Other,
}

impl From<Event> for WatchEvent {
    fn from(event: Event) -> Self {
        let kind = match event.kind {
            EventKind::Create(_) => WatchKind::Create,
            EventKind::Modify(_) => WatchKind::Modify,
            EventKind::Remove(_) => WatchKind::Remove,
            _ => WatchKind::Other,
        };
        let path = event.paths.into_iter().next().unwrap_or_default();
        WatchEvent { path, kind }
    }
}

/// Watches provider data directories and forwards events to `tx`.
pub struct FileWatcher {
    _watcher: notify::RecommendedWatcher,
}

impl FileWatcher {
    /// Watch every existing directory in `dirs` recursively.
    pub fn spawn(dirs: &[PathBuf], tx: Sender<WatchEvent>) -> notify::Result<Self> {
        let mut watcher = recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.try_send(WatchEvent::from(event));
            }
        })?;
        for dir in dirs {
            if dir.is_dir() {
                watcher.watch(dir, RecursiveMode::Recursive)?;
            }
        }
        Ok(FileWatcher { _watcher: watcher })
    }
}
