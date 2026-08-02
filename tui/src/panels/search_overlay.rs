//! Search overlay (bound to `/`). Unlike the sidebars, search is debounced and
//! fetched on a background thread so typing doesn't block the input loop.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block as RBlock, Borders, Paragraph};
use ratatui::Frame;

use grafium_core::models::Block;

use crate::data::GraphRepository;
use crate::panels::{Panel, PanelAction};
use crate::widgets::list_panel::ListPanel;
use crate::widgets::theme;

const PAGE_SIZE: i64 = 20;
const PREFETCH_THRESHOLD: usize = 3;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(180);
const SEARCH_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
struct SearchRequest {
    id: u64,
    revision: u64,
    query: String,
    limit: i64,
    offset: i64,
}

#[derive(Debug)]
struct SearchResponse {
    id: u64,
    revision: u64,
    offset: i64,
    result: Result<Vec<Block>, String>,
}

#[derive(Debug)]
struct SearchDebouncer {
    delay: Duration,
    pending_query: Option<String>,
    ready_at: Option<Instant>,
}

impl SearchDebouncer {
    fn new(delay: Duration) -> Self {
        Self {
            delay,
            pending_query: None,
            ready_at: None,
        }
    }

    fn clear(&mut self) {
        self.pending_query = None;
        self.ready_at = None;
    }

    fn schedule(&mut self, query: String, now: Instant) {
        self.pending_query = Some(query);
        self.ready_at = Some(now + self.delay);
    }

    fn take_ready(&mut self, now: Instant) -> Option<String> {
        match self.ready_at {
            Some(ready_at) if now >= ready_at => {
                self.ready_at = None;
                self.pending_query.take()
            }
            _ => None,
        }
    }

    fn next_timeout(&self, now: Instant) -> Option<Duration> {
        self.ready_at.map(|ready_at| {
            if ready_at <= now {
                Duration::ZERO
            } else {
                ready_at.duration_since(now)
            }
        })
    }

    fn is_pending(&self) -> bool {
        self.ready_at.is_some()
    }
}

pub struct SearchOverlay {
    repo: Arc<dyn GraphRepository>,
    query: String,
    results: Vec<Block>,
    panel: ListPanel,
    debouncer: SearchDebouncer,
    last_error: Option<String>,
    has_more: bool,
    loading: bool,
    next_offset: i64,
    revision: u64,
    next_request_id: u64,
    active_request: Option<SearchRequest>,
    result_tx: Sender<SearchResponse>,
    result_rx: Receiver<SearchResponse>,
}

impl SearchOverlay {
    pub fn new(repo: Arc<dyn GraphRepository>) -> Self {
        let (result_tx, result_rx) = mpsc::channel();
        Self {
            repo,
            query: String::new(),
            results: Vec::new(),
            panel: ListPanel::new(),
            debouncer: SearchDebouncer::new(SEARCH_DEBOUNCE),
            last_error: None,
            has_more: false,
            loading: false,
            next_offset: 0,
            revision: 0,
            next_request_id: 1,
            active_request: None,
            result_tx,
            result_rx,
        }
    }

    /// Reset to a blank query each time the overlay is (re)opened.
    pub fn reopen(&mut self) {
        self.query.clear();
        self.results.clear();
        self.panel.select(None);
        self.last_error = None;
        self.has_more = false;
        self.loading = false;
        self.next_offset = 0;
        self.revision = self.revision.wrapping_add(1);
        self.active_request = None;
        self.debouncer.clear();
        self.drain_results();
    }

    fn schedule_refresh(&mut self) {
        self.results.clear();
        self.panel.select(None);
        self.last_error = None;
        self.has_more = false;
        self.loading = false;
        self.next_offset = 0;
        self.revision = self.revision.wrapping_add(1);
        self.active_request = None;

        if self.query.trim().is_empty() {
            self.debouncer.clear();
        } else {
            self.debouncer.schedule(self.query.clone(), Instant::now());
        }
    }

