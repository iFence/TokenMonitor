//! Pi data source: parses `~/.pi/agent/sessions/**/*.jsonl`.
//!
//! Pi (`@earendil-works/pi-coding-agent`) appends one JSON entry per line to a
//! session file per working directory (`~/.pi/agent/sessions/--<path>--/`).
//! Token usage lives in three places:
//!
//! - `message` entries with an `assistant` role — one record per completed
//!   model turn, carrying its own `model` and `usage`;
//! - `message` entries with a `toolResult` role that embeds a nested `usage`
//!   (LLM work performed by a tool, e.g. summarization);
//! - `compaction` / `branch_summary` entries with a `usage` (LLM-generated
//!   summaries).
//!
//! All three are included — exactly the set pi itself sums into its session
//! totals — so TokenMonitor's numbers match the tool's own counters. Tool and
//! summary usage carry no model of their own, so they are attributed to the
//! last model in effect (tracked from `model_change` entries and assistant
//! messages) for pricing.
//!
//! Pi's `usage.input` already includes its separate `reasoning` bucket, so the
//! four TokenMonitor buckets are used as-is and stay disjoint: `total_tokens()`
//! agrees with pi's own `totalTokens`. TokenMonitor's dedup is `INSERT OR IGNORE`
//! and pi entry `id`s are stable, so rescans (and in-place session rewrites)
//! stay idempotent.
//!
//! On Windows, sessions are also discovered inside every WSL distro's home dir
//! (`\wsl.localhost/<distro>/home/<user>/.pi/agent/sessions`) via the shared
//! root discovery. Each distro root is labelled `wsl/<distro>/<user>`, so
//! identically-named session files on the local home and a distro never
//! collide in the dedup fingerprints.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::roots::discover_roots;
#[cfg(test)]
use super::source::scan_roots;
use super::source::{
    for_each_line, roots_fingerprint, scan_roots_incremental, FileStates, ProviderConfig,
    ProviderError, ProviderSource, ScanOutput, ScanRoot,
};

/// One session JSONL entry. Fields we don't name — assistant text, tool
/// outputs, summaries, retained tails — are skipped by serde without being
/// allocated.
#[derive(Deserialize)]
struct PiLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Stable per-entry id (8-char hex) — the dedup key component.
    id: Option<String>,
    /// Entry append time, RFC 3339.
    timestamp: Option<String>,
    /// `session` header field.
    cwd: Option<String>,
    /// `message` entries.
    message: Option<Message>,
    /// `usage` on `compaction` / `branch_summary` entries.
    usage: Option<UsagePayload>,
    /// `model_change` entry field.
    #[serde(rename = "modelId")]
    model_id: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    role: Option<String>,
    model: Option<String>,
    usage: Option<UsagePayload>,
    /// Request start time in Unix ms.
    #[serde(rename = "timestamp")]
    timestamp_ms: Option<i64>,
}

/// Pi usage buckets. `input` already includes `reasoning`, so it is not
/// folded in again (see module docs).
#[derive(Deserialize)]
struct UsagePayload {
    #[serde(rename = "input")]
    input: Option<u64>,
    #[serde(rename = "output")]
    output: Option<u64>,
    #[serde(rename = "cacheRead")]
    cache_read: Option<u64>,
    #[serde(rename = "cacheWrite")]
    cache_write: Option<u64>,
}

pub struct PiSource {
    config: ProviderConfig,
    /// Discovered scan roots, computed lazily on the first scan (background
    /// thread) so WSL discovery never blocks UI startup.
    roots: OnceLock<Vec<ScanRoot>>,
}

/// Parsing state carried across the lines of one session file.
struct SessionState {
    /// Session uuid from the header; used as the record's session id.
    session_id: String,
    /// Project = basename of the header's `cwd`.
    project: String,
    /// Last model seen, from `model_change` entries and assistant messages.
    model: String,
}

