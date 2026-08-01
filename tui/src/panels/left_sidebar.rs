//! Left sidebar: a single list panel that can show either "all pages" or
//! "journals". Both are just instances of the generic `PaginatedList<Page>`
//! with a different `PageSource` — no separate list widget code per tab.
//! Toggled between with the `g h` / `g j` leader sequences (handled by
//! `App`, which then calls `set_mode`).

use std::rc::Rc;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::Frame;

use grafium_core::models::Page;

use crate::data::GraphRepository;
use crate::data::sources::{AllPagesSource, JournalSource};
use crate::panels::{Panel, PanelAction};
use crate::widgets::paginated_list::PaginatedList;

const PAGE_SIZE: i64 = 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LeftSidebarMode {
    Pages,
    Journals,
}

pub struct LeftSidebar {
    mode: LeftSidebarMode,
    pages: PaginatedList<Page>,
    journals: PaginatedList<Page>,
}

impl LeftSidebar {
    pub fn new(repo: Rc<dyn GraphRepository>) -> Self {
        Self {
            mode: LeftSidebarMode::Pages,
            pages: PaginatedList::new(Box::new(AllPagesSource::new(repo.clone())), PAGE_SIZE),
            journals: PaginatedList::new(Box::new(JournalSource::new(repo)), PAGE_SIZE),
        }
    }

    pub fn set_mode(&mut self, mode: LeftSidebarMode) {
        self.mode = mode;
    }

    fn active(&mut self) -> &mut PaginatedList<Page> {
        match self.mode {
            LeftSidebarMode::Pages => &mut self.pages,
            LeftSidebarMode::Journals => &mut self.journals,
        }
    }
}

impl Panel for LeftSidebar {
    fn handle_key(&mut self, key: KeyEvent) -> PanelAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.active().move_selection(-1);
                PanelAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.active().move_selection(1);
                PanelAction::None
            }
            KeyCode::Enter => match self.active().selected() {
                Some(page) => PanelAction::OpenPage(page.id.clone()),
                None => PanelAction::None,
            },
            _ => PanelAction::None,
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        let title = match self.mode {
            LeftSidebarMode::Pages => "Pages (g h)",
            LeftSidebarMode::Journals => "Journals (g j)",
        };
        self.active()
            .render(f, area, title, |p: &Page| p.title.clone(), focused);
    }
}
