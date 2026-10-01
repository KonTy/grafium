use super::*;
use crate::parser::reading_notes::{parse_inline_reading_notes, source_evidence};

#[cfg(test)]
#[path = "reading_notes_delete_tests.rs"]
mod tests;

struct Deletion {
    documents: Vec<Document>,
    replacement: Option<String>,
}

/// Remove only selected envelopes and their prose references. Parsing the entire
/// source keeps code, escaped literals and ordinary footnote bodies protected.
fn without_inline_notes(content: &str, ids: &HashSet<&str>) -> Result<String> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let parsed = parse_inline_reading_notes(content);
    if !parsed.warnings.is_empty() {
        return Err(error(parsed.warnings.join("; ")));
    }
    let selected: Vec<_> = parsed
        .notes
        .iter()
        .filter(|note| ids.contains(note.metadata["id"].as_str().unwrap()))
        .collect();
    if selected.len() != ids.len() {
        return Err(error("annotation set changed; reload before deleting"));
    }
    let labels: HashSet<_> = selected.iter().map(|note| note.label.as_str()).collect();
    let mut ranges: Vec<_> = selected.iter().map(|note| note.range.clone()).collect();
    let mut in_definition = false;
    for (event, mut range) in Parser::new_ext(content, Options::all()).into_offset_iter() {
        match event {
            Event::Start(Tag::FootnoteDefinition(label)) => {
                // A second, unmanaged definition must never lose its references.
                if labels.contains(label.as_ref())
                    && !selected
                        .iter()
                        .any(|note| note.range.contains(&range.start))
                {
                    return Err(error(
                        "managed footnote label also has an ordinary definition; file preserved",
                    ));
                }
                in_definition = true;
            }
            Event::End(TagEnd::FootnoteDefinition) => in_definition = false,
            Event::FootnoteReference(label)
                if !in_definition && labels.contains(label.as_ref()) =>
            {
                if parsed
                    .notes
                    .iter()
                    .any(|note| note.range.contains(&range.start))
                {
                    continue;
                }
                if range.start > 0 && content.as_bytes()[range.start - 1] == b' ' {
                    range.start -= 1;
                }
                ranges.push(range);
            }
            _ => {}
        }
    }
    ranges.sort_by_key(|range| range.start);
    let mut output = String::new();
    let mut cursor = 0;
    for range in ranges {
        if range.start < cursor {
            return Err(error("overlapping annotation ranges; file preserved"));
        }
        output.push_str(&content[cursor..range.start]);
        cursor = range.end;
    }
    output.push_str(&content[cursor..]);
    if source_evidence(content) != source_evidence(&output) {
        return Err(error(
            "annotation deletion would change source prose; file preserved",
        ));
    }
    Ok(output)
}

impl Graph {
    /// Delete exactly the reviewed annotation, never its source page or assets.
    pub fn reading_note_delete(
        &self,
        note_id: &str,
        expected_revision: &str,
    ) -> Result<ReadingNotesDeleteReceipt> {
        let _operation = self.source_operations.lock();
        let document = self.reviewed_reading_note(note_id, expected_revision)?;
        self.delete_reading_documents(vec![document], false, |_| Ok(()))
    }

    /// The reviewed set must match the complete current page set, including
    /// legacy files. Inline changes publish once per file; different files are
    /// not an atomic transaction, so publication failures have explicit receipts.
    pub fn reading_notes_delete_for_page(
        &self,
        source_page_id: &str,
        expected_notes: &[ReadingNoteRevision],
    ) -> Result<ReadingNotesDeleteReceipt> {
        let _operation = self.source_operations.lock();
        self.db.get_page_by_id(source_page_id)?;
        let mut expected = HashMap::new();
        for note in expected_notes {
            uuid(&note.id)?;
            if !sha256(&note.revision) || expected.insert(&note.id, &note.revision).is_some() {
                return Err(error("invalid or duplicate reviewed annotation revision"));
            }
        }
        let listed = self.reading_notes_list(Some(source_page_id))?;
        if !listed.warnings.is_empty() {
            return Err(error(format!(
                "cannot verify all page annotations; no notes deleted: {}",
                listed.warnings.join("; ")
            )));
        }
        let current: Vec<_> = listed
            .notes
            .into_iter()
            .filter(|note| {
                // A resolved source wins over an obsolete anchor hint after a move.
                note.source
                    .page_id
                    .as_deref()
                    .is_none_or(|id| id == source_page_id)
            })
            .collect();
        if current.len() != expected.len()
            || current
                .iter()
                .any(|note| expected.get(&note.id).copied() != Some(&note.revision))
        {
            return Err(error(
                "page annotation set or revision changed; reload and review before deleting",
            ));
        }
        let documents = current
            .iter()
            .map(|note| self.reviewed_reading_note(&note.id, &note.revision))
            .collect::<Result<Vec<_>>>()?;
        self.delete_reading_documents(documents, true, |_| Ok(()))
    }

