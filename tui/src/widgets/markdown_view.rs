//! Read-only markdown -> styled terminal text conversion, shared by every
//! place that needs to show rendered markdown (the page/block preview and
//! the graph view's link previews). Written once so no screen re-implements
//! its own markdown-to-text logic.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};
use ratatui::style::Style;
use ratatui::text::{Line, Span, StyledGrapheme};
use unicode_width::UnicodeWidthStr;

use crate::widgets::theme;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct MarkdownCacheKey {
    content_hash: u64,
    width: u16,
}

#[derive(Default)]
pub struct MarkdownRenderCache {
    entries: HashMap<MarkdownCacheKey, Arc<Vec<Line<'static>>>>,
}

impl MarkdownRenderCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn render(&mut self, source: &str, width: u16) -> Arc<Vec<Line<'static>>> {
        self.render_with(source, width, render_markdown)
    }

    pub fn render_with<F>(
        &mut self,
        source: &str,
        width: u16,
        renderer: F,
    ) -> Arc<Vec<Line<'static>>>
    where
        F: FnOnce(&str, u16) -> Vec<Line<'static>>,
    {
        let width = width.max(1);
        let key = MarkdownCacheKey {
            content_hash: hash_source(source),
            width,
        };

        if let Some(lines) = self.entries.get(&key) {
            return Arc::clone(lines);
        }

        let rendered = Arc::new(renderer(source, width));
        if self.entries.len() >= 512 {
            self.entries.clear();
        }
        self.entries.insert(key, Arc::clone(&rendered));
        rendered
    }
}

/// Parses and wraps a markdown block into styled lines that already fit the
/// provided width, so repeated draws can reuse the cached wrapped output.
pub fn render_markdown(source: &str, width: u16) -> Vec<Line<'static>> {
    wrap_lines(parse_markdown(source), width.max(1))
}

fn hash_source(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

fn parse_markdown(source: &str) -> Vec<Line<'static>> {
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

fn wrap_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::from("")];
    }

    let mut wrapped = Vec::new();
    for line in lines {
        wrapped.extend(wrap_line(&line, width));
    }
    if wrapped.is_empty() {
        wrapped.push(Line::from(""));
    }
    wrapped
}

fn wrap_line(line: &Line<'static>, width: u16) -> Vec<Line<'static>> {
    let mut wrapped_lines: Vec<Vec<StyledGrapheme<'_>>> = Vec::new();
    let mut pending_line: Vec<StyledGrapheme<'_>> = Vec::new();
    let mut pending_word: Vec<StyledGrapheme<'_>> = Vec::new();
    let mut pending_whitespace: VecDeque<StyledGrapheme<'_>> = VecDeque::new();
    let mut line_width = 0u16;
    let mut word_width = 0u16;
    let mut whitespace_width = 0u16;
    let mut non_whitespace_previous = false;

    for grapheme in line.styled_graphemes(Style::default()) {
        let is_whitespace = grapheme.is_whitespace();
        let symbol_width = grapheme.symbol.width() as u16;

        if symbol_width > width {
            continue;
        }

        let word_found = non_whitespace_previous && is_whitespace;
        let untrimmed_overflow =
            pending_line.is_empty() && word_width + whitespace_width + symbol_width > width;

        if word_found || untrimmed_overflow {
            pending_line.extend(pending_whitespace.drain(..));
            line_width += whitespace_width;
            pending_line.append(&mut pending_word);
            line_width += word_width;

            whitespace_width = 0;
            word_width = 0;
        }

        let line_full = line_width >= width;
        let pending_word_overflow =
            symbol_width > 0 && line_width + whitespace_width + word_width >= width;

        if line_full || pending_word_overflow {
            let mut remaining_width = width.saturating_sub(line_width);
            wrapped_lines.push(std::mem::take(&mut pending_line));
            line_width = 0;

            while let Some(front) = pending_whitespace.front() {
                let front_width = front.symbol.width() as u16;
                if front_width > remaining_width {
                    break;
                }
                whitespace_width = whitespace_width.saturating_sub(front_width);
                remaining_width = remaining_width.saturating_sub(front_width);
                pending_whitespace.pop_front();
            }

            if is_whitespace && pending_whitespace.is_empty() {
                continue;
            }
        }

        if is_whitespace {
            whitespace_width += symbol_width;
            pending_whitespace.push_back(grapheme);
        } else {
            word_width += symbol_width;
            pending_word.push(grapheme);
        }

        non_whitespace_previous = !is_whitespace;
    }

    pending_line.extend(pending_whitespace.drain(..));
    pending_line.append(&mut pending_word);

    if !pending_line.is_empty() {
        wrapped_lines.push(pending_line);
    }

    if wrapped_lines.is_empty() {
        return vec![Line::from("")];
    }

    wrapped_lines.into_iter().map(styled_line_to_line).collect()
}

fn styled_line_to_line(line: Vec<StyledGrapheme<'_>>) -> Line<'static> {
    if line.is_empty() {
        return Line::from("");
    }

    let mut spans = Vec::new();
    let mut current_style = line[0].style;
    let mut current_content = String::new();

    for grapheme in line {
        if grapheme.style == current_style {
            current_content.push_str(grapheme.symbol);
            continue;
        }

        spans.push(Span::styled(
            std::mem::take(&mut current_content),
            current_style,
        ));
        current_style = grapheme.style;
        current_content.push_str(grapheme.symbol);
    }

    spans.push(Span::styled(current_content, current_style));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn render_cache_reuses_same_content_and_width() {
        let mut cache = MarkdownRenderCache::new();
        let calls = AtomicUsize::new(0);

        let first = cache.render_with("hello", 24, |source, width| {
            calls.fetch_add(1, Ordering::SeqCst);
            vec![Line::from(format!("{source}:{width}"))]
        });
        let second = cache.render_with("hello", 24, |source, width| {
            calls.fetch_add(1, Ordering::SeqCst);
            vec![Line::from(format!("{source}:{width}"))]
        });
        let third = cache.render_with("hello", 12, |source, width| {
            calls.fetch_add(1, Ordering::SeqCst);
            vec![Line::from(format!("{source}:{width}"))]
        });

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(first.as_ref(), second.as_ref());
        assert_ne!(first.as_ref(), third.as_ref());
    }
}
