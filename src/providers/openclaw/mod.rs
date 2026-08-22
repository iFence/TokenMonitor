//! OpenClaw data source: reads the per-agent JSONL session logs at
//! `~/.openclaw/agents/<agent>/sessions/<session-id>.jsonl`.
//!
//! OpenClaw (openclaw.ai) appends one JSON line per session event. A `session`
//! header carries the session id and working directory; `message` events embed
//! the token `usage` on assistant messages
//! (`message.{input,output,cacheRead,cacheWrite,reasoningTokens}`), so the
//! adapter emits one `UsageRecord` per assistant message with non-zero usage —
//! matching the per-request granularity of the other adapters. Session files
//! are append-only, so the `<rel>:<message id>` dedup fingerprints stay stable
//! across rescans, and the session files' own stats drive the cheap change
//! detector. On Windows the local home plus every WSL distro's home are scanned
//! (see `discover_roots`); the pre-rebrand state dir (`~/.clawdbot`) is read as
//! a labelled legacy root so its records never collide with `.openclaw`'s.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use walkdir::WalkDir;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::roots::discover_roots;
use super::source::{
    fingerprint, for_each_line, ProviderConfig, ProviderError, ProviderSource, ScanOutput, ScanRoot,
};

/// Per-agent session logs live in this directory (one JSONL file per session).
const SESSIONS_DIR: &str = "sessions";

/// Normalized token buckets extracted from one assistant message's `usage`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Tokens {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

/// First numeric field found among `paths` (dot-separated JSON paths).
fn pick_number(v: &Value, paths: &[&str]) -> Option<i64> {
    for path in paths {
        let mut cur = v;
        let mut found = true;
        for part in path.split('.') {
            match cur.get(part) {
                Some(next) => cur = next,
                None => {
                    found = false;
                    break;
                }
            }
        }
        if !found {
            continue;
        }
        if let Some(n) = cur.as_i64().or_else(|| cur.as_f64().map(|f| f as i64)) {
            return Some(n);
        }
    }
    None
}

/// True when any `path` exists and is not null (used for field-presence checks).
fn has_any(v: &Value, paths: &[&str]) -> bool {
    paths.iter().any(|path| {
        let mut cur = v;
        for part in path.split('.') {
            match cur.get(part) {
                Some(next) => cur = next,
                None => return false,
            }
        }
        !cur.is_null()
    })
}

/// Parse a provider usage payload into normalized buckets. OpenClaw's own
/// normalized shape (`input`/`output`/`cacheRead`/`cacheWrite`, where `input`
/// is already uncached) is the common case; raw OpenAI-style shapes where
/// `prompt_tokens`/`input_tokens` include the cached portion are re-split so
/// TokenMonitor's buckets stay additive (`input + cacheRead + cacheWrite`). Reasoning
/// tokens are folded into output, the convention OpenClaw's own conversions use.
fn parse_usage(u: &Value) -> Option<Tokens> {
    const CACHE_READ_KEYS: &[&str] = &[
        "cacheRead",
        "cache_read",
        "cache_read_input_tokens",
        "cached_tokens",
        "input_tokens_details.cached_tokens",
        "prompt_tokens_details.cached_tokens",
    ];
    const CACHE_WRITE_KEYS: &[&str] = &[
        "cacheWrite",
        "cache_write",
        "cache_creation_input_tokens",
        "input_tokens_details.cache_write_tokens",
        "prompt_tokens_details.cache_write_tokens",
    ];
    const INPUT_KEYS: &[&str] = &[
        "input",
        "inputTokens",
        "input_tokens",
        "promptTokens",
        "prompt_tokens",
        "prompt_n",
        "timings.prompt_n",
    ];
    const OUTPUT_KEYS: &[&str] = &[
        "output",
        "outputTokens",
        "output_tokens",
        "completionTokens",
        "completion_tokens",
        "predicted_n",
        "timings.predicted_n",
    ];
    const REASONING_KEYS: &[&str] = &[
        "reasoningTokens",
        "reasoning_tokens",
        "completion_tokens_details.reasoning_tokens",
        "output_tokens_details.reasoning_tokens",
        "output_tokens_details.thinking_tokens",
    ];

    let cache_read = pick_number(u, CACHE_READ_KEYS).unwrap_or(0).max(0) as u64;
    let cache_write = pick_number(u, CACHE_WRITE_KEYS).unwrap_or(0).max(0) as u64;
    let raw_input = pick_number(u, INPUT_KEYS);
    let output = pick_number(u, OUTPUT_KEYS).unwrap_or(0).max(0) as u64;
    let reasoning = pick_number(u, REASONING_KEYS).unwrap_or(0).max(0) as u64;

    // OpenAI-style prompt/input totals include cached tokens; re-split them so
    // TokenMonitor's buckets stay additive. The inclusion flags mirror OpenClaw's own
    // `normalizeUsage`: Anthropic-style `cache_creation_input_tokens` is billed
    // separately from `input`, so it never triggers a subtraction.
    let direct_input = u.get("input").and_then(Value::as_i64);
    let openai_cache_read = has_any(
        u,
        &[
            "cached_tokens",
            "input_tokens_details.cached_tokens",
            "prompt_tokens_details.cached_tokens",
        ],
    );
    let cache_write_in_input = has_any(
        u,
        &[
            "input_tokens_details.cache_write_tokens",
            "prompt_tokens_details.cache_write_tokens",
            "cache_write_input_tokens",
            "cached_input_tokens",
        ],
    );
    let subtract_cache_read = openai_cache_read
        || (direct_input.is_none() && has_any(u, &["cached_input_tokens", "cached"]));
    let normalized_input = raw_input.unwrap_or(0)
        - if subtract_cache_read {
            cache_read as i64
        } else {
            0
        }
        - if direct_input.is_none() && cache_write_in_input {
            cache_write as i64
        } else {
            0
        };

    let input = normalized_input.max(0) as u64;
    let output = output.saturating_add(reasoning);
    if input + output + cache_read + cache_write == 0 {
        return None;
    }
    Some(Tokens {
        input,
        output,
        cache_read,
        cache_write,
    })
}

