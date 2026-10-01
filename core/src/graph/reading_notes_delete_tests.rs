use super::*;

fn fixture() -> Result<(tempfile::TempDir, Graph, Page)> {
    let directory = tempfile::tempdir_in(".")?;
    let graph = Graph::open(directory.path())?;
    let page = graph.create_page_with_content(
        "Synthetic",
        false,
        "- Original prose [^author]\n\n[^author]: Ordinary footnote.\n",
    )?;
    Ok((directory, graph, page))
}

fn create(graph: &Graph, page: &Page, legacy: bool, body: &str) -> Result<ReadingNote> {
    let id = Uuid::new_v4().to_string();
    if legacy {
        graph.create_file_reading_note(&id, &page.id, None, body)
    } else {
        graph.reading_note_create(&id, &page.id, None, body)
    }
}

fn reviewed(graph: &Graph, page: &Page) -> Vec<ReadingNoteRevision> {
    let list = graph.reading_notes_list(Some(&page.id)).unwrap();
    assert!(list.warnings.is_empty(), "{:?}", list.warnings);
    list.notes
        .into_iter()
        .map(|note| ReadingNoteRevision {
            id: note.id,
            revision: note.revision,
        })
        .collect()
}

#[test]
fn single_inline_delete_preserves_prose_other_notes_and_ordinary_footnotes() -> Result<()> {
    let (directory, graph, page) = fixture()?;
    let original = graph.get_page_source(&page.id)?;
    let original_blocks = graph.db.list_blocks_for_page(&page.id)?;
    let first = create(&graph, &page, false, "Delete me")?;
    let second = create(&graph, &page, false, "Keep me, including [^grafium-note-1]")?;
    let before_delete = graph.get_page_source(&page.id)?;
    let receipt = graph.reading_note_delete(&first.id, &first.revision)?;
    assert_eq!(receipt.deleted_ids, [first.id.clone()]);
    assert_eq!(receipt.deleted_count, 1);
    assert!(receipt.failures.is_empty());
    assert_eq!(receipt.backups.len(), 1);
    assert_eq!(receipt.backups[0].file_path, first.file_path);
    assert_eq!(
        fs::read_to_string(graph.root_dir.join(&receipt.backups[0].backup_path))?,
        before_delete
    );
    let source = graph.get_page_source(&page.id)?;
    assert_eq!(source_evidence(&source), source_evidence(&original));
    assert!(source.contains("[^author]: Ordinary footnote."));
    assert!(source.contains("Keep me, including [^grafium-note-1]"));
    assert!(graph
        .db
        .get_block_by_id(first.note_block_id.as_ref().unwrap())
        .is_err());
    for block in original_blocks {
        assert!(graph.db.get_block_by_id(&block.id).is_ok());
    }
    let notes = graph.reading_notes_list(Some(&page.id))?.notes;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].id, second.id);
    assert_eq!(notes[0].revision, second.revision);
    drop(graph);
    let graph = Graph::open(directory.path())?;
    graph.reindex_all()?;
    assert_eq!(reviewed(&graph, &page).len(), 1);
    graph.reading_note_delete(&second.id, &second.revision)?;
    assert_eq!(
        source_evidence(&graph.get_page_source(&page.id)?),
        source_evidence(&original)
    );
    assert!(graph.db.get_page_by_id(&page.id).is_ok());
    Ok(())
}

#[test]
fn inline_delete_keeps_literal_markers_and_author_footnote_bodies() -> Result<()> {
    let (_directory, graph, page) = fixture()?;
    let note = create(&graph, &page, false, "Delete me")?;
    let path = graph.root_dir.join(&note.file_path);
    let content = fs::read_to_string(&path)?.replace(
        "[^author]: Ordinary footnote.",
        "[^author]: Ordinary footnote with [^grafium-note-1].",
    );
    let content = format!("{content}\n- `[^grafium-note-1]` and \\[^grafium-note-1]\n\n```md\n[^grafium-note-1]\n```\n");
    fs::write(&path, &content)?;
    let receipt = graph.reading_note_delete(&note.id, &note.revision)?;
    assert!(receipt.failures.is_empty(), "{:?}", receipt.failures);
    let after = fs::read_to_string(&path)?;
    assert!(after.contains("[^author]: Ordinary footnote with [^grafium-note-1]."));
    assert!(after.contains("`[^grafium-note-1]` and \\[^grafium-note-1]"));
    assert!(after.contains("```md\n[^grafium-note-1]\n```"));
    assert_eq!(source_evidence(&content), source_evidence(&after));
    Ok(())
}

