//! Right sidebar: backlinks for whatever page is open in the center panel.
//! Reuses the generic `ListPanel` widget for rendering/selection (no bespoke
//! list code) and the `GraphRepository::get_backlinks` seam for data.

use std::rc::Rc;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::Frame;

use grafium_core::models::{Block, Link};

use crate::data::GraphRepository;
use crate::panels::{Panel, PanelAction};
use crate::widgets::list_panel::ListPanel;

pub struct RightSidebar {
    repo: Rc<dyn GraphRepository>,
    items: Vec<(Link, Block, String)>,
    panel: ListPanel,
    error: Option<String>,
}

impl RightSidebar {
    pub fn new(repo: Rc<dyn GraphRepository>) -> Self {
        Self {
            repo,
            items: Vec::new(),
            panel: ListPanel::new(),
            error: None,
        }
    }

    /// Reload backlinks for the page that just became active in the center panel.
    pub fn set_target_page(&mut self, page_id: &str) {
        self.panel.select(None);
        match self.repo.get_backlinks(page_id) {
            Ok(items) => {
                self.items = items;
                self.error = None;
            }
            Err(e) => {
                self.items = Vec::new();
                self.error = Some(e);
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
