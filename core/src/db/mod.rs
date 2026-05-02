mod schema;
mod pages;
mod blocks;
mod links;
mod tasks;
mod flashcards;
mod audio;
mod favorites;
mod graph_support;
mod raw_query;

use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use crate::error::Result;

pub struct Database {
    pool: Pool<SqliteConnectionManager>,
}

impl Database {
    pub fn new(path: &str) -> Result<Self> {
        let manager = SqliteConnectionManager::file(path);
        let pool = Pool::builder()
            .max_size(8)
            .build(manager)?;

        let db = Self { pool };
        db.initialize()?;
        Ok(db)
    }

    pub fn in_memory() -> Result<Self> {
        let manager = SqliteConnectionManager::memory();
        let pool = Pool::builder()
            .max_size(1)
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
        Ok(())
    }
}
