//! App-private Library content index.
//!
//! This index is deliberately separate from graph search and graph vectors. It
//! stores its own FTS table and optional on-device embeddings under app data.

use crate::ai::traits::Embedder;
use crate::error::{CoreError, Result};
use crate::media::{Transcript, TranscriptSegment};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_CHUNK_TEXT: usize = 8 * 1024;
const MAX_SNIPPET: usize = 300;
const MAX_QUOTE: usize = 160;
const MAX_REASON_CHARS: usize = 500;
const BOOK_CHUNK_MIN: usize = 1_000;
const BOOK_CHUNK_MAX: usize = 1_400;
const MEDIA_WINDOW_MIN_MS: i64 = 45_000;
const MEDIA_WINDOW_MAX_MS: i64 = 75_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryItemKind {
    Audio,
    Video,
    Epub,
    Youtube,
}

impl LibraryItemKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Epub => "epub",
            Self::Youtube => "youtube",
        }
    }

    fn from_db(value: &str) -> Self {
        match value {
            "audio" => Self::Audio,
            "video" => Self::Video,
            "youtube" => Self::Youtube,
            _ => Self::Epub,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryIndexSettings {
    pub enabled: bool,
    pub transcribe_media: bool,
}

impl Default for LibraryIndexSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            transcribe_media: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFileInput {
    pub track_id: Option<String>,
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub size: u64,
    pub mtime_ms: i64,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItemInput {
    pub book_id: String,
    pub title: String,
    pub kind: LibraryItemKind,
    pub source_url: Option<String>,
    pub available: bool,
    pub files: Vec<LibraryFileInput>,
}

impl LibraryItemInput {
    pub fn fingerprint(&self) -> String {
        if self.source_url.is_some() {
            return format!("url:{}", self.source_url.as_deref().unwrap_or_default());
        }
        fingerprint_files(&self.files)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaAction {
    New(String),
    Changed(String),
    Removed(String),
    Unchanged(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibrarySemanticState {
    Ready,
    Unavailable,
    Stale,
    Embedding,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryTranscriptionState {
    Ready,
    Off,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItemCounts {
    pub total: usize,
    pub indexed: usize,
    pub pending: usize,
    pub failed: usize,
    pub title_only: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryIndexError {
    pub book_id: String,
    pub title: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryIndexStatus {
    pub enabled: bool,
    pub transcribe_media: bool,
    pub running: bool,
    pub job_id: Option<String>,
    pub items: LibraryItemCounts,
    pub chunks: usize,
    pub semantic: LibrarySemanticState,
    pub semantic_reason: Option<String>,
    pub transcription: LibraryTranscriptionState,
    pub transcription_reason: Option<String>,
    pub last_indexed_at: Option<i64>,
    pub errors: Vec<LibraryIndexError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySearchHit {
    pub chunk_id: String,
    pub book_id: String,
    pub title: String,
    pub kind: LibraryItemKind,
    pub snippet: String,
    pub track_id: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub chapter: Option<String>,
    pub quote: Option<String>,
    pub score: f32,
    #[serde(rename = "match")]
    pub match_kind: LibraryMatchKind,
    #[serde(skip)]
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryMatchKind {
    Keyword,
    Semantic,
    Both,
    Title,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrarySource {
    pub index: usize,
    pub book_id: String,
    pub title: String,
    pub kind: LibraryItemKind,
    pub track_id: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub chapter: Option<String>,
    pub quote: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LibraryAssistantSource {
    pub index_path: PathBuf,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub struct LibraryContextEntry {
    pub source: LibrarySource,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct LibraryContext {
    pub entries: Vec<LibraryContextEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaSlice {
    Data {
        start_ms: i64,
        end_ms: i64,
        is_final: bool,
    },
    NoAudio,
    EndOfFile,
}

pub trait MediaSliceSource {
    fn slice(&mut self, start_ms: i64, duration_ms: i64) -> Result<MediaSlice>;
}

pub trait MediaSliceTranscriber {
    fn transcribe_slice(&mut self, start_ms: i64, end_ms: i64) -> Result<Vec<TranscriptSegment>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaIndexOutcome {
    Completed,
    Paused(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFileState {
    pub relative_path: String,
    pub track_id: Option<String>,
    pub fingerprint: String,
    pub status: String,
    pub status_message: Option<String>,
    pub attempt_count: i64,
    pub next_retry_at: Option<i64>,
    pub transcribed_until_ms: i64,
}

#[allow(clippy::too_many_arguments)]
pub fn index_media_file_slices(
    store: &LibraryIndexStore,
    item: &LibraryItemInput,
    file: &LibraryFileInput,
    slice_ms: i64,
    force: bool,
    mut pause_reason: impl FnMut() -> Option<String>,
    source: &mut dyn MediaSliceSource,
    transcriber: &mut dyn MediaSliceTranscriber,
) -> Result<MediaIndexOutcome> {
    let mut start_ms = if force {
        store.set_file_progress(item, file, 0)?;
        0
    } else {
        store
            .file_states(&item.book_id)?
            .into_iter()
            .find(|state| state.relative_path == file.relative_path)
            .map(|state| state.transcribed_until_ms)
            .unwrap_or(0)
    };
    for _ in 0..10_000 {
        if let Some(reason) = pause_reason() {
            return Ok(MediaIndexOutcome::Paused(reason));
        }
        let slice = match source.slice(start_ms, slice_ms) {
            Ok(slice) => slice,
            Err(error) => {
                let message = error.to_string();
                store.mark_file_failed(item, file, &message, now_ms())?;
                return Ok(MediaIndexOutcome::Failed(message));
            }
        };
        let MediaSlice::Data {
            start_ms: slice_start,
            end_ms: slice_end,
            is_final,
        } = slice
        else {
            // End of file, or a file with no audio at all: only this file is
            // done. Its sibling tracks keep their own state and progress.
            store.mark_file_indexed(item, file)?;
            return Ok(MediaIndexOutcome::Completed);
        };
        let segments = match transcriber.transcribe_slice(slice_start, slice_end) {
            Ok(segments) => segments,
            Err(error) => {
                let message = error.to_string();
                store.mark_file_failed(item, file, &message, now_ms())?;
                return Ok(MediaIndexOutcome::Failed(message));
            }
        };
        let transcript = Transcript::from_segments(
            segments
                .into_iter()
                .map(|mut segment| {
                    segment.start_ms = segment.start_ms.saturating_add(slice_start);
                    segment.end_ms = segment.end_ms.saturating_add(slice_start);
                    segment
                })
                .collect(),
        );
        let track_id = file.track_id.as_deref().unwrap_or(&item.book_id);
        let chunks = chunk_transcript(&item.book_id, &item.title, item.kind, track_id, &transcript);
        store.index_track_slice_chunks(item, file, &chunks, slice_start, slice_end)?;
        start_ms = slice_end;
        if is_final {
            store.mark_file_indexed(item, file)?;
            return Ok(MediaIndexOutcome::Completed);
        }
    }
    let message = "Media transcription exceeded the slice limit".to_string();
    store.mark_file_failed(item, file, &message, now_ms())?;
    Ok(MediaIndexOutcome::Failed(message))
}

pub struct LibraryIndexStore {
    path: PathBuf,
}

impl LibraryIndexStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        init(&conn)?;
        Ok(Self { path })
    }

    fn conn(&self) -> Result<Connection> {
        let conn = Connection::open(&self.path)?;
        init(&conn)?;
        Ok(conn)
    }

    pub fn settings(&self) -> Result<LibraryIndexSettings> {
        let conn = self.conn()?;
        let enabled = setting_bool(&conn, "enabled", true)?;
        let transcribe_media = setting_bool(&conn, "transcribe_media", true)?;
        Ok(LibraryIndexSettings {
            enabled,
            transcribe_media,
        })
    }

    pub fn set_settings(&self, settings: &LibraryIndexSettings) -> Result<()> {
        let conn = self.conn()?;
        set_setting_bool(&conn, "enabled", settings.enabled)?;
        set_setting_bool(&conn, "transcribe_media", settings.transcribe_media)?;
        Ok(())
    }

    pub fn status(
        &self,
        running: bool,
        job_id: Option<String>,
        semantic: LibrarySemanticState,
        semantic_reason: Option<String>,
        transcription: LibraryTranscriptionState,
        transcription_reason: Option<String>,
    ) -> Result<LibraryIndexStatus> {
        let conn = self.conn()?;
        let settings = self.settings()?;
        let mut counts = LibraryItemCounts {
            total: 0,
            indexed: 0,
            pending: 0,
            failed: 0,
            title_only: 0,
        };
        let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM items GROUP BY status")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, usize>(1)?)))? {
            let (status, count) = row?;
            counts.total += count;
            match status.as_str() {
                "indexed" => counts.indexed = count,
                "pending" => counts.pending = count,
                "failed" => counts.failed = count,
                "title_only" => counts.title_only = count,
                _ => {}
            }
        }
        let chunks = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
        let last_indexed_at = conn
            .query_row(
                "SELECT value FROM metadata WHERE key = 'last_indexed_at'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|v| v.parse().ok());
        let mut errors = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT book_id, title, COALESCE(status_message, '') FROM items \
             WHERE status = 'failed' ORDER BY updated_at DESC LIMIT 20",
        )?;
        for row in stmt.query_map([], |r| {
            Ok(LibraryIndexError {
                book_id: r.get(0)?,
                title: r.get(1)?,
                message: cap_reason(&r.get::<_, String>(2)?),
            })
        })? {
            errors.push(row?);
        }
        Ok(LibraryIndexStatus {
            enabled: settings.enabled,
            transcribe_media: settings.transcribe_media,
            running,
            job_id,
            items: counts,
            chunks,
            semantic,
            semantic_reason: semantic_reason.map(|reason| cap_reason(&reason)),
            transcription,
            transcription_reason: transcription_reason.map(|reason| cap_reason(&reason)),
            last_indexed_at,
            errors,
        })
    }

    pub fn delta_actions(&self, inputs: &[LibraryItemInput]) -> Result<Vec<DeltaAction>> {
        let conn = self.conn()?;
        delta_actions(&conn, inputs)
    }

    pub fn item_statuses(&self) -> Result<HashMap<String, (String, Option<String>)>> {
        let conn = self.conn()?;
        let statuses = conn
            .prepare("SELECT book_id, status, status_message FROM items")?
            .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
            .collect::<std::result::Result<_, _>>()
            .map_err(CoreError::from)?;
        Ok(statuses)
    }

    pub fn file_states(&self, book_id: &str) -> Result<Vec<LibraryFileState>> {
        let conn = self.conn()?;
        file_states(&conn, book_id)
    }

    pub fn due_files(
        &self,
        item: &LibraryItemInput,
        force: bool,
        now_ms: i64,
    ) -> Result<Vec<LibraryFileInput>> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        upsert_item_tx(&tx, item, "pending", None)?;
        let mut due = Vec::new();
        for file in &item.files {
            if !file.available {
                continue;
            }
            let fingerprint = file_fingerprint(file);
            let existing = file_state_tx(&tx, &item.book_id, &file.relative_path)?;
            let changed = existing
                .as_ref()
                .is_none_or(|state| state.fingerprint != fingerprint);
            let should_process = force
                || changed
                || existing
                    .as_ref()
                    .is_none_or(|state| state.status == "pending")
                || existing
                    .as_ref()
                    .is_some_and(|state| state.status == "title_only")
                || existing.as_ref().is_some_and(|state| {
                    state.status == "failed" && state.next_retry_at.is_none_or(|at| at <= now_ms)
                });
            let status = if changed {
                "pending"
            } else {
                existing
                    .as_ref()
                    .map(|s| s.status.as_str())
                    .unwrap_or("pending")
            };
            let attempts = if changed {
                0
            } else {
                existing.as_ref().map(|s| s.attempt_count).unwrap_or(0)
            };
            let progress = if changed {
                0
            } else {
                existing
                    .as_ref()
                    .map(|s| s.transcribed_until_ms)
                    .unwrap_or(0)
            };
            tx.execute(
                "INSERT INTO files(book_id, track_id, relative_path, fingerprint, status, status_message, attempt_count, next_retry_at, transcribed_until_ms, updated_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(book_id, relative_path) DO UPDATE SET
                 track_id = excluded.track_id, fingerprint = excluded.fingerprint,
                 status = excluded.status, attempt_count = excluded.attempt_count,
                 transcribed_until_ms = excluded.transcribed_until_ms, updated_at = excluded.updated_at",
                params![
                    &item.book_id,
                    file.track_id.as_deref(),
                    &file.relative_path,
                    fingerprint,
                    status,
                    existing.as_ref().and_then(|s| s.status_message.as_deref()),
                    attempts,
                    existing.as_ref().and_then(|s| s.next_retry_at),
                    progress,
                    now_ms,
                ],
            )?;
            if should_process {
                due.push(file.clone());
            }
        }
        recompute_item_status_tx(&tx, &item.book_id)?;
        tx.commit()?;
        Ok(due)
    }

    pub fn remove_absent(&self, active_ids: &HashSet<String>) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let ids = existing_ids(&conn)?;
        let removed: Vec<String> = ids
            .into_iter()
            .filter(|id| !active_ids.contains(id))
            .collect();
        for id in &removed {
            delete_item(&conn, id)?;
        }
        Ok(removed)
    }

    pub fn index_title_only(&self, item: &LibraryItemInput, reason: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        upsert_item_tx(&tx, item, "title_only", Some(reason))?;
        tx.execute(
            "DELETE FROM chunks WHERE book_id = ?1 AND track_id IS NULL",
            [&item.book_id],
        )?;
        insert_chunk_tx(
            &tx,
            &ChunkRecord {
                book_id: item.book_id.clone(),
                title: item.title.clone(),
                kind: item.kind,
                ordinal: 0,
                text: item.title.clone(),
                track_id: None,
                start_ms: None,
                end_ms: None,
                chapter: None,
                quote: None,
            },
        )?;
        for file in &item.files {
            tx.execute(
                "INSERT INTO files(book_id, track_id, relative_path, fingerprint, status, status_message, attempt_count, next_retry_at, transcribed_until_ms, updated_at)
                 VALUES(?1, ?2, ?3, ?4, 'title_only', ?5, 0, NULL, 0, ?6)
                 ON CONFLICT(book_id, relative_path) DO UPDATE SET
                 track_id = excluded.track_id, fingerprint = excluded.fingerprint,
                 status = CASE WHEN files.status = 'indexed' THEN files.status ELSE 'title_only' END,
                 status_message = CASE WHEN files.status = 'indexed' THEN files.status_message ELSE excluded.status_message END,
                 transcribed_until_ms = CASE WHEN files.status = 'indexed' THEN files.transcribed_until_ms ELSE 0 END,
                 updated_at = excluded.updated_at",
                params![
                    &item.book_id,
                    file.track_id.as_deref(),
                    &file.relative_path,
                    file_fingerprint(file),
                    cap_reason(reason),
                    now_ms(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn index_failed(&self, item: &LibraryItemInput, message: &str) -> Result<()> {
        let conn = self.conn()?;
        upsert_item(&conn, item, "failed", Some(message))
    }

    pub fn index_chunks(
        &self,
        item: &LibraryItemInput,
        chunks: &[ChunkRecord],
        vectors: Option<(&str, &[Vec<f32>])>,
        message: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        upsert_item_tx(&tx, item, "indexed", message)?;
        delete_chunks_tx(&tx, &item.book_id)?;
        for chunk in chunks {
            insert_chunk_tx(&tx, chunk)?;
        }
        if let Some((scheme, vectors)) = vectors {
            let ids: Vec<i64> = tx
                .prepare("SELECT id FROM chunks WHERE book_id = ?1 ORDER BY ordinal")?
                .query_map([&item.book_id], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?;
            for (chunk_id, vector) in ids.into_iter().zip(vectors.iter()) {
                tx.execute(
                    "INSERT OR REPLACE INTO vectors(chunk_id, scheme, dim, vector) VALUES(?1, ?2, ?3, ?4)",
                    params![chunk_id, scheme, vector.len() as i64, vec_to_blob(vector)],
                )?;
            }
        }
        for file in &item.files {
            tx.execute(
                "INSERT INTO files(book_id, track_id, relative_path, fingerprint, status, status_message, attempt_count, next_retry_at, transcribed_until_ms, updated_at)
                 VALUES(?1, ?2, ?3, ?4, 'indexed', NULL, 0, NULL, 0, ?5)
                 ON CONFLICT(book_id, relative_path) DO UPDATE SET
                 track_id = excluded.track_id, fingerprint = excluded.fingerprint, status = 'indexed',
                 status_message = NULL, attempt_count = 0, next_retry_at = NULL, updated_at = excluded.updated_at",
                params![
                    &item.book_id,
                    file.track_id.as_deref(),
                    &file.relative_path,
                    file_fingerprint(file),
                    now_ms(),
                ],
            )?;
        }
        recompute_item_status_tx(&tx, &item.book_id)?;
        set_metadata_tx(&tx, "last_indexed_at", &now_ms().to_string())?;
        tx.commit()?;
        Ok(())
    }

    pub fn index_track_chunks(
        &self,
        item: &LibraryItemInput,
        file: &LibraryFileInput,
        chunks: &[ChunkRecord],
        message: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        upsert_item_tx(&tx, item, "pending", message)?;
        let track_id = file.track_id.as_deref().unwrap_or(&item.book_id);
        delete_track_chunks_tx(&tx, &item.book_id, track_id)?;
        for chunk in chunks {
            insert_chunk_tx(&tx, chunk)?;
        }
        tx.execute(
            "INSERT INTO files(book_id, track_id, relative_path, fingerprint, status, status_message, attempt_count, next_retry_at, transcribed_until_ms, updated_at)
             VALUES(?1, ?2, ?3, ?4, 'indexed', NULL, 0, NULL, 0, ?5)
             ON CONFLICT(book_id, relative_path) DO UPDATE SET
             track_id = excluded.track_id, fingerprint = excluded.fingerprint, status = 'indexed',
             status_message = NULL, attempt_count = 0, next_retry_at = NULL, updated_at = excluded.updated_at",
            params![
                &item.book_id,
                file.track_id.as_deref(),
                &file.relative_path,
                file_fingerprint(file),
                now_ms(),
            ],
        )?;
        recompute_item_status_tx(&tx, &item.book_id)?;
        set_metadata_tx(&tx, "last_indexed_at", &now_ms().to_string())?;
        tx.commit()?;
        Ok(())
    }

    pub fn index_track_slice_chunks(
        &self,
        item: &LibraryItemInput,
        file: &LibraryFileInput,
        chunks: &[ChunkRecord],
        slice_start_ms: i64,
        slice_end_ms: i64,
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        upsert_item_tx(&tx, item, "pending", None)?;
        let track_id = file.track_id.as_deref().unwrap_or(&item.book_id);
        tx.execute(
            "DELETE FROM chunks WHERE book_id = ?1 AND track_id = ?2 AND COALESCE(start_ms, 0) >= ?3 AND COALESCE(start_ms, 0) < ?4",
            params![&item.book_id, track_id, slice_start_ms, slice_end_ms],
        )?;
        for chunk in chunks {
            insert_chunk_tx(&tx, chunk)?;
        }
        tx.execute(
            "INSERT INTO files(book_id, track_id, relative_path, fingerprint, status, status_message, attempt_count, next_retry_at, transcribed_until_ms, updated_at)
             VALUES(?1, ?2, ?3, ?4, 'pending', NULL, 0, NULL, ?5, ?6)
             ON CONFLICT(book_id, relative_path) DO UPDATE SET
             track_id = excluded.track_id, fingerprint = excluded.fingerprint, status = 'pending',
             status_message = NULL, transcribed_until_ms = excluded.transcribed_until_ms,
             updated_at = excluded.updated_at",
            params![
                &item.book_id,
                file.track_id.as_deref(),
                &file.relative_path,
                file_fingerprint(file),
                slice_end_ms,
                now_ms(),
            ],
        )?;
        recompute_item_status_tx(&tx, &item.book_id)?;
        set_metadata_tx(&tx, "last_indexed_at", &now_ms().to_string())?;
        tx.commit()?;
        Ok(())
    }

    pub fn mark_file_indexed(
        &self,
        item: &LibraryItemInput,
        file: &LibraryFileInput,
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE files SET status = 'indexed', status_message = NULL, attempt_count = 0,
             next_retry_at = NULL, transcribed_until_ms = 0, updated_at = ?1
             WHERE book_id = ?2 AND relative_path = ?3",
            params![now_ms(), &item.book_id, &file.relative_path],
        )?;
        recompute_item_status_tx(&tx, &item.book_id)?;
        tx.commit()?;
        Ok(())
    }

    pub fn mark_file_failed(
        &self,
        item: &LibraryItemInput,
        file: &LibraryFileInput,
        message: &str,
        now_ms: i64,
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        upsert_item_tx(&tx, item, "pending", Some(message))?;
        let prior = file_state_tx(&tx, &item.book_id, &file.relative_path)?;
        let attempt = prior
            .as_ref()
            .map(|s| s.attempt_count)
            .unwrap_or(0)
            .saturating_add(1);
        let delay_ms = retry_delay_ms(attempt);
        tx.execute(
            "INSERT INTO files(book_id, track_id, relative_path, fingerprint, status, status_message, attempt_count, next_retry_at, transcribed_until_ms, updated_at)
             VALUES(?1, ?2, ?3, ?4, 'failed', ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(book_id, relative_path) DO UPDATE SET
             track_id = excluded.track_id, fingerprint = excluded.fingerprint, status = 'failed',
             status_message = excluded.status_message, attempt_count = excluded.attempt_count,
             next_retry_at = excluded.next_retry_at, transcribed_until_ms = excluded.transcribed_until_ms,
             updated_at = excluded.updated_at",
            params![
                &item.book_id,
                file.track_id.as_deref(),
                &file.relative_path,
                file_fingerprint(file),
                cap_reason(message),
                attempt,
                now_ms.saturating_add(delay_ms),
                prior.as_ref().map(|s| s.transcribed_until_ms).unwrap_or(0),
                now_ms,
            ],
        )?;
        recompute_item_status_tx(&tx, &item.book_id)?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_file_progress(
        &self,
        item: &LibraryItemInput,
        file: &LibraryFileInput,
        transcribed_until_ms: i64,
    ) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE files SET transcribed_until_ms = ?1, updated_at = ?2 WHERE book_id = ?3 AND relative_path = ?4",
            params![transcribed_until_ms, now_ms(), &item.book_id, &file.relative_path],
        )?;
        Ok(())
    }

    pub fn mark_vectors_stale(&self, current_scheme: Option<&str>) -> Result<bool> {
        let conn = self.conn()?;
        let Some(scheme) = current_scheme else {
            return Ok(false);
        };
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
        if total == 0 {
            return Ok(false);
        }
        let ready: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chunks c JOIN vectors v ON v.chunk_id = c.id WHERE v.scheme = ?1",
            [scheme],
            |r| r.get(0),
        )?;
        Ok(ready < total)
    }

    pub fn unembedded_chunks(&self, scheme: &str, limit: usize) -> Result<Vec<(i64, String)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.text FROM chunks c LEFT JOIN vectors v ON v.chunk_id = c.id AND v.scheme = ?1 \
             WHERE v.chunk_id IS NULL ORDER BY c.id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![scheme, limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        rows.collect::<std::result::Result<_, _>>()
            .map_err(Into::into)
    }

    pub fn upsert_vectors(&self, scheme: &str, vectors: &[(i64, Vec<f32>)]) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        for (chunk_id, vector) in vectors {
            tx.execute(
                "INSERT OR REPLACE INTO vectors(chunk_id, scheme, dim, vector) VALUES(?1, ?2, ?3, ?4)",
                params![chunk_id, scheme, vector.len() as i64, vec_to_blob(vector)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn search(
        &self,
        query: &str,
        limit: usize,
        semantic: Option<(&str, &[f32])>,
    ) -> Result<Vec<LibrarySearchHit>> {
        if !self.settings()?.enabled {
            return Err(CoreError::Other("Library index is off".into()));
        }
        search_index(&self.conn()?, query, limit.min(100), semantic)
    }

    pub fn retrieve_context(
        &self,
        query: &str,
        limit: usize,
        semantic: Option<(&str, &[f32])>,
    ) -> Result<LibraryContext> {
        if !self.settings()?.enabled {
            return Err(CoreError::Other("Library index is off".into()));
        }
        let hits = self.search(query, limit, semantic)?;
        let entries = hits
            .into_iter()
            .enumerate()
            .map(|(idx, hit)| LibraryContextEntry {
                text: hit.text.clone(),
                source: LibrarySource {
                    index: idx + 1,
                    book_id: hit.book_id,
                    title: hit.title,
                    kind: hit.kind,
                    track_id: hit.track_id,
                    start_ms: hit.start_ms,
                    end_ms: hit.end_ms,
                    chapter: hit.chapter,
                    quote: hit.quote,
                },
            })
            .collect();
        Ok(LibraryContext { entries })
    }
}

#[derive(Debug, Clone)]
pub struct ChunkRecord {
    pub book_id: String,
    pub title: String,
    pub kind: LibraryItemKind,
    pub ordinal: usize,
    pub text: String,
    pub track_id: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub chapter: Option<String>,
    pub quote: Option<String>,
}

pub fn chunk_book_blocks(book_id: &str, title: &str, blocks: &[String]) -> Vec<ChunkRecord> {
    let mut chunks = Vec::new();
    let mut chapter: Option<String> = None;
    let mut buf = String::new();
    for block in blocks {
        let trimmed = block.trim();
        if let Some(heading) = trimmed.strip_prefix("# ") {
            if !buf.trim().is_empty() {
                push_book_chunk(book_id, title, &mut chunks, &buf, chapter.clone());
                buf.clear();
            }

            chapter = Some(heading.trim().to_string());
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        if buf.len() + trimmed.len() + 2 > BOOK_CHUNK_MAX && buf.len() >= BOOK_CHUNK_MIN {
            push_book_chunk(book_id, title, &mut chunks, &buf, chapter.clone());
            buf.clear();
        }
        if !buf.is_empty() {
            buf.push_str("\n\n");
        }
        buf.push_str(trimmed);
    }
    if !buf.trim().is_empty() {
        push_book_chunk(book_id, title, &mut chunks, &buf, chapter);
    }
    chunks
}

pub fn extract_book_chunks_from_path(
    book_id: &str,
    title: &str,
    path: &Path,
) -> Result<(Vec<ChunkRecord>, Option<String>)> {
    let text = crate::import::books::extract_original_text(path)?;
    Ok((
        chunk_book_blocks(book_id, title, &text.blocks),
        text.warning,
    ))
}

fn push_book_chunk(
    book_id: &str,
    title: &str,
    chunks: &mut Vec<ChunkRecord>,
    text: &str,
    chapter: Option<String>,
) {
    let text = bounded_text(text, MAX_CHUNK_TEXT);
    chunks.push(ChunkRecord {
        book_id: book_id.to_string(),
        title: title.to_string(),
        kind: LibraryItemKind::Epub,
        ordinal: chunks.len(),
        quote: Some(make_quote(&text)),
        text,
        track_id: None,
        start_ms: None,
        end_ms: None,
        chapter,
    });
}

pub fn chunk_transcript(
    book_id: &str,
    title: &str,
    kind: LibraryItemKind,
    track_id: &str,
    transcript: &Transcript,
) -> Vec<ChunkRecord> {
    let mut chunks = Vec::new();
    let mut window: Vec<&TranscriptSegment> = Vec::new();
    for segment in transcript
        .segments
        .iter()
        .filter(|s| !s.text.trim().is_empty())
    {
        if let Some(first) = window.first() {
            let span = segment.end_ms.saturating_sub(first.start_ms);
            if span > MEDIA_WINDOW_MAX_MS
                || (span >= MEDIA_WINDOW_MIN_MS
                    && window.iter().map(|s| s.text.len()).sum::<usize>() > 600)
            {
                push_media_chunk(book_id, title, kind, track_id, &mut chunks, &window);
                window.clear();
            }
        }
        window.push(segment);
    }
    if !window.is_empty() {
        push_media_chunk(book_id, title, kind, track_id, &mut chunks, &window);
    }
    chunks
}

fn push_media_chunk(
    book_id: &str,
    title: &str,
    kind: LibraryItemKind,
    track_id: &str,
    chunks: &mut Vec<ChunkRecord>,
    segments: &[&TranscriptSegment],
) {
    let text = bounded_text(
        &segments
            .iter()
            .map(|s| s.text.trim())
            .collect::<Vec<_>>()
            .join(" "),
        MAX_CHUNK_TEXT,
    );
    chunks.push(ChunkRecord {
        book_id: book_id.to_string(),
        title: title.to_string(),
        kind,
        ordinal: chunks.len(),
        text,
        track_id: Some(track_id.to_string()),
        start_ms: segments.first().map(|s| s.start_ms.max(0)),
        end_ms: segments.last().map(|s| s.end_ms.max(0)),
        chapter: None,
        quote: None,
    });
}

pub fn sanitize_fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .map(str::trim)
        .filter(|s| s.len() >= 2)
        .take(16)
        .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

pub fn fingerprint_files(files: &[LibraryFileInput]) -> String {
    let mut parts: Vec<_> = files
        .iter()
        .map(|f| format!("{}:{}:{}", f.relative_path, f.size, f.mtime_ms))
        .collect();
    parts.sort();
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

pub async fn embed_documents_on_device(
    embedder: Option<&dyn Embedder>,
    texts: &[String],
    cancel: Option<&AtomicBool>,
) -> Result<Option<(String, Vec<Vec<f32>>)>> {
    let Some(embedder) = embedder else {
        return Ok(None);
    };
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
        return Err(CoreError::Other("Library indexing cancelled".into()));
    }
    let scheme = embedder.embedding_scheme_id();
    let vectors = embedder.embed_documents(texts).await?;
    Ok(Some((scheme, vectors)))
}

fn init(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS items(
            book_id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            kind TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            status TEXT NOT NULL,
            status_message TEXT,
            source_url TEXT,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS chunks(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            book_id TEXT NOT NULL REFERENCES items(book_id) ON DELETE CASCADE,
            title TEXT NOT NULL,
            kind TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            text TEXT NOT NULL,
            track_id TEXT,
            start_ms INTEGER,
            end_ms INTEGER,
            chapter TEXT,
            quote TEXT
        );
        CREATE TABLE IF NOT EXISTS files(
            book_id TEXT NOT NULL REFERENCES items(book_id) ON DELETE CASCADE,
            track_id TEXT,
            relative_path TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            status TEXT NOT NULL,
            status_message TEXT,
            attempt_count INTEGER NOT NULL DEFAULT 0,
            next_retry_at INTEGER,
            transcribed_until_ms INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(book_id, relative_path)
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
            title, text, content='chunks', content_rowid='id', tokenize='unicode61'
        );
        CREATE TABLE IF NOT EXISTS vectors(
            chunk_id INTEGER NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
            scheme TEXT NOT NULL,
            dim INTEGER NOT NULL,
            vector BLOB NOT NULL,
            PRIMARY KEY(chunk_id, scheme)
        );
        CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
            INSERT INTO chunks_fts(rowid, title, text) VALUES (new.id, new.title, new.text);
        END;
        CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
            INSERT INTO chunks_fts(chunks_fts, rowid, title, text) VALUES('delete', old.id, old.title, old.text);
        END;
        "#,
    )?;
    ensure_column(conn, "files", "attempt_count", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(conn, "files", "next_retry_at", "INTEGER")?;
    ensure_column(
        conn,
        "files",
        "transcribed_until_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    Ok(())
}

fn ensure_column(conn: &Connection, table: &str, column: &str, definition: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == column);
    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn setting_bool(conn: &Connection, key: &str, default: bool) -> Result<bool> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .map(|v| v == "true")
        .unwrap_or(default))
}

fn set_setting_bool(conn: &Connection, key: &str, value: bool) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO settings(key, value) VALUES(?1, ?2)",
        params![key, if value { "true" } else { "false" }],
    )?;
    Ok(())
}

fn set_metadata_tx(tx: &rusqlite::Transaction<'_>, key: &str, value: &str) -> Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO metadata(key, value) VALUES(?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

fn delta_actions(conn: &Connection, inputs: &[LibraryItemInput]) -> Result<Vec<DeltaAction>> {
    let existing: HashMap<String, (String, String, Option<String>)> = conn
        .prepare("SELECT book_id, fingerprint, status, status_message FROM items")?
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
        .collect::<std::result::Result<_, _>>()?;
    let mut active = HashSet::new();
    let mut actions = Vec::new();
    for item in inputs {
        active.insert(item.book_id.clone());
        let fingerprint = item.fingerprint();
        match existing.get(&item.book_id) {
            None => actions.push(DeltaAction::New(item.book_id.clone())),
            Some(_) if !item.available || item.files.iter().any(|f| !f.available) => {
                actions.push(DeltaAction::Unchanged(item.book_id.clone()))
            }
            Some((old, status, _)) if old != &fingerprint || status == "failed" => {
                actions.push(DeltaAction::Changed(item.book_id.clone()))
            }
            Some(_) => actions.push(DeltaAction::Unchanged(item.book_id.clone())),
        }
    }
    for id in existing.keys() {
        if !active.contains(id) {
            actions.push(DeltaAction::Removed(id.clone()));
        }
    }
    Ok(actions)
}

fn existing_ids(conn: &Connection) -> Result<HashSet<String>> {
    conn.prepare("SELECT book_id FROM items")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()
        .map_err(Into::into)
}

fn file_fingerprint(file: &LibraryFileInput) -> String {
    format!("{}:{}:{}", file.relative_path, file.size, file.mtime_ms)
}

fn file_states(conn: &Connection, book_id: &str) -> Result<Vec<LibraryFileState>> {
    conn.prepare(
        "SELECT relative_path, track_id, fingerprint, status, status_message, attempt_count, next_retry_at, transcribed_until_ms
         FROM files WHERE book_id = ?1 ORDER BY relative_path",
    )?
    .query_map([book_id], |r| {
        Ok(LibraryFileState {
            relative_path: r.get(0)?,
            track_id: r.get(1)?,
            fingerprint: r.get(2)?,
            status: r.get(3)?,
            status_message: r.get(4)?,
            attempt_count: r.get(5)?,
            next_retry_at: r.get(6)?,
            transcribed_until_ms: r.get(7)?,
        })
    })?
    .collect::<std::result::Result<_, _>>()
    .map_err(Into::into)
}

fn file_state_tx(
    tx: &rusqlite::Transaction<'_>,
    book_id: &str,
    relative_path: &str,
) -> Result<Option<LibraryFileState>> {
    Ok(tx
        .query_row(
            "SELECT relative_path, track_id, fingerprint, status, status_message, attempt_count, next_retry_at, transcribed_until_ms
             FROM files WHERE book_id = ?1 AND relative_path = ?2",
            params![book_id, relative_path],
            |r| {
                Ok(LibraryFileState {
                    relative_path: r.get(0)?,
                    track_id: r.get(1)?,
                    fingerprint: r.get(2)?,
                    status: r.get(3)?,
                    status_message: r.get(4)?,
                    attempt_count: r.get(5)?,
                    next_retry_at: r.get(6)?,
                    transcribed_until_ms: r.get(7)?,
                })
            },
        )
        .optional()?)
}

fn recompute_item_status_tx(tx: &rusqlite::Transaction<'_>, book_id: &str) -> Result<()> {
    let states: Vec<(String, Option<String>)> = tx
        .prepare("SELECT status, status_message FROM files WHERE book_id = ?1")?
        .query_map([book_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    if states.is_empty() {
        return Ok(());
    }
    let status = if states.iter().all(|(s, _)| s == "indexed") {
        "indexed"
    } else if states.iter().any(|(s, _)| s == "failed") {
        "failed"
    } else if states.iter().any(|(s, _)| s == "title_only") {
        "title_only"
    } else {
        "pending"
    };
    let message = states
        .iter()
        .find_map(|(s, m)| (s == "failed").then(|| m.clone()).flatten())
        .map(|m| cap_reason(&m));
    tx.execute(
        "UPDATE items SET status = ?1, status_message = ?2, updated_at = ?3 WHERE book_id = ?4",
        params![status, message.as_deref(), now_ms(), book_id],
    )?;
    Ok(())
}

fn retry_delay_ms(attempt: i64) -> i64 {
    match attempt {
        i if i <= 1 => 10 * 60 * 1000,
        2 => 60 * 60 * 1000,
        3 => 6 * 60 * 60 * 1000,
        _ => 24 * 60 * 60 * 1000,
    }
}

fn upsert_item(
    conn: &Connection,
    item: &LibraryItemInput,
    status: &str,
    message: Option<&str>,
) -> Result<()> {
    let message = message.map(cap_reason);
    let fingerprint = if !item.available || item.files.iter().any(|f| !f.available) {
        existing_fingerprint(conn, &item.book_id)?.unwrap_or_else(|| item.fingerprint())
    } else {
        item.fingerprint()
    };
    conn.execute(
        "INSERT INTO items(book_id, title, kind, fingerprint, status, status_message, source_url, updated_at) \
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(book_id) DO UPDATE SET \
         title = excluded.title, kind = excluded.kind, fingerprint = excluded.fingerprint, \
         status = excluded.status, status_message = excluded.status_message, \
         source_url = excluded.source_url, updated_at = excluded.updated_at",
        params![
            &item.book_id,
            &item.title,
            item.kind.as_str(),
            fingerprint,
            status,
            message.as_deref(),
            item.source_url.as_deref(),
            now_ms()
        ],
    )?;
    Ok(())
}

fn upsert_item_tx(
    tx: &rusqlite::Transaction<'_>,
    item: &LibraryItemInput,
    status: &str,
    message: Option<&str>,
) -> Result<()> {
    let message = message.map(cap_reason);
    let fingerprint = if !item.available || item.files.iter().any(|f| !f.available) {
        existing_fingerprint_tx(tx, &item.book_id)?.unwrap_or_else(|| item.fingerprint())
    } else {
        item.fingerprint()
    };
    tx.execute(
        "INSERT INTO items(book_id, title, kind, fingerprint, status, status_message, source_url, updated_at) \
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(book_id) DO UPDATE SET \
         title = excluded.title, kind = excluded.kind, fingerprint = excluded.fingerprint, \
         status = excluded.status, status_message = excluded.status_message, \
         source_url = excluded.source_url, updated_at = excluded.updated_at",
        params![
            &item.book_id,
            &item.title,
            item.kind.as_str(),
            fingerprint,
            status,
            message.as_deref(),
            item.source_url.as_deref(),
            now_ms()
        ],
    )?;
    Ok(())
}

fn existing_fingerprint(conn: &Connection, book_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT fingerprint FROM items WHERE book_id = ?1",
            [book_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?)
}

fn existing_fingerprint_tx(
    tx: &rusqlite::Transaction<'_>,
    book_id: &str,
) -> Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT fingerprint FROM items WHERE book_id = ?1",
            [book_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?)
}

fn delete_item(conn: &Connection, book_id: &str) -> Result<()> {
    conn.execute("DELETE FROM items WHERE book_id = ?1", [book_id])?;
    Ok(())
}

fn delete_chunks_tx(tx: &rusqlite::Transaction<'_>, book_id: &str) -> Result<()> {
    tx.execute("DELETE FROM chunks WHERE book_id = ?1", [book_id])?;
    Ok(())
}

fn delete_track_chunks_tx(
    tx: &rusqlite::Transaction<'_>,
    book_id: &str,
    track_id: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM chunks WHERE book_id = ?1 AND track_id = ?2",
        params![book_id, track_id],
    )?;
    Ok(())
}

fn insert_chunk_tx(tx: &rusqlite::Transaction<'_>, chunk: &ChunkRecord) -> Result<()> {
    tx.execute(
        "INSERT INTO chunks(book_id, title, kind, ordinal, text, track_id, start_ms, end_ms, chapter, quote) \
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            &chunk.book_id,
            &chunk.title,
            chunk.kind.as_str(),
            chunk.ordinal as i64,
            bounded_text(&chunk.text, MAX_CHUNK_TEXT),
            chunk.track_id.as_deref(),
            chunk.start_ms,
            chunk.end_ms,
            chunk.chapter.as_deref(),
            chunk.quote.as_deref(),
        ],
    )?;
    Ok(())
}

fn search_index(
    conn: &Connection,
    query: &str,
    limit: usize,
    semantic: Option<(&str, &[f32])>,
) -> Result<Vec<LibrarySearchHit>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut scores: HashMap<i64, (f32, bool, bool)> = HashMap::new();
    if let Some(fts) = sanitize_fts_query(query) {
        let mut stmt = conn.prepare(
            "SELECT c.id, bm25(chunks_fts) AS rank FROM chunks_fts \
             JOIN chunks c ON c.id = chunks_fts.rowid WHERE chunks_fts MATCH ?1 \
             ORDER BY rank LIMIT ?2",
        )?;
        for (rank, row) in stmt
            .query_map(params![fts, (limit * 4).max(20) as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, f32>(1)?))
            })?
            .enumerate()
        {
            let (id, bm25) = row?;
            let rrf = 1.0 / (60.0 + rank as f32 + bm25.abs().min(10.0) * 0.01);
            scores.insert(id, (rrf, true, false));
        }
    }
    if let Some((scheme, vector)) = semantic {
        let mut stmt = conn.prepare("SELECT chunk_id, vector FROM vectors WHERE scheme = ?1")?;
        let mut dense = Vec::new();
        for row in stmt.query_map([scheme], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })? {
            let (id, blob) = row?;
            let stored = blob_to_vec(&blob);
            if stored.len() == vector.len() {
                dense.push((id, cosine(vector, &stored)));
            }
        }
        dense.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (rank, (id, _)) in dense.into_iter().take((limit * 4).max(20)).enumerate() {
            let add = 1.0 / (60.0 + rank as f32);
            scores
                .entry(id)
                .and_modify(|s| {
                    s.0 += add;
                    s.2 = true;
                })
                .or_insert((add, false, true));
        }
    }
    if scores.is_empty() {
        let like = format!("%{}%", query.trim().replace('%', "\\%").replace('_', "\\_"));
        let mut stmt =
            conn.prepare("SELECT id FROM chunks WHERE title LIKE ?1 ESCAPE '\\' LIMIT ?2")?;
        for (rank, row) in stmt
            .query_map(params![like, limit as i64], |r| r.get::<_, i64>(0))?
            .enumerate()
        {
            scores.insert(row?, (1.0 / (60.0 + rank as f32), false, false));
        }
    }
    let mut ids: Vec<_> = scores.into_iter().collect();
    ids.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0));
    let mut hits = Vec::new();
    for (id, (score, keyword, semantic_hit)) in ids.into_iter().take(limit) {
        let hit = load_hit(conn, id, query, score, keyword, semantic_hit)?;
        hits.push(hit);
    }
    Ok(hits)
}

fn load_hit(
    conn: &Connection,
    id: i64,
    query: &str,
    score: f32,
    keyword: bool,
    semantic: bool,
) -> Result<LibrarySearchHit> {
    conn.query_row(
        "SELECT book_id, title, kind, text, track_id, start_ms, end_ms, chapter, quote FROM chunks WHERE id = ?1",
        [id],
        |r| {
            let text: String = r.get(3)?;
            Ok(LibrarySearchHit {
                chunk_id: id.to_string(),
                book_id: r.get(0)?,
                title: r.get(1)?,
                kind: LibraryItemKind::from_db(&r.get::<_, String>(2)?),
                snippet: snippet(&text, query),
                track_id: r.get(4)?,
                start_ms: r.get(5)?,
                end_ms: r.get(6)?,
                chapter: r.get(7)?,
                quote: r.get(8)?,
                score,
                match_kind: match (keyword, semantic) {
                    (true, true) => LibraryMatchKind::Both,
                    (true, false) => LibraryMatchKind::Keyword,
                    (false, true) => LibraryMatchKind::Semantic,
                    (false, false) => LibraryMatchKind::Title,
                },
                text,
            })
        },
    ).map_err(Into::into)
}

fn make_quote(text: &str) -> String {
    word_boundary(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        MAX_QUOTE,
    )
}

fn snippet(text: &str, query: &str) -> String {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| t.len() > 2)
        .map(|t| t.to_lowercase())
        .collect();
    let mut pos = 0;
    for (idx, _) in text.char_indices() {
        let tail = text[idx..]
            .chars()
            .take(80)
            .collect::<String>()
            .to_lowercase();
        if terms.iter().any(|term| tail.starts_with(term)) {
            pos = idx;
            break;
        }
    }
    let start = text[..pos]
        .char_indices()
        .rev()
        .nth(80)
        .map(|(i, _)| i)
        .unwrap_or(0);
    word_boundary(&text[start..], MAX_SNIPPET)
}

fn bounded_text(text: &str, max: usize) -> String {
    word_boundary(text.trim(), max)
}

fn word_boundary(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = text
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|i| *i <= max)
        .last()
        .unwrap_or(0);
    if let Some(space) = text[..end].rfind(char::is_whitespace) {
        end = space;
    }
    text[..end].trim_end().to_string()
}

pub fn cap_reason(value: &str) -> String {
    const ELLIPSIS: &str = "...";
    let value = value.trim();
    if value.chars().count() <= MAX_REASON_CHARS {
        return value.to_string();
    }
    let keep = MAX_REASON_CHARS.saturating_sub(ELLIPSIS.len());
    let mut out: String = value.chars().take(keep).collect();
    out.push_str(ELLIPSIS);
    out
}

fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn blob_to_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0;
    let mut an = 0.0;
    let mut bn = 0.0;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        an += x * x;
        bn += y * y;
    }
    if an == 0.0 || bn == 0.0 {
        0.0
    } else {
        dot / (an.sqrt() * bn.sqrt())
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn scratch(name: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("library-index-tests")
            .join(format!("{name}-{}", N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn item(id: &str, title: &str) -> LibraryItemInput {
        LibraryItemInput {
            book_id: id.into(),
            title: title.into(),
            kind: LibraryItemKind::Epub,
            source_url: None,
            available: true,
            files: vec![LibraryFileInput {
                track_id: None,
                relative_path: format!("{id}.epub"),
                absolute_path: PathBuf::from(format!("{id}.epub")),
                size: 10,
                mtime_ms: 1,
                available: true,
            }],
        }
    }

    fn audio_item() -> LibraryItemInput {
        LibraryItemInput {
            book_id: "audio".into(),
            title: "Audio".into(),
            kind: LibraryItemKind::Audio,
            source_url: None,
            available: true,
            files: vec![LibraryFileInput {
                track_id: Some("track".into()),
                relative_path: "track.mp3".into(),
                absolute_path: PathBuf::from("track.mp3"),
                size: 10,
                mtime_ms: 1,
                available: true,
            }],
        }
    }

    struct FakeSlicer {
        ends: Vec<i64>,
        final_starts: Vec<i64>,
        fail_at: Option<i64>,
        no_audio_at: Vec<i64>,
    }

    impl MediaSliceSource for FakeSlicer {
        fn slice(&mut self, start_ms: i64, duration_ms: i64) -> Result<MediaSlice> {
            if self.fail_at == Some(start_ms) {
                return Err(CoreError::Other("slice failed".into()));
            }
            if self.no_audio_at.contains(&start_ms) {
                return Ok(MediaSlice::NoAudio);
            }
            if self.ends.contains(&start_ms) {
                return Ok(MediaSlice::EndOfFile);
            }
            Ok(MediaSlice::Data {
                start_ms,
                end_ms: start_ms + duration_ms,
                is_final: self.final_starts.contains(&start_ms),
            })
        }
    }

    #[derive(Default)]
    struct FakeTranscriber {
        fail_at: Option<i64>,
        calls: Vec<i64>,
    }

    impl MediaSliceTranscriber for FakeTranscriber {
        fn transcribe_slice(
            &mut self,
            start_ms: i64,
            _end_ms: i64,
        ) -> Result<Vec<TranscriptSegment>> {
            self.calls.push(start_ms);
            if self.fail_at == Some(start_ms) {
                return Err(CoreError::Other("transcribe failed".into()));
            }
            Ok(vec![TranscriptSegment {
                start_ms: 0,
                end_ms: 1_000,
                text: format!("slice-{start_ms}"),
            }])
        }
    }

    fn track_hits(store: &LibraryIndexStore) -> Vec<LibrarySearchHit> {
        store.search("slice", 20, None).unwrap()
    }

    #[test]
    fn media_loop_eof_indexes_all_slices_with_offsets() {
        let store = LibraryIndexStore::open(scratch("loop-eof").join("index.sqlite")).unwrap();
        let item = audio_item();
        let file = item.files[0].clone();
        store.due_files(&item, false, 0).unwrap();
        let mut slicer = FakeSlicer {
            ends: vec![10_000],
            final_starts: vec![],
            fail_at: None,
            no_audio_at: vec![],
        };
        let mut transcriber = FakeTranscriber::default();
        assert_eq!(
            index_media_file_slices(
                &store,
                &item,
                &file,
                5_000,
                false,
                || None,
                &mut slicer,
                &mut transcriber,
            )
            .unwrap(),
            MediaIndexOutcome::Completed
        );
        let hits = track_hits(&store);
        assert_eq!(hits.len(), 2);
        let starts: HashSet<_> = hits.into_iter().map(|h| h.start_ms.unwrap()).collect();
        assert_eq!(starts, HashSet::from([0, 5_000]));
        assert_eq!(store.file_states("audio").unwrap()[0].status, "indexed");
    }

    #[test]
    fn media_loop_no_audio_file_is_done_without_touching_sibling_tracks() {
        let store = LibraryIndexStore::open(scratch("loop-no-audio").join("index.sqlite")).unwrap();
        let mut item = audio_item();
        let mut second = item.files[0].clone();
        second.track_id = Some("track-2".into());
        second.relative_path = "track-2.mp3".into();
        second.absolute_path = PathBuf::from("track-2.mp3");
        item.files.push(second.clone());
        let file = item.files[0].clone();
        store.due_files(&item, false, 0).unwrap();
        // The other track was paused 40 seconds in.
        store.set_file_progress(&item, &second, 40_000).unwrap();
        let mut slicer = FakeSlicer {
            ends: vec![],
            final_starts: vec![],
            fail_at: None,
            no_audio_at: vec![0],
        };
        let mut transcriber = FakeTranscriber::default();
        assert_eq!(
            index_media_file_slices(
                &store,
                &item,
                &file,
                5_000,
                false,
                || None,
                &mut slicer,
                &mut transcriber
            )
            .unwrap(),
            MediaIndexOutcome::Completed
        );
        let states = store.file_states("audio").unwrap();
        let silent = states.iter().find(|state| state.relative_path == file.relative_path).unwrap();
        assert_eq!(silent.status, "indexed");
        let paused = states.iter().find(|state| state.relative_path == second.relative_path).unwrap();
        assert_eq!(paused.transcribed_until_ms, 40_000);
        assert_ne!(paused.status, "title_only");
        assert!(track_hits(&store).is_empty());
    }

    #[test]
    fn media_loop_short_slice_marks_indexed_without_extra_slice() {
        let store =
            LibraryIndexStore::open(scratch("loop-short-final").join("index.sqlite")).unwrap();
        let item = audio_item();
        let file = item.files[0].clone();
        store.due_files(&item, false, 0).unwrap();
        let mut slicer = FakeSlicer {
            ends: vec![],
            final_starts: vec![5_000],
            fail_at: None,
            no_audio_at: vec![],
        };
        let mut transcriber = FakeTranscriber::default();
        assert_eq!(
            index_media_file_slices(
                &store,
                &item,
                &file,
                5_000,
                false,
                || None,
                &mut slicer,
                &mut transcriber
            )
            .unwrap(),
            MediaIndexOutcome::Completed
        );
        assert_eq!(transcriber.calls, vec![0, 5_000]);
        assert_eq!(store.file_states("audio").unwrap()[0].status, "indexed");
    }

    #[test]
    fn media_loop_pause_persists_chunks_and_resume_appends_without_gaps() {
        let store = LibraryIndexStore::open(scratch("loop-pause").join("index.sqlite")).unwrap();
        let item = audio_item();
        let file = item.files[0].clone();
        store.due_files(&item, false, 0).unwrap();
        let mut slicer = FakeSlicer {
            ends: vec![15_000],
            final_starts: vec![],
            fail_at: None,
            no_audio_at: vec![],
        };
        let mut transcriber = FakeTranscriber::default();
        let mut checks = 0;
        assert_eq!(
            index_media_file_slices(
                &store,
                &item,
                &file,
                5_000,
                false,
                || {
                    checks += 1;
                    (checks > 2).then(|| "paused".to_string())
                },
                &mut slicer,
                &mut transcriber,
            )
            .unwrap(),
            MediaIndexOutcome::Paused("paused".into())
        );
        assert_eq!(track_hits(&store).len(), 2);
        let state = &store.file_states("audio").unwrap()[0];
        assert_eq!(state.status, "pending");
        assert_eq!(state.attempt_count, 0);
        assert_eq!(state.transcribed_until_ms, 10_000);

        let mut transcriber = FakeTranscriber::default();
        let mut slicer = FakeSlicer {
            ends: vec![15_000],
            final_starts: vec![],
            fail_at: None,
            no_audio_at: vec![],
        };
        assert_eq!(
            index_media_file_slices(
                &store,
                &item,
                &file,
                5_000,
                false,
                || None,
                &mut slicer,
                &mut transcriber,
            )
            .unwrap(),
            MediaIndexOutcome::Completed
        );
        assert_eq!(transcriber.calls, vec![10_000]);
        let starts: HashSet<_> = track_hits(&store)
            .into_iter()
            .map(|h| h.start_ms.unwrap())
            .collect();
        assert_eq!(starts, HashSet::from([0, 5_000, 10_000]));
    }

    #[test]
    fn media_loop_failure_keeps_prior_chunks_and_retry_resumes() {
        let store = LibraryIndexStore::open(scratch("loop-fail").join("index.sqlite")).unwrap();
        let item = audio_item();
        let file = item.files[0].clone();
        store.due_files(&item, false, 0).unwrap();
        let mut slicer = FakeSlicer {
            ends: vec![15_000],
            final_starts: vec![],
            fail_at: None,
            no_audio_at: vec![],
        };
        let mut transcriber = FakeTranscriber {
            fail_at: Some(10_000),
            calls: Vec::new(),
        };
        assert!(matches!(
            index_media_file_slices(
                &store,
                &item,
                &file,
                5_000,
                false,
                || None,
                &mut slicer,
                &mut transcriber,
            )
            .unwrap(),
            MediaIndexOutcome::Failed(_)
        ));
        assert_eq!(track_hits(&store).len(), 2);
        let state = &store.file_states("audio").unwrap()[0];
        assert_eq!(state.status, "failed");
        assert_eq!(state.transcribed_until_ms, 10_000);
        assert_eq!(state.attempt_count, 1);

        let mut slicer = FakeSlicer {
            ends: vec![15_000],
            final_starts: vec![],
            fail_at: None,
            no_audio_at: vec![],
        };
        let mut transcriber = FakeTranscriber::default();
        assert_eq!(
            index_media_file_slices(
                &store,
                &item,
                &file,
                5_000,
                false,
                || None,
                &mut slicer,
                &mut transcriber,
            )
            .unwrap(),
            MediaIndexOutcome::Completed
        );
        assert_eq!(transcriber.calls, vec![10_000]);
        assert_eq!(track_hits(&store).len(), 3);
    }

    #[test]
    fn pending_file_is_due_on_next_delta() {
        let store = LibraryIndexStore::open(scratch("pending-due").join("index.sqlite")).unwrap();
        let item = audio_item();
        let due = store.due_files(&item, false, 0).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(store.due_files(&item, false, 1).unwrap().len(), 1);
    }

    #[test]
    fn rebuild_replaces_slices_incrementally_without_dropping_later_windows() {
        let store =
            LibraryIndexStore::open(scratch("rebuild-incremental").join("index.sqlite")).unwrap();
        let item = audio_item();
        let file = item.files[0].clone();
        store.due_files(&item, false, 0).unwrap();
        let mut slicer = FakeSlicer {
            ends: vec![10_000],
            final_starts: vec![],
            fail_at: None,
            no_audio_at: vec![],
        };
        let mut transcriber = FakeTranscriber::default();
        index_media_file_slices(
            &store,
            &item,
            &file,
            5_000,
            false,
            || None,
            &mut slicer,
            &mut transcriber,
        )
        .unwrap();
        assert_eq!(track_hits(&store).len(), 2);

        let slice = Transcript::from_segments(vec![TranscriptSegment {
            start_ms: 0,
            end_ms: 1000,
            text: "slice-new".into(),
        }]);
        let chunks = chunk_transcript("audio", "Audio", LibraryItemKind::Audio, "track", &slice);
        store
            .index_track_slice_chunks(&item, &file, &chunks, 0, 5_000)
            .unwrap();
        let hits = track_hits(&store);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|h| h.snippet.contains("slice-new")));
        assert!(hits.iter().any(|h| h.start_ms == Some(5_000)));
    }

    #[test]
    fn title_only_preserves_completed_transcripts_and_resets_incomplete_progress() {
        let store =
            LibraryIndexStore::open(scratch("title-only-preserve").join("index.sqlite")).unwrap();
        let mut item = audio_item();
        item.files.push(LibraryFileInput {
            track_id: Some("incomplete".into()),
            relative_path: "incomplete.mp3".into(),
            absolute_path: PathBuf::from("incomplete.mp3"),
            size: 10,
            mtime_ms: 1,
            available: true,
        });
        let complete = item.files[0].clone();
        let incomplete = item.files[1].clone();
        store.due_files(&item, false, 0).unwrap();
        let chunk = ChunkRecord {
            book_id: "audio".into(),
            title: "Audio".into(),
            kind: LibraryItemKind::Audio,
            ordinal: 0,
            text: "completed transcript".into(),
            track_id: complete.track_id.clone(),
            start_ms: Some(0),
            end_ms: Some(1_000),
            chapter: None,
            quote: None,
        };
        store
            .index_track_chunks(&item, &complete, &[chunk], None)
            .unwrap();
        store.set_file_progress(&item, &incomplete, 5_000).unwrap();
        store.index_title_only(&item, "off").unwrap();
        assert_eq!(
            store
                .search("completed transcript", 10, None)
                .unwrap()
                .len(),
            1
        );
        let states = store.file_states("audio").unwrap();
        assert_eq!(
            states
                .iter()
                .find(|s| s.relative_path == "track.mp3")
                .unwrap()
                .status,
            "indexed"
        );
        let incomplete_state = states
            .iter()
            .find(|s| s.relative_path == "incomplete.mp3")
            .unwrap();
        assert_eq!(incomplete_state.status, "title_only");
        assert_eq!(incomplete_state.transcribed_until_ms, 0);
    }

    #[test]
    fn serde_contract_uses_fixed_wire_keys() {
        let hit = LibrarySearchHit {
            chunk_id: "1".into(),
            book_id: "b".into(),
            title: "Title".into(),
            kind: LibraryItemKind::Epub,
            snippet: "snippet".into(),
            track_id: None,
            start_ms: None,
            end_ms: None,
            chapter: Some("Chapter".into()),
            quote: Some("Quote".into()),
            score: 1.0,
            match_kind: LibraryMatchKind::Keyword,
            text: "full text".into(),
        };
        let value = serde_json::to_value(&hit).unwrap();
        assert_eq!(value["chunkId"], "1");
        assert_eq!(value["match"], "keyword");
        assert!(value.get("matchKind").is_none());
        assert!(value.get("text").is_none());

        let status = LibraryIndexStatus {
            enabled: true,
            transcribe_media: true,
            running: false,
            job_id: None,
            items: LibraryItemCounts {
                total: 1,
                indexed: 1,
                pending: 0,
                failed: 0,
                title_only: 0,
            },
            chunks: 1,
            semantic: LibrarySemanticState::Ready,
            semantic_reason: None,
            transcription: LibraryTranscriptionState::Ready,
            transcription_reason: None,
            last_indexed_at: Some(1),
            errors: vec![],
        };
        let value = serde_json::to_value(&status).unwrap();
        assert!(value.get("transcribeMedia").is_some());
        assert!(value.get("semanticReason").is_some());
        assert!(value.get("lastIndexedAt").is_some());
    }

    #[test]
    fn chunks_books_with_chapter_and_quote() {
        let blocks = vec![
            "# Chapter One".into(),
            "alpha ".repeat(260),
            "bravo ".repeat(260),
            "# Chapter Two".into(),
            "charlie ".repeat(200),
        ];
        let chunks = chunk_book_blocks("b1", "Book", &blocks);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].chapter.as_deref(), Some("Chapter One"));
        assert!(chunks[0].quote.as_ref().unwrap().len() <= MAX_QUOTE);
        assert_eq!(
            chunks.last().unwrap().chapter.as_deref(),
            Some("Chapter Two")
        );
    }

    #[test]
    fn chunks_transcripts_into_time_windows() {
        let segments = (0..6)
            .map(|i| TranscriptSegment {
                start_ms: i * 15_000,
                end_ms: i * 15_000 + 10_000,
                text: format!("segment {i}"),
            })
            .collect();
        let transcript = Transcript::from_segments(segments);
        let chunks = chunk_transcript("m", "Media", LibraryItemKind::Audio, "t1", &transcript);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].track_id.as_deref(), Some("t1"));
        assert!(chunks[0].end_ms.unwrap() - chunks[0].start_ms.unwrap() <= MEDIA_WINDOW_MAX_MS);
    }

    #[test]
    fn fts_query_sanitizes_punctuation() {
        let q = sanitize_fts_query("\"fuel\" OR fail NEAR/1 drop").unwrap();
        assert!(q.contains("\"fuel\""));
        assert!(q.contains("\"drop\""));
        assert!(!q.contains("NEAR/1"));
    }

    #[test]
    fn delta_fingerprints_track_new_changed_removed_unchanged() {
        let store = LibraryIndexStore::open(scratch("delta").join("index.sqlite")).unwrap();
        let first = item("a", "A");
        store.index_title_only(&first, "seed").unwrap();
        let actions = store.delta_actions(std::slice::from_ref(&first)).unwrap();
        assert!(actions.contains(&DeltaAction::Unchanged("a".into())));
        let mut changed = first.clone();
        changed.files[0].size = 11;
        let new = item("b", "B");
        let actions = store.delta_actions(&[changed, new]).unwrap();
        assert!(actions.contains(&DeltaAction::Changed("a".into())));
        assert!(actions.contains(&DeltaAction::New("b".into())));
        assert!(actions.contains(&DeltaAction::Removed("a".into())) == false);
        let actions = store.delta_actions(&[]).unwrap();
        assert!(actions.contains(&DeltaAction::Removed("a".into())));
    }

    #[test]
    fn unavailable_source_keeps_existing_fingerprint_and_chunks() {
        let store = LibraryIndexStore::open(scratch("unavailable").join("index.sqlite")).unwrap();
        let first = item("a", "A");
        let chunk = ChunkRecord {
            book_id: "a".into(),
            title: "A".into(),
            kind: LibraryItemKind::Epub,
            ordinal: 0,
            text: "old indexed text".into(),
            track_id: None,
            start_ms: None,
            end_ms: None,
            chapter: None,
            quote: Some("old".into()),
        };
        store.index_chunks(&first, &[chunk], None, None).unwrap();
        let mut unavailable = first.clone();
        unavailable.available = false;
        unavailable.files[0].available = false;
        unavailable.files[0].size = 0;
        unavailable.files[0].mtime_ms = 0;
        assert!(store
            .delta_actions(std::slice::from_ref(&unavailable))
            .unwrap()
            .contains(&DeltaAction::Unchanged("a".into())));
        store.index_failed(&unavailable, "Source missing").unwrap();
        assert_eq!(store.search("old indexed", 10, None).unwrap().len(), 1);
    }

    #[test]
    fn disabled_index_refuses_stale_search_reads() {
        let store = LibraryIndexStore::open(scratch("disabled").join("index.sqlite")).unwrap();
        let item = item("a", "A");
        store.index_title_only(&item, "seed").unwrap();
        store
            .set_settings(&LibraryIndexSettings {
                enabled: false,
                transcribe_media: true,
            })
            .unwrap();
        let error = store.search("A", 10, None).unwrap_err().to_string();
        assert_eq!(error, "Library index is off");
    }

    #[test]
    fn persisted_reasons_are_capped() {
        let store = LibraryIndexStore::open(scratch("reason-cap").join("index.sqlite")).unwrap();
        let item = item("a", "A");
        store.index_failed(&item, &"é".repeat(700)).unwrap();
        let status = store
            .status(
                false,
                None,
                LibrarySemanticState::Ready,
                None,
                LibraryTranscriptionState::Ready,
                None,
            )
            .unwrap();
        assert_eq!(status.errors.len(), 1);
        assert!(status.errors[0].message.ends_with("..."));
        assert!(status.errors[0].message.chars().count() <= 500);
    }

    #[test]
    fn per_track_commit_keeps_finished_tracks_when_later_track_fails() {
        let store = LibraryIndexStore::open(scratch("per-track").join("index.sqlite")).unwrap();
        let mut item = item("audio", "Audio folder");
        item.kind = LibraryItemKind::Audio;
        item.files = vec![
            LibraryFileInput {
                track_id: Some("track-ok".into()),
                relative_path: "one.mp3".into(),
                absolute_path: PathBuf::from("one.mp3"),
                size: 10,
                mtime_ms: 1,
                available: true,
            },
            LibraryFileInput {
                track_id: Some("track-bad".into()),
                relative_path: "two.mp3".into(),
                absolute_path: PathBuf::from("two.mp3"),
                size: 10,
                mtime_ms: 1,
                available: true,
            },
        ];
        let chunk = ChunkRecord {
            book_id: "audio".into(),
            title: "Audio folder".into(),
            kind: LibraryItemKind::Audio,
            ordinal: 0,
            text: "finished track transcript".into(),
            track_id: Some("track-ok".into()),
            start_ms: Some(0),
            end_ms: Some(10_000),
            chapter: None,
            quote: None,
        };
        store
            .index_track_chunks(&item, &item.files[0], &[chunk], None)
            .unwrap();
        store.index_failed(&item, "track-bad failed").unwrap();
        let hits = store.search("finished transcript", 10, None).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].track_id.as_deref(), Some("track-ok"));
    }

    #[test]
    fn due_files_only_returns_new_track_in_existing_audio_book() {
        let store = LibraryIndexStore::open(scratch("due-new-track").join("index.sqlite")).unwrap();
        let mut item = item("audio", "Audio folder");
        item.kind = LibraryItemKind::Audio;
        item.files = vec![
            LibraryFileInput {
                track_id: Some("one".into()),
                relative_path: "one.mp3".into(),
                absolute_path: PathBuf::from("one.mp3"),
                size: 10,
                mtime_ms: 1,
                available: true,
            },
            LibraryFileInput {
                track_id: Some("two".into()),
                relative_path: "two.mp3".into(),
                absolute_path: PathBuf::from("two.mp3"),
                size: 10,
                mtime_ms: 1,
                available: true,
            },
        ];
        for file in item.files.clone() {
            let chunk = ChunkRecord {
                book_id: "audio".into(),
                title: "Audio folder".into(),
                kind: LibraryItemKind::Audio,
                ordinal: 0,
                text: format!("{} transcript", file.relative_path),
                track_id: file.track_id.clone(),
                start_ms: Some(0),
                end_ms: Some(1000),
                chapter: None,
                quote: None,
            };
            store
                .index_track_chunks(&item, &file, &[chunk], None)
                .unwrap();
        }
        item.files.push(LibraryFileInput {
            track_id: Some("three".into()),
            relative_path: "three.mp3".into(),
            absolute_path: PathBuf::from("three.mp3"),
            size: 10,
            mtime_ms: 1,
            available: true,
        });
        let due = store.due_files(&item, false, 0).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].relative_path, "three.mp3");
    }

    #[test]
    fn failed_file_waits_for_retry_backoff() {
        let store = LibraryIndexStore::open(scratch("backoff").join("index.sqlite")).unwrap();
        let mut item = item("audio", "Audio folder");
        item.kind = LibraryItemKind::Audio;
        item.files[0].track_id = Some("bad".into());
        item.files[0].relative_path = "bad.mp3".into();
        store
            .mark_file_failed(&item, &item.files[0], "corrupt", 1_000)
            .unwrap();
        assert!(store.due_files(&item, false, 1_001).unwrap().is_empty());
        assert_eq!(
            store
                .due_files(&item, false, 1_000 + 10 * 60 * 1000)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn hybrid_search_fuses_keyword_and_vectors() {
        let store = LibraryIndexStore::open(scratch("hybrid").join("index.sqlite")).unwrap();
        let item = item("a", "Repairs");
        let chunks = vec![
            ChunkRecord {
                book_id: "a".into(),
                title: "Repairs".into(),
                kind: LibraryItemKind::Epub,
                ordinal: 0,
                text: "replace fuel filter on ford escape".into(),
                track_id: None,
                start_ms: None,
                end_ms: None,
                chapter: Some("DIY".into()),
                quote: Some("replace fuel".into()),
            },
            ChunkRecord {
                ordinal: 1,
                text: "unrelated cooking".into(),
                ..ChunkRecord {
                    book_id: "a".into(),
                    title: "Repairs".into(),
                    kind: LibraryItemKind::Epub,
                    ordinal: 1,
                    text: String::new(),
                    track_id: None,
                    start_ms: None,
                    end_ms: None,
                    chapter: None,
                    quote: None,
                }
            },
        ];
        store
            .index_chunks(
                &item,
                &chunks,
                Some(("s", &[vec![1.0, 0.0], vec![0.0, 1.0]])),
                None,
            )
            .unwrap();
        let hits = store
            .search("fuel filter", 10, Some(("s", &[1.0, 0.0])))
            .unwrap();
        assert_eq!(hits[0].snippet.contains("fuel"), true);
        assert_eq!(hits[0].match_kind, LibraryMatchKind::Both);
    }

    #[test]
    fn settings_persist() {
        let path = scratch("settings").join("index.sqlite");
        let store = LibraryIndexStore::open(&path).unwrap();
        store
            .set_settings(&LibraryIndexSettings {
                enabled: false,
                transcribe_media: false,
            })
            .unwrap();
        let reopened = LibraryIndexStore::open(&path).unwrap();
        assert!(!reopened.settings().unwrap().enabled);
        assert!(!reopened.settings().unwrap().transcribe_media);
    }

    #[test]
    fn network_items_are_title_only_without_fetching() {
        let store = LibraryIndexStore::open(scratch("privacy").join("index.sqlite")).unwrap();
        let mut link = item("y", "A video");
        link.kind = LibraryItemKind::Youtube;
        link.source_url = Some("https://www.youtube.com/watch?v=abc".into());
        link.files.clear();
        store
            .index_title_only(&link, "Network Library items are not fetched")
            .unwrap();
        let hits = store.search("video", 10, None).unwrap();
        assert_eq!(hits[0].match_kind, LibraryMatchKind::Keyword);
        assert_eq!(hits[0].kind, LibraryItemKind::Youtube);
    }

    #[test]
    fn snippet_handles_unicode_case_expansion_without_panic() {
        let store =
            LibraryIndexStore::open(scratch("unicode-snippet").join("index.sqlite")).unwrap();
        let item = item("city", "City");
        let chunk = ChunkRecord {
            book_id: "city".into(),
            title: "City".into(),
            kind: LibraryItemKind::Epub,
            ordinal: 0,
            text: "İstanbul'da şehir hakkında kısa bir not".into(),
            track_id: None,
            start_ms: None,
            end_ms: None,
            chapter: None,
            quote: None,
        };
        store.index_chunks(&item, &[chunk], None, None).unwrap();
        let hits = store.search("istanbul şehir", 10, None).unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].snippet.contains("İstanbul"));
    }
}
