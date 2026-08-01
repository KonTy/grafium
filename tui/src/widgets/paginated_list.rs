//! Combines a `Paginator<T>` (data/loading) with a `ListPanel` (rendering +
//! selection) into the one reusable "infinite-scroll list" building block.
//! Every paginated list in the app (all pages, journals, search results) is
//! just an instance of this struct with a different `PageSource<T>`.

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::data::{PageSource, Paginator};
use crate::widgets::list_panel::ListPanel;

/// How many extra items must remain below the cursor before we prefetch the
/// next page — mirrors the `IntersectionObserver` firing near the bottom of
/// the scroll container in `JournalView.svelte`.
const PREFETCH_THRESHOLD: usize = 3;

pub struct PaginatedList<T> {
    paginator: Paginator<T>,
    panel: ListPanel,
}

impl<T> PaginatedList<T> {
    pub fn new(source: Box<dyn PageSource<T>>, page_size: i64) -> Self {
        let mut paginator = Paginator::new(source, page_size);
        paginator.load_more();
        Self {
            paginator,
            panel: ListPanel::new(),
        }
    }

    /// Replace the underlying data source (e.g. a new search query) and
    /// reload from scratch, keeping the same widget instance/selection state.
    pub fn reset(&mut self, source: Box<dyn PageSource<T>>) {
        self.paginator.reset(source);
        self.panel.select(None);
        self.paginator.load_more();
    }

    /// Part of this widget's public surface for callers that need to inspect
    /// the currently loaded window (e.g. to show a count elsewhere); not
    /// every consumer needs it today.
    #[allow(dead_code)]
    pub fn items(&self) -> &[T] {
        self.paginator.items()
    }

    pub fn selected(&self) -> Option<&T> {
        self.panel
            .selected()
            .and_then(|i| self.paginator.items().get(i))
    }

    #[allow(dead_code)]
    pub fn last_error(&self) -> Option<&str> {
        self.paginator.last_error()
    }

    pub fn move_selection(&mut self, delta: i32) {
        if let Some(idx) = self
            .panel
            .move_selection(delta, self.paginator.items().len())
        {
            self.paginator.ensure_loaded_near(idx, PREFETCH_THRESHOLD);
        }
    }

    pub fn render(
        &mut self,
        f: &mut Frame,
        area: Rect,
        title: &str,
        label: impl Fn(&T) -> String,
        focused: bool,
    ) {
        let display_title = if let Some(err) = self.paginator.last_error() {
            format!("{title} (error: {err})")
        } else {
            format!("{title} [{}]", self.paginator.items().len())
        };
        self.panel.render(
            f,
            area,
            &display_title,
            self.paginator.items(),
            label,
            focused,
        );
    }
}
