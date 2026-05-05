//! Extra database methods needed by the Graph layer for file-first indexing.

use crate::models::{Block, BlockType};
use crate::error::Result;
use super::Database;
use chrono::Utc;
use rusqlite::params;
use uuid::Uuid;
use crate::models::Page;

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
        let now = Utc::now().timestamp_millis();

        // Check if page already exists
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM pages WHERE title = ?1",
                params![title],
                |row| row.get(0),
            )
            .ok();

        let id = if let Some(existing_id) = existing {
            // Update existing
            conn.execute(
                "UPDATE pages SET file_path = ?1, updated_at = ?2, is_journal = ?3, properties = ?4 WHERE id = ?5",
                params![file_path, now, is_journal as i32, properties.to_string(), existing_id],
            )?;
            existing_id
        } else {
            // Insert new
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
        // Delete normalized block properties
        conn.execute(
            "DELETE FROM block_properties WHERE block_id IN (SELECT id FROM blocks WHERE page_id = ?1)",
            params![page_id],
        )?;
        // Delete FTS entries first
        conn.execute(
            "DELETE FROM fts_blocks WHERE block_id IN (SELECT id FROM blocks WHERE page_id = ?1)",
            params![page_id],
        )?;
        // Delete links from these blocks
        conn.execute(
            "DELETE FROM links WHERE from_block_id IN (SELECT id FROM blocks WHERE page_id = ?1)",
            params![page_id],
        )?;
        // Delete tasks
        conn.execute(
            "DELETE FROM tasks WHERE block_id IN (SELECT id FROM blocks WHERE page_id = ?1)",
            params![page_id],
        )?;
        // Delete flashcards
        conn.execute(
            "DELETE FROM flashcards WHERE block_id IN (SELECT id FROM blocks WHERE page_id = ?1)",
            params![page_id],
        )?;
        // Delete blocks
        conn.execute("DELETE FROM blocks WHERE page_id = ?1", params![page_id])?;
        Ok(())
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
        let now = Utc::now().timestamp_millis();

        conn.execute(
            "INSERT OR REPLACE INTO blocks (id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, page_id, parent_id, order_index, content, block_type.as_str(), properties.to_string(), now, now],
        )?;

        // Update FTS
        conn.execute("DELETE FROM fts_blocks WHERE block_id = ?1", params![id])?;
        conn.execute(
            "INSERT INTO fts_blocks (block_id, content) VALUES (?1, ?2)",
            params![id, content],
        )?;

        Ok(())
    }

    /// Get a single block by ID.
    pub fn get_block_by_id(&self, id: &str) -> Result<Block> {
        self.get_block(id)
    }

    /// Clear all indexed data (for full re-index).
    pub fn clear_all(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch("
            DELETE FROM fts_blocks;
            DELETE FROM links;
            DELETE FROM tasks;
            DELETE FROM flashcards;
            DELETE FROM block_properties;
            DELETE FROM page_properties;
            DELETE FROM blocks;
            DELETE FROM pages;
        ")?;
        Ok(())
    }
}