    fn reviewed_reading_note(&self, id: &str, revision: &str) -> Result<Document> {
        if !sha256(revision) {
            return Err(error(
                "expectedRevision must be the annotation SHA-256 revision",
            ));
        }
        let document = self
            .find_reading_note(id)?
            .ok_or_else(|| error("annotation no longer exists; reload before deleting"))?;
        if document_revision(&document)? != revision {
            return Err(error(
                "revision conflict; reload and review before deleting",
            ));
        }
        Ok(document)
    }

    fn delete_reading_documents(
        &self,
        documents: Vec<Document>,
        all_inline: bool,
        mut before_replace: impl FnMut(&Path) -> Result<()>,
    ) -> Result<ReadingNotesDeleteReceipt> {
        let mut grouped = std::collections::BTreeMap::<PathBuf, Vec<Document>>::new();
        for document in documents {
            grouped
                .entry(document.path.clone())
                .or_default()
                .push(document);
        }
        // Prepare every replacement and verify every file before publishing any.
        let mut plan = Vec::new();
        for (path, documents) in grouped {
            let first = &documents[0];
            self.reading_path(&self.relative_graph_path(&path))?;
            if fs::read_to_string(&path)? != first.content
                || documents.iter().any(|doc| doc.content != first.content)
            {
                return Err(error("revision conflict; no annotations deleted"));
            }
            self.ensure_reading_note_files_unique(&path, &first.content)?;
            let replacement = if first.storage == ReadingNoteStorage::Inline {
                if all_inline
                    && parse_inline_reading_notes(&first.content).notes.len() != documents.len()
                {
                    return Err(error(
                        "page annotation set changed during review; no annotations deleted",
                    ));
                }
                Some(without_inline_notes(
                    &first.content,
                    &documents
                        .iter()
                        .map(|doc| doc.metadata.id.as_str())
                        .collect(),
                )?)
            } else {
                if documents.len() != 1
                    || first.metadata.version != 1
                    || !parse_inline_reading_notes(&first.content).notes.is_empty()
                {
                    return Err(error(
                        "legacy note contains other annotations; file preserved",
                    ));
                }
                None
            };
            plan.push(Deletion {
                documents,
                replacement,
            });
        }
        let mut receipt = ReadingNotesDeleteReceipt::default();
        // Keep verified independent copies, not hard links: external in-place
        // edits must not mutate the recovery bytes. Back up the entire plan
        // before the first source replacement or legacy-file retirement.
        for deletion in &plan {
            receipt
                .backups
                .push(self.backup_reading_note_deletion(&deletion.documents[0])?);
        }
        let mut stopped = false;
        for deletion in plan {
            let first = &deletion.documents[0];
            if stopped {
                for doc in deletion.documents {
                    receipt.failures.push(ReadingNoteDeleteFailure {
                        id: doc.metadata.id,
                        message: "Not attempted after an earlier deletion failure; reload before retrying".into(),
                    });
                }
                continue;
            }
            let result = if let Some(content) = &deletion.replacement {
                (|| {
                    let mut conn = self.db.conn()?;
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    self.persist_reading_note_with_hook(
                        tx,
                        &first.path,
                        Some(&first.content),
                        content,
                        &mut before_replace,
                    )
                })()
            } else {
                self.delete_legacy_reading_document(first, &mut before_replace)
            };
            if let Err(failure) = result {
                stopped = true;
                // A guarded write can publish successfully but fail to index.
                // Count only deletions provable from the authoritative file.
                let current = fs::read_to_string(&first.path);
                for doc in &deletion.documents {
                    let absent = match (&current, doc.storage) {
                        (Err(e), ReadingNoteStorage::File) => {
                            e.kind() == std::io::ErrorKind::NotFound
                        }
                        (Ok(content), ReadingNoteStorage::Inline) => {
                            let parsed = parse_inline_reading_notes(content);
                            parsed.warnings.is_empty()
                                && !parsed.notes.iter().any(|note| {
                                    note.metadata["id"].as_str() == Some(&doc.metadata.id)
                                })
                        }
                        _ => false,
                    };
                    if absent {
                        receipt.deleted_ids.push(doc.metadata.id.clone());
                    }
                    receipt.failures.push(ReadingNoteDeleteFailure {
                        id: doc.metadata.id.clone(),
                        message: format!(
                            "{failure}. Reload before retrying; file deletion confirmed: {absent}"
                        ),
                    });
                }
            } else {
                receipt
                    .deleted_ids
                    .extend(deletion.documents.into_iter().map(|doc| doc.metadata.id));
            }
        }
        receipt.deleted_count = receipt.deleted_ids.len();
        Ok(receipt)
    }

