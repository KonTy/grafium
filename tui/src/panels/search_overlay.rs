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
}

#[derive(Debug)]
struct SearchResponse {
    id: u64,
    revision: u64,
    limit: i64,
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
    requested_limit: i64,
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
            requested_limit: PAGE_SIZE,
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
        self.requested_limit = PAGE_SIZE;
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
        self.requested_limit = PAGE_SIZE;
        self.revision = self.revision.wrapping_add(1);
        self.active_request = None;

        if self.query.trim().is_empty() {
            self.debouncer.clear();
        } else {
            self.debouncer.schedule(self.query.clone(), Instant::now());
        }
    }

    fn start_search(&mut self, limit: i64) {
        if self.query.trim().is_empty() {
            return;
        }

        let request = SearchRequest {
            id: self.next_request_id,
            revision: self.revision,
            query: self.query.clone(),
            limit,
        };
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.requested_limit = limit;
        self.loading = true;
        self.active_request = Some(request.clone());

        let repo = Arc::clone(&self.repo);
        let sender = self.result_tx.clone();
        std::thread::spawn(move || {
            let result = repo.search_blocks(&request.query, request.limit);
            let _ = sender.send(SearchResponse {
                id: request.id,
                revision: request.revision,
                limit: request.limit,
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
                self.start_search(self.requested_limit + PAGE_SIZE);
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
                self.has_more = results.len() as i64 >= response.limit;
                self.results = results;
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
                self.results.clear();
                self.panel.select(None);
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
            self.start_search(PAGE_SIZE);
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
}
