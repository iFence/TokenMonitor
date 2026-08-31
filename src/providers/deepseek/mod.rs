//! DeepSeek Harness data source: parses `~/.dsh/sessions/**/*.jsonl.zstd`.
//!
//! The DeepSeek coding harness appends one JSON event per line to a
//! zstd-compressed session file. Four event types matter here: `session`
//! (session id + cwd), `request/header` (current model), `assistant/chunk`
//! (a streaming usage delta), and `assistant/message` (the authoritative
//! per-call usage). This adapter emits one record per `assistant/message` —
//! the completed-call total — and ignores `assistant/chunk`, matching how the
//! Claude/Codex/CodeBuddy adapters only count finished messages. TokenMonitor's
//! dedup is `INSERT OR IGNORE` (append-only, no upsert), so folding the
//! stream into a single record per `(turn, step)` is what keeps rescans
//! idempotent without double-counting.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::roots::discover_roots;
use super::source::{
    fingerprint, ProviderConfig, ProviderError, ProviderSource, ScanOutput, ScanRoot,
};

/// Default model when neither the message nor a `request/header` names one.
const DEFAULT_MODEL: &str = "deepseek-v4-pro";

/// One event line. Fields not named here — the assistant response text and
/// other large payloads — are skipped by serde without being allocated.
#[derive(Deserialize)]
struct EventLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    time: Option<Value>,
    /// `session` event fields (top-level, absent on other events).
    id: Option<String>,
    cwd: Option<String>,
    data: Option<EventData>,
}

#[derive(Deserialize)]
struct EventData {
    turn: Option<Value>,
    step: Option<Value>,
    usage: Option<HarnessUsage>,
    message: Option<Message>,
    header: Option<Header>,
}

/// DeepSeek usage buckets. `outputTokens` includes `reasoningTokens`; TokenMonitor
/// has no reasoning bucket, so reasoning stays folded into output — this keeps
/// both `total_tokens()` and the cost (billed on the full output) aligned with
/// tokei's `raw_out` handling.
#[derive(Deserialize)]
struct HarnessUsage {
    #[serde(rename = "inputTokens")]
    input_tokens: Option<Value>,
    #[serde(rename = "outputTokens")]
    output_tokens: Option<Value>,
    #[serde(rename = "cacheReadTokens")]
    cache_read_tokens: Option<Value>,
    #[serde(rename = "cacheWriteTokens")]
    cache_write_tokens: Option<Value>,
}

#[derive(Deserialize)]
struct Message {
    source: Option<Source>,
}

#[derive(Deserialize)]
struct Source {
    model: Option<String>,
}

#[derive(Deserialize)]
struct Header {
    config: Option<Config>,
}

#[derive(Deserialize)]
struct Config {
    model: Option<String>,
}

pub struct DeepSeekSource {
    config: ProviderConfig,
    /// Discovered scan roots, computed lazily on the first scan (background
    /// thread) so WSL discovery never blocks UI startup.
    roots: OnceLock<Vec<ScanRoot>>,
}

impl DeepSeekSource {
    pub fn new(config: ProviderConfig) -> Self {
        DeepSeekSource {
            config,
            roots: OnceLock::new(),
        }
    }

    fn roots(&self) -> &[ScanRoot] {
        self.roots.get_or_init(|| {
            if let Some(dir) = &self.config.data_dir_override {
                vec![ScanRoot {
                    dir: dir.clone(),
                    label: None,
                }]
            } else {
                discover_roots(&[".dsh", "sessions"])
            }
        })
    }

    fn existing_roots(&self) -> Vec<ScanRoot> {
        self.roots()
            .iter()
            .filter(|r| r.dir.is_dir())
            .cloned()
            .collect()
    }

