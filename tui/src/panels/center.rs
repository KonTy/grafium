//! Center panel: the page/block editor (default) and a lightweight text
//! "graph" view of outgoing links (`g g`), which pairs with the right
//! sidebar's backlinks so together they cover both edge directions without
//! either panel duplicating the other's data or rendering code.

use std::rc::Rc;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block as RBlock, Borders, Paragraph, Wrap};
use ratatui::Frame;

use grafium_core::models::{Block, Link, Page};

use crate::data::GraphRepository;
use crate::panels::{Panel, PanelAction};
use crate::widgets::editor_pane::EditorPane;
use crate::widgets::list_panel::ListPanel;
use crate::widgets::markdown_view::render_markdown;
use crate::widgets::theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CenterMode {
    Blocks,
    Graph,
}

pub struct CenterPanel {
    repo: Rc<dyn GraphRepository>,
    mode: CenterMode,
    page: Option<Page>,
    blocks: Vec<Block>,
    block_list: ListPanel,
    /// Index of the first block currently visible in the document view
    /// (block-granularity scrolling, not pixel-granularity).
    scroll_offset: usize,
    editor: EditorPane<'static>,
    outlinks: Vec<(Link, String)>,
    outlinks_panel: ListPanel,
    error: Option<String>,
}

impl CenterPanel {
    pub fn new(repo: Rc<dyn GraphRepository>) -> Self {
        Self {
            repo,
            mode: CenterMode::Blocks,
            page: None,
            blocks: Vec::new(),
            block_list: ListPanel::new(),
            scroll_offset: 0,
            editor: EditorPane::new(),
            outlinks: Vec::new(),
            outlinks_panel: ListPanel::new(),
            error: None,
        }
    }

    pub fn current_page_id(&self) -> Option<String> {
        self.page.as_ref().map(|p| p.id.clone())
    }

    pub fn is_editing(&self) -> bool {
        self.editor.is_editing()
    }

    /// Loads a page's blocks into the center panel, saving any pending edit
    /// on the previously open page first.
    pub fn open_page(&mut self, page_id: &str) -> Result<(), String> {
        if let Err(err) = self.save_pending_edit() {
            let blocking_error =
                format!("Cannot open another page until the current edit saves: {err}");
            self.error = Some(blocking_error.clone());
            return Err(blocking_error);
        }
        self.mode = CenterMode::Blocks;
        match (
            self.repo.get_page_by_id(page_id),
            self.repo.list_blocks_for_page(page_id),
        ) {
            (Ok(page), Ok(blocks)) => {
                self.page = Some(page);
                self.blocks = blocks;
                self.block_list.select(if self.blocks.is_empty() {
                    None
                } else {
                    Some(0)
                });
                self.scroll_offset = 0;
                self.error = None;
                Ok(())
            }
            (Err(e), _) | (_, Err(e)) => {
                self.error = Some(e.clone());
                Err(e)
            }
        }
    }

    pub fn toggle_graph_mode(&mut self) {
        self.mode = match self.mode {
            CenterMode::Blocks => {
                self.load_outlinks();
                CenterMode::Graph
            }
            CenterMode::Graph => CenterMode::Blocks,
        };
    }

    fn load_outlinks(&mut self) {
        let Some(page_id) = self.current_page_id() else {
            self.outlinks = Vec::new();
            return;
        };
        self.outlinks = self
            .repo
            .get_links_from_page(&page_id)
            .unwrap_or_default()
            .into_iter()
            .map(|link| {
                let title = self
                    .repo
                    .get_page_by_id(&link.to_page_id)
                    .map(|p| p.title)
                    .unwrap_or_else(|_| link.to_page_id.clone());
                (link, title)
            })
            .collect();
        self.outlinks_panel.select(None);
    }