    fn start_search(&mut self, offset: i64) {
        if self.query.trim().is_empty() {
            return;
        }

        let request = SearchRequest {
            id: self.next_request_id,
            revision: self.revision,
            query: self.query.clone(),
            limit: PAGE_SIZE,
            offset,
        };
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.loading = true;
        self.active_request = Some(request.clone());

        let repo = Arc::clone(&self.repo);
        let sender = self.result_tx.clone();
        std::thread::spawn(move || {
            let result = repo.search_blocks(&request.query, request.limit, request.offset);
            let _ = sender.send(SearchResponse {
                id: request.id,
                revision: request.revision,
                offset: request.offset,
                result,
            });
        });
    }

    fn maybe_load_more(&mut self) {
        if self.loading || !self.has_more {
            return;
        }

        if let Some(selected) = self.panel.selected() {
            if selected + PREFETCH_THRESHOLD >= self.results.len() {
                self.start_search(self.next_offset);
            }
        }
    }

    fn apply_response(&mut self, response: SearchResponse) -> bool {
        let Some(active_request) = self.active_request.as_ref() else {
            return false;
        };

        if response.revision != self.revision || response.id != active_request.id {
            return false;
        }

        self.loading = false;
        self.active_request = None;

        match response.result {
            Ok(results) => {
                let result_count = results.len();
                self.has_more = result_count as i64 == PAGE_SIZE;
                self.next_offset = response.offset + result_count as i64;
                if response.offset == 0 {
                    self.results = results;
                } else {
                    self.results.extend(results);
                }
                self.last_error = None;
                if let Some(selected) = self.panel.selected() {
                    if self.results.is_empty() {
                        self.panel.select(None);
                    } else if selected >= self.results.len() {
                        self.panel.select(Some(self.results.len() - 1));
                    }
                }
            }
            Err(error) => {
                if response.offset == 0 {
                    self.results.clear();
                    self.panel.select(None);
                }
                self.has_more = false;
                self.last_error = Some(error);
            }
        }

        true
    }

    fn drain_results(&mut self) {
        while self.result_rx.try_recv().is_ok() {}
    }

