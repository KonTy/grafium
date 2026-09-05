//! Extra database methods needed by the Graph layer for file-first indexing.

use super::Database;
use crate::error::Result;
use crate::models::Page;
use crate::models::{Block, BlockType};
use chrono::Utc;
use rusqlite::{params, Connection};
use uuid::Uuid;

fn upsert_page_on_conn(
    conn: &Connection,
    title: &str,
    is_journal: bool,
    file_path: Option<&str>,
    properties: &serde_json::Value,
) -> Result<Page> {
    let now = Utc::now().timestamp_millis();

    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM pages WHERE title = ?1",
            params![title],
            |row| row.get(0),
        )
        .ok();

    let id = if let Some(existing_id) = existing {
        conn.execute(
            "UPDATE pages SET file_path = ?1, updated_at = ?2, is_journal = ?3, properties = ?4 WHERE id = ?5",
            params![file_path, now, is_journal as i32, properties.to_string(), existing_id],
        )?;
        existing_id
    } else {
        let new_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO pages (id, title, file_path, created_at, updated_at, is_journal, properties) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![new_id, title, file_path, now, now, is_journal as i32, properties.to_string()],
        )?;
        new_id
    };

    Ok(Page {
        id,
        title: title.to_string(),
        file_path: file_path.map(|s| s.to_string()),
        created_at: now,
        updated_at: now,
        is_journal,
        properties: properties.clone(),
    })
}

fn delete_blocks_for_page_on_conn(conn: &Connection, page_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM block_properties WHERE block_id IN (SELECT id FROM blocks WHERE page_id = ?1)",
        params![page_id],
    )?;
    conn.execute(
        "DELETE FROM fts_blocks WHERE rowid IN (
            SELECT fts_rowid FROM fts_block_rowid
            WHERE block_id IN (SELECT id FROM blocks WHERE page_id = ?1)
        )",
        params![page_id],
    )?;
    conn.execute(
        "DELETE FROM fts_block_rowid WHERE block_id IN (SELECT id FROM blocks WHERE page_id = ?1)",
        params![page_id],
    )?;
    conn.execute(
        "DELETE FROM links WHERE from_block_id IN (SELECT id FROM blocks WHERE page_id = ?1)",
        params![page_id],
    )?;
    conn.execute(
        "DELETE FROM tasks WHERE block_id IN (SELECT id FROM blocks WHERE page_id = ?1)",
        params![page_id],
    )?;
    conn.execute(
        "DELETE FROM flashcards WHERE block_id IN (SELECT id FROM blocks WHERE page_id = ?1)",
        params![page_id],
    )?;
    conn.execute("DELETE FROM blocks WHERE page_id = ?1", params![page_id])?;
    Ok(())
}

fn insert_block_raw_on_conn(
    conn: &Connection,
    id: &str,
    page_id: &str,
    parent_id: Option<&str>,
    order_index: i32,
    content: &str,
    block_type: BlockType,
    properties: &serde_json::Value,
) -> Result<()> {
    let now = Utc::now().timestamp_millis();

    conn.execute(
        "INSERT OR REPLACE INTO blocks (id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![id, page_id, parent_id, order_index, content, block_type.as_str(), properties.to_string(), now, now],
    )?;

    super::fts_replace_block(conn, id, content)?;
    Ok(())
}

impl Database {
    /// Insert or update a page by title.
    pub fn upsert_page(
        &self,
        title: &str,
        is_journal: bool,
        file_path: Option<&str>,
        properties: &serde_json::Value,
    ) -> Result<Page> {
        let conn = self.conn()?;
        upsert_page_on_conn(&conn, title, is_journal, file_path, properties)
    }

