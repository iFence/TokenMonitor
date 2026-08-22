//! Antigravity data source: parses per-conversation SQLite stores from both
//! the CLI and the IDE.
//!
//! Antigravity (Google's coding agent, formerly "Gemini CLI"; the CLI binary
//! is `agy`) keeps one SQLite database per conversation under each app data
//! dir:
//!
//! - `~/.gemini/antigravity-cli/conversations/<uuid>.db` (CLI)
//! - `~/.gemini/antigravity-ide/conversations/<uuid>.db` (IDE)
//!
//! Both apps share an identical schema, so a single adapter scans both. Every
//! model generation is a `steps` row with `step_type = 15`; the token counts
//! and start time live in that row's `metadata` protobuf blob:
//!
//! ```text
//! steps.metadata (protobuf)
//!   field 1 : google.protobuf.Timestamp { seconds, nanos }  (start time)
//!   field 9 : generation stats {
//!     field 1 : model enum (varint)     - opaque, no readable name in the DB
//!     field 2 : input tokens (varint)   - includes the cached prefix
//!     field 3 : output tokens (varint)  - verified vs response byte size
//!     field 5 : cached-context tokens (varint, optional)
//!   }
//! ```
//!
//! The conversation's workspace is the first `file:///` URI in
//! `trajectory_metadata_blob.data`, and the model's display name comes from the
//! app dir's `settings.json` `model` field (falling back to "gemini" so the
//! pricer's gemini branch prices it). One `UsageRecord` is emitted per
//! generation, deduped on `<root-label>/<conversation>.db:<step_idx>`, and the
//! cheap change detector aggregates the `conversations/*.db` files plus their
//! `-wal`/`-shm` sidecars.
//!
//! The cached-prefix split is an estimate. `f5` grows as the session's cached
//! context and resets at sub-conversation boundaries, while `f2` is the whole
//! request input, so `cache_read = min(input, cached)` and
//! `input = input - cache_read`: a context-refresh step (input >> cached)
//! keeps the fresh part as input, while a normal cached turn (cached >= input)
//! is charged entirely at the cache rate. `total_tokens()` stays `f2 + f3`,
//! matching the request's real token count regardless of the split.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::roots::discover_roots;
use super::source::{
    fingerprint, FileStates, ProviderConfig, ProviderError, ProviderSource, ScanOutput, ScanRoot,
};

mod proto;

/// A model generation decoded from a type-15 step's `metadata` blob.
struct GenUsage {
    started_at: DateTime<Utc>,
    input: u64,
    output: u64,
    cache_read: u64,
}

/// One conversation store file, or its WAL/SHM sidecar. `conversation_summaries.db`
/// lives at the app dir root and is deliberately excluded.
fn is_store_file(name: &str) -> bool {
    name.ends_with(".db") || name.ends_with(".db-wal") || name.ends_with(".db-shm")
}

