use crate::models::{Link, LinkType, Block, BlockType};
use crate::error::Result;
use super::Database;
use rusqlite::params;

impl Database {
    pub fn insert_link(&self, from_block_id: &str, to_page_id: &str, link_type: LinkType) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR IGNORE INTO links (from_block_id, to_page_id, link_type) VALUES (?1, ?2, ?3)",
            params![from_block_id, to_page_id, link_type.as_str()],
        )?;
        Ok(())
    }

    pub fn delete_links_from_block(&self, block_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM links WHERE from_block_id = ?1", params![block_id])?;
        Ok(())
    }

    pub fn get_backlinks(&self, page_id: &str) -> Result<Vec<(Link, Block)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT l.from_block_id, l.to_page_id, l.link_type,
                    b.id, b.page_id, b.parent_id, b.order_index, b.content, b.block_type, b.properties, b.created_at, b.updated_at
             FROM links l
             JOIN blocks b ON b.id = l.from_block_id
             WHERE l.to_page_id = ?1
             ORDER BY b.updated_at DESC"
        )?;
        let results = stmt.query_map(params![page_id], |row| {
            let link = Link {
                from_block_id: row.get(0)?,
                to_page_id: row.get(1)?,
                link_type: LinkType::from_str(&row.get::<_, String>(2)?),
            };
            let block = Block {
                id: row.get(3)?,
                page_id: row.get(4)?,
                parent_id: row.get(5)?,
                order_index: row.get(6)?,
                content: row.get(7)?,
                block_type: BlockType::from_str(&row.get::<_, String>(8)?),
                properties: serde_json::from_str(&row.get::<_, String>(9)?).unwrap_or_default(),
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            };
            Ok((link, block))
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(results)
    }

    pub fn get_links_from_page(&self, page_id: &str) -> Result<Vec<Link>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT l.from_block_id, l.to_page_id, l.link_type
             FROM links l
             JOIN blocks b ON b.id = l.from_block_id
             WHERE b.page_id = ?1"
        )?;
        let links = stmt.query_map(params![page_id], |row| {
            Ok(Link {
                from_block_id: row.get(0)?,
                to_page_id: row.get(1)?,
                link_type: LinkType::from_str(&row.get::<_, String>(2)?),
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(links)
    }
}
