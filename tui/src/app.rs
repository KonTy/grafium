//! Application shell: owns every panel, the visible/focus state, and the
//! `g`/`t` leader-key state machine (mirrors the app's documented shortcuts:
//! `g h`/`g j`/`g g` to navigate, `t l`/`t r` to toggle sidebars, `/` to
//! search). Panels never talk to each other directly — they return a
//! `PanelAction` and only `App` interprets it, so each panel stays a small,
//! independently reasoned-about unit.

use std::sync::Arc;
use std::time::Duration;

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use crate::data::GraphRepository;
use crate::panels::center::CenterPanel;
use crate::panels::left_sidebar::{LeftSidebar, LeftSidebarMode};
use crate::panels::right_sidebar::RightSidebar;
use crate::panels::search_overlay::SearchOverlay;
use crate::panels::{Panel, PanelAction};
use crate::widgets::status_bar;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Left,
    Center,
    Right,
}

pub struct App {
    repo: Arc<dyn GraphRepository>,
    left: LeftSidebar,
    center: CenterPanel,
    right: RightSidebar,
    search: SearchOverlay,

    focus: Focus,
    left_visible: bool,
    right_visible: bool,
    search_open: bool,

    /// First key of a two-key `g ...` / `t ...` shortcut, waiting for its
    /// second key.
    pending_leader: Option<char>,
    status: String,
    pub should_quit: bool,
}

impl App {
    pub fn new(repo: Arc<dyn GraphRepository>) -> Self {
        let mut app = Self {
            left: LeftSidebar::new(repo.clone()),
            center: CenterPanel::new(repo.clone()),
            right: RightSidebar::new(repo.clone()),
            search: SearchOverlay::new(repo.clone()),
            repo,
            focus: Focus::Left,
            left_visible: true,
            right_visible: true,
            search_open: false,
            pending_leader: None,
            status: "Welcome to Grafium TUI".to_string(),
            should_quit: false,
        };
        // Open today's journal by default, same as the desktop app landing on
        // the journal/home view.
        if let Ok(page) = app.repo.get_or_create_today_journal() {
            app.open_page(&page.id);
        }
        app
    }

    pub fn handle_event(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) => {
                self.on_key(key);
                true
            }
            Event::Resize(_, _) => true,
            _ => false,
        }
    }

    pub fn poll_timeout(&self) -> Duration {
        if self.search_open {
            self.search
                .poll_timeout()
                .unwrap_or_else(|| Duration::from_secs(1))
        } else {
            Duration::from_secs(60)
        }
    }

    pub fn tick(&mut self) -> bool {
        if self.search_open {
            self.search.tick()
        } else {
            false
        }
    }

    fn open_page(&mut self, page_id: &str) {
        match self.center.open_page(page_id) {
            Ok(()) => {
                self.right.set_target_page(page_id);
                self.focus = Focus::Center;
            }
            Err(message) => {
                self.status = message;
                self.focus = Focus::Center;
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        if self.search_open {
            self.handle_search_key(key);
            return;
        }

        // The editor and the search box need to consume raw characters
        // ('g', 't', 'q', 'j', 'k'...) as text, so leader shortcuts and
        // global keys only apply when nothing is capturing free-form input.
        let capturing_text = self.focus == Focus::Center && self.center.is_editing();

        if !capturing_text {
            if let Some(leader) = self.pending_leader.take() {
                self.handle_leader_sequence(leader, key);
                return;
            }
            match key.code {
                KeyCode::Char('g') | KeyCode::Char('t') => {
                    self.pending_leader = Some(if key.code == KeyCode::Char('g') {
                        'g'
                    } else {
                        't'
                    });
                    return;
                }
                KeyCode::Char('/') => {
                    self.search_open = true;
                    self.search.reopen();
                    return;
                }
                KeyCode::Tab => {
                    self.cycle_focus();
                    return;
                }
                KeyCode::Char('q') => {
                    self.should_quit = true;
                    return;
                }
                _ => {}
            }
        }

        self.dispatch_to_focused(key);
    }

    fn handle_leader_sequence(&mut self, leader: char, key: KeyEvent) {
        match (leader, key.code) {
            ('g', KeyCode::Char('h')) => {
                self.left.set_mode(LeftSidebarMode::Pages);
                self.left_visible = true;
                self.focus = Focus::Left;
                self.status = "Pages".to_string();
            }
            ('g', KeyCode::Char('j')) => {
                self.left.set_mode(LeftSidebarMode::Journals);
                self.left_visible = true;
                self.focus = Focus::Left;
                self.status = "Journals".to_string();
            }
            ('g', KeyCode::Char('g')) => {
                self.center.toggle_graph_mode();
                self.focus = Focus::Center;
                self.status = "Graph view (outgoing links)".to_string();
            }
            ('g', KeyCode::Char('f')) => {
                self.status = "Flashcards aren't implemented in the TUI yet".to_string();
            }
            ('t', KeyCode::Char('l')) => {
                self.left_visible = !self.left_visible;
                if !self.left_visible && self.focus == Focus::Left {
                    self.focus = Focus::Center;
                }
            }
            ('t', KeyCode::Char('r')) => {
                self.right_visible = !self.right_visible;
                self.right.set_visible(self.right_visible);
                if !self.right_visible && self.focus == Focus::Right {
                    self.focus = Focus::Center;
                }
            }
            _ => {
                self.status = format!("Unknown shortcut: {leader} {:?}", key.code);
            }
        }
    }

    fn cycle_focus(&mut self) {
        let order = [Focus::Left, Focus::Center, Focus::Right];
        let visible = |f: &Focus| match f {
            Focus::Left => self.left_visible,
            Focus::Center => true,
            Focus::Right => self.right_visible,
        };
        let start = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        for step in 1..=order.len() {
            let candidate = order[(start + step) % order.len()];
            if visible(&candidate) {
                self.focus = candidate;
                break;
            }
        }
    }

    fn dispatch_to_focused(&mut self, key: KeyEvent) {
        let action = match self.focus {
            Focus::Left => self.left.handle_key(key),
            Focus::Center => self.center.handle_key(key),
            Focus::Right => self.right.handle_key(key),
        };
        self.apply_action(action);
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.search_open = false;
            self.focus = Focus::Center;
            return;
        }
        let action = self.search.handle_key(key);
        if matches!(action, PanelAction::OpenPage(_)) {
            self.search_open = false;
        }
        self.apply_action(action);
    }

    fn apply_action(&mut self, action: PanelAction) {
        match action {
            PanelAction::None => {}
            PanelAction::OpenPage(id) => self.open_page(&id),
            PanelAction::Status(msg) => self.status = msg,
        }
    }

    pub fn draw(&mut self, f: &mut Frame) {
        let root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(f.area());

        if self.search_open {
            self.search.draw(f, root[0], true);
        } else {
            self.draw_workspace(f, root[0]);
        }

        status_bar::render(f, root[1], &self.status, self.pending_leader);
    }

    fn draw_workspace(&mut self, f: &mut Frame, area: Rect) {
        let mut constraints = Vec::new();
        if self.left_visible {
            constraints.push(Constraint::Percentage(22));
        }
        constraints.push(Constraint::Min(20));
        if self.right_visible {
            constraints.push(Constraint::Percentage(28));
        }
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(area);

        let mut idx = 0;
        if self.left_visible {
            self.left.draw(f, chunks[idx], self.focus == Focus::Left);
            idx += 1;
        }
        self.center
            .draw(f, chunks[idx], self.focus == Focus::Center);
        idx += 1;
        if self.right_visible {
            self.right.draw(f, chunks[idx], self.focus == Focus::Right);
        }
    }
}
