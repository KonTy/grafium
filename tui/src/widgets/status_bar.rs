//! Bottom status/help bar — single place that renders the current status
//! message and the leader-key hint, reused by `App` regardless of which
//! panel is focused.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::widgets::theme;

const HELP: &str =
    "g h/j/g:nav  t l/r:toggle sidebars  /:search  Tab:focus  Enter:open/edit  Esc:back  q:quit";

pub fn render(f: &mut Frame, area: Rect, status: &str, pending_leader: Option<char>) {
    let mut spans = vec![Span::raw(status.to_string())];
    if let Some(leader) = pending_leader {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(format!("{leader}…"), theme::focused()));
    }
    spans.push(Span::raw("   "));
    spans.push(Span::styled(HELP, theme::muted()));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
