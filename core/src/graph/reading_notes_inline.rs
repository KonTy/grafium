use super::*;
use crate::parser::reading_notes::{
    parse_inline_reading_notes, serialize_inline_reading_note, source_evidence,
    strip_label_references,
};

#[cfg(test)]
#[path = "reading_notes_inline_tests.rs"]
mod tests;

fn table_ast(content: &str) -> Vec<String> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut inside = false;
    let mut result = Vec::new();
    for event in Parser::new_ext(content, Options::all()) {
        if matches!(event, Event::Start(Tag::Table(_))) {
            inside = true;
        }
        if inside {
            result.push(format!("{event:?}"));
        }
        if matches!(event, Event::End(TagEnd::Table)) {
            inside = false;
        }
    }
    result
}

fn has_structural_reference_boundary(content: &str) -> bool {
    use pulldown_cmark::{Event, Options, Parser, Tag};
    Parser::new_ext(content, Options::all()).any(|event| {
        matches!(
            event,
            Event::Start(
                Tag::Table(_)
                    | Tag::Heading { .. }
                    | Tag::CodeBlock(_)
                    | Tag::HtmlBlock
                    | Tag::FootnoteDefinition(_)
            ) | Event::Rule
        )
    })
}

fn identities(content: &str) -> Result<Vec<(String, String)>> {
    let mut result = Vec::new();
    if let Some((properties, _)) = header(content) {
        if let Ok(meta) = metadata(&properties) {
            result.push((meta.id, meta.body_block_id));
        }
    }
    let parsed = parse_inline_reading_notes(content);
    if !parsed.warnings.is_empty() {
        return Err(error(parsed.warnings.join("; ")));
    }
    let mut labels = HashSet::new();
    for note in parsed.notes {
        if !labels.insert(note.label) {
            return Err(error("duplicate managed footnote label; file quarantined"));
        }
        result.push((
            note.metadata["id"].as_str().unwrap().into(),
            note.metadata["bodyBlockId"].as_str().unwrap().into(),
        ));
    }
    Ok(result)
}

impl Graph {
    pub(crate) fn try_update_inline_source_block(
        &self,
        block: &Block,
        content: &str,
        properties: Option<&serde_json::Value>,
    ) -> Result<bool> {
        let page = self.db.get_page_by_id(&block.page_id)?;
        let Some(relative) = page.file_path.as_deref() else {
            return Ok(false);
        };
        let path = self.reading_path(relative)?;
        let original = fs::read_to_string(&path)?;
        let annotations = parse_inline_reading_notes(&original);
        if annotations.notes.is_empty() && annotations.warnings.is_empty() {
            return Ok(false);
        }
        if !annotations.warnings.is_empty() {
            return Err(error(annotations.warnings.join("; ")));
        }
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let blocks = self.db.list_blocks_for_page_in_connection(&tx, &page.id)?;
        let mut ordered: Vec<_> = scoped_block_order(&blocks, None)?
            .into_iter()
            .map(|i| blocks[i].clone())
            .collect();
        let parsed_page = parser::parse_page(
            &original,
            path.file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("source.md"),
        );
        let mut parsed = Vec::new();
        flattened(&parsed_page.blocks, &mut parsed);
        if parsed.len() != ordered.len()
            || parsed_page.properties != page.properties
            || parsed.iter().zip(&ordered).any(|(parsed, saved)| {
                parsed.content != saved.content
                    || parsed.id.as_ref().is_some_and(|id| id != &saved.id)
                    || parsed.properties != saved.properties
            })
        {
            return Err(error("revision conflict; source or annotations changed externally. Refresh before editing"));
        }
        let edited = ordered
            .iter_mut()
            .find(|b| b.id == block.id)
            .ok_or_else(|| error("source block disappeared"))?;
        if edited.content != block.content {
            return Err(error("source block changed concurrently"));
        }
        edited.content = content.into();
        if let Some(properties) = properties {
            edited.properties = properties.clone();
        }
        let output = parser::serialize_page(&page.properties, &ordered);
        self.persist_reading_note(tx, &path, Some(&original), &output)?;
        Ok(true)
    }

