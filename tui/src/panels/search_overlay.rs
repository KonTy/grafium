//! Search overlay (bound to `/`). Reuses the same generic `PaginatedList`
//! used by the sidebars — this time backed by `SearchSource` — so "infinite
//! scroll" behaves identically for search results as it does for page lists.

use std::rc::Rc;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block as RBlock, Borders, Paragraph};
use ratatui::Frame;

use grafium_core::models::Block;

use crate::data::GraphRepository;
use crate::data::sources::SearchSource;
use crate::panels::{Panel, PanelAction};
use crate::widgets::paginated_list::PaginatedList;
use crate::widgets::theme;

const PAGE_SIZE: i64 = 20;

pub struct SearchOverlay {
    repo: Rc<dyn GraphRepository>,
    query: String,
    results: PaginatedList<Block>,
}

impl SearchOverlay {
    pub fn new(repo: Rc<dyn GraphRepository>) -> Self {
        let results = PaginatedList::new(
            Box::new(SearchSource::new(repo.clone(), String::new())),
            PAGE_SIZE,
        );
        Self {
            repo,
            query: String::new(),
            results,
        }
    }

    /// Reset to a blank query each time the overlay is (re)opened.
    pub fn reopen(&mut self) {
        self.query.clear();
        self.results
            .reset(Box::new(SearchSource::new(self.repo.clone(), String::new())));
    }

    fn refresh(&mut self) {
        self.results.reset(Box::new(SearchSource::new(
            self.repo.clone(),
            self.query.clone(),
        )));
    }
}

impl Panel for SearchOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> PanelAction {
        match key.code {
            KeyCode::Char(c) => {
                self.query.push(c);
                self.refresh();
                PanelAction::None
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refresh();
                PanelAction::None
            }
            KeyCode::Up => {
                self.results.move_selection(-1);
                PanelAction::None
            }
            KeyCode::Down => {
                self.results.move_selection(1);
                PanelAction::None
            }
            KeyCode::Enter => match self.results.selected() {
                Some(block) => PanelAction::OpenPage(block.page_id.clone()),
                None => PanelAction::None,
            },
            _ => PanelAction::None,
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .split(area);

        let input = Paragraph::new(self.query.as_str()).block(
            RBlock::default()
                .title("Search (/)")
                .borders(Borders::ALL)
                .border_style(theme::focused()),
        );
        f.render_widget(input, chunks[0]);

        let label = |b: &Block| {
            let snippet: String = b.content.chars().take(72).collect();
            snippet.replace('\n', " ")
        };
        self.results
            .render(f, chunks[1], "Results", label, focused);
    }
}