    /// Stream one zstd-compressed session file line-by-line, carrying the
    /// cross-line state (session id, project, current model) and emitting one
    /// record per `assistant/message` without holding the whole decompressed
    /// file in memory.
    fn parse_file(
        path: &Path,
        rel: &Path,
        emit: &mut dyn FnMut(UsageRecord),
    ) -> Result<(), String> {
        let mut session_id = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| {
                n.strip_suffix(".jsonl.zstd")
                    .or_else(|| n.strip_suffix(".jsonl"))
                    .unwrap_or(n)
                    .to_string()
            })
            .unwrap_or_default();
        let mut project = String::new();
        let mut model = DEFAULT_MODEL.to_string();

        for_each_session_line(path, |line| {
            if let Some(r) = Self::parse_line(line, &mut session_id, &mut project, &mut model, rel)
            {
                emit(r);
            }
        })
    }

    fn parse_line(
        line: &str,
        session_id: &mut String,
        project: &mut String,
        model: &mut String,
        rel: &Path,
    ) -> Option<UsageRecord> {
        if line.trim().is_empty() {
            return None;
        }
        let value: EventLine = serde_json::from_str(line).ok()?;
        match value.kind.as_deref() {
            Some("session") => {
                if let Some(id) = value.id.filter(|s| !s.is_empty()) {
                    *session_id = id;
                }
                if let Some(cwd) = value.cwd.filter(|s| !s.is_empty()) {
                    *project = Path::new(&cwd)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "unknown".to_string());
                }
                None
            }
            Some("request/header") => {
                if let Some(m) = value
                    .data
                    .as_ref()
                    .and_then(|d| d.header.as_ref())
                    .and_then(|h| h.config.as_ref())
                    .and_then(|c| c.model.as_deref())
                    .filter(|m| !m.is_empty())
                {
                    *model = m.to_string();
                }
                None
            }
            Some("assistant/message") => {
                Self::message_record(&value, session_id, project, model, rel)
            }
            _ => None,
        }
    }

    fn message_record(
        value: &EventLine,
        session_id: &str,
        project: &str,
        fallback_model: &str,
        rel: &Path,
    ) -> Option<UsageRecord> {
        let data = value.data.as_ref()?;
        let turn = data.turn.as_ref().and_then(Value::as_i64)?;
        let step = data.step.as_ref().and_then(Value::as_i64)?;
        let usage = data.usage.as_ref()?;
        let input = usage
            .input_tokens
            .as_ref()
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = usage
            .output_tokens
            .as_ref()
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_read = usage
            .cache_read_tokens
            .as_ref()
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_write = usage
            .cache_write_tokens
            .as_ref()
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if input + output + cache_read + cache_write == 0 {
            return None;
        }
        let started_at = value
            .time
            .as_ref()
            .and_then(Value::as_i64)
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single())?;
        let model = data
            .message
            .as_ref()
            .and_then(|m| m.source.as_ref())
            .and_then(|s| s.model.as_deref())
            .filter(|m| !m.is_empty())
            .unwrap_or(fallback_model)
            .to_string();

        Some(UsageRecord::new(
            Provider::DeepSeek,
            if project.is_empty() {
                "unknown".to_string()
            } else {
                project.to_string()
            },
            session_id.to_string(),
            Usage {
                model,
                started_at,
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cache_read,
                cache_write_tokens: cache_write,
                cost_micros: 0, // pricing applied in a later pipeline stage
            },
            0,
            format!("{}:{turn}:{step}", rel.display()),
        ))
    }
}

/// A DeepSeek harness session file: `.jsonl.zstd` (compressed) or `.jsonl`
/// (uncompressed when the harness wrote it without compression).
fn is_harness_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".jsonl.zstd") || n.ends_with(".jsonl"))
}

