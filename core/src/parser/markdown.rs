use regex::Regex;
use std::sync::LazyLock;
use crate::models::{BlockType, TaskState};

static PROPERTY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([a-zA-Z_-]+)::(.*)$").unwrap());
static TASK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(TODO|DOING|DONE|CANCELED|CANCELLED|LATER|NOW)\s+(.*)").unwrap());
static SCHEDULED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"SCHEDULED:\s*<(\d{4}-\d{2}-\d{2})>").unwrap());
static DEADLINE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"DEADLINE:\s*<(\d{4}-\d{2}-\d{2})>").unwrap());
static FLASHCARD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#flashcard").unwrap());
static FLASHCARD_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*::\s*").unwrap());
static QUERY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{query\s+(.+?)\}\}").unwrap());

#[derive(Debug, Clone)]
pub struct ParsedBlock {
    pub id: Option<String>,
    pub content: String,
    pub indent_level: u32,
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

    // If no title property, derive from filename
    if page_title.is_none() {
        page_title = Some(title_from_filename(filename));
    }

    // Parse blocks (Logseq uses "- " prefix with indentation)
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
        blocks,
        is_journal,
    }
}

fn parse_block_at(lines: &[&str], start: usize) -> (ParsedBlock, usize) {
    let line = lines[start];
    let indent_level = count_indent(line);
    let raw_content = strip_bullet(line.trim_start());

    let mut properties = serde_json::Map::new();
    let mut block_id: Option<String> = None;
    let mut consumed = 1;

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
            consumed += 1;
        } else {
            break;
        }
    }

    // Detect task
    let task_state = TASK_RE.captures(&full_content)
        .and_then(|cap| TaskState::from_str(&cap[1]));

    // Detect scheduled/deadline
    let scheduled_date = SCHEDULED_RE.captures(&full_content)
        .map(|cap| cap[1].to_string());
    let deadline_date = DEADLINE_RE.captures(&full_content)
        .map(|cap| cap[1].to_string());

    // Detect flashcard
    let is_flashcard = FLASHCARD_RE.is_match(&full_content);
    let (flashcard_front, flashcard_back) = if is_flashcard || full_content.contains(" :: ") {
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
    // Logseq uses 2 spaces or tab per indent level
    (spaces / 2) as u32
}

fn strip_bullet(line: &str) -> &str {
    if line.starts_with("- ") {
        &line[2..]
    } else {
        line
    }
}

fn is_journal_filename(filename: &str) -> bool {
    let name = filename.trim_end_matches(".md");
    // Match patterns like 2024_01_01 or 2024-01-01
    let re = Regex::new(r"^\d{4}[-_]\d{2}[-_]\d{2}$").unwrap();
    re.is_match(name)
}

fn title_from_filename(filename: &str) -> String {
    let name = filename.trim_end_matches(".md");
    // Replace URL-encoded characters
    name.replace("%2F", "/").replace('_', " ")
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
        assert_eq!(parsed.blocks[0].scheduled_date, Some("2024-01-15".to_string()));
    }

    #[test]
    fn test_journal_detection() {
        assert!(is_journal_filename("2024_01_15.md"));
        assert!(is_journal_filename("2024-01-15.md"));
        assert!(!is_journal_filename("my_page.md"));
    }

    #[test]
    fn test_flashcard_parsing() {
        let content = "- Capital of France :: Paris #flashcard";
        let parsed = parse_page(content, "test.md");
        assert!(parsed.blocks[0].is_flashcard);
        assert_eq!(parsed.blocks[0].flashcard_front, Some("Capital of France".to_string()));
    }
}