    fn backup_reading_note_deletion(
        &self,
        document: &Document,
    ) -> Result<ReadingNoteDeletionBackup> {
        use std::io::Write;
        let path = &document.path;
        let relative = self.relative_graph_path(path);
        self.reading_path(&relative)?;
        if fs::read_to_string(path)? != document.content {
            return Err(error(
                "revision conflict before backup; no annotations deleted",
            ));
        }
        let backup = path.with_file_name(format!(".reading-note-{}.deleted", Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&backup)?;
            file.write_all(document.content.as_bytes())?;
            file.sync_all()?;
            if fs::read(&backup)? != document.content.as_bytes() {
                return Err(error("deletion backup verification failed"));
            }
            #[cfg(unix)]
            fs::File::open(path.parent().unwrap())?.sync_all()?;
            self.reading_path(&relative)?;
            if fs::read_to_string(path)? != document.content {
                return Err(error("revision conflict after backup"));
            }
            Ok(())
        })();
        result.map_err(|failure| {
            error(format!(
                "{failure}; no annotations deleted; preserve any recovery copy at {}",
                backup.display()
            ))
        })?;
        Ok(ReadingNoteDeletionBackup {
            file_path: relative,
            backup_path: self.relative_graph_path(&backup),
        })
    }

    fn delete_legacy_reading_document(
        &self,
        document: &Document,
        before_retire: &mut impl FnMut(&Path) -> Result<()>,
    ) -> Result<()> {
        let path = &document.path;
        self.reading_path(&self.relative_graph_path(path))?;
        if fs::read_to_string(path)? != document.content {
            return Err(error("revision conflict; legacy note preserved"));
        }
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Retire only this file's index rows, never namespaced children/assets.
        let relative = self.relative_graph_path(path);
        let pages: Vec<_> = self
            .db
            .list_pages_by_file_path_prefix(&relative)?
            .into_iter()
            .filter(|page| page.file_path.as_deref() == Some(&relative))
            .collect();
        for page in &pages {
            self.db.retire_source_in_connection(&tx, &page.id)?;
        }
        self.db.collect_generated_pages_in_connection(&tx)?;
        let backup = path.with_file_name(format!(".reading-note-{}.deleted", Uuid::new_v4()));
        before_retire(path)?;
        fs::rename(path, &backup)?;
        let result = (|| -> Result<()> {
            if fs::symlink_metadata(&backup)?.file_type().is_symlink()
                || fs::read_to_string(&backup)? != document.content
            {
                return Err(error(
                    "revision conflict at deletion; actual external file preserved",
                ));
            }
            #[cfg(unix)]
            fs::File::open(path.parent().unwrap())?.sync_all()?;
            tx.commit()?;
            Ok(())
        })();
        if let Err(failure) = result {
            // No-clobber restore: a newly created external file wins.
            if fs::hard_link(&backup, path).is_ok() {
                let _ = fs::remove_file(&backup);
                return Err(error(format!(
                    "{failure}; note restored at {}",
                    path.display()
                )));
            }
            return Err(error(format!(
                "{failure}; external destination preserved; recovery at {}",
                backup.display()
            )));
        }
        self.forget_indexed_content(path);
        self.forget_canonical_reading_note(path);
        self.note_self_write(path);
        for page in pages {
            self.mark_page_dirty(&page.id);
        }
        fs::remove_file(&backup).map_err(|e| {
            error(format!(
                "note deleted but backup cleanup failed: {e}; recovery at {}",
                backup.display()
            ))
        })?;
        Ok(())
    }
}
