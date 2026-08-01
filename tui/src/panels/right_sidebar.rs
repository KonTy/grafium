//! Right sidebar: backlinks for whatever page is open in the center panel.
//! Reuses the generic `ListPanel` widget for rendering/selection (no bespoke
//! list code) and the `GraphRepository::get_backlinks` seam for data.

use std::sync::Arc;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::Frame;

use grafium_core::models::{Block, Link};

use crate::data::GraphRepository;
use crate::panels::{Panel, PanelAction};
use crate::widgets::list_panel::ListPanel;

pub struct RightSidebar {
    repo: Arc<dyn GraphRepository>,
    items: Vec<(Link, Block, String)>,
    panel: ListPanel,
    error: Option<String>,
    visible: bool,
    target_page_id: Option<String>,
    loaded_page_id: Option<String>,
}

impl RightSidebar {
    pub fn new(repo: Arc<dyn GraphRepository>) -> Self {
        Self {
            repo,
            items: Vec::new(),
            panel: ListPanel::new(),
            error: None,
            visible: true,
            target_page_id: None,
            loaded_page_id: None,
        }
    }

    /// Reload backlinks for the page that just became active in the center panel.
    pub fn set_target_page(&mut self, page_id: &str) {
        self.panel.select(None);
        self.target_page_id = Some(page_id.to_string());
        self.loaded_page_id = None;
        self.load_if_visible();
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        self.load_if_visible();
    }

    fn load_if_visible(&mut self) {
        if !self.visible {
            return;
        }
        let Some(page_id) = self.target_page_id.as_deref() else {
            self.items.clear();
            self.error = None;
            self.loaded_page_id = None;
            return;
        };
        if self.loaded_page_id.as_deref() == Some(page_id) {
            return;
        }
        match self.repo.get_backlinks(page_id) {
            Ok(items) => {
                self.items = items;
                self.error = None;
                self.loaded_page_id = Some(page_id.to_string());
            }
            Err(e) => {
                self.items = Vec::new();
                self.error = Some(e);
                self.loaded_page_id = None;
            }
        }
    }
}

impl Panel for RightSidebar {
    fn handle_key(&mut self, key: KeyEvent) -> PanelAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.panel.move_selection(-1, self.items.len());
                PanelAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.panel.move_selection(1, self.items.len());
                PanelAction::None
            }
            KeyCode::Enter => self
                .panel
                .selected()
                .and_then(|i| self.items.get(i))
                .map(|(_, block, _)| PanelAction::OpenPage(block.page_id.clone()))
                .unwrap_or(PanelAction::None),
            _ => PanelAction::None,
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        let title = match &self.error {
            Some(e) => format!("Backlinks (t r) [error: {e}]"),
            None => format!("Backlinks (t r) [{}]", self.items.len()),
        };
        let label = |(_, block, page_title): &(Link, Block, String)| {
            let snippet: String = block.content.chars().take(48).collect();
            format!("{page_title} — {snippet}")
        };
        self.panel
            .render(f, area, &title, &self.items, label, focused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use grafium_core::models::{BlockType, Page};

    use crate::data::RepoResult;

    #[derive(Default)]
    struct CountingRepo {
        backlink_calls: AtomicUsize,
    }

    impl GraphRepository for CountingRepo {
        fn list_pages(&self, _limit: i64, _offset: i64) -> RepoResult<Vec<Page>> {
            Ok(Vec::new())
        }

        fn list_journal_pages(&self, _limit: i64, _offset: i64) -> RepoResult<Vec<Page>> {
            Ok(Vec::new())
        }

        fn search_blocks(&self, _query: &str, _limit: i64) -> RepoResult<Vec<Block>> {
            Ok(Vec::new())
        }

        fn get_page_by_id(&self, page_id: &str) -> RepoResult<Page> {
            Ok(Page {
                id: page_id.to_string(),
                title: page_id.to_string(),
                file_path: None,
                created_at: 0,
                updated_at: 0,
                is_journal: false,
                properties: serde_json::json!({}),
            })
        }

        fn list_blocks_for_page(&self, _page_id: &str) -> RepoResult<Vec<Block>> {
            Ok(Vec::new())
        }

        fn update_block(&self, _block_id: &str, _content: &str) -> RepoResult<()> {
            Ok(())
        }

        fn create_block(
            &self,
            page_id: &str,
            _order_index: i32,
            content: &str,
        ) -> RepoResult<Block> {
            Ok(Block {
                id: "block-1".to_string(),
                page_id: page_id.to_string(),
                parent_id: None,
                order_index: 0,
                content: content.to_string(),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            })
        }

        fn get_backlinks(&self, _page_id: &str) -> RepoResult<Vec<(Link, Block, String)>> {
            self.backlink_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }

        fn get_links_from_page(&self, _page_id: &str) -> RepoResult<Vec<Link>> {
            Ok(Vec::new())
        }

        fn get_or_create_today_journal(&self) -> RepoResult<Page> {
            Err("unused".to_string())
        }
    }

    #[test]
    fn defers_backlink_loading_until_sidebar_becomes_visible() {
        let repo = Arc::new(CountingRepo::default());
        let mut sidebar = RightSidebar::new(repo.clone());

        sidebar.set_visible(false);
        sidebar.set_target_page("page-1");

        assert_eq!(repo.backlink_calls.load(Ordering::SeqCst), 0);

        sidebar.set_visible(true);

        assert_eq!(repo.backlink_calls.load(Ordering::SeqCst), 1);
    }
}
