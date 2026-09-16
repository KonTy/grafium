//! File-authoritative annotations. New notes are version-2 Markdown footnotes
//! inside their source file; SQLite is only an index. Version-1 standalone
//! `reading-note:: <JSON string>` pages remain readable and safely editable.

use super::*;
use crate::knowledge::engine::reading_scope::{normalize, rendered_text};
use crate::knowledge::scoped_context::scoped_block_order;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use std::path::Component;

const PROPERTY: &str = "reading-note";
const DIRECTORY: &str = "pages/Reading Notes";

#[cfg(test)]
#[path = "reading_notes_tests.rs"]
mod tests;

#[path = "reading_notes_inline.rs"]
mod inline;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadingSelection {
    pub page_id: String,
    pub block_ids: Vec<String>,
    pub text: String,
    pub kind: SelectionKind,
    pub parts: Vec<ReadingSelectionPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_range: Option<ReadingDocumentRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelectionKind {
    Source,
    Rendered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadingSelectionPart {
    pub block_id: String,
    pub text: String,
    pub from: usize,
    pub to: usize,
    pub prefix: String,
    pub suffix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadingDocumentRange {
    pub from: usize,
    pub to: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingNote {
    pub id: String,
    pub note_page_id: String,
    pub file_path: String,
    pub body: String,
    /// Exact inline envelope hash; legacy notes hash their entire file.
    pub revision: String,
    pub source: ReadingNoteSource,
    pub quote: String,
    pub status: ReadingNoteStatus,
    pub status_message: String,
    pub target_block_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub storage: ReadingNoteStorage,
    pub footnote_label: Option<String>,
    pub note_block_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadingNoteStorage {
    Inline,
    File,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingNoteSource {
    pub page_id: Option<String>,
    pub page_title: String,
    pub file_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadingNoteStatus {
    Attached,
    Recovered,
    Ambiguous,
    Orphaned,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingNotesList {
    pub notes: Vec<ReadingNote>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Metadata {
    version: u32,
    id: String,
    body_block_id: String,
    created_at: String,
    updated_at: String,
    create_request_hash: Option<String>,
    anchor: Anchor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    footnote_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Anchor {
    source_path: String,
    source_page_id: String,
    source_title: String,
    source_file_sha256: String,
    offset_unit: String,
    kind: SelectionKind,
    quote: String,
    parts: Vec<AnchorPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hash_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AnchorPart {
    block_id: String,
    persisted_id: bool,
    ordinal: usize,
    raw_fingerprint: String,
    text: String,
    from: usize,
    to: usize,
    prefix: String,
    suffix: String,
}

struct Document {
    path: PathBuf,
    content: String,
    properties: serde_json::Value,
    metadata: Metadata,
    body: String,
    storage: ReadingNoteStorage,
    revision_range: std::ops::Range<usize>,
}

struct Source {
    page: Page,
    path: PathBuf,
    hash: String,
    blocks: Vec<Block>,
    parsed: Vec<ParsedBlock>,
    raw_hash: String,
}

fn error(message: impl std::fmt::Display) -> CoreError {
    CoreError::Other(format!("Reading notes: {message}"))
}

fn document_revision(document: &Document) -> Result<String> {
    let content = document
        .content
        .get(document.revision_range.clone())
        .ok_or_else(|| error("invalid annotation revision range; file preserved"))?;
    Ok(Graph::content_hash(content))
}

fn identical_create_request(
    document: &Document,
    source_page_id: &str,
    selection: Option<&ReadingSelection>,
    body: &str,
    request_hash: &str,
) -> bool {
    let anchor = &document.metadata.anchor;
    document.metadata.create_request_hash.as_deref() == Some(request_hash)
        && document.body == body
        && anchor.source_page_id == source_page_id
        && match selection {
            None => anchor.parts.is_empty() && anchor.quote.is_empty(),
            Some(selection) => {
                anchor.kind == selection.kind
                    && anchor.quote == selection.text
                    && anchor.parts.len() == selection.parts.len()
                    && anchor
                        .parts
                        .iter()
                        .zip(&selection.parts)
                        .all(|(saved, requested)| {
                            saved.block_id == requested.block_id
                                && saved.text == requested.text
                                && saved.from == requested.from
                                && saved.to == requested.to
                                && saved.prefix
                                    == parser::strip_reading_note_references(&requested.prefix)
                                && saved.suffix
                                    == parser::strip_reading_note_references(&requested.suffix)
                        })
            }
        }
}

fn uuid(value: &str) -> Result<()> {
    if Uuid::parse_str(value).is_err() {
        return Err(error("note ID must be a UUID"));
    }
    Ok(())
}

fn sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}

fn relative_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if value.contains('\\')
        || !path.components().all(|c| matches!(c, Component::Normal(_)))
        || !matches!(path.components().next(), Some(Component::Normal(c)) if c == "pages" || c == "journals" || c == "knowledge")
        || path.extension().and_then(|s| s.to_str()) != Some("md")
    {
        return Err(error(
            "invalid source or note path (must be graph-relative Markdown)",
        ));
    }
    Ok(())
}

/// Unlike the outline parser, only the initial property section is metadata.
/// Everything after its first blank line is the exact, user-owned body.
fn header(content: &str) -> Option<(serde_json::Value, &str)> {
    let mut properties = serde_json::Map::new();
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        let value = line.trim_end_matches(['\r', '\n']);
        offset += line.len();
        if value.is_empty() {
            return properties
                .contains_key(PROPERTY)
                .then(|| (serde_json::Value::Object(properties), &content[offset..]));
        }
        let (key, value) = value.split_once("::")?;
        if key.is_empty()
            || !key
                .bytes()
                .all(|c| c.is_ascii_alphabetic() || c == b'-' || c == b'_')
        {
            return None;
        }
        // A duplicate would silently overwrite metadata.
        if properties.insert(key.into(), value.trim().into()).is_some() {
            return None;
        }
    }
    None
}

fn metadata(properties: &serde_json::Value) -> Result<Metadata> {
    let value = properties
        .get(PROPERTY)
        .and_then(|v| v.as_str())
        .ok_or_else(|| error("missing reading-note string property"))?;
    let result: Metadata = serde_json::from_str(value)
        .map_err(|e| error(format!("malformed annotation metadata: {e}")))?;
    if !matches!(result.version, 1 | 2)
        || result.anchor.offset_unit != "utf-16"
        || (result.version == 2
            && !result
                .footnote_label
                .as_deref()
                .is_some_and(parser::reading_notes::valid_label))
        || result
            .anchor
            .hash_mode
            .as_deref()
            .is_some_and(|mode| mode != "annotation-free-v1")
    {
        return Err(error(
            "unsupported annotation version or offset unit; file preserved",
        ));
    }
    uuid(&result.id)?;
    uuid(&result.body_block_id)?;
    relative_path(&result.anchor.source_path)?;
    if !sha256(&result.anchor.source_file_sha256)
        || result.anchor.source_page_id.is_empty()
        || result.anchor.parts.iter().any(|p| {
            !sha256(&p.raw_fingerprint)
                || p.to <= p.from
                || p.text.is_empty()
                || p.block_id.is_empty()
                || p.text.encode_utf16().count() != p.to - p.from
        })
        || result
            .anchor
            .parts
            .windows(2)
            .any(|parts| parts[0].ordinal >= parts[1].ordinal)
        || result
            .anchor
            .parts
            .iter()
            .map(|p| &p.block_id)
            .collect::<HashSet<_>>()
            .len()
            != result.anchor.parts.len()
        || result.anchor.quote
            != result
                .anchor
                .parts
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        || chrono::DateTime::parse_from_rfc3339(&result.created_at).is_err()
        || chrono::DateTime::parse_from_rfc3339(&result.updated_at).is_err()
    {
        return Err(error(
            "invalid anchor fingerprint, offsets, or timestamps; file preserved",
        ));
    }
    Ok(result)
}

pub(crate) fn valid_managed_metadata(value: &serde_json::Value) -> bool {
    metadata(&serde_json::json!({ PROPERTY: value.to_string() })).is_ok()
}

fn encode(properties: &serde_json::Value, body: &str) -> String {
    let mut output = String::new();
    if let Some(object) = properties.as_object() {
        for (key, value) in object {
            if let Some(value) = value.as_str() {
                output.push_str(&format!("{key}:: {value}\n"));
            }
        }
    }
    output.push('\n');
    output.push_str(body);
    output
}

/// Narrow parser hook: a note's entire Markdown body is one editable block.
pub(crate) fn parse_note_page(content: &str, filename: &str) -> Option<parser::ParsedPage> {
    let (properties, body) = header(content)?;
    let metadata = metadata(&properties).ok()?;
    if metadata.version != 1 {
        return None;
    }
    Some(parser::ParsedPage {
        title: properties
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        properties,
        is_journal: parser::canonical_journal_title(filename).is_some(),
        blocks: vec![ParsedBlock {
            id: Some(metadata.body_block_id.clone()),
            content: body.into(),
            indent_level: 0,
            source_line_range: content[..content.len() - body.len()].lines().count()
                ..content.lines().count(),
            block_type: BlockType::Text,
            properties: parser::reading_notes::note_properties(
                &serde_json::to_value(&metadata).ok()?,
                "file",
            ),
            task_state: None,
            scheduled_date: None,
            deadline_date: None,
            is_flashcard: false,
            flashcard_front: None,
            flashcard_back: None,
            children: Vec::new(),
        }],
    })
}

/// Keep arbitrary Markdown (including property-like lines and trailing blanks)
/// literal. If ordinary editing splits the block, preserve the full outline,
/// including custom block properties, rather than throwing anything away.
pub(crate) fn serialize_note_page(
    properties: &serde_json::Value,
    blocks: &[Block],
) -> Option<String> {
    if metadata(properties).ok()?.version != 1 {
        return None;
    }
    let body = if blocks.len() == 1
        && blocks[0].parent_id.is_none()
        && blocks[0]
            .properties
            .as_object()
            .is_some_and(|p| p.keys().all(|k| k == "id" || k.starts_with("reading-note")))
    {
        blocks[0].content.clone()
    } else {
        parser::serialize_page(&serde_json::json!({}), blocks)
    };
    Some(encode(properties, &body))
}

pub(crate) fn is_note_page(properties: &serde_json::Value) -> bool {
    metadata(properties).is_ok()
}

fn flattened(blocks: &[ParsedBlock], out: &mut Vec<ParsedBlock>) {
    for block in blocks {
        out.push(block.clone());
        flattened(&block.children, out);
    }
}

fn utf16_slice(text: &str, from: usize, to: usize) -> Option<String> {
    if to < from {
        return None;
    }
    let units: Vec<_> = text.encode_utf16().collect();
    String::from_utf16(units.get(from..to)?).ok()
}

fn occurrences(text: &str, quote: &str, prefix: &str, suffix: &str) -> Vec<usize> {
    if quote.is_empty() {
        return Vec::new();
    }
    text.char_indices()
        .filter_map(|(i, _)| {
            (text[i..].starts_with(quote)
                && text[..i].ends_with(prefix)
                && text[i + quote.len()..].starts_with(suffix))
            .then_some(i)
        })
        .collect()
}

fn representation(content: &str, kind: SelectionKind) -> String {
    match kind {
        SelectionKind::Source => content.into(),
        SelectionKind::Rendered => rendered_text(content),
    }
}

fn context(value: &str, kind: SelectionKind) -> String {
    match kind {
        SelectionKind::Source => value.into(),
        SelectionKind::Rendered => normalize(value),
    }
}

fn context_occurrences(content: &str, part: &AnchorPart, kind: SelectionKind) -> Vec<usize> {
    let text = representation(content, kind);
    let quote = context(&part.text, kind);
    // Normalization trims edge whitespace. Restore a boundary when the
    // original DOM quote/context had whitespace at that edge.
    let edge = |s: &str, prefix: bool| {
        let mut result = context(s, kind);
        if kind == SelectionKind::Rendered && !result.is_empty() {
            if prefix
                && (s.chars().last().is_some_and(char::is_whitespace)
                    || part.text.chars().next().is_some_and(char::is_whitespace))
            {
                result.push(' ');
            }
            if !prefix
                && (s.chars().next().is_some_and(char::is_whitespace)
                    || part.text.chars().last().is_some_and(char::is_whitespace))
            {
                result.insert(0, ' ');
            }
        }
        result
    };
    occurrences(
        &text,
        &quote,
        &edge(&part.prefix, true),
        &edge(&part.suffix, false),
    )
}

impl Graph {
    fn reading_path(&self, relative: &str) -> Result<PathBuf> {
        relative_path(relative)?;
        let path = self.root_dir.join(relative);
        let mut ancestor = path.clone();
        loop {
            match fs::symlink_metadata(&ancestor) {
                Ok(meta) => {
                    // Do not follow even an internal symlink: replacing it can
                    // otherwise unexpectedly alter which file owns the note.
                    if meta.file_type().is_symlink() {
                        return Err(error(format!(
                            "symlink path is not a reading-note/source file: {relative}"
                        )));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            if ancestor == self.root_dir {
                break;
            }
            if !ancestor.pop() {
                return Err(error("path escapes graph"));
            }
        }
        let mut existing = path.clone();
        while !existing.exists() {
            existing.pop();
        }
        if !existing
            .canonicalize()?
            .starts_with(self.root_dir.canonicalize()?)
        {
            return Err(error("path escapes graph"));
        }
        Ok(path)
    }

    fn reading_document(&self, path: &Path) -> Result<Document> {
        let relative = path
            .strip_prefix(&self.root_dir)
            .map_err(|_| error("note outside graph"))?
            .to_string_lossy();
        self.reading_path(&relative)?;
        let content = fs::read_to_string(path)?;
        let (properties, body) = header(&content)
            .ok_or_else(|| error("invalid annotation property header; file preserved"))?;
        let metadata = metadata(&properties)?;
        let body = body.to_string();
        let revision_range = 0..content.len();
        Ok(Document {
            path: path.into(),
            content,
            properties,
            metadata,
            body,
            storage: ReadingNoteStorage::File,
            revision_range,
        })
    }

    fn reading_source(&self, conn: &Connection, page_id: &str) -> Result<Source> {
        let page = self.db.get_page_by_id_in_connection(conn, page_id)?;
        let path = self.reading_path(
            page.file_path
                .as_deref()
                .ok_or_else(|| error("source has no file"))?,
        )?;
        let content = fs::read_to_string(&path)?;
        let parsed_page = parser::parse_page(
            &content,
            path.file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("page.md"),
        );
        let mut parsed = Vec::new();
        flattened(&parsed_page.blocks, &mut parsed);
        parsed.retain(|b| {
            !is_note_page(&b.properties)
                && !(b.content.contains("[^grafium-note-")
                    && parser::strip_reading_note_references(&b.content)
                        .trim()
                        .is_empty())
        });
        let db_blocks = self.db.list_blocks_for_page_in_connection(conn, page_id)?;
        let mut blocks: Vec<_> = scoped_block_order(&db_blocks, None)?
            .into_iter()
            .map(|i| db_blocks[i].clone())
            .filter(|b| {
                !parser::is_reading_note_block(b)
                    && !(b.content.contains("[^grafium-note-")
                        && parser::strip_reading_note_references(&b.content)
                            .trim()
                            .is_empty())
            })
            .collect();
        if parsed.len() != blocks.len()
            || parsed
                .iter()
                .zip(&blocks)
                .any(|(p, b)| p.content != b.content || p.id.as_ref().is_some_and(|id| id != &b.id))
        {
            return Err(error(
                "source changed on disk; refresh the source before selecting text",
            ));
        }
        for block in &mut blocks {
            block.content = parser::strip_reading_note_references(&block.content);
        }
        for block in &mut parsed {
            block.content = parser::strip_reading_note_references(&block.content);
        }
        Ok(Source {
            page,
            path,
            hash: Self::content_hash(&parser::reading_notes::source_evidence(&content)),
            blocks,
            parsed,
            raw_hash: Self::content_hash(&content),
        })
    }

    fn reading_anchor(
        &self,
        conn: &Connection,
        page_id: &str,
        selection: Option<&ReadingSelection>,
    ) -> Result<Anchor> {
        let source = self.reading_source(conn, page_id)?;
        let mut anchor = Anchor {
            source_path: source.page.file_path.clone().unwrap(),
            source_page_id: source.page.id.clone(),
            source_title: source.page.title.clone(),
            source_file_sha256: source.hash,
            offset_unit: "utf-16".into(),
            kind: selection.map_or(SelectionKind::Source, |s| s.kind),
            quote: selection.map_or(String::new(), |s| s.text.clone()),
            parts: Vec::new(),
            hash_mode: Some("annotation-free-v1".into()),
        };
        if let Some(selection) = selection {
            if selection.page_id != page_id
                || selection.parts.is_empty()
                || selection.block_ids
                    != selection
                        .parts
                        .iter()
                        .map(|p| p.block_id.clone())
                        .collect::<Vec<_>>()
                || selection.text
                    != selection
                        .parts
                        .iter()
                        .map(|p| p.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                || selection.text.trim().is_empty()
            {
                return Err(error("selection page, block IDs, and parts do not match"));
            }
            if let Some(range) = &selection.document_range {
                if selection.kind != SelectionKind::Source
                    || range.to <= range.from
                    || range.text.encode_utf16().count() != range.to - range.from
                    || utf16_slice(&fs::read_to_string(&source.path)?, range.from, range.to)
                        .as_deref()
                        != Some(&range.text)
                {
                    return Err(error("invalid UTF-16 document selection"));
                }
            }
            let mut previous = None;
            for part in &selection.parts {
                let ordinal = source
                    .blocks
                    .iter()
                    .position(|b| b.id == part.block_id)
                    .ok_or_else(|| error("selected block is not on the source page"))?;
                if previous.is_some_and(|p| p >= ordinal)
                    || part.to <= part.from
                    || part.text.encode_utf16().count() != part.to - part.from
                    || part.text.is_empty()
                {
                    return Err(error(
                        "selection must use ordered distinct blocks and valid UTF-16 offsets",
                    ));
                }
                previous = Some(ordinal);
                let block = &source.blocks[ordinal];
                let saved = AnchorPart {
                    block_id: block.id.clone(),
                    persisted_id: source.parsed[ordinal].id.as_ref() == Some(&block.id),
                    ordinal,
                    raw_fingerprint: Self::content_hash(&block.content),
                    text: part.text.clone(),
                    from: part.from,
                    to: part.to,
                    prefix: parser::strip_reading_note_references(&part.prefix),
                    suffix: parser::strip_reading_note_references(&part.suffix),
                };
                if selection.kind == SelectionKind::Source {
                    let before = utf16_slice(&block.content, 0, part.from).unwrap_or_default();
                    let after = utf16_slice(
                        &block.content,
                        part.to,
                        block.content.encode_utf16().count(),
                    )
                    .unwrap_or_default();
                    if utf16_slice(&block.content, part.from, part.to).as_deref()
                        != Some(&part.text)
                        || !before.ends_with(&saved.prefix)
                        || !after.starts_with(&saved.suffix)
                    {
                        return Err(error("selected source text changed; select it again"));
                    }
                } else if context_occurrences(&block.content, &saved, selection.kind).len() != 1 {
                    return Err(error("rendered selection is stale or ambiguous; select source text for exact offsets"));
                }
                anchor.parts.push(saved);
            }
        }
        // Selection is checked against one file snapshot, never a rewritten
        // source just to persist IDs.
        if Self::content_hash(&fs::read_to_string(&source.path)?) != source.raw_hash {
            return Err(error("source changed while capturing the annotation"));
        }
        Ok(anchor)
    }

    fn note_files(&self, warnings: &mut Vec<String>) -> Result<Vec<PathBuf>> {
        fn walk(
            graph: &Graph,
            directory: &Path,
            out: &mut Vec<PathBuf>,
            warnings: &mut Vec<String>,
        ) -> Result<()> {
            if fs::symlink_metadata(directory).is_ok_and(|m| m.file_type().is_symlink()) {
                warnings.push(format!("Skipped symlink directory {}", directory.display()));
                return Ok(());
            }
            for entry in fs::read_dir(directory)? {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        warnings.push(e.to_string());
                        continue;
                    }
                };
                let path = entry.path();
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    walk(graph, &path, out, warnings)?;
                } else if kind.is_symlink() {
                    warnings.push(format!("Skipped symlink {}", path.display()));
                } else if path.extension().and_then(|v| v.to_str()) == Some("md") {
                    match fs::read_to_string(&path) {
                        Ok(content) => {
                            let in_folder = path.starts_with(graph.root_dir.join(DIRECTORY));
                            if in_folder
                                || content.contains(parser::reading_notes::OPEN)
                                || content
                                    .lines()
                                    .take_while(|l| !l.is_empty())
                                    .any(|l| l.starts_with("reading-note::"))
                            {
                                out.push(path);
                            }
                        }
                        Err(e) => warnings.push(format!("{}: {e}", path.display())),
                    }
                }
            }
            Ok(())
        }
        let mut files = Vec::new();
        for dir in [&self.pages_dir, &self.journals_dir, &self.knowledge_dir] {
            if dir.exists() {
                walk(self, dir, &mut files, warnings)?;
            }
        }
        files.sort();
        Ok(files)
    }

    fn find_reading_note(&self, id: &str) -> Result<Option<Document>> {
        uuid(id)?;
        let mut warnings = Vec::new();
        let mut found = None;
        for path in self.note_files(&mut warnings)? {
            for document in self.reading_documents(&path, &mut warnings)? {
                if document.metadata.id == id {
                    if found.is_some() {
                        return Err(error(
                            "duplicate note ID in files; resolve the collision before editing",
                        ));
                    }
                    found = Some(document);
                }
            }
        }
        Ok(found)
    }

    fn reading_page_id(&self, path: &Path) -> Result<Option<String>> {
        let relative = path
            .strip_prefix(&self.root_dir)
            .map_err(|_| error("path outside graph"))?
            .to_string_lossy();
        let conn = self.db.conn()?;
        Ok(conn
            .query_row(
                "SELECT id FROM pages WHERE file_path = ?1",
                [relative.as_ref()],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Source renames are recovered only by unique, verified file identity.
    /// Equal titles, nearby positions and graph-wide fuzzy text never qualify.
    fn resolve_reading_source(&self, anchor: &Anchor) -> Result<Option<(Source, bool)>> {
        let original = self.reading_path(&anchor.source_path)?;
        if original.exists() {
            self.index_file(&original)?;
            let id = self
                .reading_page_id(&original)?
                .ok_or_else(|| error("source could not be indexed"))?;
            return Ok(Some((self.reading_source(&*self.db.conn()?, &id)?, false)));
        }
        let mut matches = Vec::new();
        for (_, relative) in self.db.list_file_backed_page_paths()? {
            let path = self.reading_path(&relative)?;
            if !path.exists() {
                continue;
            }
            let content = fs::read_to_string(&path)?;
            let identical = Self::content_hash(&if anchor.hash_mode.is_some() {
                parser::reading_notes::source_evidence(&content)
            } else {
                content.clone()
            }) == anchor.source_file_sha256;
            let stable = !anchor.parts.is_empty() && anchor.parts.iter().all(|p| p.persisted_id);
            if !identical && !stable {
                continue;
            }
            if !identical {
                let parsed = parser::parse_page(
                    &content,
                    path.file_name()
                        .and_then(|v| v.to_str())
                        .unwrap_or("page.md"),
                );
                let mut blocks = Vec::new();
                flattened(&parsed.blocks, &mut blocks);
                if !anchor.parts.iter().all(|p| {
                    blocks
                        .iter()
                        .filter(|b| {
                            b.id.as_ref() == Some(&p.block_id)
                                && context_occurrences(&b.content, p, anchor.kind).len() == 1
                        })
                        .count()
                        == 1
                }) {
                    continue;
                }
            }
            self.index_file(&path)?;
            if let Some(id) = self.reading_page_id(&path)? {
                matches.push(self.reading_source(&*self.db.conn()?, &id)?);
            }
        }
        if matches.len() > 1 {
            return Err(error(
                "source identity matches multiple files; reattach explicitly",
            ));
        }
        Ok(matches.pop().map(|s| (s, true)))
    }

    fn reading_dto(&self, document: &Document) -> Result<ReadingNote> {
        self.reading_dto_with_index(document, true)
    }

    fn reading_dto_with_index(&self, document: &Document, index: bool) -> Result<ReadingNote> {
        if index {
            self.index_file(&document.path).map_err(|e| {
                error(format!(
                    "note is safe at {}; index failed: {e}",
                    document.path.display()
                ))
            })?;
        }
        let note_page_id = self
            .reading_page_id(&document.path)?
            .ok_or_else(|| error("note index is missing"))?;
        let anchor = &document.metadata.anchor;
        let mut note = ReadingNote {
            id: document.metadata.id.clone(),
            note_page_id,
            file_path: document
                .path
                .strip_prefix(&self.root_dir)
                .unwrap()
                .to_string_lossy()
                .into(),
            body: document.body.clone(),
            revision: document_revision(document)?,
            source: ReadingNoteSource {
                page_id: None,
                page_title: anchor.source_title.clone(),
                file_path: anchor.source_path.clone(),
            },
            quote: anchor.quote.clone(),
            status: ReadingNoteStatus::Orphaned,
            status_message:
                "Source file is missing. The note is preserved; reattach it explicitly.".into(),
            target_block_id: None,
            created_at: document.metadata.created_at.clone(),
            updated_at: document.metadata.updated_at.clone(),
            storage: document.storage,
            footnote_label: document.metadata.footnote_label.clone(),
            note_block_id: Some(document.metadata.body_block_id.clone()),
        };
        let resolution = if document.storage == ReadingNoteStorage::Inline {
            self.reading_source(&*self.db.conn()?, &note.note_page_id)
                .map(|s| {
                    let renamed = s.page.file_path.as_deref() != Some(anchor.source_path.as_str());
                    Some((s, renamed))
                })
        } else {
            self.resolve_reading_source(anchor)
        };
        let source = match resolution {
            Ok(Some(source)) => source,
            Ok(None) => return Ok(note),
            Err(e) => {
                note.status_message = e.to_string();
                if note.status_message.contains("multiple files") {
                    note.status = ReadingNoteStatus::Ambiguous;
                }
                return Ok(note);
            }
        };
        let (source, renamed) = source;
        note.source = ReadingNoteSource {
            page_id: Some(source.page.id.clone()),
            page_title: source.page.title.clone(),
            file_path: source.page.file_path.clone().unwrap(),
        };
        let unchanged = if anchor.hash_mode.is_some() {
            &source.hash
        } else {
            &source.raw_hash
        } == &anchor.source_file_sha256;
        let mut resolved = Vec::new();
        let mut recovered = renamed || source.page.id != anchor.source_page_id;
        for part in &anchor.parts {
            let mut candidates = Vec::new();
            // Persisted UUIDs qualify only when the quote still exists in that
            // block. An in-memory slot-reused UUID is NOT stable identity.
            if part.persisted_id {
                for (i, block) in source.blocks.iter().enumerate() {
                    if block.id == part.block_id && source.parsed[i].id.as_ref() == Some(&block.id)
                    {
                        let text = representation(&block.content, anchor.kind);
                        let quote = context(&part.text, anchor.kind);
                        if occurrences(&text, &quote, "", "").len() == 1 {
                            candidates.push(i);
                        } else if context_occurrences(&block.content, part, anchor.kind).len() == 1
                        {
                            candidates.push(i);
                        }
                    }
                }
            }
            if candidates.is_empty() && unchanged {
                if let Some(block) = source.blocks.get(part.ordinal) {
                    if Self::content_hash(&block.content) == part.raw_fingerprint {
                        candidates.push(part.ordinal);
                        recovered |= block.id != part.block_id;
                    }
                }
            }
            if candidates.is_empty() && !unchanged {
                for (i, block) in source.blocks.iter().enumerate() {
                    let hits = context_occurrences(&block.content, part, anchor.kind);
                    candidates.extend(std::iter::repeat(i).take(hits.len()));
                }
                recovered = true;
            }
            if candidates.len() != 1 {
                note.status = if candidates.len() > 1 {
                    ReadingNoteStatus::Ambiguous
                } else {
                    ReadingNoteStatus::Orphaned
                };
                note.status_message = if candidates.len() > 1 {
                    "Several passages match the quote and context. Reattach explicitly; no position was guessed."
                } else {
                    "The selected passage changed or was removed. The note and original quote are preserved."
                }.into();
                return Ok(note);
            }
            let i = candidates[0];
            if resolved.last().is_some_and(|previous| *previous >= i) {
                note.status = ReadingNoteStatus::Ambiguous;
                note.status_message = "The selected blocks no longer resolve in their original order. Reattach explicitly.".into();
                return Ok(note);
            }
            resolved.push(i);
        }
        note.target_block_id = resolved.first().map(|i| source.blocks[*i].id.clone());
        note.status = if recovered {
            ReadingNoteStatus::Recovered
        } else {
            ReadingNoteStatus::Attached
        };
        note.status_message = if recovered {
            "Recovered using verified source identity and exact passage evidence."
        } else if anchor.parts.is_empty() {
            "Attached to this page."
        } else {
            "Attached to the selected passage."
        }
        .into();
        Ok(note)
    }

    pub fn reading_notes_list(&self, page_id: Option<&str>) -> Result<ReadingNotesList> {
        let mut result = ReadingNotesList::default();
        let scoped_path = page_id
            .and_then(|id| self.db.get_page_by_id(id).ok())
            .and_then(|page| page.file_path)
            .map(|path| self.root_dir.join(path));
        let mut documents = Vec::new();
        let paths = if page_id.is_some() {
            self.scoped_reading_note_files(scoped_path.as_deref(), &mut result.warnings)?
        } else {
            self.note_files(&mut result.warnings)?
        };
        for path in paths {
            documents.extend(self.reading_documents(&path, &mut result.warnings)?);
        }
        let mut ids = HashMap::new();
        let mut block_ids = HashMap::new();
        for doc in &documents {
            *ids.entry(doc.metadata.id.as_str()).or_insert(0) += 1;
            *block_ids
                .entry(doc.metadata.body_block_id.as_str())
                .or_insert(0) += 1;
        }
        let duplicate_paths: HashSet<_> = documents
            .iter()
            .filter(|doc| {
                ids[doc.metadata.id.as_str()] > 1
                    || block_ids[doc.metadata.body_block_id.as_str()] > 1
            })
            .map(|doc| doc.path.clone())
            .collect();
        let mut index_results = HashMap::new();
        for document in &documents {
            if duplicate_paths.contains(&document.path) {
                result.warnings.push(format!(
                    "{}: duplicate annotation UUID/bodyBlockId; file quarantined, nothing indexed",
                    document.path.display()
                ));
                continue;
            }
            if page_id.is_some()
                && document.storage == ReadingNoteStorage::Inline
                && scoped_path.as_ref() != Some(&document.path)
            {
                continue;
            }
            let path = &document.path;
            let indexed = index_results
                .entry(path.clone())
                .or_insert_with(|| self.index_file(path).map_err(|e| e.to_string()));
            if let Err(e) = indexed {
                result.warnings.push(format!("{}: {e}", path.display()));
                continue;
            }
            match self
                .reading_dto_with_index(document, false)
                .map(|note| (&document.metadata.anchor.source_page_id, note))
            {
                Ok((hint, note))
                    if page_id.is_none_or(|id| {
                        note.source.page_id.as_deref() == Some(id) || hint == id
                    }) =>
                {
                    result.notes.push(note)
                }
                Ok(_) => {}
                Err(e) => result.warnings.push(format!("{}: {e}", path.display())),
            }
        }
        result
            .notes
            .sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(result)
    }

    #[cfg(test)]
    fn create_file_reading_note(
        &self,
        note_id: &str,
        source_page_id: &str,
        selection: Option<&ReadingSelection>,
        body: &str,
    ) -> Result<ReadingNote> {
        uuid(note_id)?;
        let request =
            Self::content_hash(&serde_json::to_string(&(source_page_id, selection, body))?);
        if let Some(document) = self.find_reading_note(note_id)? {
            if identical_create_request(&document, source_page_id, selection, body, &request) {
                return self.reading_dto(&document);
            }
            return Err(error(format!(
                "note ID collision; existing file preserved at {}",
                document.path.display()
            )));
        }
        let path = self.reading_path(&format!("{DIRECTORY}/{note_id}.md"))?;
        if fs::symlink_metadata(&path).is_ok() {
            return Err(error(format!(
                "file already exists; preserved at {}",
                path.display()
            )));
        }
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let anchor = self.reading_anchor(&tx, source_page_id, selection)?;
        let now = Utc::now().to_rfc3339();
        let metadata = Metadata {
            version: 1,
            id: note_id.into(),
            body_block_id: Uuid::new_v4().to_string(),
            created_at: now.clone(),
            updated_at: now,
            create_request_hash: Some(request),
            anchor,
            footnote_label: None,
        };
        let properties = serde_json::json!({ PROPERTY: serde_json::to_string(&metadata)? });
        let content = encode(&properties, body);
        self.persist_reading_note(tx, &path, None, &content)?;
        self.finish_reading_note(&path)
    }

    pub fn reading_note_update(
        &self,
        note_id: &str,
        expected_revision: &str,
        body: &str,
    ) -> Result<ReadingNote> {
        self.change_reading_note(note_id, expected_revision, Some(body), None)
    }

    pub fn reading_note_reattach(
        &self,
        note_id: &str,
        expected_revision: &str,
        source_page_id: &str,
        selection: Option<&ReadingSelection>,
    ) -> Result<ReadingNote> {
        self.change_reading_note(
            note_id,
            expected_revision,
            None,
            Some((source_page_id, selection)),
        )
    }

    fn change_reading_note(
        &self,
        note_id: &str,
        expected_revision: &str,
        body: Option<&str>,
        source: Option<(&str, Option<&ReadingSelection>)>,
    ) -> Result<ReadingNote> {
        if !sha256(expected_revision) {
            return Err(error(
                "expectedRevision must be the annotation SHA-256 revision",
            ));
        }
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut document = self
            .find_reading_note(note_id)?
            .ok_or_else(|| error("note file not found; no file was overwritten"))?;
        if document_revision(&document)? != expected_revision {
            return Err(error(format!(
                "revision conflict; reload the external edit at {}",
                document.path.display()
            )));
        }
        if document.storage == ReadingNoteStorage::Inline {
            return self.change_inline_reading_note(tx, document, body, source);
        }
        if let Some((page_id, selection)) = source {
            document.metadata.anchor = self.reading_anchor(&tx, page_id, selection)?;
        }
        document.metadata.updated_at = Utc::now().to_rfc3339();
        document.metadata.create_request_hash = None;
        document.properties[PROPERTY] = serde_json::to_string(&document.metadata)?.into();
        let content = encode(&document.properties, body.unwrap_or(&document.body));
        self.persist_reading_note(tx, &document.path, Some(&document.content), &content)?;
        self.finish_reading_note(&document.path)
    }

    fn finish_reading_note(&self, path: &Path) -> Result<ReadingNote> {
        self.reading_document(path).and_then(|document| self.reading_dto(&document))
            .map_err(|e| error(format!("file was saved at {} but could not be read back: {e}; preserve this recovery file", path.display())))
    }

    fn persist_reading_note(
        &self,
        tx: rusqlite::Transaction<'_>,
        path: &Path,
        original: Option<&str>,
        content: &str,
    ) -> Result<()> {
        self.persist_reading_note_with_hook(tx, path, original, content, |_| Ok(()))
    }

    fn persist_reading_note_with_hook(
        &self,
        tx: rusqlite::Transaction<'_>,
        path: &Path,
        original: Option<&str>,
        content: &str,
        before_replace: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<()> {
        let relative = path
            .strip_prefix(&self.root_dir)
            .map_err(|_| error("note outside graph"))?
            .to_string_lossy()
            .to_string();
        self.reading_path(&relative)?;
        self.ensure_reading_note_files_unique(path, content)?;
        self.guard_managed_block_ids(
            &tx,
            path,
            &parser::parse_page(
                content,
                path.file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or("note.md"),
            ),
        )?;
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent)?;
        self.reading_path(&relative)?;
        let pending = parent.join(format!(".reading-note-{}.pending", Uuid::new_v4()));
        Self::atomic_write(&pending, content).map_err(|e| {
            error(format!(
                "write failed ({e}); draft recovery path: {}",
                pending.display()
            ))
        })?;
        let mut backup = None;
        if let Some(original) = original {
            if fs::read_to_string(path).ok().as_deref() != Some(original) {
                return Err(error(format!(
                    "revision conflict; external edit preserved at {}. New draft: {}",
                    path.display(),
                    pending.display()
                )));
            }
            before_replace(path)?;
            let displaced = super::reading_note_replace::replace_preserving_displaced(path, &pending)
                .map_err(|e| error(format!("safe replacement failed ({e}); original: {}; draft: {}; possible displaced-file recovery: {}", path.display(), pending.display(), pending.with_extension("displaced").display())))?;
            if fs::symlink_metadata(&displaced)?.file_type().is_symlink()
                || fs::read_to_string(&displaced).ok().as_deref() != Some(original)
            {
                return Err(error(format!("revision conflict AT replacement. Draft now at {}; ACTUAL displaced external file preserved at {}. Merge these files; no successful save was reported.", path.display(), displaced.display())));
            }
            backup = Some(displaced);
        } else {
            // Hard-link publication is atomic and fails if ANY file already owns
            // the destination. A pre-check plus rename would clobber a race.
            fs::hard_link(&pending, path).map_err(|e| {
                error(format!(
                    "creation collision or filesystem failure ({e}); draft preserved at {}",
                    pending.display()
                ))
            })?;
            let _ = fs::remove_file(&pending);
        }
        #[cfg(unix)]
        fs::File::open(parent).and_then(|file| file.sync_all())
            .map_err(|e| error(format!("file published at {} but durability flush failed: {e}; preserve this file and retry", path.display())))?;
        let result = (|| -> Result<()> {
            if fs::read_to_string(path)? != content {
                return Err(error("concurrent file edit preserved; reload"));
            }
            let parsed = parser::parse_page(
                content,
                path.file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or("note.md"),
            );
            let filename = path
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("note.md");
            let is_journal = path.starts_with(&self.journals_dir) && parsed.is_journal;
            let title = if path.starts_with(&self.knowledge_dir) {
                format!(
                    "Knowledge/{}",
                    Self::title_from_file_path(&self.knowledge_dir, path, filename)
                )
            } else {
                let canonical = if is_journal {
                    parser::canonical_journal_title(filename)
                } else {
                    None
                };
                canonical.or(parsed.title.clone()).unwrap_or_else(|| {
                    let base = if path.starts_with(&self.journals_dir) {
                        &self.journals_dir
                    } else {
                        &self.pages_dir
                    };
                    Self::title_from_file_path(base, path, filename)
                })
            };
            let conflicting: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pages WHERE title = ?1 AND file_path IS NOT NULL AND file_path != ?2)",
                rusqlite::params![title, relative], |r| r.get(0),
            )?;
            if conflicting {
                return Err(error("another indexed file owns this page title"));
            }
            let page = self.db.upsert_page_in_connection(
                &tx,
                &title,
                is_journal,
                Some(&relative),
                &parsed.properties,
            )?;
            self.db
                .sync_page_properties_in_connection(&tx, &page.id, &parsed.properties)?;
            self.apply_parsed_blocks_in_connection(&tx, &page.id, &parsed.blocks)?;
            if fs::read_to_string(path)? != content {
                return Err(error("concurrent external edit preserved; reload"));
            }
            tx.commit()?;
            Ok(())
        })();
        if let Err(e) = result {
            let recovery = if let Some(backup) = &backup {
                format!(" Original backup: {}", backup.display())
            } else {
                String::new()
            };
            return Err(error(format!("file saved at {} but index update failed: {e}. Reopen/reindex to recover.{recovery}", path.display())));
        }
        if let Some(backup) = &backup {
            let _ = fs::remove_file(backup);
        }
        self.note_self_write(path);
        self.remember_indexed_content_hash(path, Self::content_hash(content));
        self.forget_canonical_reading_note(path);
        if let Some(id) = self.reading_page_id(path)? {
            self.mark_page_dirty(&id);
            self.record_page_edit(&id, "file");
        }
        Ok(())
    }

    fn forget_canonical_reading_note(&self, path: &Path) {
        if let Ok(mut hashes) = self.canonical_content_hashes.lock() {
            hashes.remove(path);
        }
    }
}
