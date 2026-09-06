mod audio;
mod blocks;
mod collections;
mod favorites;
mod flashcards;
mod graph_support;
mod ink;
mod links;
mod pages;
mod properties;
mod raw_query;
mod retrieval;
mod schema;
mod tasks;

use crate::error::Result;
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;

pub(crate) use blocks::chat_salient_terms;
pub use retrieval::BlockPageMeta;

struct FunctionCustomizer;

impl std::fmt::Debug for FunctionCustomizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FunctionCustomizer")
    }
}

impl r2d2::CustomizeConnection<rusqlite::Connection, rusqlite::Error> for FunctionCustomizer {
    fn on_acquire(
        &self,
        conn: &mut rusqlite::Connection,
    ) -> std::result::Result<(), rusqlite::Error> {
        use chrono::Utc;

        conn.create_scalar_function(
            "days_ago",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let days: i64 = ctx.get(0)?;
                let ms = Utc::now().timestamp_millis() - (days * 86_400_000);
                Ok(ms)
            },
        )?;

        conn.create_scalar_function(
            "hours_ago",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let hours: i64 = ctx.get(0)?;
                let ms = Utc::now().timestamp_millis() - (hours * 3_600_000);
                Ok(ms)
            },
        )?;

        conn.create_scalar_function(
            "now_ms",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |_ctx| Ok(Utc::now().timestamp_millis()),
        )?;

        Ok(())
    }
}

pub struct Database {
    pool: Pool<SqliteConnectionManager>,
}

impl Database {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let manager = SqliteConnectionManager::file(path.as_ref());
        let pool = Pool::builder()
            .max_size(8)
            .connection_customizer(Box::new(FunctionCustomizer))
            .build(manager)?;

        let db = Self { pool };
        db.initialize()?;
        Ok(db)
    }

    pub fn in_memory() -> Result<Self> {
        let manager = SqliteConnectionManager::memory();
        let pool = Pool::builder()
            .max_size(1)
            .connection_customizer(Box::new(FunctionCustomizer))
            .build(manager)?;

        let db = Self { pool };
        db.initialize()?;
        Ok(db)
    }

    pub(crate) fn conn(&self) -> Result<PooledConnection<SqliteConnectionManager>> {
        Ok(self.pool.get()?)
    }

    fn initialize(&self) -> Result<()> {
        let conn = self.conn()?;
        // Performance pragmas for millions of records
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA cache_size = -64000;
            PRAGMA foreign_keys = ON;
            PRAGMA temp_store = MEMORY;
            PRAGMA mmap_size = 268435456;
            PRAGMA page_size = 4096;
        ",
        )?;
        schema::create_tables(&conn)?;

        // Migration: recreate task_events without CASCADE to preserve history across reindexes
        let has_fk: bool = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='task_events'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|sql| sql.contains("ON DELETE CASCADE"))
            .unwrap_or(false);

        if has_fk {
            conn.execute_batch("
                CREATE TABLE IF NOT EXISTS task_events_new (
                    id TEXT PRIMARY KEY,
                    block_id TEXT NOT NULL,
                    from_state TEXT,
                    to_state TEXT NOT NULL,
                    timestamp INTEGER NOT NULL
                );
                INSERT OR IGNORE INTO task_events_new SELECT * FROM task_events;
                DROP TABLE task_events;
                ALTER TABLE task_events_new RENAME TO task_events;
                CREATE INDEX IF NOT EXISTS idx_task_events_block ON task_events(block_id, timestamp);
                CREATE INDEX IF NOT EXISTS idx_task_events_ts ON task_events(timestamp DESC);
            ")?;
        }

        // Widen `tasks` for graphs created before it carried times, repeats,
        // priority and a completion timestamp. Adding a nullable column is
        // cheap and rewrites nothing, so this just runs every open.
        for (column, decl) in [
            ("scheduled_time", "TEXT"),
            ("deadline_time", "TEXT"),
            ("repeat_rule", "TEXT"),
            ("priority", "TEXT"),
            ("closed_at", "INTEGER"),
        ] {
            let exists = conn
                .prepare("SELECT 1 FROM pragma_table_info('tasks') WHERE name = ?1")
                .and_then(|mut stmt| stmt.exists([column]))
                .unwrap_or(true);
            if !exists {
                // A failure here must not stop the graph opening; the column is
                // additive and the next launch tries again.
                let _ = conn.execute(&format!("ALTER TABLE tasks ADD COLUMN {column} {decl}"), []);
            }
        }
        let _ = conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_tasks_closed ON tasks(closed_at) WHERE closed_at IS NOT NULL;
             CREATE INDEX IF NOT EXISTS idx_tasks_priority ON tasks(priority) WHERE priority IS NOT NULL;",
        );

        // Backfill normalized properties if tables are empty but JSON blobs have data
        let prop_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM page_properties", [], |row| row.get(0))
            .unwrap_or(0);
        drop(conn);

        if prop_count == 0 {
            let _ = self.backfill_properties();
        }

        Ok(())
    }

    /// Populate `fts_block_rowid` for every existing FTS row that isn't mapped
    /// yet. Walks `fts_blocks` in small rowid-cursor chunks, each in its own
    /// short transaction with a brief pause between, so it never holds a long
    /// read snapshot (which balloons the WAL) or starves the UI. Idempotent and
    /// cheap to call when already populated.
    ///
    /// Intended to run once on a background thread the first time an older graph
    /// (indexed before the map existed) is opened, so that block edits stop
    /// full-scanning the FTS index.
    pub fn backfill_fts_rowid_map(&self) -> Result<usize> {
        {
            let conn = self.conn()?;
            let map_count: i64 =
                conn.query_row("SELECT count(*) FROM fts_block_rowid", [], |r| r.get(0))?;
            let fts_count: i64 =
                conn.query_row("SELECT count(*) FROM fts_blocks", [], |r| r.get(0))?;
            if map_count >= fts_count {
                return Ok(0);
            }
        }

        let batch: i64 = 20_000;
        let mut cursor: i64 = 0;
        let mut total = 0usize;

        loop {
            let mut conn = self.conn()?;

            // Read one chunk (rowid is FTS5's primary key, so this range scan is
            // fast even mid-table). Collect and drop the read statement before
            // opening the write transaction.
            let rows: Vec<(i64, String)> = {
                let mut stmt = conn.prepare(
                    "SELECT rowid, block_id FROM fts_blocks WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
                )?;
                let mapped = stmt.query_map(rusqlite::params![cursor, batch], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?;
                mapped.collect::<std::result::Result<Vec<_>, _>>()?
            };
            if rows.is_empty() {
                break;
            }
            cursor = rows.last().map(|(id, _)| *id).unwrap_or(cursor);

            let tx = conn.transaction()?;
            {
                let mut stmt = tx.prepare(
                    "INSERT OR IGNORE INTO fts_block_rowid (block_id, fts_rowid) VALUES (?1, ?2)",
                )?;
                for (rowid, block_id) in &rows {
                    stmt.execute(rusqlite::params![block_id, rowid])?;
                }
            }
            tx.commit()?;
            total += rows.len();
            drop(conn);

            // Yield so block edits and UI queries aren't starved, and so the WAL
            // can checkpoint between chunks during this one-time backfill.
            std::thread::sleep(std::time::Duration::from_millis(15));
        }

        Ok(total)
    }
}