/// `file:///C:/Users/Ann/AnnProjects/zhy-ui` -> `zhy-ui`.
fn workspace_name(uri: &str) -> Option<String> {
    let path = uri.strip_prefix("file:///")?;
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

pub struct AntigravitySource {
    config: ProviderConfig,
    /// Discovered scan roots, computed lazily on the first scan (background
    /// thread) so WSL discovery never blocks UI startup.
    roots: OnceLock<Vec<ScanRoot>>,
}

impl AntigravitySource {
    pub fn new(config: ProviderConfig) -> Self {
        AntigravitySource {
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
            // The CLI root is the primary (unlabelled) root; the IDE root is
            // labelled so its dedup fingerprints never collide with the CLI's.
            let mut roots = Vec::new();
            let mut cli = discover_roots(&[".gemini", "antigravity-cli"]);
            let mut ide = discover_roots(&[".gemini", "antigravity-ide"]);
            for root in &mut ide {
                match &root.label {
                    None => root.label = Some("ide".to_string()),
                    Some(label) => root.label = Some(format!("{label}/ide")),
                }
            }
            roots.append(&mut cli);
            roots.append(&mut ide);
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

    /// Open a conversation store without writing to it. Prefers a genuinely
    /// read-only connection so TokenMonitor never contends with a live session; when
    /// that fails (e.g. a WAL file whose `-shm` is missing), falls back to a
    /// `query_only` connection like TokenMonitor's own read path.
    fn open_db(path: &Path) -> Result<Connection, String> {
        let conn = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
            Ok(c) => c,
            Err(_) => {
                let c = Connection::open(path).map_err(|e| format!("open {:?}: {e}", path))?;
                c.pragma_update(None, "query_only", "ON")
                    .map_err(|e| format!("query_only {:?}: {e}", path))?;
                c
            }
        };
        conn.busy_timeout(Duration::from_secs(2))
            .map_err(|e| format!("busy_timeout {:?}: {e}", path))?;
        Ok(conn)
    }

    /// Change-detection stats over one root's `conversations/` directory (each
    /// per-conversation store plus its WAL/SHM sidecars). Shared by `scan` and
    /// `scan_fingerprint` so the cheap check and a full scan always agree.
    fn store_stats(root: &ScanRoot) -> (u64, i64, u64) {
        let mut found = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        let conv_dir = root.dir.join("conversations");
        let Ok(entries) = std::fs::read_dir(&conv_dir) else {
            return (0, 0, 0);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !is_store_file(&name) {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
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

    /// Project name = basename of the conversation's first `file:///` workspace
    /// URI, stored in `trajectory_metadata_blob`.
    fn conversation_project(conn: &Connection) -> Option<String> {
        let blob: Vec<u8> = conn
            .query_row(
                "SELECT data FROM trajectory_metadata_blob WHERE id = 'main' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok()?;
        let fields = proto::parse_fields(&blob)?;
        let uri = proto::first_file_uri(&fields)?;
        workspace_name(std::str::from_utf8(uri).ok()?)
    }

    /// The app dir's configured model (`settings.json`), or `"gemini"` so the
    /// pricer's gemini branch prices it when unreadable.
    fn root_model(root: &ScanRoot) -> String {
        #[derive(Deserialize)]
        struct Settings {
            #[serde(default)]
            model: Option<String>,
        }
        let path = root.dir.join("settings.json");
        let model = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .and_then(|s| s.model);
        model
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| "gemini".to_string())
    }

    /// Decode a type-15 step's `metadata` blob into token counts + start time.
    /// Returns `None` for malformed blobs or generations without usage.
    fn decode_gen_metadata(buf: &[u8]) -> Option<GenUsage> {
        let fields = proto::parse_fields(buf)?;
        // Start time: google.protobuf.Timestamp { seconds: 1, nanos: 2 }.
        let ts = proto::first_len(&fields, 1)?;
        let ts_fields = proto::parse_fields(ts)?;
        let seconds = proto::first_varint(&ts_fields, 1)? as i64;
        let nanos = proto::first_varint(&ts_fields, 2).unwrap_or(0) as u32;
        let started_at = Utc.timestamp_opt(seconds, nanos).single()?;

        // Token stats in field 9 (see module docs for the mapping).
        let stats = proto::first_len(&fields, 9)?;
        let stats_fields = proto::parse_fields(stats)?;
        let input_total = proto::first_varint(&stats_fields, 2).unwrap_or(0);
        let output = proto::first_varint(&stats_fields, 3).unwrap_or(0);
        let cached = proto::first_varint(&stats_fields, 5).unwrap_or(0);

        // `input_total` includes the cached prefix; `cached` is the running
        // context size. A turn whose whole input is cached costs at the cache
        // rate, while a refresh (input >> cached) keeps the fresh part as
        // input - see the module docs.
        let cache_read = input_total.min(cached);
        let input = input_total - cache_read;
        if input + output + cache_read == 0 {
            return None;
        }
        Some(GenUsage {
            started_at,
            input,
            output,
            cache_read,
        })
    }

    /// Stream one record per model generation with non-zero usage in a single
    /// conversation store.
    fn scan_conversation(
        conn: &Connection,
        root: &ScanRoot,
        session_id: &str,
        emit: &mut dyn FnMut(UsageRecord),
    ) -> Result<(), String> {
        let project = Self::conversation_project(conn).unwrap_or_else(|| "unknown".to_string());
        let model = Self::root_model(root);

        let mut stmt = conn
            .prepare("SELECT idx, metadata FROM steps WHERE step_type = 15 ORDER BY idx")
            .map_err(|e| format!("prepare steps query: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<Vec<u8>>>(1)?))
            })
            .map_err(|e| format!("query steps: {e}"))?;

        for row in rows {
            let (idx, metadata) = row.map_err(|e| format!("row: {e}"))?;
            let Some(metadata) = metadata else {
                continue;
            };
            let Some(gen) = Self::decode_gen_metadata(&metadata) else {
                continue;
            };

            // Namespace the dedup key per root so identically-named stores
            // from the CLI, the IDE and WSL distros never collide.
            let rel = match &root.label {
                Some(label) => Path::new(label).join(format!("{session_id}.db")),
                None => PathBuf::from(format!("{session_id}.db")),
            };

            emit(UsageRecord::new(
                Provider::Antigravity,
                project.clone(),
                session_id.to_string(),
                Usage {
                    model: model.clone(),
                    started_at: gen.started_at,
                    input_tokens: gen.input,
                    output_tokens: gen.output,
                    cache_read_tokens: gen.cache_read,
                    cache_write_tokens: 0, // not reported by Antigravity
                    cost_micros: 0,        // pricing applied in a later pipeline stage
                },
                metadata.len() as u64,
                format!("{}:{idx}", rel.display()),
            ));
        }
        Ok(())
    }
}

impl ProviderSource for AntigravitySource {
    fn provider(&self) -> Provider {
        Provider::Antigravity
    }

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let dirs: Vec<PathBuf> = self.existing_roots().into_iter().map(|r| r.dir).collect();
        if dirs.is_empty() {
            Err(ProviderError::DataDirNotFound(Provider::Antigravity))
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
        _known: &FileStates,
    ) -> Result<ScanOutput, ProviderError> {
        // SQLite + WAL: new steps in a live conversation live in the -wal
        // file, so per-file (mtime, size) state could miss them. Always do a
        // full scan (exactly like the opencode adapter); INSERT OR IGNORE
        // keeps rescans idempotent.
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::Antigravity));
        }
        let mut errors = Vec::new();
        let mut found_files = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        for root in &roots {
            let (found, mtime, bytes) = Self::store_stats(root);
            found_files += found;
            max_mtime = max_mtime.max(mtime);
            total_bytes += bytes;

            let conv_dir = root.dir.join("conversations");
            let entries = match std::fs::read_dir(&conv_dir) {
                Ok(e) => e,
                // No conversations yet - nothing to scan, not an error.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    errors.push(format!("read {:?}: {e}", conv_dir));
                    continue;
                }
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !name.ends_with(".db") {
                    continue;
                }
                let Some(session_id) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let conn = match Self::open_db(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                };
                if let Err(e) = Self::scan_conversation(&conn, root, session_id, emit) {
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
            return Err(ProviderError::DataDirNotFound(Provider::Antigravity));
        }
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        let mut found = 0u64;
        for root in &roots {
            let (f, m, b) = Self::store_stats(root);
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
    use std::time::Duration;

    use rusqlite::{params, Connection};
    use tempfile::tempdir;

    use super::*;

    /// Wire-encode a base-128 varint.
    fn varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut b = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                return out;
            }
        }
    }

    /// Encode a varint field (wire type 0).
    fn varint_field(number: u32, value: u64) -> Vec<u8> {
        let mut out = varint(u64::from(number) << 3);
        out.extend(varint(value));
        out
    }

    /// Encode a length-delimited field (wire type 2).
    fn len_field(number: u32, value: &[u8]) -> Vec<u8> {
        let mut out = varint(u64::from(number) << 3 | 2);
        out.extend(varint(value.len() as u64));
        out.extend_from_slice(value);
        out
    }

    /// Encode a google.protobuf.Timestamp { seconds, nanos } as metadata field 1.
    fn timestamp(seconds: i64, nanos: u32) -> Vec<u8> {
        let mut inner = varint_field(1, seconds as u64);
        inner.extend(varint_field(2, u64::from(nanos)));
        len_field(1, &inner)
    }

    /// Encode the generation-stats sub-message
    /// { model: 1, input: 2, output: 3, cached: 5 } as metadata field 9.
    fn stats(model: u64, input: u64, output: u64, cached: u64) -> Vec<u8> {
        let mut inner = varint_field(1, model);
        inner.extend(varint_field(2, input));
        inner.extend(varint_field(3, output));
        inner.extend(varint_field(5, cached));
        len_field(9, &inner)
    }

    /// Encode a type-15 step's `metadata` blob: timestamp + stats.
    fn gen_metadata(seconds: i64, input: u64, output: u64, cached: u64) -> Vec<u8> {
        let mut blob = timestamp(seconds, 0);
        blob.extend(stats(1072, input, output, cached));
        blob
    }

    /// Create a conversation store under `dir` (app-data-dir layout) with the
    /// given `(idx, step_type, metadata)` steps and a workspace URI.
    fn write_conversation_store(
        dir: &Path,
        uuid: &str,
        workspace_uri: &str,
        rows: &[(i64, i64, Option<&[u8]>)],
    ) {
        let conv_dir = dir.join("conversations");
        fs::create_dir_all(&conv_dir).unwrap();
        let conn = Connection::open(conv_dir.join(format!("{uuid}.db"))).unwrap();
        conn.execute_batch(
            "CREATE TABLE steps (
                idx INTEGER PRIMARY KEY,
                step_type INTEGER NOT NULL DEFAULT 0,
                status INTEGER NOT NULL DEFAULT 0,
                has_subtrajectory NUMERIC NOT NULL DEFAULT false,
                metadata BLOB
            );
            CREATE TABLE trajectory_metadata_blob (
                id TEXT DEFAULT 'main',
                data BLOB,
                PRIMARY KEY (id)
            );",
        )
        .unwrap();
        // Workspace URI: data field 1 wraps a sub-message whose field 1 is the URI.
        let inner = len_field(1, workspace_uri.as_bytes());
        let data = len_field(1, &inner);
        conn.execute(
            "INSERT INTO trajectory_metadata_blob (id, data) VALUES ('main', ?1)",
            params![data],
        )
        .unwrap();
        for (idx, step_type, metadata) in rows {
            conn.execute(
                "INSERT INTO steps (idx, step_type, status, metadata) VALUES (?1, ?2, 3, ?3)",
                params![idx, step_type, metadata.map(|m| m.to_vec())],
            )
            .unwrap();
        }
    }

    fn source_for(dir: &Path) -> AntigravitySource {
        AntigravitySource::new(ProviderConfig {
            provider: Provider::Antigravity,
            data_dir_override: Some(dir.to_path_buf()),
            ..ProviderConfig::default()
        })
    }

    fn scan_collect(src: &AntigravitySource) -> (ScanOutput, Vec<UsageRecord>) {
        let mut records = Vec::new();
        let out = src.scan(&mut |r| records.push(r)).unwrap();
        (out, records)
    }

    #[test]
    fn parses_generations_into_records() {
        let dir = tempdir().unwrap();
        let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeffff0000";
        let gen_a = gen_metadata(1_750_000_000, 12984, 452, 8245);
        let gen_b = gen_metadata(1_750_000_100, 5689, 115, 16478);
        let rows: Vec<(i64, i64, Option<&[u8]>)> = vec![
            (0, 14, None), // user input - skipped
            (1, 15, Some(&gen_a)),
            (2, 15, Some(&gen_b)),
            (3, 8, None), // tool result - skipped
        ];
        write_conversation_store(
            dir.path(),
            uuid,
            "file:///C:/Users/Ann/AnnProjects/zhy-ui",
            &rows,
        );

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert!(out.errors.is_empty());
        assert_eq!(records.len(), 2);

        let first = &records[0];
        assert_eq!(first.provider, Provider::Antigravity);
        assert_eq!(first.project, "zhy-ui");
        assert_eq!(first.session_id, uuid);
        assert_eq!(first.usage.model, "gemini"); // no settings.json
                                                 // input_total 12984, cached 8245 -> cache_read 8245, input 4739.
        assert_eq!(first.usage.input_tokens, 4739);
        assert_eq!(first.usage.output_tokens, 452);
        assert_eq!(first.usage.cache_read_tokens, 8245);
        assert_eq!(first.usage.cache_write_tokens, 0);
        assert_eq!(first.usage.total_tokens(), 12984 + 452);
        assert_eq!(first.usage.started_at.timestamp(), 1_750_000_000);
        // Dedup key is the store path + step idx.
        assert!(first.fingerprint.ends_with(":1"));

        // Second generation: cached >= input -> the whole input is cached.
        let second = &records[1];
        assert_eq!(second.usage.input_tokens, 0);
        assert_eq!(second.usage.cache_read_tokens, 5689);
        assert_eq!(second.usage.output_tokens, 115);
        assert_eq!(second.usage.total_tokens(), 5689 + 115);
    }

    #[test]
    fn skips_non_generation_and_malformed_steps() {
        let dir = tempdir().unwrap();
        let gen_a = gen_metadata(1_750_000_000, 0, 0, 0);
        let malformed = [0x00, 0xff, 0x12, 0x08];
        let gen_b = gen_metadata(1_750_000_100, 10, 5, 0);
        let rows: Vec<(i64, i64, Option<&[u8]>)> = vec![
            (0, 15, Some(&gen_a)),     // zero tokens
            (1, 15, Some(&malformed)), // malformed
            (2, 15, Some(&gen_b)),
            (3, 15, None), // no metadata
        ];
        write_conversation_store(dir.path(), "aaaa-0000", "file:///p", &rows);
        let (out, records) = scan_collect(&source_for(dir.path()));
        assert!(out.errors.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].usage.total_tokens(), 15);
    }

    #[test]
    fn reads_model_from_settings_json() {
        let dir = tempdir().unwrap();
        let uuid = "aaaa-1111";
        fs::write(
            dir.path().join("settings.json"),
            r#"{"model": "Gemini 3.7 Flash (Medium)", "colorScheme": "dark"}"#,
        )
        .unwrap();
        let gen = gen_metadata(1_750_000_000, 10, 5, 0);
        let rows: Vec<(i64, i64, Option<&[u8]>)> = vec![(0, 15, Some(&gen))];
        write_conversation_store(dir.path(), uuid, "file:///p", &rows);
        let (_, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(records[0].usage.model, "Gemini 3.7 Flash (Medium)");
    }

    #[test]
    fn data_dir_missing_is_data_dir_not_found() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("no-such-dir");
        let err = source_for(&missing).data_dirs().unwrap_err();
        assert!(matches!(
            err,
            ProviderError::DataDirNotFound(Provider::Antigravity)
        ));
    }

    #[test]
    fn fingerprint_stable_across_rescans() {
        let dir = tempdir().unwrap();
        let src = source_for(dir.path());
        write_conversation_store(
            dir.path(),
            "aaaa-2222",
            "file:///p",
            &[(0, 15, Some(&gen_metadata(1_750_000_000, 10, 5, 0)))],
        );
        let fp1 = src.scan_fingerprint().unwrap();
        let fp2 = src.scan_fingerprint().unwrap();
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn fingerprint_changes_when_store_rewritten() {
        let dir = tempdir().unwrap();
        let src = source_for(dir.path());
        let db_path = dir.path().join("conversations").join("aaaa-3333.db");
        write_conversation_store(dir.path(), "aaaa-3333", "file:///p", &[]);
        let fp1 = src.scan_fingerprint().unwrap();

        // The fingerprint only has one-second resolution; wait out the current
        // second so the rewritten store's mtime actually differs.
        std::thread::sleep(Duration::from_secs(2));
        fs::remove_file(&db_path).unwrap();
        write_conversation_store(
            dir.path(),
            "aaaa-3333",
            "file:///p",
            &[(0, 15, Some(&gen_metadata(1_750_000_000, 10, 5, 0)))],
        );
        assert_ne!(fp1, src.scan_fingerprint().unwrap());
    }

    #[test]
    fn namespaces_ide_root_fingerprints() {
        // Two identical stores under a primary root and a labelled root must
        // not collide in dedup fingerprints.
        let uuid = "bbbb-4444";
        let metadata = gen_metadata(1_750_000_000, 10, 5, 0);

        let primary_dir = tempdir().unwrap();
        let ide_dir = tempdir().unwrap();
        write_conversation_store(
            primary_dir.path(),
            uuid,
            "file:///p1",
            &[(0, 15, Some(&metadata))],
        );
        write_conversation_store(
            ide_dir.path(),
            uuid,
            "file:///p2",
            &[(0, 15, Some(&metadata))],
        );

        let primary = ScanRoot {
            dir: primary_dir.path().to_path_buf(),
            label: None,
        };
        let ide = ScanRoot {
            dir: ide_dir.path().to_path_buf(),
            label: Some("ide".to_string()),
        };

        let mut records = Vec::new();
        for root in [&primary, &ide] {
            let conn = AntigravitySource::open_db(
                &root.dir.join("conversations").join(format!("{uuid}.db")),
            )
            .unwrap();
            AntigravitySource::scan_conversation(&conn, root, uuid, &mut |r| records.push(r))
                .unwrap();
        }
        assert_eq!(records.len(), 2);
        assert_ne!(records[0].fingerprint, records[1].fingerprint);
        // Primary root's key is a plain path prefix; the labelled one carries
        // the "ide" namespace.
        assert!(records[0].fingerprint.starts_with(&format!("{uuid}.db:")));
        let label = Path::new("ide");
        assert!(Path::new(&records[1].fingerprint).starts_with(label));
    }
}
