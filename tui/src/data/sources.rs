//! Concrete `PageSource` implementations. Each one is a thin adapter from a
//! `GraphRepository` query to the generic `Paginator<T>` — no pagination
//! logic lives here, only "how do I fetch page N of this particular list".

use std::sync::Arc;

use grafium_core::models::Page;

use crate::data::pagination::PageSource;
use crate::data::repository::{GraphRepository, RepoResult};

pub struct AllPagesSource {
    repo: Arc<dyn GraphRepository>,
}

impl AllPagesSource {
    pub fn new(repo: Arc<dyn GraphRepository>) -> Self {
        Self { repo }
    }
}

impl PageSource<Page> for AllPagesSource {
    fn fetch(&self, limit: i64, offset: i64) -> RepoResult<Vec<Page>> {
        self.repo.list_pages(limit, offset)
    }
}

pub struct JournalSource {
    repo: Arc<dyn GraphRepository>,
}

impl JournalSource {
    pub fn new(repo: Arc<dyn GraphRepository>) -> Self {
        Self { repo }
    }
}

impl PageSource<Page> for JournalSource {
    fn fetch(&self, limit: i64, offset: i64) -> RepoResult<Vec<Page>> {
        self.repo.list_journal_pages(limit, offset)
    }
}
