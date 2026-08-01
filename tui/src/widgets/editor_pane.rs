//! The one text-editing widget used everywhere a block's markdown source
//! needs to be edited. Wraps `ratatui_textarea::TextArea` (so cursor
//! movement, undo/redo, selection, unicode handling are not reimplemented)
//! and adds the domain behaviour Grafium needs: normal/insert modes, dirty
//! tracking, and saving through the `GraphRepository` seam.

use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block as RBlock, Borders};
use ratatui::Frame;
use ratatui_textarea::TextArea;

use crate::data::GraphRepository;
use crate::widgets::theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditorMode {
    /// Not capturing keystrokes; arrow/movement keys navigate the app instead.
    Normal,
    /// Capturing keystrokes for the textarea.
    Insert,
}

pub struct EditorPane<'a> {
    textarea: TextArea<'a>,
    block_id: Option<String>,
    mode: EditorMode,
    dirty: bool,
}

impl<'a> EditorPane<'a> {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default());
        textarea.set_placeholder_text("Select a block and press Enter to edit");
        Self {
            textarea,
            block_id: None,
            mode: EditorMode::Normal,
            dirty: false,
        }
    }

    pub fn is_editing(&self) -> bool {
        self.mode == EditorMode::Insert
    }

    /// Loads a block's content into the editor, discarding any unsaved edits
    /// for whatever was previously loaded (callers must save before switching).
    pub fn load(&mut self, block_id: &str, content: &str) {
        let lines: Vec<String> = content.split('\n').map(str::to_string).collect();
        self.textarea = TextArea::new(lines);
        self.block_id = Some(block_id.to_string());
        self.dirty = false;
    }

    pub fn enter_insert_mode(&mut self) {
        if self.block_id.is_some() {
            self.mode = EditorMode::Insert;
        }
    }

    pub fn content(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// How many rows the raw source currently needs — used by the document
    /// view to size this block's slot while it's being edited (it grows as
    /// the user adds lines, shrinks as they remove them).
    pub fn line_count(&self) -> usize {
        self.textarea.lines().len().max(1)
    }

    pub fn block_id(&self) -> Option<&str> {
        self.block_id.as_deref()
    }

    /// Feeds a key event to the underlying textarea. Only meaningful while
    /// `mode() == EditorMode::Insert`.
    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.textarea.input(key) {
            self.dirty = true;
        }
    }

    /// Leaves insert mode and persists the content via the repository if it
    /// changed. Returns `Ok(true)` if a save actually happened.
    pub fn exit_and_save(&mut self, repo: &dyn GraphRepository) -> Result<bool, String> {
        self.mode = EditorMode::Normal;
        if !self.dirty {
            return Ok(false);
        }
        let Some(id) = self.block_id.clone() else {
            return Ok(false);
        };
        repo.update_block(&id, &self.content())?;
        self.dirty = false;
        Ok(true)
    }

    /// Renders the raw, editable source inline (no full border box — this
    /// widget is embedded directly in the document flow alongside rendered
    /// blocks, not shown in a separate pane). A thin top rule with a hint is
    /// the only chrome, styled purely with modifiers per `widgets::theme`.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.textarea.set_block(
            RBlock::default()
                .borders(Borders::TOP)
                .title("raw markdown — Esc to save & render")
                .border_style(theme::focused()),
        );
        f.render_widget(&self.textarea, area);
    }
}