pub struct OpenClawSource {
    config: ProviderConfig,
    /// Discovered data roots, computed lazily on the first scan (background
    /// thread) so WSL discovery never blocks UI startup.
    roots: OnceLock<Vec<ScanRoot>>,
}

impl OpenClawSource {
    pub fn new(config: ProviderConfig) -> Self {
        OpenClawSource {
            config,
            roots: OnceLock::new(),
        }
    }

    fn roots(&self) -> &[ScanRoot] {
        self.roots.get_or_init(|| {
            if let Some(dir) = &self.config.data_dir_override {
                return vec![ScanRoot {
                    dir: dir.clone(),
                    label: None,
                }];
            }
            let mut roots = discover_roots(&[".openclaw"]);
            // Pre-rebrand state dir (`~/.clawdbot`): labelled so records from it
            // never collide with the `.openclaw` root's fingerprints.
            for root in discover_roots(&[".clawdbot"]) {
                let label = match root.label {
                    Some(wsl) => format!("clawdbot/{wsl}"),
                    None => "clawdbot".to_string(),
                };
                roots.push(ScanRoot {
                    dir: root.dir,
                    label: Some(label),
                });
            }
            roots
        })
    }

    fn existing_roots(&self) -> Vec<ScanRoot> {
        self.roots()
            .iter()
            .filter(|r| r.dir.is_dir())
            .cloned()
            .collect()
    }

    /// Every per-agent session JSONL file under one root. Shared by `scan` and
    /// `scan_fingerprint` so the cheap check and a full scan always agree on the
    /// file set. The full trajectory traces (`*.trajectory.jsonl`) are not the
    /// session log and are excluded, as are any `.jsonl` files outside a
    /// `sessions` directory.
    fn session_files(root: &ScanRoot, max_depth: usize) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in WalkDir::new(&root.dir)
            .max_depth(max_depth)
            .follow_links(false)
        {
            let Ok(entry) = entry else {
                continue;
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path
                .extension()
                .is_none_or(|e| !e.eq_ignore_ascii_case("jsonl"))
            {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".trajectory.jsonl") {
                continue;
            }
            if path
                .parent()
                .and_then(|p| p.file_name())
                .is_none_or(|p| p != SESSIONS_DIR)
            {
                continue;
            }
            files.push(path.to_path_buf());
        }
        files
    }