    pub fn poll_timeout(&self) -> Option<Duration> {
        let now = Instant::now();
        match (
            self.debouncer.next_timeout(now),
            self.loading.then_some(SEARCH_POLL_INTERVAL),
        ) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(timeout), None) | (None, Some(timeout)) => Some(timeout),
            (None, None) => None,
        }
    }

    pub fn tick(&mut self) -> bool {
        let mut dirty = false;
        let now = Instant::now();

        if self.debouncer.take_ready(now).is_some() {
            self.start_search(0);
            dirty = true;
        }

        loop {
            match self.result_rx.try_recv() {
                Ok(response) => {
                    dirty |= self.apply_response(response);
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }

        dirty
    }

    fn results_title(&self) -> String {
        if let Some(error) = &self.last_error {
            format!("Results (error: {error})")
        } else if self.loading {
            format!("Results [{} loading…]", self.results.len())
        } else if self.debouncer.is_pending() {
            format!("Results [{} waiting…]", self.results.len())
        } else {
            format!("Results [{}]", self.results.len())
        }
    }
}

impl Panel for SearchOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> PanelAction {
        match key.code {
            KeyCode::Char(c) => {
                self.query.push(c);
                self.schedule_refresh();
                PanelAction::None
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.schedule_refresh();
                PanelAction::None
            }
            KeyCode::Up => {
                self.panel.move_selection(-1, self.results.len());
                PanelAction::None
            }
            KeyCode::Down => {
                self.panel.move_selection(1, self.results.len());
                self.maybe_load_more();
                PanelAction::None
            }
            KeyCode::Enter => match self
                .panel
                .selected()
                .and_then(|selected| self.results.get(selected))
            {
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
        self.panel.render(
            f,
            chunks[1],
            &self.results_title(),
            &self.results,
            label,
            focused,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use grafium_core::models::{BlockType, Link, Page};

    use crate::data::RepoResult;

    #[derive(Default)]
    struct RecordingRepo {
        calls: Mutex<Vec<(String, i64, i64)>>,
    }

    impl RecordingRepo {
        fn calls(&self) -> Vec<(String, i64, i64)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl GraphRepository for RecordingRepo {
        fn list_pages(&self, _limit: i64, _offset: i64) -> RepoResult<Vec<Page>> {
            Ok(Vec::new())
        }

        fn list_journal_pages(&self, _limit: i64, _offset: i64) -> RepoResult<Vec<Page>> {
            Ok(Vec::new())
        }

        fn search_blocks(&self, query: &str, limit: i64, offset: i64) -> RepoResult<Vec<Block>> {
            self.calls
                .lock()
                .unwrap()
                .push((query.to_string(), limit, offset));

            let block = |index: i64| Block {
                id: format!("block-{index}"),
                page_id: format!("page-{index}"),
                parent_id: None,
                order_index: index as i32,
                content: format!("result {index}"),
                block_type: BlockType::Text,
                properties: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            };

            Ok(match offset {
                0 => (0..PAGE_SIZE).map(block).collect(),
                PAGE_SIZE => vec![block(PAGE_SIZE)],
                _ => Vec::new(),
            })
        }

        fn get_page_by_id(&self, _page_id: &str) -> RepoResult<Page> {
            Err("unused".to_string())
        }

        fn list_blocks_for_page(&self, _page_id: &str) -> RepoResult<Vec<Block>> {
            Ok(Vec::new())
        }

        fn update_block(&self, _block_id: &str, _content: &str) -> RepoResult<()> {
            Ok(())
        }

        fn create_block(
            &self,
            _page_id: &str,
            _order_index: i32,
            _content: &str,
        ) -> RepoResult<Block> {
            Err("unused".to_string())
        }

        fn get_backlinks(&self, _page_id: &str) -> RepoResult<Vec<(Link, Block, String)>> {
            Ok(Vec::new())
        }

        fn get_links_from_page(&self, _page_id: &str) -> RepoResult<Vec<Link>> {
            Ok(Vec::new())
        }

        fn get_or_create_today_journal(&self) -> RepoResult<Page> {
            Err("unused".to_string())
        }
    }

    fn apply_next_response(overlay: &mut SearchOverlay) {
        let response = overlay
            .result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("search response should arrive");
        assert!(overlay.apply_response(response));
    }

    #[test]
    fn debounce_coalesces_rapid_keystrokes() {
        let mut debouncer = SearchDebouncer::new(Duration::from_millis(180));
        let base = Instant::now();

        debouncer.schedule("g".to_string(), base);
        assert_eq!(
            debouncer.take_ready(base + Duration::from_millis(100)),
            None
        );

        debouncer.schedule("gr".to_string(), base + Duration::from_millis(100));
        debouncer.schedule("gra".to_string(), base + Duration::from_millis(150));

        assert_eq!(
            debouncer.take_ready(base + Duration::from_millis(320)),
            None
        );
        assert_eq!(
            debouncer.take_ready(base + Duration::from_millis(331)),
            Some("gra".to_string())
        );
        assert_eq!(
            debouncer.take_ready(base + Duration::from_millis(600)),
            None
        );
    }

    #[test]
    fn load_more_fetches_windowed_pages_and_appends_results() {
        let repo = Arc::new(RecordingRepo::default());
        let mut overlay = SearchOverlay::new(repo.clone());
        overlay.query = "graf".to_string();

        overlay.start_search(0);
        apply_next_response(&mut overlay);

        assert_eq!(overlay.results.len(), PAGE_SIZE as usize);
        assert!(overlay.has_more);
        assert_eq!(overlay.next_offset, PAGE_SIZE);
        assert_eq!(repo.calls(), vec![("graf".to_string(), PAGE_SIZE, 0)]);

        overlay
            .panel
            .select(Some(PAGE_SIZE as usize - PREFETCH_THRESHOLD));
        overlay.maybe_load_more();
        apply_next_response(&mut overlay);

        assert_eq!(overlay.results.len(), PAGE_SIZE as usize + 1);
        assert_eq!(overlay.results[0].id, "block-0");
        assert_eq!(
            overlay.results.last().map(|block| block.id.as_str()),
            Some("block-20")
        );
        assert!(!overlay.has_more);
        assert_eq!(overlay.next_offset, PAGE_SIZE + 1);
        assert_eq!(
            repo.calls(),
            vec![
                ("graf".to_string(), PAGE_SIZE, 0),
                ("graf".to_string(), PAGE_SIZE, PAGE_SIZE)
            ]
        );
    }
}