    fn save_pending_edit(&mut self) -> Result<Option<String>, String> {
        if self.editor.is_editing() {
            let block_id = self.editor.block_id().map(str::to_string);
            let new_content = self.editor.content();
            return match self.editor.exit_and_save(self.repo.as_ref()) {
                Ok(true) => {
                    if let Some(id) = block_id {
                        if let Some(b) = self.blocks.iter_mut().find(|b| b.id == id) {
                            b.content = new_content;
                        }
                    }
                    self.error = None;
                    Ok(Some("Saved.".to_string()))
                }
                Ok(false) => {
                    self.error = None;
                    Ok(None)
                }
                Err(e) => {
                    let msg = format!("Save failed: {e}");
                    self.error = Some(msg.clone());
                    Err(msg)
                }
            };
        }
        Ok(None)
    }

    fn panel_title(&self) -> String {
        let base = match &self.page {
            Some(page) => format!(
                "{} ({}) [{}] — Enter:edit  n:new  Esc:save",
                page.title,
                if page.is_journal { "journal" } else { "page" },
                self.blocks.len()
            ),
            None => "Center — Enter:edit  n:new  Esc:save".to_string(),
        };

        match &self.error {
            Some(err) => format!("{base} [error: {err}]"),
            None => base,
        }
    }

    fn error_banner_text(&self) -> Option<String> {
        self.error.as_ref().map(|err| format!("Error: {err}"))
    }

    fn handle_blocks_mode_key(&mut self, key: KeyEvent) -> PanelAction {
        if self.editor.is_editing() {
            if key.code == KeyCode::Esc {
                return match self.save_pending_edit() {
                    Ok(Some(msg)) => PanelAction::Status(msg),
                    Ok(None) => PanelAction::None,
                    Err(msg) => PanelAction::Status(msg),
                };
            }
            self.editor.handle_key(key);
            return PanelAction::None;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.block_list.move_selection(-1, self.blocks.len());
                PanelAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.block_list.move_selection(1, self.blocks.len());
                PanelAction::None
            }
            KeyCode::Enter => {
                if let Some(idx) = self.block_list.selected() {
                    if let Some(block) = self.blocks.get(idx) {
                        self.editor.load(&block.id, &block.content);
                        self.editor.enter_insert_mode();
                    }
                }
                PanelAction::None
            }
            KeyCode::Char('n') => {
                let Some(page_id) = self.current_page_id() else {
                    return PanelAction::None;
                };
                let order = self.blocks.len() as i32;
                match self.repo.create_block(&page_id, order, "") {
                    Ok(block) => {
                        self.blocks.push(block.clone());
                        self.block_list.select(Some(self.blocks.len() - 1));
                        self.editor.load(&block.id, &block.content);
                        self.editor.enter_insert_mode();
                        PanelAction::None
                    }
                    Err(e) => PanelAction::Status(format!("Create block failed: {e}")),
                }
            }
            _ => PanelAction::None,
        }
    }

    fn handle_graph_mode_key(&mut self, key: KeyEvent) -> PanelAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.outlinks_panel.move_selection(-1, self.outlinks.len());
                PanelAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.outlinks_panel.move_selection(1, self.outlinks.len());
                PanelAction::None
            }
            KeyCode::Enter => self
                .outlinks_panel
                .selected()
                .and_then(|i| self.outlinks.get(i))
                .map(|(link, _)| PanelAction::OpenPage(link.to_page_id.clone()))
                .unwrap_or(PanelAction::None),
            _ => PanelAction::None,
        }
    }

    /// Height (in terminal rows) this block currently needs: the raw
    /// textarea's line count while it's being edited, or the rendered
    /// markdown's line count otherwise. Used to lay out the document flow.
    fn block_height(&self, block: &Block) -> u16 {
        if self.editor.is_editing() && self.editor.block_id() == Some(block.id.as_str()) {
            self.editor.line_count() as u16
        } else {
            render_markdown(&block.content).len().max(1) as u16
        }
    }

    /// Ensures the selected block is visible by adjusting `scroll_offset`,
    /// then renders each visible block either as live raw source (the one
    /// being edited) or as read-only rendered markdown — exactly the
    /// desktop editor's "focused: raw, blurred: rendered" behaviour, but
    /// applied per block instead of splitting the screen into a separate
    /// list pane and preview pane.
    fn draw_document(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        if self.blocks.is_empty() {
            f.render_widget(
                Paragraph::new("Empty page — press 'n' to add a block."),
                area,
            );
            return;
        }

        let selected = self.block_list.selected().unwrap_or(0);
        if selected < self.scroll_offset {
            self.scroll_offset = selected;
        }
        // Grow the offset until the selected block's bottom edge fits within `area`.
        loop {
            let used: u16 = self.blocks[self.scroll_offset..=selected]
                .iter()
                .map(|b| self.block_height(b))
                .sum();
            if used <= area.height || self.scroll_offset >= selected {
                break;
            }
            self.scroll_offset += 1;
        }

        let editing_id = self
            .editor
            .is_editing()
            .then(|| self.editor.block_id().map(str::to_string))
            .flatten();
        let mut y = area.y;
        let bottom = area.y + area.height;
        for (i, block) in self.blocks.iter().enumerate().skip(self.scroll_offset) {
            if y >= bottom {
                break;
            }
            let is_editing_this = editing_id.as_deref() == Some(block.id.as_str());
            let height = self.block_height(block).min(bottom - y);
            let rect = Rect {
                x: area.x,
                y,
                width: area.width,
                height,
            };

            if is_editing_this {
                self.editor.render(f, rect);
            } else {
                let is_selected = i == selected;
                let marker_style = if is_selected && focused {
                    theme::selected()
                } else {
                    Style::default()
                };
                let marker = if is_selected { "▌" } else { " " };
                let mut lines = render_markdown(&block.content);
                for line in &mut lines {
                    line.spans
                        .insert(0, Span::styled(format!("{marker} "), marker_style));
                }
                f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), rect);
            }
            y += height;
        }
    }
}