#[test]
fn bulk_delete_covers_both_formats_but_only_one_source_page() -> Result<()> {
    let (directory, graph, page) = fixture()?;
    let original = graph.get_page_source(&page.id)?;
    let inline = create(&graph, &page, false, "Inline one")?;
    create(&graph, &page, false, "Inline two")?;
    let legacy = create(&graph, &page, true, "Standalone")?;
    let inline_before = graph.get_page_source(&page.id)?;
    let legacy_before = fs::read_to_string(graph.root_dir.join(&legacy.file_path))?;
    let other = graph.create_page_with_content("Other", false, "- Other prose\n")?;
    let other_inline = create(&graph, &other, false, "Other inline")?;
    let other_legacy = create(&graph, &other, true, "Other legacy")?;
    let other_bytes = graph.get_page_source(&other.id)?;
    let legacy_bytes = fs::read(graph.root_dir.join(&other_legacy.file_path))?;
    let asset = graph.root_dir.join("assets/keep.txt");
    fs::create_dir_all(asset.parent().unwrap())?;
    fs::write(&asset, "user-owned asset")?;
    let receipt = graph.reading_notes_delete_for_page(&page.id, &reviewed(&graph, &page))?;
    assert_eq!(receipt.deleted_count, 3);
    assert!(receipt.deleted_ids.contains(&inline.id));
    assert!(receipt.deleted_ids.contains(&legacy.id));
    assert!(receipt.failures.is_empty(), "{:?}", receipt.failures);
    assert_eq!(receipt.backups.len(), 2);
    for backup in &receipt.backups {
        let expected = if backup.file_path == inline.file_path {
            &inline_before
        } else {
            assert_eq!(backup.file_path, legacy.file_path);
            &legacy_before
        };
        assert_eq!(
            &fs::read_to_string(graph.root_dir.join(&backup.backup_path))?,
            expected
        );
        assert!(!crate::fsutil::is_authoritative_source(Path::new(
            &backup.backup_path
        )));
    }
    assert!(!graph.root_dir.join(legacy.file_path).exists());
    assert_eq!(graph.get_page_source(&other.id)?, other_bytes);
    assert_eq!(
        fs::read(graph.root_dir.join(other_legacy.file_path))?,
        legacy_bytes
    );
    assert_eq!(fs::read_to_string(asset)?, "user-owned asset");
    assert_eq!(
        source_evidence(&graph.get_page_source(&page.id)?),
        source_evidence(&original)
    );
    assert!(reviewed(&graph, &page).is_empty());
    assert_eq!(
        graph
            .reading_notes_delete_for_page(&page.id, &[])?
            .deleted_count,
        0
    );
    drop(graph);
    let graph = Graph::open(directory.path())?;
    graph.reindex_all()?;
    let notes = graph.reading_notes_list(None)?;
    assert!(notes.warnings.is_empty(), "{:?}", notes.warnings);
    assert_eq!(notes.notes.len(), 2);
    assert!(notes.notes.iter().any(|note| note.id == other_inline.id));
    Ok(())
}

#[test]
fn stale_bulk_review_and_single_revisions_never_delete_anything() -> Result<()> {
    let (_directory, graph, page) = fixture()?;
    let first = create(&graph, &page, false, "One")?;
    let old = reviewed(&graph, &page);
    let second = create(&graph, &page, true, "Two")?;
    let source = graph.get_page_source(&page.id)?;
    let legacy_path = graph.root_dir.join(&second.file_path);
    let legacy = fs::read(&legacy_path)?;
    assert!(graph.reading_notes_delete_for_page(&page.id, &old).is_err());
    let all = reviewed(&graph, &page);
    let mut duplicate = all.clone();
    duplicate.push(all[0].clone());
    assert!(graph
        .reading_notes_delete_for_page(&page.id, &duplicate)
        .is_err());
    let mut stale = all.clone();
    stale[1].revision = "0".repeat(64);
    assert!(graph
        .reading_notes_delete_for_page(&page.id, &stale)
        .is_err());
    let mut foreign = all.clone();
    foreign[0].id = Uuid::new_v4().to_string();
    assert!(graph
        .reading_notes_delete_for_page(&page.id, &foreign)
        .is_err());
    assert!(graph
        .reading_note_delete(&first.id, "not a revision")
        .is_err());
    assert!(graph
        .reading_note_delete(&second.id, &"0".repeat(64))
        .is_err());
    assert_eq!(graph.get_page_source(&page.id)?, source);
    assert_eq!(fs::read(&legacy_path)?, legacy);
    graph.reading_note_delete(&second.id, &second.revision)?;
    assert!(graph.reading_notes_delete_for_page(&page.id, &all).is_err());
    assert_eq!(graph.get_page_source(&page.id)?, source);
    Ok(())
}