/// Insert a block into the FTS index and record its rowid in `fts_block_rowid`.
pub(crate) fn fts_insert_block(
    conn: &rusqlite::Connection,
    block_id: &str,
    content: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO fts_blocks (block_id, content) VALUES (?1, ?2)",
        rusqlite::params![block_id, content],
    )?;
    let rowid = conn.last_insert_rowid();
    conn.execute(
        "INSERT OR REPLACE INTO fts_block_rowid (block_id, fts_rowid) VALUES (?1, ?2)",
        rusqlite::params![block_id, rowid],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Database;
    use crate::error::Result;
    use tempfile::tempdir;

    #[test]
    fn database_new_accepts_path_and_initializes() -> Result<()> {
        let temp = tempdir()?;
        let db_path = temp.path().join("index.db");
        let db = Database::new(db_path.as_path())?;

        assert_eq!(db.count_pages()?, 0);
        Ok(())
    }
}

/// Delete a block's FTS row by its mapped rowid (O(1)). Falls back to the slow
/// UNINDEXED scan only for legacy rows not yet in `fts_block_rowid`.
pub(crate) fn fts_delete_block(conn: &rusqlite::Connection, block_id: &str) -> Result<()> {
    use rusqlite::OptionalExtension;
    let rowid: Option<i64> = conn
        .query_row(
            "SELECT fts_rowid FROM fts_block_rowid WHERE block_id = ?1",
            rusqlite::params![block_id],
            |r| r.get(0),
        )
        .optional()?;

    if let Some(rowid) = rowid {
        conn.execute(
            "DELETE FROM fts_blocks WHERE rowid = ?1",
            rusqlite::params![rowid],
        )?;
        conn.execute(
            "DELETE FROM fts_block_rowid WHERE block_id = ?1",
            rusqlite::params![block_id],
        )?;
    } else {
        // Legacy row not mapped yet: slow path (full FTS scan). Only happens
        // before the one-time backfill maps this block.
        conn.execute(
            "DELETE FROM fts_blocks WHERE block_id = ?1",
            rusqlite::params![block_id],
        )?;
    }
    Ok(())
}

/// Replace a block's FTS content (delete old row by rowid, insert new, remap).
pub(crate) fn fts_replace_block(
    conn: &rusqlite::Connection,
    block_id: &str,
    content: &str,
) -> Result<()> {
    fts_delete_block(conn, block_id)?;
    fts_insert_block(conn, block_id, content)?;
    Ok(())
}
