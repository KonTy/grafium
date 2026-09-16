//! Portable, managed Markdown footnotes. Author footnotes are not managed.

use super::ParsedBlock;
use crate::models::{Block, BlockType};
use pulldown_cmark::{Event, Options, Parser};
use serde_json::Value;
use std::ops::Range;

pub const OPEN: &str = "<!-- grafium-reading-note ";
pub const CLOSE: &str = "<!-- /grafium-reading-note -->";
pub const METADATA: &str = "reading-note";

#[derive(Debug, Clone)]
pub struct InlineReadingNote {
    pub metadata: Value,
    pub label: String,
    pub body: String,
    pub range: Range<usize>,
    pub line_range: Range<usize>,
}

#[derive(Debug, Default)]
pub struct InlineReadingNotes {
    pub notes: Vec<InlineReadingNote>,
    pub warnings: Vec<String>,
}

pub fn valid_label(label: &str) -> bool {
    label
        .strip_prefix("grafium-note-")
        .is_some_and(|n| !n.starts_with('0') && n.parse::<u64>().is_ok_and(|n| n > 0))
}

pub fn is_reading_note_block(block: &Block) -> bool {
    block
        .properties
        .get(METADATA)
        .and_then(Value::as_str)
        .and_then(|json| serde_json::from_str::<Value>(json).ok())
        .is_some_and(|metadata| {
            crate::graph::reading_notes::valid_managed_metadata(&metadata)
                && metadata["bodyBlockId"].as_str() == Some(block.id.as_str())
        })
}

pub fn note_properties(metadata: &Value, storage: &str) -> Value {
    serde_json::json!({
        "reading-note": metadata.to_string(),
        "reading-note-storage": storage,
        "reading-note-label": metadata.get("footnoteLabel").and_then(Value::as_str).unwrap_or(""),
        "reading-note-id": metadata["id"],
    })
}

pub fn parse_inline_reading_notes(content: &str) -> InlineReadingNotes {
    if !content.contains(OPEN) {
        return InlineReadingNotes::default();
    }
    let html_starts: std::collections::HashSet<usize> = Parser::new_ext(content, Options::all())
        .into_offset_iter()
        .filter_map(|(event, range)| {
            matches!(event, Event::Html(_) | Event::InlineHtml(_)).then_some(range.start)
        })
        .collect();
    let lines: Vec<_> = content.split_inclusive('\n').collect();
    let mut offsets = vec![0];
    for line in &lines {
        offsets.push(offsets.last().unwrap() + line.len());
    }
    let mut result = InlineReadingNotes::default();
    let mut i = 0;
    while i < lines.len() {
        let start = i;
        let line = lines[i].trim_end_matches(['\r', '\n']);
        let Some(json) = line.strip_prefix(OPEN).and_then(|s| s.strip_suffix(" -->")) else {
            i += 1;
            continue;
        };
        if !html_starts.contains(&offsets[i]) {
            i += 1;
            continue;
        }
        let parsed = (|| -> std::result::Result<InlineReadingNote, String> {
            let metadata: Value =
                serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
            if metadata["version"] != 2
                || !crate::graph::reading_notes::valid_managed_metadata(&metadata)
            {
                return Err("unsupported or malformed managed footnote metadata".into());
            }
            let label = metadata["footnoteLabel"]
                .as_str()
                .filter(|s| valid_label(s))
                .ok_or("invalid footnote label")?
                .to_string();
            let definition = lines
                .get(i + 1)
                .ok_or("missing footnote definition")?
                .trim_end_matches('\n');
            let first = definition
                .strip_prefix(&format!("[^{label}]:"))
                .ok_or("metadata/definition label mismatch")?;
            let first = first.strip_prefix(' ').unwrap_or(first);
            let mut body = vec![first.to_string()];
            let mut end = i + 2;
            while let Some(line) = lines.get(end) {
                let line = line.trim_end_matches('\n');
                if line.trim_end_matches('\r') == CLOSE {
                    return Ok(InlineReadingNote {
                        metadata,
                        label,
                        body: body.join("\n"),
                        range: offsets[start]..offsets[end + 1],
                        line_range: start..end + 1,
                    });
                }
                body.push(
                    line.strip_prefix("    ")
                        .ok_or("footnote continuation must have four spaces; file preserved")?
                        .to_string(),
                );
                end += 1;
            }
            Err("missing managed footnote closing comment".into())
        })();
        match parsed {
            Ok(note) => {
                i = note.line_range.end;
                result.notes.push(note);
            }
            Err(e) => {
                result.warnings.push(format!("line {}: {e}", i + 1));
                i += 1;
            }
        }
    }
    result
}

