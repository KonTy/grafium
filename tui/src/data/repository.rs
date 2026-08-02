//! A single seam between the TUI and `grafium-core`.
//!
//! Every screen/widget talks to the graph exclusively through the
//! [`GraphRepository`] trait instead of poking at `grafium_core::Graph`
//! directly. This keeps all core-specific plumbing in one place, makes the
//! UI layer trivially mockable/testable, and means that if the underlying
//! query changes shape, only this file needs to change.

use grafium_core::models::{Block, Link, Page};
use grafium_core::Graph;
use std::collections::{BTreeSet, HashMap};

pub type RepoResult<T> = Result<T, String>;

/// Everything the TUI needs to read from / write to a graph.
pub trait GraphRepository: Send + Sync {
    /// Non-journal pages, most-recently-updated first, paginated.
    fn list_pages(&self, limit: i64, offset: i64) -> RepoResult<Vec<Page>>;
    /// Journal pages, most recent first, paginated.
    fn list_journal_pages(&self, limit: i64, offset: i64) -> RepoResult<Vec<Page>>;
    /// Full-text search across block content, paginated with SQL LIMIT/OFFSET windows.
    fn search_blocks(&self, query: &str, limit: i64, offset: i64) -> RepoResult<Vec<Block>>;

    fn get_page_by_id(&self, page_id: &str) -> RepoResult<Page>;
    fn list_blocks_for_page(&self, page_id: &str) -> RepoResult<Vec<Block>>;
    fn update_block(&self, block_id: &str, content: &str) -> RepoResult<()>;
    fn create_block(&self, page_id: &str, order_index: i32, content: &str) -> RepoResult<Block>;

    /// Blocks (and their page titles) that link *to* this page.
    fn get_backlinks(&self, page_id: &str) -> RepoResult<Vec<(Link, Block, String)>>;
    /// Pages linked *from* this page's blocks.
    fn get_links_from_page(&self, page_id: &str) -> RepoResult<Vec<Link>>;

    fn get_or_create_today_journal(&self) -> RepoResult<Page>;
}

/// Default implementation backed by a real, open `grafium_core::Graph`.
pub struct CoreRepository {
    graph: Graph,
}

impl CoreRepository {
    pub fn new(graph: Graph) -> Self {
        Self { graph }
    }

    fn label_error<E: std::fmt::Display>(context: &str, err: E) -> String {
        format!("{context}: {err}")
    }
}

