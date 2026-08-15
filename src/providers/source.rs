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

/// Cheap change detector: walk `dir` and stat each JSONL file without reading
/// it. Files skipped by a full scan (non-JSONL, oversized, unreadable metadata)
/// are excluded the same way, so an unchanged tree yields the same fingerprint
/// every call. A few ms for hundreds of files — safe to run every poll cycle.
pub(crate) fn dir_fingerprint(
    dir: &Path,
    max_depth: usize,
    max_file_size: u64,
) -> Result<String, ProviderError> {
    roots_fingerprint(
        &[ScanRoot {
            dir: dir.to_path_buf(),
            label: None,
        }],
        max_depth,
        max_file_size,
    )
}

/// Multi-root variant of [`dir_fingerprint`]: aggregates file count, newest
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

/// The result of scanning one provider's data directory. Records are not
/// included: they stream out of `ProviderSource::scan` through its emit
/// callback so a scan never holds the full record set in memory.
#[derive(Debug, Default)]
pub struct ScanOutput {
    pub found_files: u64,
    /// Cheap change detector: `"<file_count>:<max_mtime_unix>:<total_bytes>"`.
    pub fingerprint: String,
    pub errors: Vec<String>,
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

/// Walk the JSONL files under `dir`, streaming each file through
/// `parse_file(path, rel, emit)`. File discovery, the size cap and the dir
/// fingerprint math live here once, shared by every JSONL-backed provider.
/// Returns `(found_files, fingerprint)`; per-file parse failures are pushed
/// onto `errors` and the walk continues.
pub(crate) fn scan_jsonl_dir(
    dir: &Path,
    config: &ProviderConfig,
    emit: &mut dyn FnMut(UsageRecord),
    errors: &mut Vec<String>,
    parse_file: &mut dyn FnMut(&Path, &Path, &mut dyn FnMut(UsageRecord)) -> Result<(), String>,
) -> (u64, String) {
    scan_roots(
        &[ScanRoot {
            dir: dir.to_path_buf(),
            label: None,
        }],
        config,
        emit,
        errors,
        parse_file,
    )
}

/// Multi-root variant of [`scan_jsonl_dir`]: walks every root and, when a root
/// carries a label, prefixes each file's relative path with it so the resulting
/// dedup fingerprints are namespaced per root.
pub(crate) fn scan_roots(
    roots: &[ScanRoot],
    config: &ProviderConfig,
    emit: &mut dyn FnMut(UsageRecord),
    errors: &mut Vec<String>,
    parse_file: &mut dyn FnMut(&Path, &Path, &mut dyn FnMut(UsageRecord)) -> Result<(), String>,
) -> (u64, String) {
    let mut max_mtime = 0i64;
    let mut total_bytes = 0u64;
    let mut found_files = 0u64;

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
            if let Ok(modified) = meta.modified() {
                if let Ok(unix) = modified.duration_since(std::time::UNIX_EPOCH) {
                    max_mtime = max_mtime.max(unix.as_secs() as i64);
                }
            }
            let rel = path.strip_prefix(&root.dir).unwrap_or(path);
            let rel = match &root.label {
                Some(label) => Path::new(label).join(rel),
                None => rel.to_path_buf(),
            };
            if let Err(e) = parse_file(path, &rel, emit) {
                errors.push(e);
            }
        }
    }

    (
        found_files,
        fingerprint(found_files, max_mtime, total_bytes),
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

    /// Cheap fingerprint of the source state; the scheduler skips rescan when unchanged.
    fn scan_fingerprint(&self) -> Result<String, ProviderError>;
}