pub fn serialize_inline_reading_note(metadata: &Value, body: &str) -> String {
    let json = metadata
        .to_string()
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace("--", "\\u002d\\u002d");
    let label = metadata["footnoteLabel"].as_str().unwrap_or("");
    let mut parts = body.split('\n');
    let mut result = format!(
        "{OPEN}{json} -->\n[^{label}]: {}\n",
        parts.next().unwrap_or("")
    );
    for line in parts {
        result.push_str(&format!("    {line}\n"));
    }
    result.push_str(CLOSE);
    result.push('\n');
    result
}

pub fn mask_inline_reading_notes(content: &str, notes: &[InlineReadingNote]) -> String {
    let mut result = String::new();
    let mut cursor = 0;
    for note in notes {
        result.push_str(&content[cursor..note.range.start]);
        for _ in note.line_range.clone() {
            result.push('\n');
        }
        cursor = note.range.end;
    }
    result.push_str(&content[cursor..]);
    result
}

pub fn strip_reading_note_references(content: &str) -> String {
    strip_label_references(content, None)
}

pub(crate) fn strip_label_references(content: &str, label: Option<&str>) -> String {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r" ?\[\^(grafium-note-[1-9][0-9]*)\]").unwrap()
    });
    let protected = super::links::protected_reference_context_spans(content);
    let mut result = String::new();
    let mut cursor = 0;
    for capture in RE.captures_iter(content) {
        let span = capture.get(0).unwrap();
        let marker_start = span.start() + usize::from(span.as_str().starts_with(' '));
        if label.is_some_and(|label| label != &capture[1])
            || content[span.end()..].starts_with(':')
            || content[..marker_start]
                .chars()
                .rev()
                .take_while(|c| *c == '\\')
                .count()
                % 2
                == 1
            || protected
                .iter()
                .any(|(start, end)| *start < span.end() && *end > marker_start)
        {
            continue;
        }
        result.push_str(&content[cursor..span.start()]);
        cursor = span.end();
    }
    result.push_str(&content[cursor..]);
    result
}

pub fn source_evidence(content: &str) -> String {
    let notes = parse_inline_reading_notes(content);
    let mut result = String::new();
    let mut cursor = 0;
    for note in notes.notes {
        result.push_str(&content[cursor..note.range.start]);
        cursor = note.range.end;
    }
    result.push_str(&content[cursor..]);
    strip_reading_note_references(&result)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

pub(crate) fn parsed_note(note: &InlineReadingNote) -> ParsedBlock {
    ParsedBlock {
        id: note.metadata["bodyBlockId"].as_str().map(str::to_string),
        content: note.body.clone(),
        indent_level: 0,
        source_line_range: note.line_range.clone(),
        block_type: BlockType::Text,
        properties: note_properties(&note.metadata, "inline"),
        task_state: None,
        scheduled_date: None,
        deadline_date: None,
        is_flashcard: false,
        flashcard_front: None,
        flashcard_back: None,
        children: Vec::new(),
    }
}

pub(crate) fn serialize_inline_page(properties: &Value, blocks: &[Block]) -> Option<String> {
    let notes: Vec<_> = blocks
        .iter()
        .filter(|b| is_reading_note_block(b) && b.properties["reading-note-storage"] == "inline")
        .collect();
    if notes.is_empty() {
        return None;
    }
    let note_ids: std::collections::HashSet<_> = notes.iter().map(|b| b.id.as_str()).collect();
    let mut owned = note_ids.clone();
    loop {
        let before = owned.len();
        for block in blocks {
            if block
                .parent_id
                .as_deref()
                .is_some_and(|id| owned.contains(id))
            {
                owned.insert(block.id.as_str());
            }
        }
        if owned.len() == before {
            break;
        }
    }
    let source: Vec<_> = blocks
        .iter()
        .filter(|b| !owned.contains(b.id.as_str()))
        .cloned()
        .collect();
    let mut result = super::serialize_page(properties, &source);
    for note in notes {
        if !result.ends_with("\n\n") {
            result.push('\n');
        }
        let metadata: Value =
            serde_json::from_str(note.properties[METADATA].as_str().unwrap()).ok()?;
        let mut body = note.content.clone();
        for child in blocks
            .iter()
            .filter(|b| b.parent_id.as_deref() == Some(note.id.as_str()))
        {
            body.push('\n');
            body.push_str(&super::serializer::serialize_block_subtree(
                blocks, &child.id, 0,
            )?);
        }
        result.push_str(&serialize_inline_reading_note(&metadata, &body));
    }
    Some(result)
}
