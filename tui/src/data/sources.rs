//! Concrete `PageSource` implementations. Each one is a thin adapter from a
//! `GraphRepository` query to the generic `Paginator<T>` — no pagination
//! logic lives here, only "how do I fetch page N of this particular list".

use std::rc::Rc;

use grafium_core::models::{Block, Page};

use crate::data::pagination::PageSource;
use crate::data::repository::{GraphRepository, RepoResult};

pub struct AllPagesSource {
    repo: Rc<dyn GraphRepository>,
}

impl AllPagesSource {
    pub fn new(repo: Rc<dyn GraphRepository>) -> Self {
        Self { repo }
    }
}

impl PageSource<Page> for AllPagesSource {
    fn fetch(&self, limit: i64, offset: i64) -> RepoResult<Vec<Page>> {
        self.repo.list_pages(limit, offset)
    }
}

pub struct JournalSource {
    repo: Rc<dyn GraphRepository>,
}

impl JournalSource {
    pub fn new(repo: Rc<dyn GraphRepository>) -> Self {
        Self { repo }
    }
}

impl PageSource<Page> for JournalSource {
    fn fetch(&self, limit: i64, offset: i64) -> RepoResult<Vec<Page>> {
        self.repo.list_journal_pages(limit, offset)
    }
}

/// FTS search doesn't have real offset-based paging in `grafium-core`
/// (`search_fts` takes a single `limit`), so "load more" simply re-queries
/// with a larger limit and only the newly-revealed tail is appended. This
/// keeps the same `Paginator` interface working for search results too.
pub struct SearchSource {
    repo: Rc<dyn GraphRepository>,
    query: String,
}

impl SearchSource {
    pub fn new(repo: Rc<dyn GraphRepository>, query: String) -> Self {
        Self { repo, query }
    }
}

impl PageSource<Block> for SearchSource {
    fn fetch(&self, limit: i64, offset: i64) -> RepoResult<Vec<Block>> {
        if self.query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let widened_limit = offset + limit;
        let all = self.repo.search_blocks(&self.query, widened_limit)?;
        Ok(all.into_iter().skip(offset as usize).collect())
    }
}