/// Stream `path` line-by-line (lossy UTF-8, mirroring `source::for_each_line`),
/// decompressing zstd first when the file is `.jsonl.zstd`; never holds the
/// whole file in memory.
fn for_each_session_line(path: &Path, mut on_line: impl FnMut(&str)) -> Result<(), String> {
    let file = File::open(path).map_err(|e| format!("open {path:?}: {e}"))?;
    let compressed = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".jsonl.zstd"));
    let mut reader: Box<dyn BufRead> = if compressed {
        let decoder = zstd::stream::read::Decoder::new(BufReader::new(file))
            .map_err(|e| format!("zstd {path:?}: {e}"))?;
        Box::new(BufReader::new(decoder))
    } else {
        Box::new(BufReader::new(file))
    };
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(e) => return Err(format!("read {path:?}: {e}")),
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        on_line(&String::from_utf8_lossy(&buf));
    }
}

/// Count/stat every `.jsonl.zstd` file under a root, excluding oversized files
/// exactly like a full scan. Shared by `scan` and `scan_fingerprint` so the
/// cheap change detector and a full scan always agree.
fn root_stats(root: &ScanRoot, max_depth: usize, max_file_size: u64) -> (u64, i64, u64) {
    let mut found = 0u64;
    let mut max_mtime = 0i64;
    let mut total_bytes = 0u64;
    for entry in WalkDir::new(&root.dir)
        .max_depth(max_depth)
        .follow_links(false)
    {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() || !is_harness_file(entry.path()) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(entry.path()) else {
            continue;
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
    (found, max_mtime, total_bytes)
}

impl ProviderSource for DeepSeekSource {
    fn provider(&self) -> Provider {
        Provider::DeepSeek
    }

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let dirs: Vec<PathBuf> = self.existing_roots().into_iter().map(|r| r.dir).collect();
        if dirs.is_empty() {
            Err(ProviderError::DataDirNotFound(Provider::DeepSeek))
        } else {
            Ok(dirs)
        }
    }

    fn scan(&self, emit: &mut dyn FnMut(UsageRecord)) -> Result<ScanOutput, ProviderError> {
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::DeepSeek));
        }
        let mut errors = Vec::new();
        let mut found_files = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        for root in &roots {
            let (found, mtime, bytes) =
                root_stats(root, self.config.max_depth, self.config.max_file_size);
            found_files += found;
            max_mtime = max_mtime.max(mtime);
            total_bytes += bytes;

            for entry in WalkDir::new(&root.dir)
                .max_depth(self.config.max_depth)
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
                if !is_harness_file(path) {
                    continue;
                }
                let meta = match std::fs::metadata(path) {
                    Ok(m) => m,
                    Err(e) => {
                        errors.push(format!("metadata {path:?}: {e}"));
                        continue;
                    }
                };
                if meta.len() > self.config.max_file_size {
                    errors.push(format!("skip oversized {path:?}"));
                    continue;
                }
                // Namespace the dedup key per root so identical session files
                // from the local home and WSL distros never collide.
                let rel = path.strip_prefix(&root.dir).unwrap_or(path);
                let rel = match &root.label {
                    Some(label) => Path::new(label).join(rel),
                    None => rel.to_path_buf(),
                };
                if let Err(e) = Self::parse_file(path, &rel, emit) {
                    errors.push(e);
                }
            }
        }
        Ok(ScanOutput {
            found_files,
            fingerprint: fingerprint(found_files, max_mtime, total_bytes),
            errors,
            ..Default::default()
        })
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::DeepSeek));
        }
        let mut found = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        for root in &roots {
            let (f, m, b) = root_stats(root, self.config.max_depth, self.config.max_file_size);
            found += f;
            max_mtime = max_mtime.max(m);
            total_bytes += b;
        }
        Ok(fingerprint(found, max_mtime, total_bytes))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::*;

    /// Compress a JSONL body into a `*.jsonl.zstd` file under `dir`.
    fn write_harness_file(dir: &Path, name: &str, jsonl: &str) {
        let compressed = zstd::bulk::compress(jsonl.as_bytes(), 3).unwrap();
        fs::write(dir.join(name), compressed).unwrap();
    }

    fn source_for(dir: &Path) -> DeepSeekSource {
        DeepSeekSource::new(ProviderConfig {
            provider: Provider::DeepSeek,
            data_dir_override: Some(dir.to_path_buf()),
            ..ProviderConfig::default()
        })
    }

    fn scan_collect(src: &DeepSeekSource) -> (ScanOutput, Vec<UsageRecord>) {
        let mut records = Vec::new();
        let out = src.scan(&mut |r| records.push(r)).unwrap();
        (out, records)
    }

    fn message_event(time: i64, turn: i64, step: i64, model: &str) -> String {
        format!(
            r#"{{"type":"assistant/message","time":{time},"data":{{"turn":{turn},"step":{step},"usage":{{"inputTokens":100,"outputTokens":40,"cacheReadTokens":1000,"cacheWriteTokens":5,"reasoningTokens":15}},"message":{{"source":{{"kind":"model","provider":"deepseek-official","model":"{model}"}}}}}}}}"#
        )
    }

    #[test]
    fn parses_final_message_into_record() {
        let dir = tempdir().unwrap();
        let body = format!(
            "{}\n{}\n{}\n{}\n",
            r#"{"type":"session","id":"session-1","cwd":"/tmp/deepseek-project","createdAt":1704672000000}"#,
            r#"{"type":"request/header","time":1704671999000,"data":{"header":{"config":{"provider":"deepseek-official","model":"deepseek-v4-pro"}}}}"#,
            r#"{"type":"assistant/chunk","time":1704672000000,"data":{"turn":1,"step":2,"chunk":{"type":"usage","usage":{"inputTokens":7,"outputTokens":9,"cacheReadTokens":11,"reasoningTokens":5}}}}"#,
            message_event(1704672001000, 1, 2, "deepseek-v4-pro"),
        );
        write_harness_file(dir.path(), "session-1.jsonl.zstd", &body);

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1, "one harness file per root");
        assert!(out.errors.is_empty());
        // Only the final message is emitted; the streaming chunk is ignored.
        assert_eq!(records.len(), 1);

        let r = &records[0];
        assert_eq!(r.provider, Provider::DeepSeek);
        assert_eq!(r.project, "deepseek-project");
        assert_eq!(r.session_id, "session-1");
        assert_eq!(r.usage.model, "deepseek-v4-pro");
        // outputTokens (40) already includes reasoningTokens (15); it is kept
        // whole so total and cost match tokei's raw_out handling.
        assert_eq!(r.usage.input_tokens, 100);
        assert_eq!(r.usage.output_tokens, 40);
        assert_eq!(r.usage.cache_read_tokens, 1000);
        assert_eq!(r.usage.cache_write_tokens, 5);
        assert_eq!(r.usage.total_tokens(), 1145);
        // Dedup key is `<rel>:<turn>:<step>`, stable across rescans.
        assert!(r.fingerprint.ends_with(":1:2"));
    }

    #[test]
    fn parses_uncompressed_jsonl_session() {
        let dir = tempdir().unwrap();
        // The harness may write an uncompressed `session.jsonl` (no zstd).
        let body = format!(
            "{}\n{}\n",
            r#"{"type":"session","id":"session-plain","cwd":"/tmp/plain-project"}"#,
            message_event(1704672001000, 1, 1, "deepseek-v4-pro"),
        );
        fs::write(dir.path().join("session.jsonl"), body).unwrap();

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1, "uncompressed jsonl is counted");
        assert!(out.errors.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "session-plain");
        assert_eq!(records[0].project, "plain-project");
        assert_eq!(records[0].usage.total_tokens(), 1145);
        // Fingerprint retains the on-disk file name (no .zstd rewrite).
        assert!(records[0].fingerprint.starts_with("session.jsonl:"));
    }

    #[test]
    fn skips_chunk_and_zero_usage_messages() {
        let dir = tempdir().unwrap();
        write_harness_file(
            dir.path(),
            "interrupted.jsonl.zstd",
            r#"{"type":"assistant/chunk","time":1704672000000,"data":{"turn":1,"step":1,"chunk":{"type":"usage","usage":{"inputTokens":7,"outputTokens":9,"cacheReadTokens":11}}}}
"#,
        );
        // A completed message with all-zero buckets is also skipped.
        write_harness_file(
            dir.path(),
            "empty.jsonl.zstd",
            r#"{"type":"assistant/message","time":1704672000000,"data":{"turn":1,"step":1,"usage":{"inputTokens":0,"outputTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0}}}}
"#,
        );

        let (_, records) = scan_collect(&source_for(dir.path()));
        assert!(records.is_empty());
    }

    #[test]
    fn falls_back_to_header_model_and_session_id() {
        let dir = tempdir().unwrap();
        let body = format!(
            "{}\n{}\n",
            r#"{"type":"request/header","time":1704671999000,"data":{"header":{"config":{"model":"deepseek-v4-flash"}}}}"#,
            // Message source omits the model, so the header value wins.
            r#"{"type":"assistant/message","time":1704672000000,"data":{"turn":3,"step":4,"usage":{"inputTokens":7,"outputTokens":9,"cacheReadTokens":11},"message":{"source":{"kind":"model"}}}}"#,
        );
        write_harness_file(dir.path(), "session-2.jsonl.zstd", &body);

        let (_, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].usage.model, "deepseek-v4-flash");
        // No `session` event: session id falls back to the file name.
        assert_eq!(records[0].session_id, "session-2");
    }

    #[test]
    fn data_dir_missing_is_data_dir_not_found() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("no-such-dir");
        let err = source_for(&missing).data_dirs().unwrap_err();
        assert!(matches!(
            err,
            ProviderError::DataDirNotFound(Provider::DeepSeek)
        ));
    }

    #[test]
    fn namespaces_wsl_root_fingerprints() {
        let line = message_event(1704672000000, 1, 2, "deepseek-v4-pro");
        let value: EventLine = serde_json::from_str(&line).unwrap();

        let local = DeepSeekSource::message_record(
            &value,
            "sid",
            "proj",
            "deepseek-v4-pro",
            Path::new("session.jsonl.zstd"),
        )
        .unwrap();
        let remote = DeepSeekSource::message_record(
            &value,
            "sid",
            "proj",
            "deepseek-v4-pro",
            &Path::new("wsl")
                .join("Ubuntu-20.04")
                .join("zhy")
                .join("session.jsonl.zstd"),
        )
        .unwrap();

        assert_ne!(local.fingerprint, remote.fingerprint);
        assert!(local.fingerprint.starts_with("session.jsonl.zstd:"));
        assert!(Path::new(&remote.fingerprint).starts_with(Path::new("wsl")));
    }

    #[test]
    fn fingerprint_changes_when_file_rewritten() {
        let dir = tempdir().unwrap();
        let src = source_for(dir.path());
        write_harness_file(
            dir.path(),
            "session.jsonl.zstd",
            &format!(
                "{}\n",
                message_event(1704672000000, 1, 1, "deepseek-v4-pro")
            ),
        );

        let fp1 = src.scan_fingerprint().unwrap();
        let fp2 = src.scan_fingerprint().unwrap();
        assert_eq!(fp1, fp2, "unchanged tree keeps the same fingerprint");

        // Rewriting the file changes size/mtime, so the scheduler rescans.
        // mtime only has one-second resolution, so wait out the current second.
        std::thread::sleep(std::time::Duration::from_secs(2));
        write_harness_file(
            dir.path(),
            "session.jsonl.zstd",
            &format!(
                "{}\n{}\n",
                message_event(1704672000000, 1, 1, "deepseek-v4-pro"),
                message_event(1704672000000, 2, 1, "deepseek-v4-pro"),
            ),
        );
        assert_ne!(fp1, src.scan_fingerprint().unwrap());
    }
}