    /// Stream one record per assistant message with non-zero usage. The session
    /// header (first line) supplies the session id and working directory, which
    /// every later message line shares.
    fn scan_file(path: &Path, rel: &Path, emit: &mut dyn FnMut(UsageRecord)) -> Result<(), String> {
        let mut session_id: Option<String> = None;
        let mut cwd: Option<String> = None;
        for_each_line(path, |line, _| {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => return,
            };
            match v.get("type").and_then(Value::as_str) {
                Some("session") => {
                    session_id = v.get("id").and_then(Value::as_str).map(str::to_owned);
                    cwd = v.get("cwd").and_then(Value::as_str).map(str::to_owned);
                }
                Some("message") => {
                    if let Some(r) = Self::record_from_message(
                        &v,
                        line.len() as u64,
                        rel,
                        session_id.as_deref(),
                        cwd.as_deref(),
                    ) {
                        emit(r);
                    }
                }
                _ => {}
            }
        })
    }

    fn record_from_message(
        event: &Value,
        raw_bytes: u64,
        rel: &Path,
        session_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Option<UsageRecord> {
        let message = event.get("message")?;
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            return None;
        }
        let tokens = parse_usage(message.get("usage")?)?;

        let started_at = event
            .get("timestamp")
            .and_then(Self::timestamp_millis)
            .or_else(|| message.get("timestamp").and_then(Value::as_i64))
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
            .unwrap_or_else(Utc::now);
        let model = message
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let project = cwd
            .and_then(|cwd| {
                Path::new(cwd)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "unknown".to_string());
        let session = session_id.unwrap_or_default().to_string();
        let id = event.get("id").and_then(Value::as_str).unwrap_or_default();

        Some(UsageRecord::new(
            Provider::OpenClaw,
            project,
            session,
            Usage {
                model,
                started_at,
                input_tokens: tokens.input,
                output_tokens: tokens.output,
                cache_read_tokens: tokens.cache_read,
                cache_write_tokens: tokens.cache_write,
                cost_micros: 0, // pricing applied in a later pipeline stage
            },
            raw_bytes,
            format!("{}:{}", rel.display(), id),
        ))
    }

    /// `event.timestamp` is either an ISO-8601 string or a millisecond integer.
    fn timestamp_millis(v: &Value) -> Option<i64> {
        if let Some(ms) = v.as_i64() {
            return Some(ms);
        }
        let s = v.as_str()?;
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.timestamp_millis())
    }

    /// Change-detection stats over one session file. Shared by `scan` and
    /// `scan_fingerprint` so the cheap check and a full scan always agree.
    fn file_stats(path: &Path) -> (u64, i64, u64) {
        let Ok(meta) = std::fs::metadata(path) else {
            return (0, 0, 0);
        };
        let mut max_mtime = 0i64;
        if let Ok(modified) = meta.modified() {
            if let Ok(unix) = modified.duration_since(std::time::UNIX_EPOCH) {
                max_mtime = unix.as_secs() as i64;
            }
        }
        (1, max_mtime, meta.len())
    }
}

