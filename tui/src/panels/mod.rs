//! Cross-panel contract. Panels never call into each other directly; they
//! report an intent via `PanelAction` and `App` (the only place that knows
//! about every panel) decides what to do with it. This keeps each panel a
//! self-contained, independently testable unit.

use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

pub enum PanelAction {
    None,
    /// Ask the app to open this page in the center panel.
    OpenPage(String),
    /// Ask the app to show this message in the status bar.
    Status(String),
}

pub trait Panel {
    fn handle_key(&mut self, key: KeyEvent) -> PanelAction;
    fn draw(&mut self, f: &mut Frame, area: Rect, focused: bool);
}

pub mod center;
pub mod left_sidebar;
pub mod right_sidebar;
pub mod search_overlay;
