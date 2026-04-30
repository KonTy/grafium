use crate::models::{Block, BlockType};
use crate::error::Result;
use super::Database;
use chrono::Utc;
use rusqlite::params;
use uuid::Uuid;

impl Database {
    pub fn create_block(
        &self,
        page_id: &str,
        parent_id: Option<&str>,
        order_index: i32,
        content: &str,
        block_type: BlockType,
        properties: serde_json::Value,
    ) -> Result<Block> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        let id = Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO blocks (id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, page_id, parent_id, order_index, content, block_type.as_str(), properties.to_string(), now, now],
        )?;

        // Update FTS
        conn.execute(
            "INSERT INTO fts_blocks (block_id, content) VALUES (?1, ?2)",
            params![id, content],
        )?;

        Ok(Block {
            id,
            page_id: page_id.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            order_index,
            content: content.to_string(),
            block_type,
            properties,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn create_block_with_id(
        &self,
        id: &str,
        page_id: &str,
        parent_id: Option<&str>,
        order_index: i32,
        content: &str,
        block_type: BlockType,
        properties: serde_json::Value,
    ) -> Result<Block> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();

        conn.execute(
            "INSERT OR REPLACE INTO blocks (id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, page_id, parent_id, order_index, content, block_type.as_str(), properties.to_string(), now, now],
        )?;

        // Update FTS
        conn.execute(
            "DELETE FROM fts_blocks WHERE block_id = ?1",
            params![id],
        )?;
        conn.execute(
            "INSERT INTO fts_blocks (block_id, content) VALUES (?1, ?2)",
            params![id, content],
        )?;

        Ok(Block {
            id: id.to_string(),
            page_id: page_id.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            order_index,
            content: content.to_string(),
            block_type,
            properties,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn get_block(&self, id: &str) -> Result<Block> {
        let conn = self.conn()?;
        let block = conn.query_row(
            "SELECT id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at FROM blocks WHERE id = ?1",
            params![id],
            |row| {
                Ok(Block {
                    id: row.get(0)?,
                    page_id: row.get(1)?,
                    parent_id: row.get(2)?,
                    order_index: row.get(3)?,
                    content: row.get(4)?,
                    block_type: BlockType::from_str(&row.get::<_, String>(5)?),
                    properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        )?;
        Ok(block)
    }

    pub fn list_blocks_for_page(&self, page_id: &str) -> Result<Vec<Block>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at FROM blocks WHERE page_id = ?1 ORDER BY order_index"
        )?;
        let blocks = stmt.query_map(params![page_id], |row| {
            Ok(Block {
                id: row.get(0)?,
                page_id: row.get(1)?,
                parent_id: row.get(2)?,
                order_index: row.get(3)?,
                content: row.get(4)?,
                block_type: BlockType::from_str(&row.get::<_, String>(5)?),
                properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(blocks)
    }

    pub fn list_child_blocks(&self, parent_id: &str) -> Result<Vec<Block>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at FROM blocks WHERE parent_id = ?1 ORDER BY order_index"
        )?;
        let blocks = stmt.query_map(params![parent_id], |row| {
            Ok(Block {
                id: row.get(0)?,
                page_id: row.get(1)?,
                parent_id: row.get(2)?,
                order_index: row.get(3)?,
                content: row.get(4)?,
                block_type: BlockType::from_str(&row.get::<_, String>(5)?),
                properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(blocks)
    }

    pub fn update_block(&self, id: &str, content: &str, properties: Option<&serde_json::Value>) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();

        if let Some(props) = properties {
            conn.execute(
                "UPDATE blocks SET content = ?1, properties = ?2, updated_at = ?3 WHERE id = ?4",
                params![content, props.to_string(), now, id],
            )?;
        } else {
            conn.execute(
                "UPDATE blocks SET content = ?1, updated_at = ?2 WHERE id = ?3",
                params![content, now, id],
            )?;
        }

        // Update FTS
        conn.execute("DELETE FROM fts_blocks WHERE block_id = ?1", params![id])?;
        conn.execute(
            "INSERT INTO fts_blocks (block_id, content) VALUES (?1, ?2)",
            params![id, content],
        )?;

        Ok(())
    }

    pub fn delete_block(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM fts_blocks WHERE block_id = ?1", params![id])?;
        conn.execute("DELETE FROM blocks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn reorder_blocks(&self, page_id: &str, block_ids: &[String]) -> Result<()> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "UPDATE blocks SET order_index = ?1 WHERE id = ?2 AND page_id = ?3"
        )?;
        for (i, id) in block_ids.iter().enumerate() {
            stmt.execute(params![i as i32, id, page_id])?;
        }
        Ok(())
    }

    pub fn move_block(&self, id: &str, new_parent_id: Option<&str>, order_index: i32) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE blocks SET parent_id = ?1, order_index = ?2, updated_at = ?3 WHERE id = ?4",
            params![new_parent_id, order_index, now, id],
        )?;
        Ok(())
    }

    pub fn search_fts(&self, query: &str, limit: i64) -> Result<Vec<Block>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT b.id, b.page_id, b.parent_id, b.order_index, b.content, b.block_type, b.properties, b.created_at, b.updated_at
             FROM fts_blocks f
             JOIN blocks b ON b.id = f.block_id
             WHERE fts_blocks MATCH ?1
             ORDER BY rank
             LIMIT ?2"
        )?;
        let blocks = stmt.query_map(params![query, limit], |row| {
            Ok(Block {
                id: row.get(0)?,
                page_id: row.get(1)?,
                parent_id: row.get(2)?,
                order_index: row.get(3)?,
                content: row.get(4)?,
                block_type: BlockType::from_str(&row.get::<_, String>(5)?),
                properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(blocks)
    }
}
