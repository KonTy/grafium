//! SQLite-based vector store using manual cosine similarity.
//!
//! Why not LanceDB right now:
//! - LanceDB's Rust API is still maturing and has heavy Arrow dependencies
//! - For <100k vectors, SQLite with brute-force cosine similarity is fast enough
//! - We abstract behind the VectorStore trait, so swapping to LanceDB later is trivial
//!
//! Performance strategy:
//! - Vectors stored as BLOB (f32 array, native endian)
//! - Search uses batch cosine similarity in Rust (SIMD-friendly)
//! - Metadata indexed for fast filtering
//! - Search keeps only the best top-k matches in memory (no full sort needed)

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::ai::traits::{BoxFuture, ChunkEmbedding, SearchResult, VectorStore};
use crate::error::{CoreError, Result};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

/// SQLite-backed vector store.
/// Thread-safe via Arc<Mutex<Connection>>.
pub struct SqliteVectorStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteVectorStore {
    const EMBEDDING_DIMENSION_KEY: &'static str = "embedding_dimension";

    /// Open or create a vector store at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;

        // Optimize for our workload. auto_vacuum only takes effect on a new,
        // empty file; existing stores are converted by `compact`. The journal
        // limit stops a burst of indexing leaving a WAL as large as the store.
        conn.execute_batch(
            "PRAGMA auto_vacuum = INCREMENTAL;
             PRAGMA journal_size_limit = 33554432;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA mmap_size = 268435456;
             PRAGMA cache_size = -65536;",
        )?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS vectors (
                chunk_id TEXT NOT NULL,
                graph_id TEXT NOT NULL,
                page_id TEXT NOT NULL,
                block_id TEXT,
                page_title TEXT NOT NULL,
                content TEXT NOT NULL,
                embedding BLOB NOT NULL,
                metadata TEXT DEFAULT '{}',
                created_at INTEGER DEFAULT (strftime('%s','now') * 1000),
                PRIMARY KEY (graph_id, chunk_id)
            );
            CREATE TABLE IF NOT EXISTS vector_store_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_vectors_graph ON vectors(graph_id);
            CREATE INDEX IF NOT EXISTS idx_vectors_page ON vectors(graph_id, page_id);",
        )?;

        let graph_in_primary_key: bool = conn
            .prepare("PRAGMA table_info(vectors)")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .any(|(name, position)| name == "graph_id" && *position > 0);
        if !graph_in_primary_key {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(
                "CREATE TABLE vectors_scoped (
                    chunk_id TEXT NOT NULL, graph_id TEXT NOT NULL, page_id TEXT NOT NULL,
                    block_id TEXT, page_title TEXT NOT NULL, content TEXT NOT NULL,
                    embedding BLOB NOT NULL, metadata TEXT DEFAULT '{}',
                    created_at INTEGER DEFAULT (strftime('%s','now') * 1000),
                    PRIMARY KEY (graph_id, chunk_id)
                 );
                 INSERT INTO vectors_scoped SELECT chunk_id, graph_id, page_id, block_id,
                    page_title, content, embedding, metadata, created_at FROM vectors;
                 DROP TABLE vectors;
                 ALTER TABLE vectors_scoped RENAME TO vectors;
                 CREATE INDEX idx_vectors_graph ON vectors(graph_id);
                 CREATE INDEX idx_vectors_page ON vectors(graph_id, page_id);",
            )?;
            tx.commit()?;
        }

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Free pages at or above this share of the file, and at least this many
    /// bytes, justify a one-off full VACUUM to convert a store created before
    /// incremental auto-vacuum.
    const VACUUM_FREE_FRACTION: i64 = 4;
    const VACUUM_MIN_FREE_BYTES: i64 = 4 * 1024 * 1024;

    fn compact_conn(conn: &Connection) -> Result<()> {
        Self::quantize_legacy_rows(conn)?;
        let pragma = |name: &str| -> Result<i64> {
            Ok(conn.query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))?)
        };
        let free = pragma("freelist_count")?;
        if free > 0 {
            if pragma("auto_vacuum")? == 2 {
                // Each step frees one page, so drain every row.
                let mut statement = conn.prepare("PRAGMA incremental_vacuum")?;
                let mut rows = statement.query([])?;
                while rows.next()?.is_some() {}
            } else if free * Self::VACUUM_FREE_FRACTION >= pragma("page_count")?
                && free * pragma("page_size")? >= Self::VACUUM_MIN_FREE_BYTES
            {
                conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;")?;
            }
        }
        // Readers can keep a checkpoint from finishing; the next pass retries.
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    /// Open an in-memory vector store (for testing).
    pub fn in_memory() -> Result<Self> {
        Self::open(Path::new(":memory:"))
    }

    /// Compute cosine similarity between two vectors.
    #[inline]
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        debug_assert_eq!(a.len(), b.len());

        let mut dot = 0.0f32;
        let mut norm_a = 0.0f32;
        let mut norm_b = 0.0f32;

        // Process in chunks of 4 for auto-vectorization.
        let chunks = a.len() / 4;
        for i in 0..chunks {
            let base = i * 4;
            dot += a[base] * b[base]
                + a[base + 1] * b[base + 1]
                + a[base + 2] * b[base + 2]
                + a[base + 3] * b[base + 3];
            norm_a += a[base] * a[base]
                + a[base + 1] * a[base + 1]
                + a[base + 2] * a[base + 2]
                + a[base + 3] * a[base + 3];
            norm_b += b[base] * b[base]
                + b[base + 1] * b[base + 1]
                + b[base + 2] * b[base + 2]
                + b[base + 3] * b[base + 3];
        }

        // Handle remainder.
        for i in (chunks * 4)..a.len() {
            dot += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }

        let denom = (norm_a.sqrt() * norm_b.sqrt()).max(1e-10);
        dot / denom
    }

    /// Store a vector as a little-endian f32 scale followed by one signed byte
    /// per dimension: a quarter of the f32 size. Cosine similarity ignores the
    /// scale, and symmetric 8-bit rounding moves scores by well under 0.01, far
    /// below the gap between relevant and unrelated passages.
    fn quantize(v: &[f32]) -> Vec<u8> {
        let peak = v.iter().fold(0.0f32, |peak, x| peak.max(x.abs()));
        let scale = if peak.is_finite() && peak > 0.0 {
            peak / 127.0
        } else {
            0.0
        };
        let mut bytes = Vec::with_capacity(v.len() + 4);
        bytes.extend_from_slice(&scale.to_le_bytes());
        for &x in v {
            let q = if scale > 0.0 {
                (x / scale).round().clamp(-127.0, 127.0)
            } else {
                0.0
            };
            bytes.push(q as i8 as u8);
        }
        bytes
    }

    /// Decode either format. Stores written before quantization hold four
    /// bytes per dimension; `compact` converts them in the background.
    fn decode(bytes: &[u8], dimension: Option<usize>) -> Vec<f32> {
        match dimension {
            Some(d) if bytes.len() == d + 4 && bytes.len() != d * 4 => {
                let scale = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                bytes[4..].iter().map(|&q| q as i8 as f32 * scale).collect()
            }
            _ => Self::bytes_to_vec(bytes),
        }
    }

    const QUANTIZE_BATCH: usize = 2000;

    /// Rewrite full-precision rows in the compact format. Returns rows converted.
    fn quantize_legacy_rows(conn: &Connection) -> Result<usize> {
        let Some(dimension) = Self::stored_dimension(conn)? else {
            return Ok(0);
        };
        if dimension * 4 == dimension + 4 {
            return Ok(0);
        }
        let legacy_len = (dimension * 4) as i64;
        let mut converted = 0;
        loop {
            let tx = conn.unchecked_transaction()?;
            let rows = {
                let mut select = tx.prepare(
                    "SELECT rowid, embedding FROM vectors WHERE length(embedding) = ?1 LIMIT ?2",
                )?;
                let rows = select
                    .query_map(params![legacy_len, Self::QUANTIZE_BATCH as i64], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut update =
                    tx.prepare("UPDATE vectors SET embedding = ?1 WHERE rowid = ?2")?;
                for (rowid, blob) in &rows {
                    update.execute(params![Self::quantize(&Self::bytes_to_vec(blob)), rowid])?;
                }
                rows.len()
            };
            tx.commit()?;
            converted += rows;
            if rows < Self::QUANTIZE_BATCH {
                return Ok(converted);
            }
        }
    }

    /// Serialize f32 vector to bytes (the pre-quantization format).
    #[cfg(test)]
    fn vec_to_bytes(v: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(v.len() * 4);
        for &f in v {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        bytes
    }

    /// Deserialize bytes to f32 vector.
    fn bytes_to_vec(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }

    fn stored_dimension(conn: &Connection) -> Result<Option<usize>> {
        let dimension = conn
            .query_row(
                "SELECT value FROM vector_store_meta WHERE key = ?1",
                params![Self::EMBEDDING_DIMENSION_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        if let Some(dimension) = dimension {
            return dimension.parse::<usize>().map(Some).map_err(|error| {
                CoreError::Parse(format!(
                    "Stored embedding dimension is invalid ({}): {}",
                    dimension, error
                ))
            });
        }

        let legacy_blob_len = conn
            .query_row("SELECT length(embedding) FROM vectors LIMIT 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?;

        if let Some(blob_len) = legacy_blob_len {
            if blob_len % 4 != 0 {
                return Err(CoreError::Parse(format!(
                    "Stored embedding blob has invalid byte length {}",
                    blob_len
                )));
            }
            let dimension = (blob_len as usize) / 4;
            Self::persist_dimension(conn, dimension)?;
            Ok(Some(dimension))
        } else {
            Ok(None)
        }
    }

    fn persist_dimension(conn: &Connection, dimension: usize) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO vector_store_meta (key, value) VALUES (?1, ?2)",
            params![Self::EMBEDDING_DIMENSION_KEY, dimension.to_string()],
        )?;
        Ok(())
    }

    fn ensure_store_dimension(conn: &Connection, actual_dimension: usize) -> Result<usize> {
        match Self::stored_dimension(conn)? {
            Some(expected_dimension) if expected_dimension != actual_dimension => {
                Err(CoreError::Other(format!(
                    "Embedding dimension mismatch: store expects {}, got {}",
                    expected_dimension, actual_dimension
                )))
            }
            Some(expected_dimension) => Ok(expected_dimension),
            None => {
                Self::persist_dimension(conn, actual_dimension)?;
                Ok(actual_dimension)
            }
        }
    }

    fn validate_upsert_dimensions(conn: &Connection, chunks: &[ChunkEmbedding]) -> Result<()> {
        let Some(first_chunk) = chunks.first() else {
            return Ok(());
        };

        let batch_dimension = first_chunk.embedding.len();
        for chunk in chunks {
            if chunk.embedding.len() != batch_dimension {
                return Err(CoreError::Other(format!(
                    "Embedding batch contains mixed dimensions: chunk {} has {}, expected {}",
                    chunk.chunk_id,
                    chunk.embedding.len(),
                    batch_dimension
                )));
            }
        }

        Self::ensure_store_dimension(conn, batch_dimension)?;
        Ok(())
    }

    fn validate_query_dimension(
        conn: &Connection,
        query_embedding: &[f32],
    ) -> Result<Option<usize>> {
        match Self::stored_dimension(conn)? {
            Some(expected_dimension) if expected_dimension != query_embedding.len() => {
                Err(CoreError::Other(format!(
                    "Embedding dimension mismatch: store expects {}, got {}",
                    expected_dimension,
                    query_embedding.len()
                )))
            }
            Some(expected_dimension) => Ok(Some(expected_dimension)),
            None => Ok(None),
        }
    }

    fn search_with_conn(
        conn: &Connection,
        query_embedding: &[f32],
        top_k: usize,
        filter_graph_id: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let expected_dimension = Self::validate_query_dimension(conn, query_embedding)?;
        if top_k == 0 {
            return Ok(Vec::new());
        }

        let row_mapper = |row: &rusqlite::Row| -> rusqlite::Result<VectorRow> {
            Ok(VectorRow {
                chunk_id: row.get(0)?,
                graph_id: row.get(1)?,
                page_id: row.get(2)?,
                block_id: row.get(3)?,
                page_title: row.get(4)?,
                content: row.get(5)?,
                embedding: row.get::<_, Vec<u8>>(6)?,
                metadata: row.get::<_, String>(7)?,
            })
        };

        let mut best = BinaryHeap::with_capacity(top_k);

        if let Some(gid) = filter_graph_id {
            let mut stmt = conn.prepare_cached(
                "SELECT chunk_id, graph_id, page_id, block_id, page_title, content, embedding, metadata
                 FROM vectors WHERE graph_id = ?1",
            )?;
            let rows = stmt.query_map(params![gid], row_mapper)?;
            for row in rows {
                let row = row?;
                Self::consider_search_row(
                    &mut best,
                    query_embedding,
                    expected_dimension,
                    row,
                    top_k,
                )?;
            }
        } else {
            let mut stmt = conn.prepare_cached(
                "SELECT chunk_id, graph_id, page_id, block_id, page_title, content, embedding, metadata
                 FROM vectors",
            )?;
            let rows = stmt.query_map([], row_mapper)?;
            for row in rows {
                let row = row?;
                Self::consider_search_row(
                    &mut best,
                    query_embedding,
                    expected_dimension,
                    row,
                    top_k,
                )?;
            }
        }

        let mut scored = best
            .into_iter()
            .map(|Reverse(scored_row)| scored_row)
            .collect::<Vec<_>>();
        scored.sort_unstable_by(|a, b| b.cmp(a));

        Ok(scored
            .into_iter()
            .map(|scored_row| SearchResult {
                chunk_id: scored_row.row.chunk_id,
                graph_id: scored_row.row.graph_id,
                page_id: scored_row.row.page_id,
                block_id: scored_row.row.block_id,
                page_title: scored_row.row.page_title,
                content: scored_row.row.content,
                score: scored_row.score,
                metadata: serde_json::from_str(&scored_row.row.metadata).unwrap_or_default(),
            })
            .collect())
    }

    fn consider_search_row(
        best: &mut BinaryHeap<Reverse<ScoredRow>>,
        query_embedding: &[f32],
        expected_dimension: Option<usize>,
        row: VectorRow,
        top_k: usize,
    ) -> Result<()> {
        let embedding = Self::decode(&row.embedding, expected_dimension);
        if let Some(expected_dimension) = expected_dimension {
            if embedding.len() != expected_dimension {
                return Err(CoreError::Other(format!(
                    "Stored embedding dimension mismatch for chunk {}: expected {}, got {}",
                    row.chunk_id,
                    expected_dimension,
                    embedding.len()
                )));
            }
        }

        let candidate = ScoredRow {
            score: Self::cosine_similarity(query_embedding, &embedding),
            row,
        };

        if best.len() < top_k {
            best.push(Reverse(candidate));
            return Ok(());
        }

        let should_replace = best
            .peek()
            .map(|lowest| candidate.cmp(&lowest.0).is_gt())
            .unwrap_or(true);

        if should_replace {
            best.pop();
            best.push(Reverse(candidate));
        }

        Ok(())
    }
}

impl VectorStore for SqliteVectorStore {
    fn upsert<'a>(&'a self, chunks: &'a [ChunkEmbedding]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conn = self
                .conn
                .lock()
                .map_err(|e| CoreError::Other(format!("Lock error: {}", e)))?;

            Self::validate_upsert_dimensions(&conn, chunks)?;

            let tx = conn.unchecked_transaction()?;

            {
                let mut stmt = tx.prepare_cached(
                    "INSERT OR REPLACE INTO vectors
                     (chunk_id, graph_id, page_id, block_id, page_title, content, embedding, metadata)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )?;

                for chunk in chunks {
                    let embedding_bytes = Self::quantize(&chunk.embedding);
                    let metadata_str = serde_json::to_string(&chunk.metadata).unwrap_or_default();

                    stmt.execute(params![
                        chunk.chunk_id,
                        chunk.graph_id,
                        chunk.page_id,
                        chunk.block_id,
                        chunk.page_title,
                        chunk.content,
                        embedding_bytes,
                        metadata_str,
                    ])?;
                }
            }

            tx.commit()?;
            Ok(())
        })
    }

    fn search<'a>(
        &'a self,
        query_embedding: &'a [f32],
        top_k: usize,
        filter_graph_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<SearchResult>>> {
        let conn = self.conn.clone();
        let query_embedding = query_embedding.to_vec();
        let filter_graph_id = filter_graph_id.map(str::to_owned);

        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let conn = conn
                    .lock()
                    .map_err(|e| CoreError::Other(format!("Lock error: {}", e)))?;
                Self::search_with_conn(&conn, &query_embedding, top_k, filter_graph_id.as_deref())
            })
            .await
            .map_err(|e| CoreError::Other(format!("Vector search task panicked: {}", e)))?
        })
    }

    fn delete_by_page<'a>(
        &'a self,
        graph_id: &'a str,
        page_id: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conn = self
                .conn
                .lock()
                .map_err(|e| CoreError::Other(format!("Lock error: {}", e)))?;
            conn.execute(
                "DELETE FROM vectors WHERE graph_id = ?1 AND page_id = ?2",
                params![graph_id, page_id],
            )?;
            Ok(())
        })
    }

    fn delete_chunks<'a>(
        &'a self,
        graph_id: &'a str,
        chunk_ids: &'a [String],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if chunk_ids.is_empty() {
                return Ok(());
            }

            let conn = self
                .conn
                .lock()
                .map_err(|e| CoreError::Other(format!("Lock error: {}", e)))?;

            let placeholders = (0..chunk_ids.len())
                .map(|i| format!("?{}", i + 2))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "DELETE FROM vectors WHERE graph_id = ?1 AND chunk_id IN ({})",
                placeholders
            );
            let params = std::iter::once(graph_id.to_string()).chain(chunk_ids.iter().cloned());
            conn.execute(&sql, params_from_iter(params))?;
            Ok(())
        })
    }

    fn delete_by_graph<'a>(&'a self, graph_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conn = self
                .conn
                .lock()
                .map_err(|e| CoreError::Other(format!("Lock error: {}", e)))?;
            conn.execute("DELETE FROM vectors WHERE graph_id = ?1", params![graph_id])?;
            Ok(())
        })
    }

    fn count<'a>(&'a self) -> BoxFuture<'a, Result<usize>> {
        Box::pin(async move {
            let conn = self
                .conn
                .lock()
                .map_err(|e| CoreError::Other(format!("Lock error: {}", e)))?;
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))?;
            Ok(count as usize)
        })
    }

    fn count_for_graph<'a>(&'a self, graph_id: &'a str) -> BoxFuture<'a, Result<usize>> {
        Box::pin(async move {
            let conn = self
                .conn
                .lock()
                .map_err(|e| CoreError::Other(format!("Lock error: {}", e)))?;
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM vectors WHERE graph_id = ?1",
                [graph_id],
                |row| row.get(0),
            )?;
            Ok(count as usize)
        })
    }

    fn list_content_hashes<'a>(
        &'a self,
        graph_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<(String, String)>>> {
        Box::pin(async move {
            let conn = self
                .conn
                .lock()
                .map_err(|e| CoreError::Other(format!("Lock error: {}", e)))?;
            let mut stmt = conn.prepare(
                "SELECT chunk_id, json_extract(metadata, '$.content_hash') \
                 FROM vectors WHERE graph_id = ?1",
            )?;
            let rows = stmt.query_map([graph_id], |row| {
                let chunk_id: String = row.get(0)?;
                let hash: Option<String> = row.get(1)?;
                Ok((chunk_id, hash))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (chunk_id, hash) = row?;
                out.push((chunk_id, hash.unwrap_or_default()));
            }
            Ok(out)
        })
    }

    fn list_page_ids<'a>(&'a self, graph_id: &'a str) -> BoxFuture<'a, Result<Vec<String>>> {
        Box::pin(async move {
            let conn = self
                .conn
                .lock()
                .map_err(|e| CoreError::Other(format!("Lock error: {e}")))?;
            let mut statement =
                conn.prepare("SELECT DISTINCT page_id FROM vectors WHERE graph_id = ?1")?;
            let pages = statement
                .query_map([graph_id], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(pages)
        })
    }

    fn list_graph_ids<'a>(&'a self) -> BoxFuture<'a, Result<Vec<String>>> {
        Box::pin(async move {
            let conn = self
                .conn
                .lock()
                .map_err(|e| CoreError::Other(format!("Lock error: {e}")))?;
            let mut statement = conn.prepare("SELECT DISTINCT graph_id FROM vectors")?;
            let graphs = statement
                .query_map([], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(graphs)
        })
    }

    fn compact<'a>(&'a self) -> BoxFuture<'a, Result<()>> {
        let conn = Arc::clone(&self.conn);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let conn = conn
                    .lock()
                    .map_err(|e| CoreError::Other(format!("Lock error: {e}")))?;
                Self::compact_conn(&conn)
            })
            .await
            .map_err(|e| CoreError::Other(format!("Vector compaction task panicked: {e}")))?
        })
    }
}

