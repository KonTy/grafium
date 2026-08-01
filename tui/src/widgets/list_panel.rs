//! A single, generic, stateful list widget used everywhere a scrollable list
//! of items is needed (all pages, journals, search hits, backlinks, blocks).
//! Rendering is parameterised by a `label` closure so the same widget code
//! is never duplicated per data type.

use ratatui::layout::Rect;
use ratatui::widgets::{Block as RBlock, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::widgets::theme;

#[derive(Default)]
pub struct ListPanel {
    state: ListState,
}

impl ListPanel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn selected(&self) -> Option<usize> {
        self.state.selected()
    }

    pub fn select(&mut self, index: Option<usize>) {
        self.state.select(index);
    }

    /// Moves the selection by `delta` (negative = up), clamped to `len`.
    /// Returns the new selected index, if any.
    pub fn move_selection(&mut self, delta: i32, len: usize) -> Option<usize> {
        if len == 0 {
            self.state.select(None);
            return None;
        }
        let current = self.state.selected().unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, len as i32 - 1) as usize;
        self.state.select(Some(next));
        Some(next)
    }

    pub fn render<T>(
        &mut self,
        f: &mut Frame,
        area: Rect,
        title: &str,
        items: &[T],
        label: impl Fn(&T) -> String,
        focused: bool,
    ) {
        if self.state.selected().is_none() && !items.is_empty() {
            self.state.select(Some(0));
        }
        let list_items: Vec<ListItem> = items.iter().map(|it| ListItem::new(label(it))).collect();
        let border_style = if focused {
            theme::focused()
        } else {
            theme::unfocused()
        };
        let block = RBlock::default()
            .title(title.to_string())
            .borders(Borders::ALL)
            .border_style(border_style);
        let list = List::new(list_items)
            .block(block)
            .highlight_style(theme::selected())
            .highlight_symbol("> ");
        f.render_stateful_widget(list, area, &mut self.state);
    }
}
