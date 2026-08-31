use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use thiserror::Error;
use walkdir::WalkDir;

use crate::core::model::Provider;
use crate::core::usage::UsageRecord;

/// Per-provider scan configuration.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub provider: Provider,
    /// Override for the discovered data directory (e.g. from settings).
    pub data_dir_override: Option<PathBuf>,
    pub enabled: bool,
    /// Skip raw files larger than this many bytes.
    pub max_file_size: u64,
    /// Maximum directory depth when walking raw files.
    pub max_depth: usize,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        ProviderConfig {
            provider: Provider::Claude,
            data_dir_override: None,
            enabled: true,
            max_file_size: 64 * 1024 * 1024,
            max_depth: 8,
        }
    }
}

impl ProviderConfig {
    pub fn for_provider(provider: Provider) -> Self {
        ProviderConfig {
            provider,
            ..ProviderConfig::default()
        }
    }
}

/// Format the change-detection fingerprint from per-scan stats. Must stay in
/// lockstep with `dir_fingerprint` / `roots_fingerprint` so the cheap check and
/// a full scan agree.
pub(crate) fn fingerprint(found_files: u64, max_mtime: i64, total_bytes: u64) -> String {
    format!("{found_files}:{max_mtime}:{total_bytes}")
}

/// One directory a provider scans, plus an optional label prepended to each
/// file's relative path. The label namespaces per-record dedup fingerprints so
/// files from different roots (the local home dir vs. a WSL distro) never
/// collide; `None` keeps the primary root's fingerprints identical to before.
#[derive(Debug, Clone)]
pub struct ScanRoot {
    pub dir: PathBuf,
    pub label: Option<String>,
}

/// Multi-root change detector: aggregates file count, newest
/// mtime and total bytes across every root into one fingerprint string.
pub(crate) fn roots_fingerprint(
    roots: &[ScanRoot],
    max_depth: usize,
    max_file_size: u64,
) -> Result<String, ProviderError> {
    let mut max_mtime = 0i64;
    let mut total_bytes = 0u64;
    let mut found = 0u64;

    for root in roots {
        for entry in WalkDir::new(&root.dir)
            .max_depth(max_depth)
            .follow_links(false)
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            if !entry
                .path()
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("jsonl"))
            {
                continue;
            }
            let meta = match fs::metadata(entry.path()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.len() > max_file_size {
                continue;
            }
            found += 1;
            total_bytes += meta.len();
            if let Ok(modified) = meta.modified() {
                if let Ok(unix) = modified.duration_since(std::time::UNIX_EPOCH) {
                    max_mtime = max_mtime.max(unix.as_secs() as i64);
                }
            }
        }
    }

    Ok(fingerprint(found, max_mtime, total_bytes))
}

/// Per-file scan state for incremental scanning: the root-label-prefixed
/// relative path maps to `(mtime_secs, size_bytes)`. A file whose state is
/// unchanged since the last scan can be skipped without being read or parsed.
pub type FileStates = HashMap<PathBuf, (i64, u64)>;

/// The result of scanning one provider's data directory. Records are not
/// included: they stream out of `ProviderSource::scan` through its emit
/// callback so a scan never holds the full record set in memory.
#[derive(Debug, Default)]
pub struct ScanOutput {
    pub found_files: u64,
    /// Cheap change detector: `"<file_count>:<max_mtime_unix>:<total_bytes>"`.
    pub fingerprint: String,
    pub errors: Vec<String>,
    /// Per-file state observed this scan. `Some` when the adapter supports
    /// incremental scanning (and the caller should persist it for the next
    /// run); `None` means the adapter always scans in full.
    pub file_states: Option<FileStates>,
}

/// Stream `path` line-by-line without reading the whole file into memory.
/// Mirrors `str::lines()` semantics (splits on `\n`, strips a trailing
/// `\r\n`, never splits on a lone `\r`) so per-line indexes — and therefore
/// record fingerprints — stay identical to the previous whole-file parser.
/// Invalid UTF-8 is replaced lossily, matching `String::from_utf8_lossy`.
pub(crate) fn for_each_line(
    path: &Path,
    mut on_line: impl FnMut(&str, usize),
) -> Result<(), String> {
    let file = File::open(path).map_err(|e| format!("read {:?}: {e}", path))?;
    let mut reader = BufReader::new(file);
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut line_idx = 0usize;
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(e) => return Err(format!("read {:?}: {e}", path)),
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        on_line(&String::from_utf8_lossy(&buf), line_idx);
        line_idx += 1;
    }
}

/// Full multi-root scan (test-only convenience): walks every root and, when a
/// root carries a label, prefixes each file's relative path with it so the
/// resulting dedup fingerprints are namespaced per root.
#[cfg(test)]
pub(crate) fn scan_roots(
    roots: &[ScanRoot],
    config: &ProviderConfig,
    emit: &mut dyn FnMut(UsageRecord),
    errors: &mut Vec<String>,
    parse_file: &mut dyn FnMut(&Path, &Path, &mut dyn FnMut(UsageRecord)) -> Result<(), String>,
) -> (u64, String) {
    let (found, fp, _states) = scan_roots_inner(roots, config, emit, errors, parse_file, None);
    (found, fp)
}