/// Internal row representation.
struct VectorRow {
    chunk_id: String,
    graph_id: String,
    page_id: String,
    block_id: Option<String>,
    page_title: String,
    content: String,
    embedding: Vec<u8>,
    metadata: String,
}

struct ScoredRow {
    score: f32,
    row: VectorRow,
}

impl PartialEq for ScoredRow {
    fn eq(&self, other: &Self) -> bool {
        compare_scores(self.score, other.score) == Ordering::Equal
    }
}

impl Eq for ScoredRow {}

impl PartialOrd for ScoredRow {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoredRow {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_scores(self.score, other.score)
    }
}

fn compare_scores(left: f32, right: f32) -> Ordering {
    #[cfg(test)]
    SCORE_COMPARISON_COUNT.fetch_add(1, AtomicOrdering::Relaxed);

    left.total_cmp(&right)
}

#[cfg(test)]
static SCORE_COMPARISON_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reset_score_comparison_count() {
        SCORE_COMPARISON_COUNT.store(0, AtomicOrdering::Relaxed);
    }

    fn score_comparison_count() -> usize {
        SCORE_COMPARISON_COUNT.load(AtomicOrdering::Relaxed)
    }

    fn test_chunk(chunk_id: &str, dimension: usize) -> ChunkEmbedding {
        ChunkEmbedding {
            chunk_id: chunk_id.to_string(),
            graph_id: "graph-1".to_string(),
            page_id: "page-1".to_string(),
            block_id: Some("block-1".to_string()),
            page_title: "Page".to_string(),
            content: "chunk content".to_string(),
            embedding: vec![0.5; dimension],
            metadata: json!({}),
        }
    }

    fn file_bytes(path: &Path) -> u64 {
        let wal = path.with_extension("db-wal");
        std::fs::metadata(path).unwrap().len()
            + std::fs::metadata(wal).map(|m| m.len()).unwrap_or(0)
    }

    fn fill(graph_id: &str, count: usize) -> Vec<ChunkEmbedding> {
        (0..count)
            .map(|i| vector_chunk(&format!("{graph_id}-{i}"), graph_id, vec![0.25; 1024]))
            .collect()
    }

    fn legacy_store(path: &Path) {
        // Stores created before incremental auto-vacuum have mode 0.
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA auto_vacuum = NONE; PRAGMA journal_mode = WAL;")
            .unwrap();
    }

    #[tokio::test]
    async fn compaction_returns_deleted_space_and_converts_legacy_stores() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("vectors.db");
        legacy_store(&path);
        let store = SqliteVectorStore::open(&path)?;
        store.upsert(&fill("kept", 200)).await?;
        store.upsert(&fill("gone", 8000)).await?;
        store.compact().await?;
        let full = file_bytes(&path);

        store.delete_by_graph("gone").await?;
        store.compact().await?;
        let compacted = file_bytes(&path);
        assert!(compacted * 4 < full, "{compacted} bytes left of {full}");
        assert_eq!(store.count().await?, 200);
        let mode: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
        assert_eq!(mode, 2, "legacy store should switch to incremental vacuum");

        store.upsert(&fill("again", 2000)).await?;
        store.delete_by_graph("again").await?;
        store.compact().await?;
        assert!(file_bytes(&path) <= compacted + 64 * 1024);
        Ok(())
    }

    #[tokio::test]
    async fn quantized_vectors_are_a_quarter_size_and_rank_like_full_precision() -> Result<()> {
        let dimension = 1024;
        let seeded = |seed: u32| -> Vec<f32> {
            let mut state = seed.wrapping_mul(2_654_435_761).max(1);
            (0..dimension)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    (state as f32 / u32::MAX as f32) - 0.5
                })
                .collect()
        };
        let query = seeded(7);
        let store = SqliteVectorStore::in_memory()?;
        let chunks: Vec<_> = (0..200)
            .map(|i| vector_chunk(&format!("c{i}"), "g", seeded(100 + i)))
            .collect();
        store.upsert(&chunks).await?;

        let mut exact: Vec<(f32, &str)> = chunks
            .iter()
            .map(|c| {
                (
                    SqliteVectorStore::cosine_similarity(&query, &c.embedding),
                    c.chunk_id.as_str(),
                )
            })
            .collect();
        exact.sort_by(|a, b| b.0.total_cmp(&a.0));
        let results = store.search(&query, 10, Some("g")).await?;
        let overlap = results
            .iter()
            .filter(|r| exact[..10].iter().any(|(_, id)| *id == r.chunk_id))
            .count();
        assert!(overlap >= 9, "top-10 overlap {overlap}");
        for result in &results {
            let (score, _) = exact.iter().find(|(_, id)| *id == result.chunk_id).unwrap();
            assert!((result.score - score).abs() < 5e-3);
        }

        let blob_len: i64 = store.conn.lock().unwrap().query_row(
            "SELECT length(embedding) FROM vectors LIMIT 1",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(blob_len as usize, dimension + 4);
        Ok(())
    }

    #[tokio::test]
    async fn compaction_converts_full_precision_rows_and_search_reads_both() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("vectors.db");
        let store = SqliteVectorStore::open(&path)?;
        store
            .upsert(&[vector_chunk("new", "g", vec![0.6, 0.8, 0.0])])
            .await?;
        store.conn.lock().unwrap().execute(
            "INSERT INTO vectors (chunk_id, graph_id, page_id, page_title, content, embedding)
             VALUES ('old', 'g', 'p', 'Old', 'legacy', ?1)",
            [SqliteVectorStore::vec_to_bytes(&[1.0, 0.0, 0.0])],
        )?;

        let before = store.search(&[1.0, 0.0, 0.0], 2, Some("g")).await?;
        assert_eq!(before[0].chunk_id, "old");
        assert!((before[0].score - 1.0).abs() < 1e-6);

        store.compact().await?;
        let lengths: Vec<i64> = {
            let conn = store.conn.lock().unwrap();
            let mut statement = conn.prepare("SELECT length(embedding) FROM vectors")?;
            let lengths = statement
                .query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?;
            lengths
        };
        assert_eq!(lengths, [7, 7]);
        let after = store.search(&[1.0, 0.0, 0.0], 2, Some("g")).await?;
        assert_eq!(after[0].chunk_id, "old");
        assert!((after[0].score - 1.0).abs() < 5e-3);
        assert!((after[1].score - 0.6).abs() < 5e-3);
        Ok(())
    }

    #[tokio::test]
    async fn lists_every_graph_with_vectors() -> Result<()> {
        let store = SqliteVectorStore::in_memory()?;
        store.upsert(&fill("a", 2)).await?;
        store.upsert(&fill("b", 1)).await?;
        let mut graphs = store.list_graph_ids().await?;
        graphs.sort();
        assert_eq!(graphs, ["a", "b"]);
        Ok(())
    }

    fn vector_chunk(chunk_id: &str, graph_id: &str, embedding: Vec<f32>) -> ChunkEmbedding {
        ChunkEmbedding {
            chunk_id: chunk_id.to_string(),
            graph_id: graph_id.to_string(),
            page_id: format!("page-{chunk_id}"),
            block_id: Some(format!("block-{chunk_id}")),
            page_title: format!("Page {chunk_id}"),
            content: format!("content {chunk_id}"),
            embedding,
            metadata: json!({ "chunk": chunk_id }),
        }
    }

    #[tokio::test]
    async fn list_content_hashes_recovers_stored_hashes_per_graph() -> Result<()> {
        let store = SqliteVectorStore::in_memory()?;
        let mut a = test_chunk("chunk-a", 3);
        a.metadata = json!({ "content_hash": "hash-a" });
        let mut b = test_chunk("chunk-b", 3);
        b.metadata = json!({ "content_hash": "hash-b" });
        // A chunk in another graph must not leak into the result.
        let mut other = test_chunk("chunk-c", 3);
        other.graph_id = "graph-2".to_string();
        other.metadata = json!({ "content_hash": "hash-c" });
        store.upsert(&[a, b, other]).await?;

        let mut hashes = store.list_content_hashes("graph-1").await?;
        hashes.sort();
        assert_eq!(
            hashes,
            vec![
                ("chunk-a".to_string(), "hash-a".to_string()),
                ("chunk-b".to_string(), "hash-b".to_string()),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn legacy_primary_key_migration_preserves_vectors_and_allows_graph_scoping() -> Result<()>
    {
        let root = tempfile::tempdir()?;
        let path = root.path().join("vectors.db");
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE vectors (
                chunk_id TEXT PRIMARY KEY, graph_id TEXT NOT NULL, page_id TEXT NOT NULL,
                block_id TEXT, page_title TEXT NOT NULL, content TEXT NOT NULL,
                embedding BLOB NOT NULL, metadata TEXT DEFAULT '{}', created_at INTEGER
            );",
        )?;
        conn.execute(
            "INSERT INTO vectors VALUES('shared','graph-1','page-1','block-1','Title','Retain this text',?1,'{}',42)",
            [SqliteVectorStore::vec_to_bytes(&[0.5, 0.5, 0.5])],
        )?;
        drop(conn);
        let store = SqliteVectorStore::open(&path)?;
        let mut second = test_chunk("shared", 3);
        second.graph_id = "graph-2".into();
        store.upsert(&[second]).await?;
        assert_eq!(store.count().await?, 2);
        store.delete_by_page("graph-2", "page-1").await?;
        let original = store.search(&[0.5, 0.5, 0.5], 5, Some("graph-1")).await?;
        assert_eq!(original.len(), 1);
        assert_eq!(original[0].content, "Retain this text");
        drop(store);
        assert_eq!(SqliteVectorStore::open(&path)?.count().await?, 1);
        Ok(())
    }

    #[tokio::test]
    async fn rejects_dimension_mismatches_on_upsert_and_search() -> Result<()> {
        let store = SqliteVectorStore::in_memory()?;
        store.upsert(&[test_chunk("chunk-1", 3)]).await?;

        let upsert_error = match store.upsert(&[test_chunk("chunk-2", 2)]).await {
            Ok(_) => panic!("mismatched upsert should fail"),
            Err(error) => error,
        };
        assert!(upsert_error
            .to_string()
            .contains("Embedding dimension mismatch: store expects 3, got 2"));

        let search_error = match store.search(&[0.25, 0.75], 5, None).await {
            Ok(_) => panic!("mismatched search should fail"),
            Err(error) => error,
        };
        assert!(search_error
            .to_string()
            .contains("Embedding dimension mismatch: store expects 3, got 2"));

        Ok(())
    }

    #[tokio::test]
    async fn search_returns_top_k_in_descending_score_order() -> Result<()> {
        let store = SqliteVectorStore::in_memory()?;
        store
            .upsert(&[
                vector_chunk("exact", "graph-1", vec![1.0, 0.0]),
                vector_chunk("high", "graph-1", vec![0.8, 0.6]),
                vector_chunk("mid", "graph-1", vec![0.6, 0.8]),
                vector_chunk("other-graph", "graph-2", vec![0.99, 0.01]),
                vector_chunk("low", "graph-1", vec![0.0, 1.0]),
            ])
            .await?;

        let results = store.search(&[1.0, 0.0], 3, Some("graph-1")).await?;

        assert_eq!(results.len(), 3);
        assert_eq!(
            results
                .iter()
                .map(|result| result.chunk_id.as_str())
                .collect::<Vec<_>>(),
            vec!["exact", "high", "mid"]
        );
        assert!(results[0].score > results[1].score);
        assert!(results[1].score > results[2].score);
        // Stored vectors are 8-bit quantized.
        assert!((results[0].score - 1.0).abs() < 5e-3);
        assert!((results[1].score - 0.8).abs() < 5e-3);
        assert!((results[2].score - 0.6).abs() < 5e-3);

        Ok(())
    }

    #[tokio::test]
    async fn search_keeps_top_k_without_full_result_sort() -> Result<()> {
        const ITEM_COUNT: usize = 10_000;
        const TOP_K: usize = 5;

        let store = SqliteVectorStore::in_memory()?;
        let chunks = (0..ITEM_COUNT)
            .map(|i| {
                vector_chunk(
                    &format!("chunk-{i:05}"),
                    "graph-1",
                    vec![1.0, i as f32 + 1.0],
                )
            })
            .collect::<Vec<_>>();
        store.upsert(&chunks).await?;

        reset_score_comparison_count();
        let results = store.search(&[1.0, 0.0], TOP_K, Some("graph-1")).await?;
        let comparisons = score_comparison_count();

        assert_eq!(results.len(), TOP_K);
        assert_eq!(
            results
                .iter()
                .map(|result| result.chunk_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "chunk-00000",
                "chunk-00001",
                "chunk-00002",
                "chunk-00003",
                "chunk-00004",
            ]
        );
        assert!(
            comparisons < 50_000,
            "expected bounded top-k selection, got {comparisons} score comparisons"
        );

        Ok(())
    }
}
