//! A single seam between the TUI and `grafium-core`.
//!
//! Every screen/widget talks to the graph exclusively through the
//! [`GraphRepository`] trait instead of poking at `grafium_core::Graph`
//! directly. This keeps all core-specific plumbing in one place, makes the
//! UI layer trivially mockable/testable, and means that if the underlying
//! query changes shape, only this file needs to change.

use grafium_core::Graph;
use grafium_core::models::{Block, Link, Page};

pub type RepoResult<T> = Result<T, String>;

/// Everything the TUI needs to read from / write to a graph.
pub trait GraphRepository {
    /// Non-journal pages, most-recently-updated first, paginated.
    fn list_pages(&self, limit: i64, offset: i64) -> RepoResult<Vec<Page>>;
    /// Journal pages, most recent first, paginated.
    fn list_journal_pages(&self, limit: i64, offset: i64) -> RepoResult<Vec<Page>>;
    /// Full-text search across block content, paginated by re-querying with a growing limit
    /// (FTS ranking is stable, so this is cheap and correct for "load more").
    fn search_blocks(&self, query: &str, limit: i64) -> RepoResult<Vec<Block>>;

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

    fn search_blocks(&self, query: &str, limit: i64) -> RepoResult<Vec<Block>> {
        self.graph
            .db
            .search_fts(query, limit)
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
        raw.into_iter()
            .map(|(link, block)| {
                let title = self
                    .graph
                    .db
                    .get_block_page_title(&block.id)
                    .unwrap_or_else(|_| "(unknown page)".to_string());
                Ok((link, block, title))
            })
            .collect()
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