#[test]
fn bulk_preflight_catches_notes_added_after_the_initial_listing() -> Result<()> {
    let (_directory, graph, page) = fixture()?;
    let first = create(&graph, &page, false, "Reviewed")?;
    create(&graph, &page, false, "New, unseen annotation")?;
    let snapshot = graph.get_page_source(&page.id)?;
    let document = graph.reviewed_reading_note(&first.id, &first.revision)?;
    let result = graph.delete_reading_documents(vec![document], true, |_| {
        panic!("incomplete bulk review must not publish")
    });
    assert!(result.is_err());
    assert_eq!(graph.get_page_source(&page.id)?, snapshot);
    Ok(())
}

#[test]
fn legacy_external_revision_and_duplicate_identity_are_preserved() -> Result<()> {
    let (_directory, graph, page) = fixture()?;
    let note = create(&graph, &page, true, "Original note")?;
    let path = graph.root_dir.join(&note.file_path);
    let original = fs::read_to_string(&path)?;
    let changed = original.replace("Original note", "External revision");
    fs::write(&path, &changed)?;
    assert!(graph.reading_note_delete(&note.id, &note.revision).is_err());
    assert_eq!(fs::read_to_string(&path)?, changed);
    fs::write(&path, &original)?;
    let duplicate = graph.pages_dir.join("Duplicate.md");
    fs::copy(&path, &duplicate)?;
    assert!(graph.reading_note_delete(&note.id, &note.revision).is_err());
    assert_eq!(fs::read_to_string(&path)?, original);
    assert_eq!(fs::read_to_string(&duplicate)?, original);
    Ok(())
}

#[cfg(unix)]
#[test]
fn legacy_deletion_does_not_follow_symlink_files() -> Result<()> {
    let (_directory, graph, page) = fixture()?;
    let note = create(&graph, &page, true, "Original note")?;
    let path = graph.root_dir.join(&note.file_path);
    let original = fs::read_to_string(&path)?;
    let moved = graph.root_dir.join("preserved-note.md");
    fs::rename(&path, &moved)?;
    std::os::unix::fs::symlink(moved.canonicalize()?, &path)?;
    assert!(graph.reading_note_delete(&note.id, &note.revision).is_err());
    assert_eq!(fs::read_to_string(moved)?, original);
    assert!(fs::symlink_metadata(path)?.file_type().is_symlink());
    Ok(())
}

#[test]
fn malformed_or_duplicate_inline_notes_abort_before_legacy_deletion() -> Result<()> {
    let (_directory, graph, page) = fixture()?;
    let inline = create(&graph, &page, false, "Inline")?;
    let legacy = create(&graph, &page, true, "Legacy")?;
    let expected = reviewed(&graph, &page);
    let path = graph.root_dir.join(&inline.file_path);
    let original = fs::read_to_string(&path)?;
    for changed in [
        original.replace("<!-- /grafium-reading-note -->", "<!-- damaged -->"),
        format!("{original}\n[^grafium-note-1]: An ordinary conflicting definition.\n"),
    ] {
        fs::write(&path, &changed)?;
        assert!(graph
            .reading_notes_delete_for_page(&page.id, &expected)
            .is_err());
        assert_eq!(fs::read_to_string(&path)?, changed);
        assert!(graph.root_dir.join(&legacy.file_path).exists());
    }
    Ok(())
}