impl PiSource {
    pub fn new(config: ProviderConfig) -> Self {
        PiSource {
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
                discover_roots(&[".pi", "agent", "sessions"])
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

    /// Stream the file line-by-line, carrying the cross-line parsing state
    /// (session id, project, last model) and emitting one record per usage
    /// carrier without holding the raw text or the full record set in memory.
    fn parse_file(
        path: &Path,
        rel: &Path,
        emit: &mut dyn FnMut(UsageRecord),
    ) -> Result<(), String> {
        let mut state = SessionState {
            session_id: String::new(),
            project: String::new(),
            model: String::new(),
        };
        for_each_line(path, |line, _line_idx| {
            if let Some(r) = Self::parse_line(line, &mut state, rel) {
                emit(r);
            }
        })
    }

    fn parse_line(line: &str, state: &mut SessionState, rel: &Path) -> Option<UsageRecord> {
        if line.trim().is_empty() {
            return None;
        }
        let value: PiLine = serde_json::from_str(line).ok()?;
        let raw_bytes = line.len() as u64;
        match value.kind.as_deref()? {
            // Header line: session id + working directory. Not part of the tree.
            "session" => {
                if let Some(id) = value.id {
                    state.session_id = id;
                }
                state.project = value
                    .cwd
                    .as_deref()
                    .and_then(|cwd| Path::new(cwd).file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "unknown".to_string());
                None
            }
            "model_change" => {
                if let Some(model) = value.model_id {
                    state.model = model;
                }
                None
            }
            "message" => {
                let msg = value.message.as_ref()?;
                match msg.role.as_deref()? {
                    "assistant" => {
                        // Each completed turn carries its own model + usage;
                        // also refresh the "last model" for later tool usage.
                        if let Some(model) = msg.model.as_deref() {
                            if !model.is_empty() {
                                state.model = model.to_string();
                            }
                        }
                        Self::record(&value, msg, state, rel, raw_bytes)
                    }
                    // Nested LLM work performed by a tool: usage with no model
                    // of its own — attributed to the last model in effect.
                    "toolResult" => Self::record(&value, msg, state, rel, raw_bytes),
                    _ => None,
                }
            }
            // LLM-generated summaries: usage with no model of its own.
            "compaction" | "branch_summary" => {
                let usage = value.usage.as_ref()?;
                let entry_id = value.id.as_deref()?;
                let started_at = parse_rfc3339(value.timestamp.as_deref());
                Self::record_usage(entry_id, usage, started_at, state, rel, raw_bytes)
            }
            _ => None,
        }
    }

    fn record(
        value: &PiLine,
        msg: &Message,
        state: &SessionState,
        rel: &Path,
        raw_bytes: u64,
    ) -> Option<UsageRecord> {
        let usage = msg.usage.as_ref()?;
        let entry_id = value.id.as_deref()?;
        // Assistant messages prefer their own model; tool results fall back to
        // the last model in effect.
        let started_at = msg
            .timestamp_ms
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
            .or_else(|| parse_rfc3339(value.timestamp.as_deref()));
        Self::record_usage(entry_id, usage, started_at, state, rel, raw_bytes)
    }

    fn record_usage(
        entry_id: &str,
        usage: &UsagePayload,
        started_at: Option<DateTime<Utc>>,
        state: &SessionState,
        rel: &Path,
        raw_bytes: u64,
    ) -> Option<UsageRecord> {
        let input = usage.input.unwrap_or(0);
        let output = usage.output.unwrap_or(0);
        let cache_read = usage.cache_read.unwrap_or(0);
        let cache_write = usage.cache_write.unwrap_or(0);
        if input + output + cache_read + cache_write == 0 {
            return None;
        }
        Some(UsageRecord::new(
            Provider::Pi,
            state.project.clone(),
            state.session_id.clone(),
            Usage {
                model: state.model.clone(),
                started_at: started_at.unwrap_or_else(Utc::now),
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cache_read,
                cache_write_tokens: cache_write,
                cost_micros: 0, // pricing applied in a later pipeline stage
            },
            raw_bytes,
            format!("{}:{entry_id}", rel.display()),
        ))
    }
}

fn parse_rfc3339(s: Option<&str>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

impl ProviderSource for PiSource {
    fn provider(&self) -> Provider {
        Provider::Pi
    }

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let dirs: Vec<PathBuf> = self.existing_roots().into_iter().map(|r| r.dir).collect();
        if dirs.is_empty() {
            Err(ProviderError::DataDirNotFound(Provider::Pi))
        } else {
            Ok(dirs)
        }
    }

    fn scan(&self, emit: &mut dyn FnMut(UsageRecord)) -> Result<ScanOutput, ProviderError> {
        self.scan_incremental(emit, &FileStates::new())
    }

    fn scan_incremental(
        &self,
        emit: &mut dyn FnMut(UsageRecord),
        known: &FileStates,
    ) -> Result<ScanOutput, ProviderError> {
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::Pi));
        }
        let mut errors = Vec::new();
        let (found_files, fingerprint, file_states) = scan_roots_incremental(
            &roots,
            &self.config,
            emit,
            &mut errors,
            &mut |path, rel, file_emit| Self::parse_file(path, rel, file_emit),
            known,
        );
        Ok(ScanOutput {
            found_files,
            fingerprint,
            file_states: Some(file_states),
            errors,
        })
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::Pi));
        }
        roots_fingerprint(&roots, self.config.max_depth, self.config.max_file_size)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::*;

    /// A session file mirroring the real pi JSONL format: header, a model
    /// change, a user message, then assistant turns carrying usage.
    fn write_session_file(dir: &Path) -> PathBuf {
        let path = dir.join("2026-08-17T07-06-44-886Z_01a00e8b-4b16-7165-ac42-29c02a410ed8.jsonl");
        let body = r#"{"type":"session","version":3,"id":"01a00e8b-4b16-7165-ac42-29c02a410ed8","timestamp":"2026-08-17T07:06:44.886Z","cwd":"C:\\Users\\yulei\\RustProjects\\TokenMonitor"}
{"type":"model_change","id":"1f05f7c8","parentId":null,"timestamp":"2026-08-17T07:09:43.616Z","provider":"deepseek","modelId":"deepseek-v4-pro"}
{"type":"message","id":"5ebe36ba","parentId":"1f05f7c8","timestamp":"2026-08-17T07:09:48.376Z","message":{"role":"user","content":[{"type":"text","text":"say hi"}],"timestamp":1786950588373}}
{"type":"message","id":"6f1a2b3c","parentId":"5ebe36ba","timestamp":"2026-08-17T07:09:48.794Z","message":{"role":"assistant","content":[{"type":"text","text":"hi!"}],"provider":"deepseek","model":"deepseek-v4-pro","usage":{"input":8665,"output":33,"cacheRead":0,"cacheWrite":0,"reasoning":18,"totalTokens":8698,"cost":{"input":0.0037,"output":0.00002,"cacheRead":0,"cacheWrite":0,"total":0.0037}},"stopReason":"stop","timestamp":1786950588794}}
{"type":"message","id":"7d2c3d4e","parentId":"6f1a2b3c","timestamp":"2026-08-17T07:09:49.100Z","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"provider":"deepseek","model":"deepseek-v4-flash","usage":{"input":1171,"output":144,"cacheRead":8832,"cacheWrite":0,"reasoning":32,"totalTokens":10147,"cost":{"input":0.00016,"output":0.00004,"cacheRead":0.00002,"cacheWrite":0,"total":0.00022}},"stopReason":"toolUse","timestamp":1786950589100}}
"#;
        fs::write(&path, body).unwrap();
        path
    }

    fn source_for(dir: &Path) -> PiSource {
        PiSource::new(ProviderConfig {
            provider: Provider::Pi,
            data_dir_override: Some(dir.to_path_buf()),
            ..ProviderConfig::default()
        })
    }

    /// Scan and collect the streamed records, mirroring how tests consume a
    /// provider.
    fn scan_collect(src: &PiSource) -> (ScanOutput, Vec<UsageRecord>) {
        let mut records = Vec::new();
        let out = src.scan(&mut |r| records.push(r)).unwrap();
        (out, records)
    }

    #[test]
    fn parses_assistant_messages_into_records() {
        let dir = tempdir().unwrap();
        write_session_file(dir.path());

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1);
        assert_eq!(records.len(), 2, "one record per assistant turn");
        assert!(out.errors.is_empty());

        let first = &records[0];
        assert_eq!(first.provider, Provider::Pi);
        assert_eq!(first.project, "TokenMonitor");
        assert_eq!(first.session_id, "01a00e8b-4b16-7165-ac42-29c02a410ed8");
        assert_eq!(first.usage.model, "deepseek-v4-pro");
        // `reasoning` is already inside `input`; buckets stay disjoint and
        // total_tokens() matches pi's own totalTokens.
        assert_eq!(first.usage.input_tokens, 8665);
        assert_eq!(first.usage.output_tokens, 33);
        assert_eq!(first.usage.cache_read_tokens, 0);
        assert_eq!(first.usage.cache_write_tokens, 0);
        assert_eq!(first.usage.total_tokens(), 8698);
        // Dedup key is the relative path + entry id.
        assert!(first.fingerprint.ends_with(":6f1a2b3c"));

        let second = &records[1];
        assert_eq!(second.usage.model, "deepseek-v4-flash");
        assert_eq!(second.usage.input_tokens, 1171);
        assert_eq!(second.usage.cache_read_tokens, 8832);
        assert_eq!(second.usage.total_tokens(), 10147);
    }

    #[test]
    fn counts_tool_and_summary_usage_with_last_model() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("s2.jsonl");
        // A toolResult with nested usage, then a compaction with usage; neither
        // carries a model, so both are attributed to the last assistant model.
        let body = r#"{"type":"session","version":3,"id":"s2","timestamp":"2026-08-17T00:00:00.000Z","cwd":"/home/u/proj"}
{"type":"message","id":"a0000001","parentId":null,"timestamp":"2026-08-17T00:00:01.000Z","message":{"role":"assistant","content":[],"provider":"anthropic","model":"claude-opus-4-8","usage":{"input":100,"output":10,"cacheRead":0,"cacheWrite":0,"totalTokens":110,"cost":{"total":0.001}},"stopReason":"stop","timestamp":1000}}
{"type":"message","id":"a0000002","parentId":"a0000001","timestamp":"2026-08-17T00:00:02.000Z","message":{"role":"toolResult","toolCallId":"call_1","toolName":"summarize","content":[{"type":"text","text":"out"}],"usage":{"input":50,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":55,"cost":{"total":0.001}},"isError":false}}
{"type":"compaction","id":"a0000003","parentId":"a0000002","timestamp":"2026-08-17T00:00:03.000Z","summary":"...","tokensBefore":200,"usage":{"input":30,"output":3,"cacheRead":0,"cacheWrite":0,"totalTokens":33,"cost":{"total":0.001}}}
"#;
        fs::write(&path, body).unwrap();

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert!(out.errors.is_empty());
        assert_eq!(records.len(), 3);
        // Tool + compaction usage carry no model; attributed to the last one.
        assert_eq!(records[1].usage.model, "claude-opus-4-8");
        assert_eq!(records[1].usage.total_tokens(), 55);
        assert_eq!(records[2].usage.model, "claude-opus-4-8");
        assert_eq!(records[2].usage.total_tokens(), 33);
    }

    #[test]
    fn skips_user_and_zero_token_messages() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("s3.jsonl");
        let body = r#"{"type":"session","version":3,"id":"s3","timestamp":"2026-08-17T00:00:00.000Z","cwd":"/p"}
{"type":"message","id":"b0000001","parentId":null,"timestamp":"2026-08-17T00:00:01.000Z","message":{"role":"user","content":"hi","timestamp":1}}
{"type":"message","id":"b0000002","parentId":"b0000001","timestamp":"2026-08-17T00:00:02.000Z","message":{"role":"assistant","content":[],"provider":"deepseek","model":"deepseek-v4-flash","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"total":0}},"stopReason":"aborted","timestamp":2}}
{"type":"message","id":"b0000003","parentId":"b0000002","timestamp":"2026-08-17T00:00:03.000Z","message":{"role":"assistant","content":[],"provider":"deepseek","model":"deepseek-v4-flash","usage":{"input":5,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":5,"cost":{"total":0}},"stopReason":"stop","timestamp":3}}
"#;
        fs::write(&path, body).unwrap();

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert!(out.errors.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "s3");
        assert_eq!(records[0].usage.total_tokens(), 5);
    }

    #[test]
    fn data_dir_missing_is_data_dir_not_found() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("no-such-dir");
        let err = source_for(&missing).data_dirs().unwrap_err();
        assert!(matches!(err, ProviderError::DataDirNotFound(Provider::Pi)));
    }

    #[test]
    fn fingerprint_stable_across_rescans() {
        let dir = tempdir().unwrap();
        let src = source_for(dir.path());
        write_session_file(dir.path());

        let fp1 = src.scan_fingerprint().unwrap();
        let fp2 = src.scan_fingerprint().unwrap();
        assert_eq!(
            fp1, fp2,
            "unchanged session files keep the same fingerprint"
        );
    }

    /// Two roots (the local Windows home + a WSL distro) must both stream
    /// records into the same provider, with the labelled root's dedup
    /// fingerprints namespaced so identically-named session files in each root
    /// (the same file name and the same entry ids) don't collide.
    #[test]
    fn merges_wsl_roots_with_namespaced_fingerprints() {
        let local = tempdir().unwrap();
        let wsl = tempdir().unwrap();
        // Same file name in both roots — the collision the label prevents.
        write_session_file(local.path());
        write_session_file(wsl.path());

        let config = ProviderConfig::for_provider(Provider::Pi);
        let roots = vec![
            ScanRoot {
                dir: local.path().to_path_buf(),
                label: None,
            },
            ScanRoot {
                dir: wsl.path().to_path_buf(),
                label: Some("wsl/Ubuntu-20.04/yulei".to_string()),
            },
        ];

        let mut records = Vec::new();
        let mut errors = Vec::new();
        let (found_files, _fingerprint) = scan_roots(
            &roots,
            &config,
            &mut |r| records.push(r),
            &mut errors,
            &mut |path, rel, file_emit| PiSource::parse_file(path, rel, file_emit),
        );

        assert_eq!(found_files, 2);
        assert!(errors.is_empty());
        assert_eq!(records.len(), 4, "two assistant turns per file");

        // The labelled root's fingerprints are namespaced; the primary root's
        // are not. Use a path prefix so this is separator-agnostic.
        let label = Path::new("wsl").join("Ubuntu-20.04").join("yulei");
        let primary: Vec<_> = records
            .iter()
            .filter(|r| !Path::new(&r.fingerprint).starts_with(&label))
            .collect();
        let wsl_records: Vec<_> = records
            .iter()
            .filter(|r| Path::new(&r.fingerprint).starts_with(&label))
            .collect();
        assert_eq!(primary.len(), 2);
        assert_eq!(wsl_records.len(), 2);
        assert_eq!(primary.len() + wsl_records.len(), records.len());
    }
}