/// Incremental multi-root variant of [`scan_roots`]: files whose `(mtime, size)`
/// state matches `known` are skipped, never read or parsed. Returns the fresh
/// per-file state (every walked file, unchanged or not) so the caller can persist
/// it for the next run.
pub(crate) fn scan_roots_incremental(
    roots: &[ScanRoot],
    config: &ProviderConfig,
    emit: &mut dyn FnMut(UsageRecord),
    errors: &mut Vec<String>,
    parse_file: &mut dyn FnMut(&Path, &Path, &mut dyn FnMut(UsageRecord)) -> Result<(), String>,
    known: &FileStates,
) -> (u64, String, FileStates) {
    scan_roots_inner(roots, config, emit, errors, parse_file, Some(known))
}

/// Shared walk used by the full and incremental variants. It always stats every
/// JSONL file (so the fingerprint matches `roots_fingerprint`), but only parses
/// files whose state differs from `known` (or every file when `known` is `None`).
fn scan_roots_inner(
    roots: &[ScanRoot],
    config: &ProviderConfig,
    emit: &mut dyn FnMut(UsageRecord),
    errors: &mut Vec<String>,
    parse_file: &mut dyn FnMut(&Path, &Path, &mut dyn FnMut(UsageRecord)) -> Result<(), String>,
    known: Option<&FileStates>,
) -> (u64, String, FileStates) {
    let mut max_mtime = 0i64;
    let mut total_bytes = 0u64;
    let mut found_files = 0u64;
    let mut states: FileStates = FileStates::new();

    for root in roots {
        for entry in WalkDir::new(&root.dir)
            .max_depth(config.max_depth)
            .follow_links(false)
        {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    errors.push(format!("walk error: {e}"));
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let is_jsonl = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("jsonl"));
            if !is_jsonl {
                continue;
            }
            let meta = match fs::metadata(path) {
                Ok(m) => m,
                Err(e) => {
                    errors.push(format!("metadata {:?}: {e}", path));
                    continue;
                }
            };
            if meta.len() > config.max_file_size {
                errors.push(format!("skip oversized {:?}", path));
                continue;
            }
            found_files += 1;
            total_bytes += meta.len();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|unix| unix.as_secs() as i64)
                .unwrap_or(0);
            max_mtime = max_mtime.max(mtime);
            let rel = path.strip_prefix(&root.dir).unwrap_or(path);
            let rel = match &root.label {
                Some(label) => Path::new(label).join(rel),
                None => rel.to_path_buf(),
            };
            let state = (mtime, meta.len());
            // Skip parsing when the caller provided incremental state and this
            // file's (mtime, size) is unchanged since the last successful scan.
            if known.is_some_and(|k| k.get(&rel) == Some(&state)) {
                // Keep unchanged files in the fresh map so they stay "known".
                states.insert(rel.clone(), state);
                continue;
            }
            match parse_file(path, &rel, emit) {
                Ok(()) => {
                    // Record only after a successful parse: a read failure stays
                    // absent from the map and is retried on the next changed scan
                    // instead of being skipped forever.
                    states.insert(rel.clone(), state);
                }
                Err(e) => errors.push(e),
            }
        }
    }

    (
        found_files,
        fingerprint(found_files, max_mtime, total_bytes),
        states,
    )
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("data directory not found for {0}")]
    DataDirNotFound(Provider),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(String),
}

/// Contract every provider adapter implements.
pub trait ProviderSource: Send + Sync {
    fn provider(&self) -> Provider;

    /// Locate all existing local data directories holding raw usage files.
    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError>;

    /// Walk raw files under `data_dirs()` and parse them into normalized
    /// records, delivering each record to `emit` as soon as it is produced.
    /// Streaming keeps peak memory bounded: neither the raw file text nor the
    /// full record set is ever held at once.
    fn scan(&self, emit: &mut dyn FnMut(UsageRecord)) -> Result<ScanOutput, ProviderError>;

    /// Incremental variant of [`scan`]. `known` maps each file's root-label-
    /// prefixed relative path to the `(mtime_secs, size_bytes)` observed at the
    /// last successful scan; an adapter may skip parsing files whose state is
    /// unchanged and report the fresh state in `ScanOutput::file_states`. The
    /// default performs a full `scan` and reports no incremental support
    /// (`file_states: None`).
    fn scan_incremental(
        &self,
        emit: &mut dyn FnMut(UsageRecord),
        _known: &FileStates,
    ) -> Result<ScanOutput, ProviderError> {
        self.scan(emit)
    }

    /// Cheap fingerprint of the source state; the scheduler skips rescan when unchanged.
    fn scan_fingerprint(&self) -> Result<String, ProviderError>;
}
