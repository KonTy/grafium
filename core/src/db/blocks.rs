use super::Database;
use crate::error::Result;
use crate::models::{Block, BlockType};
use chrono::Utc;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use uuid::Uuid;

fn flatten_blocks_in_tree_order(
    grouped: &mut HashMap<Option<String>, Vec<Block>>,
    parent_id: Option<String>,
    out: &mut Vec<Block>,
) {
    if let Some(mut children) = grouped.remove(&parent_id) {
        children.sort_by(|a, b| {
            a.order_index
                .cmp(&b.order_index)
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.id.cmp(&b.id))
        });

        for block in children {
            let child_parent_id = Some(block.id.clone());
            out.push(block);
            flatten_blocks_in_tree_order(grouped, child_parent_id, out);
        }
    }
}

fn load_blocks_for_page(conn: &Connection, page_id: &str) -> Result<Vec<Block>> {
    let mut stmt = conn.prepare(
        "SELECT id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at FROM blocks WHERE page_id = ?1"
    )?;
    let blocks = stmt
        .query_map(params![page_id], |row| {
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
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut grouped: HashMap<Option<String>, Vec<Block>> = HashMap::new();
    for block in blocks {
        grouped
            .entry(block.parent_id.clone())
            .or_default()
            .push(block);
    }

    let mut ordered = Vec::new();
    flatten_blocks_in_tree_order(&mut grouped, None, &mut ordered);
    Ok(ordered)
}

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
        super::fts_insert_block(&conn, &id, content)?;

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
        super::fts_replace_block(&conn, id, content)?;

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

    pub fn get_block_page_title(&self, block_id: &str) -> Result<String> {
        let conn = self.conn()?;
        let title: String = conn.query_row(
            "SELECT p.title FROM blocks b JOIN pages p ON p.id = b.page_id WHERE b.id = ?1",
            params![block_id],
            |row| row.get(0),
        )?;
        Ok(title)
    }

    pub fn list_blocks_for_page(&self, page_id: &str) -> Result<Vec<Block>> {
        let conn = self.conn()?;
        load_blocks_for_page(&conn, page_id)
    }

    pub fn list_child_blocks(&self, parent_id: &str) -> Result<Vec<Block>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at FROM blocks WHERE parent_id = ?1 ORDER BY order_index"
        )?;
        let blocks = stmt
            .query_map(params![parent_id], |row| {
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
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(blocks)
    }

    pub fn update_block(
        &self,
        id: &str,
        content: &str,
        properties: Option<&serde_json::Value>,
    ) -> Result<()> {
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
        super::fts_replace_block(&conn, id, content)?;

        Ok(())
    }

    pub fn delete_block(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        self.delete_block_in_connection(&conn, id)
    }

    pub fn reorder_blocks(&self, page_id: &str, block_ids: &[String]) -> Result<()> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("UPDATE blocks SET order_index = ?1 WHERE id = ?2 AND page_id = ?3")?;
        for (i, id) in block_ids.iter().enumerate() {
            stmt.execute(params![i as i32, id, page_id])?;
        }
        Ok(())
    }

    pub fn move_block(
        &self,
        id: &str,
        new_parent_id: Option<&str>,
        order_index: i32,
    ) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE blocks SET parent_id = ?1, order_index = ?2, updated_at = ?3 WHERE id = ?4",
            params![new_parent_id, order_index, now, id],
        )?;
        Ok(())
    }

    pub(crate) fn list_blocks_for_page_in_connection(
        &self,
        conn: &Connection,
        page_id: &str,
    ) -> Result<Vec<Block>> {
        load_blocks_for_page(conn, page_id)
    }

    pub(crate) fn update_indexed_block_in_connection(
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
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE blocks
             SET page_id = ?1,
                 parent_id = ?2,
                 order_index = ?3,
                 content = ?4,
                 block_type = ?5,
                 properties = ?6,
                 updated_at = ?7
             WHERE id = ?8",
            params![
                page_id,
                parent_id,
                order_index,
                content,
                block_type.as_str(),
                properties.to_string(),
                now,
                id
            ],
        )?;
        super::fts_replace_block(conn, id, content)?;
        Ok(())
    }

    pub(crate) fn delete_block_in_connection(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<()> {
        super::fts_delete_block(conn, id)?;
        conn.execute("DELETE FROM blocks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn next_root_order_index(&self, page_id: &str) -> Result<i32> {
        let conn = self.conn()?;
        let max: i32 = conn.query_row(
            "SELECT COALESCE(MAX(order_index), -1) FROM blocks WHERE page_id = ?1 AND parent_id IS NULL",
            params![page_id],
            |row| row.get(0),
        )?;
        Ok(max + 1)
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
        let blocks = stmt
            .query_map(params![query, limit], |row| {
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
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(blocks)
    }

    /// Get all block content strings (for asset reference scanning).
    pub fn get_all_block_content(&self) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT content FROM blocks WHERE content != ''")?;
        let rows = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(rows)
    }
}