fn hydrate_backlinks_with_titles<F>(
    raw: Vec<(Link, Block)>,
    fetch_titles: F,
) -> RepoResult<Vec<(Link, Block, String)>>
where
    F: FnOnce(&[String]) -> RepoResult<HashMap<String, String>>,
{
    let page_ids: Vec<String> = raw
        .iter()
        .map(|(_, block)| block.page_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let titles_by_page = fetch_titles(&page_ids)?;

    Ok(raw
        .into_iter()
        .map(|(link, block)| {
            let title = titles_by_page
                .get(&block.page_id)
                .cloned()
                .unwrap_or_else(|| "(unknown page)".to_string());
            (link, block, title)
        })
        .collect())
}

impl GraphRepository for CoreRepository {
    fn list_pages(&self, limit: i64, offset: i64) -> RepoResult<Vec<Page>> {
        self.graph
            .db
            .list_pages_window(limit, offset, false)
            .map_err(|e| Self::label_error("list_pages", e))
    }

    fn list_journal_pages(&self, limit: i64, offset: i64) -> RepoResult<Vec<Page>> {
        self.graph
            .db
            .list_journal_pages(limit, offset)
            .map_err(|e| Self::label_error("list_journal_pages", e))
    }

    fn search_blocks(&self, query: &str, limit: i64, offset: i64) -> RepoResult<Vec<Block>> {
        self.graph
            .db
            .search_fts_window(query, limit, offset)
            .map_err(|e| Self::label_error("search_blocks", e))
    }

    fn get_page_by_id(&self, page_id: &str) -> RepoResult<Page> {
        self.graph
            .db
            .get_page_by_id(page_id)
            .map_err(|e| Self::label_error("get_page_by_id", e))
    }

    fn list_blocks_for_page(&self, page_id: &str) -> RepoResult<Vec<Block>> {
        self.graph
            .db
            .list_blocks_for_page(page_id)
            .map_err(|e| Self::label_error("list_blocks_for_page", e))
    }

    fn update_block(&self, block_id: &str, content: &str) -> RepoResult<()> {
        self.graph
            .update_block(block_id, content, None)
            .map_err(|e| Self::label_error("update_block", e))
    }

    fn create_block(&self, page_id: &str, order_index: i32, content: &str) -> RepoResult<Block> {
        self.graph
            .create_block(
                page_id,
                None,
                order_index,
                content,
                grafium_core::models::BlockType::Text,
                serde_json::json!({}),
            )
            .map_err(|e| Self::label_error("create_block", e))
    }

    fn get_backlinks(&self, page_id: &str) -> RepoResult<Vec<(Link, Block, String)>> {
        let raw = self
            .graph
            .db
            .get_backlinks(page_id)
            .map_err(|e| Self::label_error("get_backlinks", e))?;
        hydrate_backlinks_with_titles(raw, |page_ids| {
            self.graph
                .db
                .get_page_titles(page_ids)
                .map_err(|e| Self::label_error("get_backlinks", e))
        })
    }

    fn get_links_from_page(&self, page_id: &str) -> RepoResult<Vec<Link>> {
        self.graph
            .db
            .get_links_from_page(page_id)
            .map_err(|e| Self::label_error("get_links_from_page", e))
    }

    fn get_or_create_today_journal(&self) -> RepoResult<Page> {
        self.graph
            .get_or_create_today_journal()
            .map_err(|e| Self::label_error("get_or_create_today_journal", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::{Cell, RefCell};

    use grafium_core::models::{BlockType, LinkType};

    fn block(id: &str, page_id: &str, content: &str) -> Block {
        Block {
            id: id.to_string(),
            page_id: page_id.to_string(),
            parent_id: None,
            order_index: 0,
            content: content.to_string(),
            block_type: BlockType::Text,
            properties: serde_json::json!({}),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn link(from_block_id: &str, to_page_id: &str) -> Link {
        Link {
            from_block_id: from_block_id.to_string(),
            to_page_id: to_page_id.to_string(),
            link_type: LinkType::Page,
        }
    }

    #[test]
    fn hydrate_backlinks_with_titles_batches_lookup_once() {
        let raw = vec![
            (
                link("block-1", "target"),
                block("block-1", "page-2", "second page"),
            ),
            (
                link("block-2", "target"),
                block("block-2", "page-1", "first page"),
            ),
            (
                link("block-3", "target"),
                block("block-3", "page-2", "same page again"),
            ),
            (
                link("block-4", "target"),
                block("block-4", "page-3", "missing title"),
            ),
        ];
        let call_count = Cell::new(0);
        let requested_ids = RefCell::new(Vec::new());

        let resolved = hydrate_backlinks_with_titles(raw, |page_ids| {
            call_count.set(call_count.get() + 1);
            *requested_ids.borrow_mut() = page_ids.to_vec();
            Ok(HashMap::from([
                ("page-1".to_string(), "Page One".to_string()),
                ("page-2".to_string(), "Page Two".to_string()),
            ]))
        })
        .expect("batched title lookup should succeed");

        assert_eq!(call_count.get(), 1);
        assert_eq!(
            requested_ids.into_inner(),
            vec![
                "page-1".to_string(),
                "page-2".to_string(),
                "page-3".to_string()
            ]
        );
        assert_eq!(
            resolved
                .into_iter()
                .map(|(_, _, title)| title)
                .collect::<Vec<_>>(),
            vec![
                "Page Two".to_string(),
                "Page One".to_string(),
                "Page Two".to_string(),
                "(unknown page)".to_string()
            ]
        );
    }
}
