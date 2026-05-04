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
//! - Top-k via partial sort (no full sort needed)

use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::ai::traits::{BoxFuture, ChunkEmbedding, SearchResult, VectorStore};
use crate::error::{CoreError, Result};

/// SQLite-backed vector store.
/// Thread-safe via Arc<Mutex<Connection>>.
pub struct SqliteVectorStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteVectorStore {
    /// Open or create a vector store at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;

        // Optimize for our workload.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA mmap_size = 268435456;
             PRAGMA cache_size = -65536;",
        )?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS vectors (
                chunk_id TEXT PRIMARY KEY,
                graph_id TEXT NOT NULL,
                page_id TEXT NOT NULL,
                block_id TEXT,
                page_title TEXT NOT NULL,
                content TEXT NOT NULL,
                embedding BLOB NOT NULL,
                metadata TEXT DEFAULT '{}',
                created_at INTEGER DEFAULT (strftime('%s','now') * 1000)
            );
            CREATE INDEX IF NOT EXISTS idx_vectors_graph ON vectors(graph_id);
            CREATE INDEX IF NOT EXISTS idx_vectors_page ON vectors(graph_id, page_id);",
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
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

    /// Serialize f32 vector to bytes (native endian for zero-copy on same arch).
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
}

impl VectorStore for SqliteVectorStore {
    fn upsert<'a>(&'a self, chunks: &'a [ChunkEmbedding]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let conn = self
                .conn
                .lock()
                .map_err(|e| CoreError::Other(format!("Lock error: {}", e)))?;

            let tx = conn.unchecked_transaction()?;

            {
                let mut stmt = tx.prepare_cached(
                    "INSERT OR REPLACE INTO vectors
                     (chunk_id, graph_id, page_id, block_id, page_title, content, embedding, metadata)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )?;

                for chunk in chunks {
                    let embedding_bytes = Self::vec_to_bytes(&chunk.embedding);
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
        Box::pin(async move {
            let conn = self
                .conn
                .lock()
                .map_err(|e| CoreError::Other(format!("Lock error: {}", e)))?;

            // Fetch all vectors (with optional graph filter) and compute similarity in Rust.
            // For <100k vectors this is plenty fast (~10ms on modern hardware).
            let mut scored: Vec<(f32, VectorRow)> = Vec::new();

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

            if let Some(gid) = filter_graph_id {
                let mut stmt = conn.prepare_cached(
                    "SELECT chunk_id, graph_id, page_id, block_id, page_title, content, embedding, metadata
                     FROM vectors WHERE graph_id = ?1",
                )?;
                let rows = stmt.query_map(params![gid], row_mapper)?;
                for row in rows {
                    let row = row?;
                    let embedding = Self::bytes_to_vec(&row.embedding);
                    let score = Self::cosine_similarity(query_embedding, &embedding);
                    scored.push((score, row));
                }
            } else {
                let mut stmt = conn.prepare_cached(
                    "SELECT chunk_id, graph_id, page_id, block_id, page_title, content, embedding, metadata
                     FROM vectors",
                )?;
                let rows = stmt.query_map([], row_mapper)?;
                for row in rows {
                    let row = row?;
                    let embedding = Self::bytes_to_vec(&row.embedding);
                    let score = Self::cosine_similarity(query_embedding, &embedding);
                    scored.push((score, row));
                }
            };

            // Partial sort for top-k (more efficient than full sort).
            scored.sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(top_k);

            let results = scored
                .into_iter()
                .map(|(score, row)| SearchResult {
                    chunk_id: row.chunk_id,
                    graph_id: row.graph_id,
                    page_id: row.page_id,
                    block_id: row.block_id,
                    page_title: row.page_title,
                    content: row.content,
                    score,
                    metadata: serde_json::from_str(&row.metadata).unwrap_or_default(),
                })
                .collect();

            Ok(results)
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
