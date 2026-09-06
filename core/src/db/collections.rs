//! Queries backing the collections UI (books, projects, reading lists).
//!
//! A collection is an ordinary page carrying a `{"collection": …}` marker in
//! its `properties` (see `knowledge::collections`), and its members are the
//! blocks on that page that link out to another page. Both facts are already in
//! the tables — no membership table exists, on purpose — so these are plain
//! reads over `pages`, `blocks`, and `links` rather than anything bespoke.

use super::Database;
use crate::error::Result;
use crate::models::Page;
use rusqlite::params;

impl Database {
    /// List every page marked as a collection, paired with its member count.
    ///
    /// The count is the number of *blocks* on the page that carry at least one
    /// page link — that is the definition of a member here (each linked block
    /// is one ordered entry). It's a `COUNT(DISTINCT from_block_id)` because a
    /// block that happens to mention two `[[links]]` is still a single member,
    /// not two.
    ///
    /// Membership is resolved with a correlated subquery rather than a join +
    /// `GROUP BY` so a collection with zero members still comes back (with a
    /// count of 0) instead of being dropped by an inner join — a freshly
    /// created, still-empty book must appear in the list. The subquery runs
    /// once per collection page, and collections are few (a shelf of books, not
    /// the whole graph), so this stays a single cheap round-trip.
    pub fn list_collections(&self) -> Result<Vec<(Page, i64)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT p.id, p.title, p.file_path, p.created_at, p.updated_at, p.is_journal, p.properties,
                    (SELECT COUNT(DISTINCT l.from_block_id)
                       FROM links l
                       JOIN blocks b ON b.id = l.from_block_id
                      WHERE b.page_id = p.id AND l.link_type = 'page') AS member_count
             FROM pages p
             WHERE COALESCE(TRIM(json_extract(p.properties, '$.collection')), '') <> ''
             ORDER BY p.title ASC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let page = Page {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    file_path: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    is_journal: row.get::<_, i32>(5)? != 0,
                    properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                };
                let member_count: i64 = row.get(7)?;
                Ok((page, member_count))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Count the member blocks of one collection page.
    ///
    /// Same definition as [`Database::list_collections`] — distinct blocks that
    /// link out — exposed on its own for callers that already know the page and
    /// only need its size (e.g. rendering a single collection's header).
    pub fn count_collection_members(&self, page_id: &str) -> Result<i64> {
        let conn = self.conn()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT l.from_block_id)
               FROM links l
               JOIN blocks b ON b.id = l.from_block_id
              WHERE b.page_id = ?1 AND l.link_type = 'page'",
            params![page_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::Database;
    use crate::error::Result;
    use crate::knowledge::mark_collection;
    use crate::models::{BlockType, LinkType};

    /// Create a page, then set its collection marker through the same path the
    /// command uses (`update_page`), so the test exercises the real JSON blob
    /// round-trip and `json_extract` filter rather than a hand-built string.
    fn mark_page_as(db: &Database, title: &str, kind: &str) -> Result<String> {
        let page = db.create_page(title, false)?;
        let mut props = page.properties.clone();
        mark_collection(&mut props, kind);
        db.update_page(&page.id, None, Some(&props))?;
        Ok(page.id)
    }

    /// Add a block to `page_id` and, when `links_to` is non-empty, record a
    /// page link from that block to each target — mirroring what the indexer
    /// writes when a block's text contains `[[…]]`.
    fn add_block(
        db: &Database,
        page_id: &str,
        order: i32,
        content: &str,
        links_to: &[&str],
    ) -> Result<()> {
        let block = db.create_block(
            page_id,
            None,
            order,
            content,
            BlockType::Text,
            serde_json::json!({}),
        )?;
        for target in links_to {
            db.insert_link(&block.id, target, LinkType::Page)?;
        }
        Ok(())
    }

    #[test]
    fn list_collections_returns_only_marked_pages() -> Result<()> {
        let db = Database::in_memory()?;
        db.create_page("Just A Page", false)?;
        mark_page_as(&db, "My Novel", "book")?;
        mark_page_as(&db, "Q3 Roadmap", "project")?;

        let collections = db.list_collections()?;
        let titles: Vec<&str> = collections
            .iter()
            .map(|(page, _)| page.title.as_str())
            .collect();
        assert_eq!(titles, vec!["My Novel", "Q3 Roadmap"]);
        Ok(())
    }

    #[test]
    fn empty_collection_still_appears_with_zero_members() -> Result<()> {
        let db = Database::in_memory()?;
        mark_page_as(&db, "Empty Book", "book")?;

        let collections = db.list_collections()?;
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].1, 0);
        Ok(())
    }

    #[test]
    fn member_count_counts_distinct_linked_blocks() -> Result<()> {
        let db = Database::in_memory()?;
        let book_id = mark_page_as(&db, "My Book", "book")?;
        // Two chapters the book links to.
        let ch1 = db.create_page("Chapter One", false)?;
        let ch2 = db.create_page("Chapter Two", false)?;

        // A free-form note block with no link (not a member), one block per
        // chapter link, plus a block with two links (still one member).
        add_block(&db, &book_id, 0, "Some thoughts before we begin", &[])?;
        add_block(&db, &book_id, 1, "See [[Chapter One]]", &[&ch1.id])?;
        add_block(
            &db,
            &book_id,
            2,
            "Then [[Chapter Two]] and [[Chapter One]]",
            &[&ch2.id, &ch1.id],
        )?;

        assert_eq!(db.count_collection_members(&book_id)?, 2);
        let collections = db.list_collections()?;
        assert_eq!(collections[0].1, 2);
        Ok(())
    }
}