impl ProviderSource for OpenClawSource {
    fn provider(&self) -> Provider {
        Provider::OpenClaw
    }

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let dirs: Vec<PathBuf> = self.existing_roots().into_iter().map(|r| r.dir).collect();
        if dirs.is_empty() {
            Err(ProviderError::DataDirNotFound(Provider::OpenClaw))
        } else {
            Ok(dirs)
        }
    }

    fn scan(&self, emit: &mut dyn FnMut(UsageRecord)) -> Result<ScanOutput, ProviderError> {
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::OpenClaw));
        }
        let mut errors = Vec::new();
        let mut found_files = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;

        for root in &roots {
            for path in Self::session_files(root, self.config.max_depth) {
                let (found, mtime, bytes) = Self::file_stats(&path);
                found_files += found;
                max_mtime = max_mtime.max(mtime);
                total_bytes += bytes;

                let rel = path.strip_prefix(&root.dir).unwrap_or(&path).to_path_buf();
                let rel = match &root.label {
                    Some(label) => Path::new(label).join(rel),
                    None => rel,
                };
                if let Err(e) = Self::scan_file(&path, &rel, emit) {
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
            return Err(ProviderError::DataDirNotFound(Provider::OpenClaw));
        }
        let mut found = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        for root in &roots {
            for path in Self::session_files(root, self.config.max_depth) {
                let (f, m, b) = Self::file_stats(&path);
                found += f;
                max_mtime = max_mtime.max(m);
                total_bytes += b;
            }
        }
        Ok(fingerprint(found, max_mtime, total_bytes))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::*;

    fn session_header(id: &str, cwd: &str) -> String {
        format!(
            r#"{{"type":"session","version":3,"id":"{id}","timestamp":"2026-08-16T10:00:00Z","cwd":"{cwd}"}}"#
        )
    }

    fn assistant_line(msg_id: &str, model: &str, usage: &str) -> String {
        format!(
            r#"{{"type":"message","id":"{msg_id}","parentId":"p","timestamp":"2026-08-16T10:00:01Z","message":{{"role":"assistant","model":"{model}","usage":{usage}}}}}"#,
        )
    }

    fn user_line(msg_id: &str) -> String {
        format!(
            r#"{{"type":"message","id":"{msg_id}","parentId":"p","timestamp":"2026-08-16T10:00:01Z","message":{{"role":"user","content":"hello"}}}}"#,
        )
    }

    /// Create `agents/<agent>/sessions/<session-id>.jsonl` under `dir`.
    fn write_session(dir: &Path, agent: &str, session_id: &str, lines: &[&str]) -> PathBuf {
        let dir = dir.join("agents").join(agent).join(SESSIONS_DIR);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{session_id}.jsonl"));
        fs::write(&file, format!("{}\n", lines.join("\n"))).unwrap();
        file
    }

    fn source_for(dir: &Path) -> OpenClawSource {
        OpenClawSource::new(ProviderConfig {
            provider: Provider::OpenClaw,
            data_dir_override: Some(dir.to_path_buf()),
            ..ProviderConfig::default()
        })
    }

    fn scan_collect(src: &OpenClawSource) -> (ScanOutput, Vec<UsageRecord>) {
        let mut records = Vec::new();
        let out = src.scan(&mut |r| records.push(r)).unwrap();
        (out, records)
    }

    #[test]
    fn parses_assistant_messages_with_normalized_usage() {
        let dir = tempdir().unwrap();
        write_session(
            dir.path(),
            "dev",
            "ses_1",
            &[
                &session_header("ses_1", "/home/u/openclawSpace/workspaces/dev"),
                &assistant_line(
                    "m1",
                    "deepseek-v4-pro",
                    r#"{"input":100,"output":20,"cacheRead":5,"cacheWrite":3}"#,
                ),
                &assistant_line(
                    "m2",
                    "deepseek-v4-pro",
                    r#"{"input":10,"output":2,"cacheRead":0,"cacheWrite":0}"#,
                ),
            ],
        );

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1, "one session file per session");
        assert!(out.errors.is_empty());
        assert_eq!(records.len(), 2);

        let first = &records[0];
        assert_eq!(first.provider, Provider::OpenClaw);
        assert_eq!(
            first.project, "dev",
            "cwd basename drives the project column"
        );
        assert_eq!(first.session_id, "ses_1");
        assert_eq!(first.usage.model, "deepseek-v4-pro");
        assert_eq!(first.usage.input_tokens, 100);
        assert_eq!(first.usage.output_tokens, 20);
        assert_eq!(first.usage.cache_read_tokens, 5);
        assert_eq!(first.usage.cache_write_tokens, 3);
        assert_eq!(first.usage.total_tokens(), 128);
        assert!(first.fingerprint.ends_with(":m1"));
    }

    #[test]
    fn skips_non_assistant_and_zero_usage_events() {
        let dir = tempdir().unwrap();
        write_session(
            dir.path(),
            "dev",
            "ses_1",
            &[
                &session_header("ses_1", "/workspaces/dev"),
                &user_line("m_u"),
                // Zero-usage assistant message (aborted or not yet billed).
                &assistant_line(
                    "m_0",
                    "m",
                    r#"{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}"#,
                ),
                &assistant_line(
                    "m_1",
                    "m",
                    r#"{"input":50,"output":0,"cacheRead":0,"cacheWrite":0}"#,
                ),
            ],
        );

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert!(out.errors.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].usage.input_tokens, 50);
    }

    #[test]
    fn handles_openai_style_usage_with_cached_input() {
        let dir = tempdir().unwrap();
        // prompt_tokens includes the cached portion; TokenMonitor re-splits it.
        let data = assistant_line(
            "m1",
            "gpt-5.6",
            r#"{"prompt_tokens":110,"completion_tokens":20,"prompt_tokens_details":{"cached_tokens":90}}"#,
        );
        write_session(
            dir.path(),
            "dev",
            "ses_1",
            &[&session_header("ses_1", "/p"), &data],
        );

        let (_, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(records.len(), 1);
        let u = &records[0].usage;
        assert_eq!(u.input_tokens, 20);
        assert_eq!(u.cache_read_tokens, 90);
        assert_eq!(u.output_tokens, 20);
        assert_eq!(u.total_tokens(), 130);
    }

    #[test]
    fn folds_reasoning_tokens_into_output() {
        let dir = tempdir().unwrap();
        let data = assistant_line(
            "m1",
            "m",
            r#"{"input":10,"output":5,"reasoningTokens":4,"cacheRead":0,"cacheWrite":0}"#,
        );
        write_session(
            dir.path(),
            "dev",
            "ses_1",
            &[&session_header("ses_1", "/p"), &data],
        );

        let (_, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(records[0].usage.output_tokens, 9);
        assert_eq!(records[0].usage.total_tokens(), 19);
    }

    #[test]
    fn uses_message_timestamp_when_event_has_none() {
        let dir = tempdir().unwrap();
        let line = format!(
            r#"{{"type":"message","id":"m1","parentId":"p","message":{{"role":"assistant","model":"m","usage":{{"input":1,"output":1}},"timestamp":1786800000000}}}}"#
        );
        write_session(
            dir.path(),
            "dev",
            "ses_1",
            &[&session_header("ses_1", "/p"), &line],
        );

        let (_, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].usage.started_at.timestamp_millis(),
            1786800000000
        );
    }

    #[test]
    fn namespaces_wsl_root_fingerprints() {
        let event = serde_json::json!({
            "type": "message",
            "id": "m1",
            "message": { "role": "assistant", "model": "m", "usage": { "input": 1, "output": 1 } }
        });

        let local = OpenClawSource::record_from_message(
            &event,
            0,
            &Path::new("agents/x/sessions/ses.jsonl"),
            Some("ses"),
            Some("/p"),
        )
        .unwrap();
        let remote = OpenClawSource::record_from_message(
            &event,
            0,
            &Path::new("wsl")
                .join("Ubuntu-20.04")
                .join("zhy")
                .join("agents/x/sessions/ses.jsonl"),
            Some("ses"),
            Some("/p"),
        )
        .unwrap();
        let legacy_rec = OpenClawSource::record_from_message(
            &event,
            0,
            &Path::new("clawdbot").join("agents/x/sessions/ses.jsonl"),
            Some("ses"),
            Some("/p"),
        )
        .unwrap();

        assert_ne!(local.fingerprint, remote.fingerprint);
        assert_ne!(local.fingerprint, legacy_rec.fingerprint);
        assert!(local
            .fingerprint
            .starts_with("agents/x/sessions/ses.jsonl:"));
        assert!(Path::new(&remote.fingerprint)
            .starts_with(Path::new("wsl").join("Ubuntu-20.04").join("zhy")));
        assert!(Path::new(&legacy_rec.fingerprint).starts_with(Path::new("clawdbot")));
    }

    #[test]
    fn data_dir_missing_is_data_dir_not_found() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("no-such-dir");
        let err = source_for(&missing).data_dirs().unwrap_err();
        assert!(matches!(
            err,
            ProviderError::DataDirNotFound(Provider::OpenClaw)
        ));
    }

    #[test]
    fn ignores_trajectory_and_unrelated_jsonl_files() {
        let dir = tempdir().unwrap();
        write_session(
            dir.path(),
            "dev",
            "ses_1",
            &[
                &session_header("ses_1", "/p"),
                &assistant_line("m1", "m", r#"{"input":1,"output":1}"#),
            ],
        );
        let sessions = dir.path().join("agents").join("dev").join(SESSIONS_DIR);
        // Full trace files and JSONL outside a `sessions` dir must be ignored.
        fs::write(sessions.join("ses_1.trajectory.jsonl"), "{}\n").unwrap();
        fs::write(dir.path().join("unrelated.jsonl"), "{}\n").unwrap();

        let src = source_for(dir.path());
        let (out, records) = scan_collect(&src);
        assert_eq!(out.found_files, 1, "only the real session file counts");
        assert_eq!(records.len(), 1);
        assert_eq!(src.scan_fingerprint().unwrap(), out.fingerprint);
    }

    #[test]
    fn fingerprint_changes_when_session_rewritten() {
        let dir = tempdir().unwrap();
        let src = source_for(dir.path());
        let file = write_session(
            dir.path(),
            "dev",
            "ses_1",
            &[
                &session_header("ses_1", "/p"),
                &assistant_line("m1", "m", r#"{"input":1,"output":0}"#),
            ],
        );

        let fp1 = src.scan_fingerprint().unwrap();
        let fp2 = src.scan_fingerprint().unwrap();
        assert_eq!(fp1, fp2, "unchanged store keeps the same fingerprint");

        // Appending a new message changes the fingerprint, so the scheduler
        // rescans and picks up the new record. The fingerprint only has
        // one-second resolution, so wait out the current second first.
        std::thread::sleep(Duration::from_secs(2));
        fs::write(
            &file,
            format!(
                "{}\n{}\n{}\n",
                session_header("ses_1", "/p"),
                assistant_line("m1", "m", r#"{"input":1,"output":0}"#),
                assistant_line("m2", "m", r#"{"input":2,"output":0}"#)
            ),
        )
        .unwrap();
        assert_ne!(fp1, src.scan_fingerprint().unwrap());
    }
}