    pub(crate) fn update_reading_note_block(
        &self,
        block: &Block,
        body: &str,
        properties: Option<&serde_json::Value>,
    ) -> Result<()> {
        if properties.is_some_and(|p| p != &block.properties) {
            return Err(error("managed annotation properties are guarded; edit the body or use Notes → Reattach instead"));
        }
        let saved = metadata(&block.properties)?;
        let document = self
            .find_reading_note(&saved.id)?
            .ok_or_else(|| error("annotation file is missing"))?;
        if document.body != block.content || document.metadata.body_block_id != block.id {
            return Err(error(
                "revision conflict; annotation changed externally. Refresh before editing",
            ));
        }
        self.reading_note_update(&saved.id, &document_revision(&document)?, body)
            .map(|_| ())
    }

    pub(crate) fn update_reading_source_file(
        &self,
        path: &Path,
        original: &str,
        content: &str,
    ) -> Result<()> {
        self.update_reading_source_file_with_hook(path, original, content, |_| Ok(()))
    }

    fn update_reading_source_file_with_hook(
        &self,
        path: &Path,
        original: &str,
        content: &str,
        before_replace: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<()> {
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.persist_reading_note_with_hook(tx, path, Some(original), content, before_replace)
    }

    pub(super) fn scoped_reading_note_files(
        &self,
        source: Option<&Path>,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<PathBuf>> {
        fn legacy_files(
            directory: &Path,
            files: &mut Vec<PathBuf>,
            warnings: &mut Vec<String>,
        ) -> Result<()> {
            let entries = match fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e.into()),
            };
            for entry in entries {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_symlink() {
                    warnings.push(format!("Skipped symlink {}", entry.path().display()));
                } else if kind.is_dir() {
                    legacy_files(&entry.path(), files, warnings)?;
                } else if entry.path().extension().is_some_and(|e| e == "md") {
                    files.push(entry.path());
                }
            }
            Ok(())
        }
        let mut files = source
            .filter(|p| p.exists())
            .map(|p| vec![p.to_path_buf()])
            .unwrap_or_default();
        let legacy = self.root_dir.join(DIRECTORY);
        if !fs::symlink_metadata(&legacy).is_ok_and(|m| m.file_type().is_symlink()) {
            legacy_files(&legacy, &mut files, warnings)?;
        }
        let conn = self.db.conn()?;
        let mut statement = conn.prepare(
            "SELECT file_path FROM pages WHERE file_path IS NOT NULL AND json_extract(properties, '$.\"reading-note\"') IS NOT NULL"
        )?;
        for row in statement.query_map([], |row| row.get::<_, String>(0))? {
            let path = self.reading_path(&row?)?;
            if path.exists() {
                files.push(path);
            }
        }
        files.sort();
        files.dedup();
        Ok(files)
    }

