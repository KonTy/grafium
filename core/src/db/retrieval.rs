//! Database helpers for hybrid retrieval and small-to-big context expansion.
//!
//! These join `blocks` with `pages` so a retrieved block carries the page
//! title, journal flag, and timestamps needed to build dated, cited context
//! for the Knowledge Engine's chat/RAG path.

use super::Database;
use crate::error::Result;
use crate::models::{Block, BlockType};
use rusqlite::{params, Connection};

/// A block plus the metadata about its owning page that retrieval needs.
#[derive(Debug, Clone)]
pub struct BlockPageMeta {
    pub block_id: String,
    pub page_id: String,
    pub page_title: String,
    pub parent_id: Option<String>,
    pub content: String,
    pub is_journal: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

fn row_to_meta(row: &rusqlite::Row<'_>) -> rusqlite::Result<BlockPageMeta> {
    Ok(BlockPageMeta {
        block_id: row.get(0)?,
        page_id: row.get(1)?,
        page_title: row.get(2)?,
        parent_id: row.get(3)?,
        content: row.get(4)?,
        is_journal: row.get::<_, i32>(5)? != 0,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

const META_SELECT: &str = "SELECT b.id, b.page_id, p.title, b.parent_id, b.content, \
     p.is_journal, b.created_at, b.updated_at \
     FROM blocks b JOIN pages p ON p.id = b.page_id";

impl Database {
    /// Fetch block+page metadata for a set of block IDs, in one query.
    ///
    /// The returned order is unspecified (callers re-order by their own rank),
    /// and IDs that no longer exist are simply omitted.
    pub fn get_blocks_with_page_meta(&self, block_ids: &[String]) -> Result<Vec<BlockPageMeta>> {
        if block_ids.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn()?;
        let placeholders = std::iter::repeat_n("?", block_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("{META_SELECT} WHERE b.id IN ({placeholders})");
        let mut stmt = conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(block_ids.iter());
        let rows = stmt
            .query_map(params, |row| row_to_meta(row))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Walk `parent_id` upward from `block_id`, returning ancestors
    /// outermost-first (i.e. root ancestor at index 0, immediate parent last).
    /// The starting block itself is not included. A depth cap guards against
    /// pathological or cyclic data.
    pub fn get_ancestor_chain(&self, block_id: &str) -> Result<Vec<Block>> {
        let conn = self.conn()?;
        let mut chain = Vec::new();
        let mut current = block_parent_id(&conn, block_id)?;
        let mut guard = 0usize;
        while let Some(pid) = current {
            let block = load_block(&conn, &pid)?;
            match block {
                Some(b) => {
                    current = b.parent_id.clone();
                    chain.push(b);
                }
                None => break,
            }
            guard += 1;
            if guard >= 64 {
                break;
            }
        }
        chain.reverse();
        Ok(chain)
    }
}

fn block_parent_id(conn: &Connection, block_id: &str) -> Result<Option<String>> {
    let parent: Option<Option<String>> = conn
        .query_row(
            "SELECT parent_id FROM blocks WHERE id = ?1",
            params![block_id],
            |row| row.get(0),
        )
        .ok();
    Ok(parent.flatten())
}

fn load_block(conn: &Connection, block_id: &str) -> Result<Option<Block>> {
    let block = conn
        .query_row(
            "SELECT id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at FROM blocks WHERE id = ?1",
            params![block_id],
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
        )
        .ok();
    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;

    #[test]
    fn ancestor_chain_is_outermost_first() -> Result<()> {
        let db = Database::in_memory()?;
        let page = db.create_page("Tree", false)?;
        let gp = db.create_block(
            &page.id,
            None,
            0,
            "grandparent",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        let parent = db.create_block(
            &page.id,
            Some(&gp.id),
            0,
            "parent",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        let child = db.create_block(
            &page.id,
            Some(&parent.id),
            0,
            "child",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let chain = db.get_ancestor_chain(&child.id)?;
        let contents: Vec<&str> = chain.iter().map(|b| b.content.as_str()).collect();
        assert_eq!(contents, vec!["grandparent", "parent"]);

        // Root block has no ancestors.
        assert!(db.get_ancestor_chain(&gp.id)?.is_empty());
        Ok(())
    }

    #[test]
    fn block_meta_join_carries_page_and_journal_flag() -> Result<()> {
        let db = Database::in_memory()?;
        let journal = db.create_page("2026-01-02", true)?;
        let b = db.create_block(
            &journal.id,
            None,
            0,
            "a journal note",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let metas = db.get_blocks_with_page_meta(&[b.id.clone()])?;
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].page_title, "2026-01-02");
        assert!(metas[0].is_journal);
        assert_eq!(metas[0].content, "a journal note");

        // Missing IDs are simply omitted, not an error.
        assert!(db
            .get_blocks_with_page_meta(&["nope".to_string()])?
            .is_empty());
        assert!(db.get_blocks_with_page_meta(&[])?.is_empty());
        Ok(())
    }
}
