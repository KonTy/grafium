//! Center panel: the page/block editor (default) and a lightweight text
//! "graph" view of outgoing links (`g g`), which pairs with the right
//! sidebar's backlinks so together they cover both edge directions without
//! either panel duplicating the other's data or rendering code.

use std::rc::Rc;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
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
    pub fn open_page(&mut self, page_id: &str) {
        self.save_pending_edit();
        self.mode = CenterMode::Blocks;
        match (
            self.repo.get_page_by_id(page_id),
            self.repo.list_blocks_for_page(page_id),
        ) {
            (Ok(page), Ok(blocks)) => {
                self.page = Some(page);
                self.blocks = blocks;
                self.block_list.select(if self.blocks.is_empty() { None } else { Some(0) });
                self.scroll_offset = 0;
                self.error = None;
            }
            (Err(e), _) | (_, Err(e)) => {
                self.error = Some(e);
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

    fn save_pending_edit(&mut self) -> Option<String> {
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
                    Some("Saved.".to_string())
                }
                Ok(false) => None,
                Err(e) => Some(format!("Save failed: {e}")),
            };
        }
        None
    }

    fn handle_blocks_mode_key(&mut self, key: KeyEvent) -> PanelAction {
        if self.editor.is_editing() {
            if key.code == KeyCode::Esc {
                return match self.save_pending_edit() {
                    Some(msg) => PanelAction::Status(msg),
                    None => PanelAction::None,
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
            f.render_widget(Paragraph::new("Empty page — press 'n' to add a block."), area);
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

        let editing_id = self.editor.is_editing().then(|| self.editor.block_id().map(str::to_string)).flatten();
        let mut y = area.y;
        let bottom = area.y + area.height;
        for (i, block) in self.blocks.iter().enumerate().skip(self.scroll_offset) {
            if y >= bottom {
                break;
            }
            let is_editing_this = editing_id.as_deref() == Some(block.id.as_str());
            let height = self.block_height(block).min(bottom - y);
            let rect = Rect { x: area.x, y, width: area.width, height };

            if is_editing_this {
                self.editor.render(f, rect);
            } else {
                let is_selected = i == selected;
                let marker_style = if is_selected && focused { theme::selected() } else { Style::default() };
                let marker = if is_selected { "▌" } else { " " };
                let mut lines = render_markdown(&block.content);
                for line in &mut lines {
                    line.spans.insert(0, Span::styled(format!("{marker} "), marker_style));
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
        let Some(page) = &self.page else {
            f.render_widget(
                Paragraph::new("No page open — pick one on the left (g h / g j).")
                    .block(RBlock::default().borders(Borders::ALL).border_style(theme::unfocused())),
                area,
            );
            return;
        };
        let title = format!(
            "{} ({}) [{}] — Enter:edit  n:new  Esc:save",
            page.title,
            if page.is_journal { "journal" } else { "page" },
            self.blocks.len()
        );

        let outer = RBlock::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if focused { theme::focused() } else { theme::unfocused() });
        let inner = outer.inner(area);
        f.render_widget(outer, area);

        match self.mode {
            CenterMode::Blocks => self.draw_document(f, inner, focused),
            CenterMode::Graph => {
                let label = |(_, t): &(Link, String)| t.clone();
                self.outlinks_panel.render(
                    f,
                    inner,
                    "Outgoing links (g g to go back)",
                    &self.outlinks,
                    label,
                    focused,
                );
            }
        }
    }
}