#[test]
fn legacy_delete_race_restores_external_content_without_index_changes() -> Result<()> {
    let (_directory, graph, page) = fixture()?;
    let note = create(&graph, &page, true, "Old note")?;
    let document = graph.reviewed_reading_note(&note.id, &note.revision)?;
    let original = document.content.clone();
    let external = document
        .content
        .replace("Old note", "External changed note");
    let receipt = graph.delete_reading_documents(vec![document], false, |path| {
        assert!(fs::read_dir(path.parent().unwrap())?
            .filter_map(|entry| entry.ok())
            .any(
                |entry| entry.path().extension().is_some_and(|ext| ext == "deleted")
                    && fs::read_to_string(entry.path()).ok().as_deref() == Some(original.as_str())
            ));
        fs::write(path, &external)?;
        Ok(())
    })?;
    assert_eq!(receipt.deleted_count, 0);
    assert_eq!(receipt.backups.len(), 1);
    assert_eq!(receipt.failures.len(), 1);
    assert!(receipt.failures[0].message.contains("restored"));
    assert_eq!(
        fs::read_to_string(graph.root_dir.join(note.file_path))?,
        external
    );
    assert!(graph.db.get_page_by_id(&note.note_page_id).is_ok());
    Ok(())
}

#[test]
fn bulk_runtime_failure_reports_completed_failed_and_unattempted_notes() -> Result<()> {
    let (_directory, graph, page) = fixture()?;
    let mut notes = vec![
        create(&graph, &page, true, "One")?,
        create(&graph, &page, true, "Two")?,
        create(&graph, &page, true, "Three")?,
    ];
    notes.sort_by(|a, b| a.file_path.cmp(&b.file_path));
    let documents = notes
        .iter()
        .map(|note| graph.reviewed_reading_note(&note.id, &note.revision))
        .collect::<Result<Vec<_>>>()?;
    let mut writes = 0;
    let receipt = graph.delete_reading_documents(documents, true, |_| {
        writes += 1;
        if writes == 2 {
            Err(error("synthetic filesystem failure"))
        } else {
            Ok(())
        }
    })?;
    assert_eq!(writes, 2);
    assert_eq!(receipt.backups.len(), 3);
    assert_eq!(receipt.deleted_ids, [notes[0].id.clone()]);
    assert_eq!(receipt.deleted_count, 1);
    assert_eq!(receipt.failures.len(), 2);
    assert_eq!(receipt.failures[0].id, notes[1].id);
    assert!(receipt.failures[0]
        .message
        .contains("synthetic filesystem failure"));
    assert!(receipt.failures[1].message.contains("Not attempted"));
    assert!(!graph.root_dir.join(&notes[0].file_path).exists());
    assert!(graph.root_dir.join(&notes[1].file_path).exists());
    assert!(graph.root_dir.join(&notes[2].file_path).exists());
    Ok(())
}

#[test]
fn inline_bulk_uses_one_publication_and_reports_index_failure_after_deletion() -> Result<()> {
    let (_directory, graph, page) = fixture()?;
    create(&graph, &page, false, "One")?;
    create(&graph, &page, false, "Two")?;
    let original = graph.get_page_source(&page.id)?;
    let documents = reviewed(&graph, &page)
        .iter()
        .map(|note| graph.reviewed_reading_note(&note.id, &note.revision))
        .collect::<Result<Vec<_>>>()?;
    graph.db.conn()?.execute_batch(
        "CREATE TRIGGER deletion_failure BEFORE DELETE ON blocks
         BEGIN SELECT RAISE(ABORT, 'synthetic deletion index failure'); END;",
    )?;
    let mut writes = 0;
    let receipt = graph.delete_reading_documents(documents, true, |_| {
        let path = graph.root_dir.join(page.file_path.as_ref().unwrap());
        assert!(fs::read_dir(path.parent().unwrap())?
            .filter_map(|entry| entry.ok())
            .any(
                |entry| entry.path().extension().is_some_and(|ext| ext == "deleted")
                    && fs::read_to_string(entry.path()).ok().as_deref() == Some(&original)
            ));
        writes += 1;
        Ok(())
    })?;
    assert_eq!(writes, 1);
    assert_eq!(receipt.deleted_count, 2);
    assert_eq!(receipt.failures.len(), 2);
    assert!(receipt
        .failures
        .iter()
        .all(|failure| failure.message.contains("index update failed")));
    let path = graph.root_dir.join(page.file_path.as_ref().unwrap());
    assert!(parse_inline_reading_notes(&fs::read_to_string(&path)?)
        .notes
        .is_empty());
    assert!(fs::read_dir(path.parent().unwrap())?
        .filter_map(|e| e.ok())
        .any(|entry| entry.path() != path
            && fs::read_to_string(entry.path()).ok().as_deref() == Some(&original)));
    graph
        .db
        .conn()?
        .execute_batch("DROP TRIGGER deletion_failure;")?;
    graph.reindex_all()?;
    assert!(reviewed(&graph, &page).is_empty());
    Ok(())
}
