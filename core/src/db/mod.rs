mod schema;
mod pages;
mod blocks;
mod links;
mod tasks;
mod flashcards;
mod audio;
mod favorites;
mod graph_support;
mod ink;
mod raw_query;
mod properties;

use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use crate::error::Result;

struct FunctionCustomizer;

impl std::fmt::Debug for FunctionCustomizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FunctionCustomizer")
    }
}

impl r2d2::CustomizeConnection<rusqlite::Connection, rusqlite::Error> for FunctionCustomizer {
    fn on_acquire(&self, conn: &mut rusqlite::Connection) -> std::result::Result<(), rusqlite::Error> {
        use chrono::Utc;

        conn.create_scalar_function("days_ago", 1, rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC, |ctx| {
            let days: i64 = ctx.get(0)?;
            let ms = Utc::now().timestamp_millis() - (days * 86_400_000);
            Ok(ms)
        })?;

        conn.create_scalar_function("hours_ago", 1, rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC, |ctx| {
            let hours: i64 = ctx.get(0)?;
            let ms = Utc::now().timestamp_millis() - (hours * 3_600_000);
            Ok(ms)
        })?;

        conn.create_scalar_function("now_ms", 0, rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC, |_ctx| {
            Ok(Utc::now().timestamp_millis())
        })?;

        Ok(())
    }
}

pub struct Database {
    pool: Pool<SqliteConnectionManager>,
}

impl Database {
    pub fn new(path: &str) -> Result<Self> {
        let manager = SqliteConnectionManager::file(path);
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
        conn.execute_batch("
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA cache_size = -64000;
            PRAGMA foreign_keys = ON;
            PRAGMA temp_store = MEMORY;
            PRAGMA mmap_size = 268435456;
            PRAGMA page_size = 4096;
        ")?;
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

        // Backfill normalized properties if tables are empty but JSON blobs have data
        let prop_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM page_properties", [], |row| row.get(0)
        ).unwrap_or(0);
        drop(conn);

        if prop_count == 0 {
            let _ = self.backfill_properties();
        }

        Ok(())
    }
}