    /// Set the file_path for a page.
    pub fn set_page_file_path(&self, page_id: &str, file_path: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE pages SET file_path = ?1 WHERE id = ?2",
            params![file_path, page_id],
        )?;
        Ok(())
    }

    /// Delete all blocks belonging to a page.
    pub fn delete_blocks_for_page(&self, page_id: &str) -> Result<()> {
        let conn = self.conn()?;
        delete_blocks_for_page_on_conn(&conn, page_id)
    }

    /// Insert a block with an explicit ID (used during indexing).
    pub fn insert_block_raw(
        &self,
        id: &str,
        page_id: &str,
        parent_id: Option<&str>,
        order_index: i32,
        content: &str,
        block_type: BlockType,
        properties: &serde_json::Value,
    ) -> Result<()> {
        let conn = self.conn()?;
        insert_block_raw_on_conn(
            &conn,
            id,
            page_id,
            parent_id,
            order_index,
            content,
            block_type,
            properties,
        )
    }

    /// Get a single block by ID.
    pub fn get_block_by_id(&self, id: &str) -> Result<Block> {
        self.get_block(id)
    }

    pub(crate) fn upsert_page_in_connection(
        &self,
        conn: &Connection,
        title: &str,
        is_journal: bool,
        file_path: Option<&str>,
        properties: &serde_json::Value,
    ) -> Result<Page> {
        upsert_page_on_conn(conn, title, is_journal, file_path, properties)
    }

    pub(crate) fn insert_block_raw_in_connection(
        &self,
        conn: &Connection,
        id: &str,
        page_id: &str,
        parent_id: Option<&str>,
        order_index: i32,
        content: &str,
        block_type: BlockType,
        properties: &serde_json::Value,
    ) -> Result<()> {
        insert_block_raw_on_conn(
            conn,
            id,
            page_id,
            parent_id,
            order_index,
            content,
            block_type,
            properties,
        )
    }

    /// Clear all indexed data (for full re-index).
    pub fn clear_all(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch(
            "
            DELETE FROM fts_blocks;
            DELETE FROM fts_block_rowid;
            DELETE FROM links;
            DELETE FROM tasks;
            DELETE FROM flashcards;
            DELETE FROM block_properties;
            DELETE FROM page_properties;
            DELETE FROM blocks;
            DELETE FROM pages;
            DELETE FROM pending_reindex;
        ",
        )?;
        Ok(())
    }

    /// Mark a page as needing a vector-index refresh. Coalesces repeated
    /// edits: `marked_at` is bumped to now, so N rapid edits collapse to one
    /// pending row and the per-page debounce window restarts on every edit.
    /// Best-effort by convention — callers on the write hot path ignore the
    /// result so a marking failure never fails the user's save.
    pub fn mark_page_pending_reindex(&self, page_id: &str) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO pending_reindex (page_id, marked_at) VALUES (?1, ?2)
             ON CONFLICT(page_id) DO UPDATE SET marked_at = ?2",
            params![page_id, now],
        )?;
        Ok(())
    }

    /// Pages that have been quiescent for at least `debounce_ms` (i.e. not
    /// edited again since), oldest first, each with its `marked_at` so the
    /// caller can clear the row only if it hasn't been re-marked by a newer
    /// edit meanwhile. Bounded by `limit` to keep a startup backlog drain
    /// gentle rather than embedding everything at once.
    pub fn list_pending_reindex_due(
        &self,
        debounce_ms: i64,
        limit: i64,
    ) -> Result<Vec<(String, i64)>> {
        let conn = self.conn()?;
        let cutoff = Utc::now().timestamp_millis() - debounce_ms;
        let mut stmt = conn.prepare(
            "SELECT page_id, marked_at FROM pending_reindex
             WHERE marked_at <= ?1 ORDER BY marked_at ASC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![cutoff, limit], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Clear a page's pending row, but only if it hasn't been re-marked since
    /// `marked_at`. If an edit landed while the page was being reindexed, its
    /// `marked_at` will have advanced and the row is left pending for the next
    /// drain cycle rather than being lost. Returns whether a row was cleared.
    pub fn clear_pending_reindex(&self, page_id: &str, marked_at: i64) -> Result<bool> {
        let conn = self.conn()?;
        let affected = conn.execute(
            "DELETE FROM pending_reindex WHERE page_id = ?1 AND marked_at = ?2",
            params![page_id, marked_at],
        )?;
        Ok(affected > 0)
    }

    /// Number of pages currently awaiting a vector-index refresh, for the
    /// Chat "N pages pending" staleness indicator.
    pub fn count_pending_reindex(&self) -> Result<usize> {
        let conn = self.conn()?;
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM pending_reindex", [], |row| row.get(0))?;
        Ok(count as usize)
    }
}