impl Panel for CenterPanel {
    fn handle_key(&mut self, key: KeyEvent) -> PanelAction {
        match self.mode {
            CenterMode::Blocks => self.handle_blocks_mode_key(key),
            CenterMode::Graph => self.handle_graph_mode_key(key),
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        let outer = RBlock::default()
            .title(self.panel_title())
            .borders(Borders::ALL)
            .border_style(if focused {
                theme::focused()
            } else {
                theme::unfocused()
            });
        let inner = outer.inner(area);
        f.render_widget(outer, area);

        let content_area = if let Some(error) = self.error_banner_text() {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(inner);
            f.render_widget(Paragraph::new(error), chunks[0]);
            chunks[1]
        } else {
            inner
        };

        let Some(_) = &self.page else {
            f.render_widget(
                Paragraph::new("No page open — pick one on the left (g h / g j)."),
                content_area,
            );
            return;
        };

        match self.mode {
            CenterMode::Blocks => self.draw_document(f, content_area, focused),
            CenterMode::Graph => {
                let label = |(_, t): &(Link, String)| t.clone();
                self.outlinks_panel.render(
                    f,
                    content_area,
                    "Outgoing links (g g to go back)",
                    &self.outlinks,
                    label,
                    focused,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;
    use std::collections::HashMap;

    use grafium_core::models::BlockType;
    use ratatui::crossterm::event::KeyModifiers;

    use crate::data::RepoResult;

    struct MockRepo {
        pages: HashMap<String, Page>,
        blocks: RefCell<HashMap<String, Vec<Block>>>,
        update_error: RefCell<Option<String>>,
    }

    impl MockRepo {
        fn new(pages: Vec<Page>, blocks: Vec<Block>) -> Self {
            let mut pages_by_id = HashMap::new();
            for page in pages {
                pages_by_id.insert(page.id.clone(), page);
            }

            let mut blocks_by_page = HashMap::new();
            for block in blocks {
                blocks_by_page
                    .entry(block.page_id.clone())
                    .or_insert_with(Vec::new)
                    .push(block);
            }

            Self {
                pages: pages_by_id,
                blocks: RefCell::new(blocks_by_page),
                update_error: RefCell::new(None),
            }
        }

        fn fail_updates_with(&self, message: &str) {
            *self.update_error.borrow_mut() = Some(message.to_string());
        }
    }

    impl GraphRepository for MockRepo {
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
            self.pages
                .get(page_id)
                .cloned()
                .ok_or_else(|| format!("missing page: {page_id}"))
        }

        fn list_blocks_for_page(&self, page_id: &str) -> RepoResult<Vec<Block>> {
            self.blocks
                .borrow()
                .get(page_id)
                .cloned()
                .ok_or_else(|| format!("missing blocks for page: {page_id}"))
        }

        fn update_block(&self, block_id: &str, content: &str) -> RepoResult<()> {
            if let Some(err) = self.update_error.borrow().clone() {
                return Err(err);
            }

            for blocks in self.blocks.borrow_mut().values_mut() {
                if let Some(block) = blocks.iter_mut().find(|block| block.id == block_id) {
                    block.content = content.to_string();
                    return Ok(());
                }
            }

            Err(format!("missing block: {block_id}"))
        }

        fn create_block(
            &self,
            _page_id: &str,
            _order_index: i32,
            _content: &str,
        ) -> RepoResult<Block> {
            panic!("create_block should not be called in this test")
        }

        fn get_backlinks(&self, _page_id: &str) -> RepoResult<Vec<(Link, Block, String)>> {
            Ok(Vec::new())
        }

        fn get_links_from_page(&self, _page_id: &str) -> RepoResult<Vec<Link>> {
            Ok(Vec::new())
        }

        fn get_or_create_today_journal(&self) -> RepoResult<Page> {
            Err("unused in test".to_string())
        }
    }

    fn page(id: &str, title: &str) -> Page {
        Page {
            id: id.to_string(),
            title: title.to_string(),
            file_path: None,
            created_at: 0,
            updated_at: 0,
            is_journal: false,
            properties: serde_json::json!({}),
        }
    }

    fn block(id: &str, page_id: &str, content: &str) -> Block {
        Block {
            id: id.to_string(),
            page_id: page_id.to_string(),
            parent_id: None,
            order_index: 0,
            content: content.to_string(),
            block_type: BlockType::Text,
            properties: serde_json::json!({}),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn char_key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
    }

    #[test]
    fn failed_save_blocks_navigation_and_exposes_error_for_rendering() {
        let repo = Rc::new(MockRepo::new(
            vec![page("page-1", "Page One"), page("page-2", "Page Two")],
            vec![block("block-1", "page-1", "draft")],
        ));
        repo.fail_updates_with("disk full");

        let mut panel = CenterPanel::new(repo);
        panel.open_page("page-1").unwrap();

        assert!(matches!(
            panel.handle_key(key(KeyCode::Enter)),
            PanelAction::None
        ));
        assert!(panel.is_editing());

        assert!(matches!(panel.handle_key(char_key('!')), PanelAction::None));
        let save_action = panel.handle_key(key(KeyCode::Esc));
        let save_error = match save_action {
            PanelAction::Status(msg) => msg,
            PanelAction::None => panic!("save failure should surface a status message"),
            PanelAction::OpenPage(id) => panic!("unexpected navigation to {id}"),
        };

        assert_eq!(save_error, "Save failed: disk full");
        assert!(
            panel.is_editing(),
            "editor should stay in insert mode after a failed save"
        );
        assert_eq!(panel.editor.content(), "!draft");
        assert_eq!(panel.error.as_deref(), Some("Save failed: disk full"));
        assert_eq!(
            panel.error_banner_text().as_deref(),
            Some("Error: Save failed: disk full")
        );

        let navigation_error = panel
            .open_page("page-2")
            .expect_err("navigation should be blocked while the save is still failing");

        assert_eq!(panel.current_page_id().as_deref(), Some("page-1"));
        assert!(
            panel.is_editing(),
            "blocked navigation must preserve the in-progress edit"
        );
        assert_eq!(panel.editor.content(), "!draft");
        assert!(
            navigation_error.contains("Cannot open another page until the current edit saves"),
            "unexpected navigation error: {navigation_error}"
        );
        assert!(
            panel.panel_title().contains("[error: Cannot open another page until the current edit saves: Save failed: disk full]"),
            "title should surface the blocking error: {}",
            panel.panel_title()
        );
    }
}