    pub(super) fn reading_documents(
        &self,
        path: &Path,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<Document>> {
        let relative = path
            .strip_prefix(&self.root_dir)
            .map_err(|_| error("note outside graph"))?
            .to_string_lossy();
        self.reading_path(&relative)?;
        let content = fs::read_to_string(path)?;
        if header(&content).is_some() || path.starts_with(self.root_dir.join(DIRECTORY)) {
            match self.reading_document(path) {
                Ok(document) => return Ok(vec![document]),
                Err(e) if !content.contains(crate::parser::reading_notes::OPEN) => {
                    warnings.push(format!("{}: {e}", path.display()));
                    return Ok(Vec::new());
                }
                Err(_) => {}
            }
        }
        let parsed = parse_inline_reading_notes(&content);
        warnings.extend(
            parsed
                .warnings
                .into_iter()
                .map(|warning| format!("{}: {warning}", path.display())),
        );
        parsed
            .notes
            .into_iter()
            .map(|note| {
                let properties = parser::reading_notes::note_properties(&note.metadata, "inline");
                let metadata = metadata(&properties)?;
                Ok(Document {
                    path: path.into(),
                    content: content.clone(),
                    properties,
                    metadata,
                    body: note.body,
                    storage: ReadingNoteStorage::Inline,
                    revision_range: note.range,
                })
            })
            .collect()
    }

    /// Run before ANY indexing, including full rebuilds. A copied managed UUID
    /// cannot acquire another file's block via SQLite INSERT OR REPLACE.
    pub(crate) fn ensure_reading_note_files_unique(
        &self,
        path: &Path,
        content: &str,
    ) -> Result<()> {
        let current = identities(content)?;
        if current.is_empty() {
            return Ok(());
        }
        let mut ids = HashSet::new();
        let mut block_ids = HashSet::new();
        for (id, block_id) in &current {
            if !ids.insert(id.as_str()) || !block_ids.insert(block_id.as_str()) {
                return Err(error(format!(
                    "duplicate managed UUID inside {}; file quarantined",
                    path.display()
                )));
            }
        }
        let mut warnings = Vec::new();
        for other in self.note_files(&mut warnings)? {
            if other == path {
                continue;
            }
            let other_content = fs::read_to_string(&other)?;
            // Malformed unrelated notes should not prevent healthy books from
            // opening. Their own index/list operation reports their warning.
            for (id, body_id) in identities(&other_content).unwrap_or_default() {
                if ids.contains(id.as_str()) || block_ids.contains(body_id.as_str()) {
                    return Err(error(format!("duplicate annotation UUID/bodyBlockId; BOTH files quarantined before indexing: {} and {}", path.display(), other.display())));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn guard_managed_block_ids(
        &self,
        conn: &Connection,
        path: &Path,
        parsed: &parser::ParsedPage,
    ) -> Result<()> {
        let relative = path
            .strip_prefix(&self.root_dir)
            .map_err(|_| error("note outside graph"))?
            .to_string_lossy();
        let mut blocks = Vec::new();
        flattened(&parsed.blocks, &mut blocks);
        for block in blocks {
            let Some(id) = block.id else {
                continue;
            };
            let existing: Option<(Option<String>, String)> = conn.query_row(
                "SELECT p.file_path, b.properties FROM blocks b JOIN pages p ON p.id = b.page_id WHERE b.id = ?1",
                [&id], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            if let Some((old_path, properties)) = existing {
                let existing_managed = serde_json::from_str::<serde_json::Value>(&properties)
                    .ok()
                    .is_some_and(|p| is_note_page(&p));
                if old_path.as_deref() != Some(relative.as_ref())
                    && (is_note_page(&block.properties) || existing_managed)
                {
                    return Err(error(format!(
                        "managed bodyBlockId {id} already belongs to {}; cannot index {}",
                        old_path.as_deref().unwrap_or("another page"),
                        path.display()
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn reading_note_create(
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
                "note ID collision; existing annotation preserved at {}",
                document.path.display()
            )));
        }
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let anchor = self.reading_anchor(&tx, source_page_id, selection)?;
        let source = self.reading_source(&tx, source_page_id)?;
        if source
            .page
            .properties
            .get(PROPERTY)
            .is_some_and(|_| is_note_page(&source.page.properties))
        {
            return Err(error("legacy annotation pages remain editable, but cannot host inline source annotations"));
        }
        let original = fs::read_to_string(&source.path)?;
        let parsed = parse_inline_reading_notes(&original);
        if !parsed.warnings.is_empty() {
            return Err(error(parsed.warnings.join("; ")));
        }
        static LABEL: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
            regex::Regex::new(r"\[\^grafium-note-([1-9][0-9]*)\]").unwrap()
        });
        let next = LABEL
            .captures_iter(&original)
            .filter_map(|c| c[1].parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| error("footnote sequence exhausted"))?;
        let label = format!("grafium-note-{next}");
        let now = Utc::now().to_rfc3339();
        let metadata = Metadata {
            version: 2,
            id: note_id.into(),
            body_block_id: Uuid::new_v4().to_string(),
            created_at: now.clone(),
            updated_at: now,
            create_request_hash: Some(request),
            anchor,
            footnote_label: Some(label.clone()),
        };
        let referenced = self.place_reading_reference(&source, &original, selection, &label)?;
        let mut content = referenced;
        if !content.ends_with('\n') {
            content.push('\n');
        }
        if !content.ends_with("\n\n") {
            content.push('\n');
        }
        content.push_str(&serialize_inline_reading_note(
            &serde_json::to_value(&metadata)?,
            body,
        ));
        if source_evidence(&original) != source_evidence(&content) {
            return Err(error(
                "reference insertion could change source text; source and draft were not changed",
            ));
        }
        self.persist_reading_note(tx, &source.path, Some(&original), &content)?;
        self.finish_inline_reading_note(&source.path, note_id)
    }

    fn place_reading_reference(
        &self,
        source: &Source,
        original: &str,
        selection: Option<&ReadingSelection>,
        label: &str,
    ) -> Result<String> {
        let last = selection
            .and_then(|s| s.block_ids.last())
            .and_then(|id| source.blocks.iter().position(|b| &b.id == id));
        let ordinals: Vec<usize> = if let Some(last) = last {
            (last..source.parsed.len()).collect()
        } else {
            (0..source.parsed.len()).rev().collect()
        };
        let lines: Vec<_> = original.split_inclusive('\n').collect();
        let mut offsets = vec![0];
        for line in &lines {
            offsets.push(offsets.last().unwrap() + line.len());
        }
        let marker = format!(" [^{label}]");
        let original_tables = table_ast(original);
        for ordinal in ordinals {
            let block = &source.parsed[ordinal];
            // Appending to a table delimiter can turn the entire table into
            // prose, while appending after its final cell can hide the marker.
            if has_structural_reference_boundary(&block.content)
                || (block.content.trim_start().starts_with("[^") && block.content.contains("]:"))
            {
                continue;
            }
            let end = block
                .children
                .first()
                .map_or(block.source_line_range.end, |b| b.source_line_range.start);
            for line in (block.source_line_range.start..end.min(lines.len())).rev() {
                let raw = lines[line].trim_end_matches(['\r', '\n']);
                if raw.trim().is_empty()
                    || raw.trim().split_once("::").is_some_and(|(key, _)| {
                        key.bytes()
                            .all(|c| c.is_ascii_alphabetic() || c == b'_' || c == b'-')
                    })
                {
                    continue;
                }
                let at = offsets[line] + raw.trim_end_matches([' ', '\t']).len();
                let mut candidate = original.to_string();
                candidate.insert_str(at, &marker);
                let parsed = parser::parse_page(&candidate, "source.md");
                let mut blocks = Vec::new();
                flattened(&parsed.blocks, &mut blocks);
                let Some(edited) = blocks.iter().find(|b| {
                    b.source_line_range.start == block.source_line_range.start
                        && b.content.contains(&format!("[^{label}]"))
                }) else {
                    continue;
                };
                let Some(position) = edited.content.find(&format!("[^{label}]")) else {
                    continue;
                };
                let protected = parser::links::protected_reference_context_spans(&edited.content);
                if protected
                    .iter()
                    .any(|(start, end)| *start <= position && *end > position)
                {
                    continue;
                }
                // CommonMark parsing rules, not string heuristics, decide if
                // the reference is prose rather than code, a link or HTML.
                let probe = format!("{}\n\n[^{label}]: probe\n", edited.content);
                let recognized = pulldown_cmark::Parser::new_ext(&probe, pulldown_cmark::Options::all())
                    .any(|event| matches!(event, pulldown_cmark::Event::FootnoteReference(ref name) if name.as_ref() == label));
                if recognized
                    && source_evidence(original) == source_evidence(&candidate)
                    && table_ast(&candidate) == original_tables
                {
                    return Ok(candidate);
                }
            }
        }
        // Structural-only sources get a separate reference paragraph, never
        // a marker inserted into a table, heading, fence or code block.
        let notes = parse_inline_reading_notes(original);
        let at = notes
            .notes
            .first()
            .map_or(original.len(), |n| n.range.start);
        let mut candidate = original.to_string();
        candidate.insert_str(at, &format!("\n\n[^{label}]\n\n"));
        let probe = format!("{candidate}\n[^{label}]: probe\n");
        if !pulldown_cmark::Parser::new_ext(&probe, pulldown_cmark::Options::all())
            .any(|event| matches!(event, pulldown_cmark::Event::FootnoteReference(ref name) if name.as_ref() == label))
        {
            return Err(error("no safe paragraph boundary after the selection (possibly unclosed code); source was not changed"));
        }
        if table_ast(&candidate) != original_tables {
            return Err(error(
                "reference insertion would change a Markdown table; source was not changed",
            ));
        }
        Ok(candidate)
    }

    pub(super) fn change_inline_reading_note(
        &self,
        tx: rusqlite::Transaction<'_>,
        mut document: Document,
        body: Option<&str>,
        source: Option<(&str, Option<&ReadingSelection>)>,
    ) -> Result<ReadingNote> {
        let label = document.metadata.footnote_label.clone().unwrap();
        let mut content = document.content.clone();
        if let Some((page_id, selection)) = source {
            let destination = self.reading_source(&tx, page_id)?;
            if destination.path != document.path {
                return Err(error(format!("cross-file reattachment is not supported safely yet; neither source file was changed. Note and draft remain at {}", document.path.display())));
            }
            document.metadata.anchor = self.reading_anchor(&tx, page_id, selection)?;
            // Remove only this note's generated reference; definitions and all
            // other source bytes/annotations stay untouched.
            let notes = parse_inline_reading_notes(&content);
            let _own = notes
                .notes
                .iter()
                .find(|n| n.metadata["id"].as_str() == Some(&document.metadata.id))
                .ok_or_else(|| error("inline note disappeared"))?;
            let mut stripped = String::new();
            let mut cursor = 0;
            for note in &notes.notes {
                stripped.push_str(&strip_label_references(
                    &content[cursor..note.range.start],
                    Some(&label),
                ));
                stripped.push_str(&content[note.range.clone()]);
                cursor = note.range.end;
            }
            stripped.push_str(&strip_label_references(&content[cursor..], Some(&label)));
            content = stripped;
            // Positions before the reference do not change. Recompute parsed
            // source ranges instead of using the old file's line offsets.
            let mut adjusted = Source {
                page: destination.page,
                path: destination.path,
                hash: destination.hash,
                raw_hash: destination.raw_hash,
                blocks: destination.blocks,
                parsed: Vec::new(),
            };
            let parsed = parser::parse_page(&content, "source.md");
            flattened(&parsed.blocks, &mut adjusted.parsed);
            adjusted.parsed.retain(|b| {
                !is_note_page(&b.properties)
                    && !(b.content.contains("[^grafium-note-")
                        && parser::strip_reading_note_references(&b.content)
                            .trim()
                            .is_empty())
            });
            content = self.place_reading_reference(&adjusted, &content, selection, &label)?;
        }
        document.metadata.updated_at = Utc::now().to_rfc3339();
        document.metadata.create_request_hash = None;
        let parsed = parse_inline_reading_notes(&content);
        if !parsed.warnings.is_empty() {
            return Err(error(parsed.warnings.join("; ")));
        }
        let own = parsed
            .notes
            .iter()
            .find(|n| n.metadata["id"].as_str() == Some(&document.metadata.id))
            .ok_or_else(|| error("inline note disappeared"))?;
        content.replace_range(
            own.range.clone(),
            &serialize_inline_reading_note(
                &serde_json::to_value(&document.metadata)?,
                body.unwrap_or(&document.body),
            ),
        );
        if source_evidence(&content) != source_evidence(&document.content) {
            return Err(error(
                "annotation edit would change source text; no file was changed",
            ));
        }
        self.persist_reading_note(tx, &document.path, Some(&document.content), &content)?;
        self.finish_inline_reading_note(&document.path, &document.metadata.id)
    }

    fn finish_inline_reading_note(&self, path: &Path, id: &str) -> Result<ReadingNote> {
        let mut warnings = Vec::new();
        self.reading_documents(path, &mut warnings)?
            .into_iter()
            .find(|d| d.metadata.id == id)
            .ok_or_else(|| {
                error(format!(
                    "saved annotation could not be read; recovery file {}",
                    path.display()
                ))
            })
            .and_then(|document| self.reading_dto(&document))
    }
}
