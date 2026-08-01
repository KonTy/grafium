//! Read-only markdown -> styled terminal text conversion, shared by every
//! place that needs to show rendered markdown (the page/block preview and
//! the graph view's link previews). Written once so no screen re-implements
//! its own markdown-to-text logic.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::widgets::theme;

/// Renders a single markdown string (one block's content) into styled lines
/// ready to hand to a `Paragraph` widget.
pub fn render_markdown(source: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut style_stack: Vec<Style> = vec![Style::default()];
    let mut in_code_block = false;

    macro_rules! flush_line {
        () => {
            lines.push(Line::from(std::mem::take(&mut current)));
        };
    }

    for event in Parser::new(source) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    style_stack.push(theme::heading(level == HeadingLevel::H1));
                }
                Tag::Strong => {
                    let base = *style_stack.last().unwrap();
                    style_stack.push(base.patch(theme::heading(false)));
                }
                Tag::Emphasis => {
                    let base = *style_stack.last().unwrap();
                    style_stack.push(base.patch(theme::code()));
                }
                Tag::CodeBlock(CodeBlockKind::Fenced(_))
                | Tag::CodeBlock(CodeBlockKind::Indented) => {
                    in_code_block = true;
                    if !current.is_empty() {
                        flush_line!();
                    }
                    style_stack.push(theme::code());
                }
                Tag::Item => {
                    current.push(Span::styled("• ", theme::muted()));
                }
                Tag::Paragraph | Tag::List(_) | Tag::BlockQuote => {}
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) | TagEnd::Strong | TagEnd::Emphasis => {
                    style_stack.pop();
                }
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    style_stack.pop();
                    flush_line!();
                }
                TagEnd::Paragraph | TagEnd::Item => {
                    flush_line!();
                }
                _ => {}
            },
            Event::Text(text) => {
                let style = *style_stack.last().unwrap();
                if in_code_block {
                    for (i, l) in text.split('\n').enumerate() {
                        if i > 0 {
                            flush_line!();
                        }
                        current.push(Span::styled(l.to_string(), style));
                    }
                } else {
                    current.push(Span::styled(text.to_string(), style));
                }
            }
            Event::Code(text) => {
                current.push(Span::styled(format!("`{text}`"), theme::code()));
            }
            Event::SoftBreak => current.push(Span::raw(" ")),
            Event::HardBreak => {
                flush_line!();
            }
            Event::Rule => {
                flush_line!();
                lines.push(Line::from("─".repeat(20)));
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "[x] " } else { "[ ] " };
                current.push(Span::styled(mark, theme::heading(false)));
            }
            _ => {}
        }
    }
    if !current.is_empty() {
        flush_line!();
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}
