//! Generic, reusable "infinite scroll" pagination — mirrors the pattern used
//! by `JournalView.svelte` today (load N items, and load N more once the
//! viewport nears the end of what's loaded), but written exactly once and
//! shared by every list in the TUI (all-pages, journals, search results).

use crate::data::repository::RepoResult;

/// A source of paginated items of type `T`. One implementation per data set
/// (all pages, journal pages, FTS search results, ...). See `data::sources`.
pub trait PageSource<T> {
    fn fetch(&self, limit: i64, offset: i64) -> RepoResult<Vec<T>>;
}

/// Owns the loaded window of items plus enough bookkeeping to fetch more.
/// Behaviourally identical to the Svelte `loadedPages`/`offset`/`moreAvailable`
/// trio, just generic over the item type and the source it pulls from.
pub struct Paginator<T> {
    source: Box<dyn PageSource<T>>,
    items: Vec<T>,
    page_size: i64,
    offset: i64,
    has_more: bool,
    last_error: Option<String>,
}

impl<T> Paginator<T> {
    pub fn new(source: Box<dyn PageSource<T>>, page_size: i64) -> Self {
        Self {
            source,
            items: Vec::new(),
            page_size,
            offset: 0,
            has_more: true,
            last_error: None,
        }
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Forget everything and start over (e.g. the search query changed).
    pub fn reset(&mut self, source: Box<dyn PageSource<T>>) {
        self.source = source;
        self.items.clear();
        self.offset = 0;
        self.has_more = true;
        self.last_error = None;
    }

    /// Fetch the next page. Returns `true` if new items were appended.
    pub fn load_more(&mut self) -> bool {
        if !self.has_more {
            return false;
        }
        match self.source.fetch(self.page_size, self.offset) {
            Ok(batch) => {
                let n = batch.len() as i64;
                self.has_more = n >= self.page_size;
                self.offset += n;
                let added = n > 0;
                self.items.extend(batch);
                added
            }
            Err(e) => {
                self.has_more = false;
                self.last_error = Some(e);
                false
            }
        }
    }

    /// Call after moving the selection cursor: loads another page once the
    /// user has scrolled within `threshold` items of the end, exactly like
    /// the `IntersectionObserver` near the bottom of the journal list.
    pub fn ensure_loaded_near(&mut self, selected_index: usize, threshold: usize) {
        if self.has_more && selected_index + threshold >= self.items.len() {
            self.load_more();
        }
    }
}
