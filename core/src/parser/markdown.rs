use crate::models::{BlockType, TaskState};
use regex::Regex;
use std::ops::Range;
use std::sync::LazyLock;

static PROPERTY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-zA-Z_-]+)::(.*)$").unwrap());
static TASK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(TODO|DOING|DONE|CANCELED|CANCELLED|LATER|NOW)\s+(.*)").unwrap()
});
static SCHEDULED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"SCHEDULED:\s*<(\d{4}-\d{2}-\d{2})[^>]*>").unwrap());
static DEADLINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"DEADLINE:\s*<(\d{4}-\d{2}-\d{2})[^>]*>").unwrap());
static FLASHCARD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#flashcard").unwrap());
static FLASHCARD_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*::\s*").unwrap());
static QUERY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{query\s+(.+?)\}\}").unwrap());

static ADMONITION_BEGIN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^#\+BEGIN_(TIP|NOTE|IMPORTANT|CAUTION|PINNED|WARNING)$").unwrap()
});
static ADMONITION_END_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^#\+END_(TIP|NOTE|IMPORTANT|CAUTION|PINNED|WARNING)$").unwrap()
});

/// Remove inline-code spans (text between backticks) so that separators like
/// `::` inside code samples are not treated as flashcard/property syntax.
fn strip_inline_code(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_code = false;
    for c in s.chars() {
        if c == '`' {
            in_code = !in_code;
            continue;
        }
        if !in_code {
            out.push(c);
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct ParsedBlock {
    pub id: Option<String>,
    pub content: String,
    pub indent_level: u32,
    pub source_line_range: Range<usize>,
    pub block_type: BlockType,
    pub properties: serde_json::Value,
    pub task_state: Option<TaskState>,
    pub scheduled_date: Option<String>,
    pub deadline_date: Option<String>,
    pub is_flashcard: bool,
    pub flashcard_front: Option<String>,
    pub flashcard_back: Option<String>,
    pub children: Vec<ParsedBlock>,
}

#[derive(Debug, Clone)]
pub struct ParsedPage {
    pub title: Option<String>,
    pub properties: serde_json::Value,
    pub blocks: Vec<ParsedBlock>,
    pub is_journal: bool,
}

pub fn parse_page(content: &str, filename: &str) -> ParsedPage {
    let lines: Vec<&str> = content.lines().collect();
    let is_journal = is_journal_filename(filename);

    let mut page_title: Option<String> = None;
    let mut page_properties = serde_json::Map::new();
    let mut blocks: Vec<ParsedBlock> = Vec::new();
    let mut i = 0;

    // Parse page-level properties at the top
    while i < lines.len() {
        let line = lines[i];
        if let Some(cap) = PROPERTY_RE.captures(line) {
            let key = cap[1].to_string();
            let value = cap[2].trim().to_string();
            if key == "title" {
                page_title = Some(value);
            } else {
                page_properties.insert(key, serde_json::Value::String(value));
            }
            i += 1;
        } else {
            break;
        }
    }

    // If no title property found, leave it as None.
    // The caller (index_file) will derive the title from the file's relative path,
    // which supports hierarchical folder structures like Books/MyCoolBook/Chapter1.

    // Parse blocks (org-style uses "- " prefix with indentation)
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        let (block, consumed) = parse_block_at(&lines, i);
        blocks.push(block);
        i += consumed;
    }

    ParsedPage {
        title: page_title,
        properties: serde_json::Value::Object(page_properties),
        blocks: normalize_fenced_code_sequences(blocks),
        is_journal,
    }
}

fn normalize_fenced_code_sequences(blocks: Vec<ParsedBlock>) -> Vec<ParsedBlock> {
    let mut out: Vec<ParsedBlock> = Vec::new();
    let mut i = 0usize;

    while i < blocks.len() {
        if is_fence_open_marker(&blocks[i].content) {
            let mut close_idx: Option<usize> = None;
            let mut j = i + 1;
            while j < blocks.len() {
                if is_fence_close_marker(&blocks[j].content) {
                    close_idx = Some(j);
                    break;
                }
                j += 1;
            }

            if let Some(end) = close_idx {
                if end > i + 1 {
                    let mut merged = blocks[i].clone();
                    let mut content = String::new();
                    content.push_str(first_line_trimmed(&blocks[i].content));

                    for mid in (i + 1)..end {
                        content.push('\n');
                        content.push_str(&blocks[mid].content);
                    }

                    content.push('\n');
                    content.push_str(first_line_trimmed(&blocks[end].content));

                    merged.content = content;
                    merged.source_line_range =
                        blocks[i].source_line_range.start..blocks[end].source_line_range.end;
                    merged.children = Vec::new();
                    out.push(merged);
                    i = end + 1;
                    continue;
                }
            }
        }

        let mut block = blocks[i].clone();
        if !block.children.is_empty() {
            block.children = normalize_fenced_code_sequences(block.children);
        }
        out.push(block);
        i += 1;
    }

    out
}

fn is_fence_open_marker(content: &str) -> bool {
    let mut lines = content.lines();
    let first = lines.next().unwrap_or("").trim();
    if !first.starts_with("```") {
        return false;
    }
    lines.all(is_property_line)
}

fn is_fence_close_marker(content: &str) -> bool {
    let mut lines = content.lines();
    let first = lines.next().unwrap_or("").trim();
    if first != "```" {
        return false;
    }
    lines.all(is_property_line)
}

fn is_property_line(line: &str) -> bool {
    PROPERTY_RE.is_match(line.trim())
}

fn first_line_trimmed(content: &str) -> &str {
    content.lines().next().unwrap_or("").trim()
}

fn parse_block_at(lines: &[&str], start: usize) -> (ParsedBlock, usize) {
    let line = lines[start];
    let indent_level = count_indent(line);
    let raw_content = strip_bullet(line.trim_start());

    let mut properties = serde_json::Map::new();
    let mut block_id: Option<String> = None;
    let mut consumed = 1;
    let mut inside_code_fence = raw_content.trim_start().starts_with("```");
    // A Logseq-style admonition (`#+BEGIN_TIP` … `#+END_TIP`) is kept as a
    // single block: consume every line up to and including the matching
    // `#+END_…`, without splitting on inner blank lines.
    let mut inside_admonition = ADMONITION_BEGIN_RE.is_match(raw_content.trim());

    // If the bullet content itself is just "id:: <uuid>" (roundtrip corruption fix),
    // treat it as the block's id with empty content
    let mut full_content = if raw_content.starts_with("id:: ") && raw_content.len() > 5 {
        block_id = Some(raw_content[5..].trim().to_string());
        String::new()
    } else {
        raw_content.to_string()
    };
    while start + consumed < lines.len() {
        let next_line = lines[start + consumed];
        let next_indent = count_indent(next_line);
        let next_trimmed = next_line.trim_start();

        // While inside fenced code, keep consuming lines regardless of indentation.
        if inside_code_fence {
            let continuation_raw = if next_indent > indent_level {
                strip_continuation(next_line, indent_level + 1)
            } else {
                next_line
            };

            // Legacy corrupted fence shape sometimes stores each code line as a sibling bullet
            // at the same indentation level. In that case, drop the synthetic bullet marker.
            let continuation = if next_indent <= indent_level {
                let t = continuation_raw.trim_start();
                if t.starts_with("- ") {
                    &t[2..]
                } else {
                    continuation_raw
                }
            } else {
                continuation_raw
            };

            // Ignore synthetic metadata/property lines that came from split sibling blocks.
            if next_indent > indent_level && is_property_line(continuation.trim()) {
                consumed += 1;
                continue;
            }

            // Ignore synthetic empty bullets from prior corruption.
            if continuation.trim().is_empty() || continuation.trim() == "-" {
                consumed += 1;
                continue;
            }

            full_content.push('\n');
            full_content.push_str(continuation);
            if continuation.trim_start().starts_with("```") {
                inside_code_fence = false;
            }
            consumed += 1;
            continue;
        }

        // While inside an admonition, keep consuming every line (including
        // blank lines) until the matching `#+END_…`, so the whole callout
        // body stays in one block.
        if inside_admonition {
            let continuation = strip_continuation(next_line, indent_level + 1);
            full_content.push('\n');
            full_content.push_str(continuation);
            consumed += 1;
            if ADMONITION_END_RE.is_match(continuation.trim()) {
                inside_admonition = false;
            }
            continue;
        }

        // Property lines for this block (indented, key:: value)
        if next_indent > indent_level && !next_trimmed.starts_with("- ") {
            if let Some(cap) = PROPERTY_RE.captures(next_trimmed) {
                let key = cap[1].to_string();
                let value = cap[2].trim().to_string();
                if key == "id" {
                    block_id = Some(value);
                } else {
                    properties.insert(key, serde_json::Value::String(value));
                }
                consumed += 1;
                continue;
            }
            // Continuation of content
            full_content.push('\n');
            full_content.push_str(next_trimmed);
            if next_trimmed.starts_with("```") {
                inside_code_fence = !inside_code_fence;
            }
            consumed += 1;
        } else {
            break;
        }
    }

    // Detect task
    let task_state = TASK_RE
        .captures(&full_content)
        .and_then(|cap| TaskState::from_str(&cap[1]));

    // Detect scheduled/deadline
    let scheduled_date = SCHEDULED_RE
        .captures(&full_content)
        .map(|cap| cap[1].to_string());
    let deadline_date = DEADLINE_RE
        .captures(&full_content)
        .map(|cap| cap[1].to_string());

    // Detect flashcard. A block is a card if it carries #flashcard, OR it is
    // written as `Question :: Answer`. The `::` separator only counts when it
    // appears OUTSIDE inline code, so explanatory prose that mentions
    // `Question :: Answer` in backticks is not mistaken for a card.
    let code_stripped = strip_inline_code(&full_content);
    let is_flashcard = FLASHCARD_RE.is_match(&full_content) || code_stripped.contains(" :: ");
    let (flashcard_front, flashcard_back) = if is_flashcard {
        let parts: Vec<&str> = FLASHCARD_SPLIT_RE.splitn(&full_content, 2).collect();
        if parts.len() == 2 {
            (Some(parts[0].to_string()), Some(parts[1].to_string()))
        } else {
            (Some(full_content.clone()), None)
        }
    } else {
        (None, None)
    };

    // Detect block type
    let block_type = if is_flashcard {
        BlockType::Flashcard
    } else if QUERY_RE.is_match(&full_content) {
        BlockType::Query
    } else {
        BlockType::Text
    };

    // Parse child blocks (more indented bullet items)
    let mut children: Vec<ParsedBlock> = Vec::new();
    while start + consumed < lines.len() {
        let next_line = lines[start + consumed];
        if next_line.trim().is_empty() {
            consumed += 1;
            continue;
        }
        let next_indent = count_indent(next_line);
        let next_trimmed = next_line.trim_start();
        if next_indent > indent_level && next_trimmed.starts_with("- ") {
            let (child, child_consumed) = parse_block_at(lines, start + consumed);
            children.push(child);
            consumed += child_consumed;
        } else {
            break;
        }
    }

    let block = ParsedBlock {
        id: block_id,
        content: full_content,
        indent_level,
        source_line_range: start..start + consumed,
        block_type,
        properties: serde_json::Value::Object(properties),
        task_state,
        scheduled_date,
        deadline_date,
        is_flashcard,
        flashcard_front,
        flashcard_back,
        children,
    };

    (block, consumed)
}

fn count_indent(line: &str) -> u32 {
    let spaces = line.len() - line.trim_start().len();
    // org-style uses 2 spaces or tab per indent level
    (spaces / 2) as u32
}

fn strip_bullet(line: &str) -> &str {
    if line.starts_with("- ") {
        &line[2..]
    } else {
        line
    }
}

fn strip_continuation(line: &str, min_depth: u32) -> &str {
    let mut idx = 0usize;
    let bytes = line.as_bytes();
    let mut spaces = 0usize;
    let min_spaces = (min_depth as usize) * 2;

    while idx < bytes.len() && spaces < min_spaces {
        if bytes[idx] == b' ' {
            idx += 1;
            spaces += 1;
        } else {
            break;
        }
    }

    &line[idx..]
}

fn is_journal_filename(filename: &str) -> bool {
    let name = filename.trim_end_matches(".md");
    // Match patterns like 2024_01_01 or 2024-01-01
    let re = Regex::new(r"^\d{4}[-_]\d{2}[-_]\d{2}$").unwrap();
    re.is_match(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_page() {
        let content = "- Hello world\n- Second block\n  - Child block";
        let parsed = parse_page(content, "test.md");
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(parsed.blocks[0].content, "Hello world");
        assert_eq!(parsed.blocks[1].content, "Second block");
        assert_eq!(parsed.blocks[1].children.len(), 1);
    }

    #[test]
    fn test_task_parsing() {
        let content = "- TODO Buy groceries\n  SCHEDULED: <2024-01-15>";
        let parsed = parse_page(content, "test.md");
        assert_eq!(parsed.blocks[0].task_state, Some(TaskState::Todo));
        assert_eq!(
            parsed.blocks[0].scheduled_date,
            Some("2024-01-15".to_string())
        );
    }

    #[test]
    fn test_journal_detection() {
        assert!(is_journal_filename("2024_01_15.md"));
        assert!(is_journal_filename("2024-01-15.md"));
        assert!(!is_journal_filename("my_page.md"));
    }

    #[test]
    fn test_source_line_range_covers_entire_block_subtree() {
        let content =
            "- Parent\n  id:: parent\n  - Child\n    id:: child\n- Sibling\n  id:: sibling\n";
        let parsed = parse_page(content, "test.md");

        assert_eq!(parsed.blocks[0].source_line_range, 0..4);
        assert_eq!(parsed.blocks[1].source_line_range, 4..6);
    }

    #[test]
    fn test_flashcard_parsing() {
        let content = "- Capital of France :: Paris #flashcard";
        let parsed = parse_page(content, "test.md");
        assert!(parsed.blocks[0].is_flashcard);
        assert_eq!(
            parsed.blocks[0].flashcard_front,
            Some("Capital of France".to_string())
        );
    }

    #[test]
    fn test_code_fence_keeps_bullet_like_lines_in_same_block() {
        let content = "- ```\n  - this should stay in code\n  - and this too\n  ```";
        let parsed = parse_page(content, "test.md");

        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].children.len(), 0);
        assert_eq!(
            parsed.blocks[0].content,
            "```\n- this should stay in code\n- and this too\n```"
        );
    }

    #[test]
    fn test_normalize_split_fence_sibling_blocks() {
        let content =
            "- ```\n- this is some code block test\n- 2nd line more of it\n- 3rd line\n- ```";
        let parsed = parse_page(content, "test.md");

        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(
            parsed.blocks[0].content,
            "```\nthis is some code block test\n2nd line more of it\n3rd line\n```"
        );
    }

    #[test]
    fn test_normalize_split_fence_with_ids_and_empty_children() {
        let content = "- ```\n  id:: open\n- this is some code block test\n  id:: mid\n  - \n    id:: c1\n  - \n    id:: c2\n- 2nd line more of it\n  id:: line2\n- 3rd line\n  id:: line3\n- ```\n  id:: close";

        let parsed = parse_page(content, "test.md");

        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].children.len(), 0);
        assert_eq!(
            parsed.blocks[0].content,
            "```\nthis is some code block test\n2nd line more of it\n3rd line\n```"
        );
    }

    #[test]
    fn test_code_fence_with_unindented_lines_stays_single_block() {
        let content = "- ```mermaid\nsequenceDiagram\nparticipant U as User\nparticipant S as Server\n```\n- next";
        let parsed = parse_page(content, "test.md");

        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(
            parsed.blocks[0].content,
            "```mermaid\nsequenceDiagram\nparticipant U as User\nparticipant S as Server\n```"
        );
        assert_eq!(parsed.blocks[1].content, "next");
    }

    #[test]
    fn test_admonition_stays_single_block() {
        let content = "- #+BEGIN_TIP\n  This is a helpful tip.\n  #+END_TIP";
        let parsed = parse_page(content, "test.md");

        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].children.len(), 0);
        assert_eq!(
            parsed.blocks[0].content,
            "#+BEGIN_TIP\nThis is a helpful tip.\n#+END_TIP"
        );
    }

    #[test]
    fn test_admonition_keeps_inner_blank_lines_in_same_block() {
        let content =
            "- #+BEGIN_NOTE\n  first line\n  \n  second line\n  #+END_NOTE\n- After the callout";
        let parsed = parse_page(content, "test.md");

        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(
            parsed.blocks[0].content,
            "#+BEGIN_NOTE\nfirst line\n\nsecond line\n#+END_NOTE"
        );
        assert_eq!(parsed.blocks[1].content, "After the callout");
    }

    #[test]
    fn test_admonition_empty_body_and_id_roundtrip_shape() {
        // Mirrors what the serializer writes for a freshly-inserted callout:
        // an empty body line plus the block id property line after `#+END_`.
        let content = "- #+BEGIN_WARNING\n  \n  #+END_WARNING\n  id:: cb-1";
        let parsed = parse_page(content, "test.md");

        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].id, Some("cb-1".to_string()));
        assert_eq!(parsed.blocks[0].content, "#+BEGIN_WARNING\n\n#+END_WARNING");
    }

    #[test]
    fn test_admonition_is_case_insensitive() {
        let content = "- #+begin_important\n  Body text\n  #+end_important";
        let parsed = parse_page(content, "test.md");

        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(
            parsed.blocks[0].content,
            "#+begin_important\nBody text\n#+end_important"
        );
    }
}
