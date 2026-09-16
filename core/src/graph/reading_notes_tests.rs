use super::*;

fn fixture(raw: &str) -> Result<(tempfile::TempDir, Graph, Page)> {
    let directory = tempfile::tempdir_in(".")?;
    let graph = Graph::open(directory.path())?;
    let page = graph.create_page_with_content("Books/Synthetic/Chapter", false, raw)?;
    Ok((directory, graph, page))
}

fn select(graph: &Graph, page: &Page, ordinal: usize, quote: &str) -> ReadingSelection {
    let conn = graph.db.conn().unwrap();
    let source = graph.reading_source(&conn, &page.id).unwrap();
    let block = &source.blocks[ordinal];
    let byte = block.content.find(quote).unwrap();
    let from = block.content[..byte].encode_utf16().count();
    let to = from + quote.encode_utf16().count();
    ReadingSelection {
        page_id: page.id.clone(),
        block_ids: vec![block.id.clone()],
        text: quote.into(),
        kind: SelectionKind::Source,
        parts: vec![ReadingSelectionPart {
            block_id: block.id.clone(),
            text: quote.into(),
            from,
            to,
            prefix: block.content[..byte].into(),
            suffix: block.content[byte + quote.len()..].into(),
        }],
        document_range: None,
    }
}

fn create(
    graph: &Graph,
    page: &Page,
    selection: Option<&ReadingSelection>,
    body: &str,
) -> Result<ReadingNote> {
    graph.create_file_reading_note(&Uuid::new_v4().to_string(), &page.id, selection, body)
}

fn only(graph: &Graph) -> ReadingNote {
    let mut result = graph.reading_notes_list(None).unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.notes.len(), 1);
    result.notes.remove(0)
}

#[test]
fn reading_notes_survive_reopen_reindex_and_fresh_database_without_source_ids() -> Result<()> {
    let raw = "- # Chapter\n  - Same repeated passage.\n  - Same repeated passage.\n- Tail\n";
    let (directory, graph, page) = fixture(raw)?;
    let selection = select(&graph, &page, 2, "repeated");
    let note = create(&graph, &page, Some(&selection), "A note")?;
    assert_eq!(note.status, ReadingNoteStatus::Attached);
    assert_eq!(
        note.target_block_id.as_deref(),
        Some(selection.block_ids[0].as_str())
    );
    let source_path = graph.root_dir.join(page.file_path.as_ref().unwrap());
    assert_eq!(fs::read_to_string(&source_path)?, raw);
    drop(graph);
    let graph = Graph::open(directory.path())?;
    assert_eq!(only(&graph).body, "A note");
    graph.reindex_all()?;
    let rebuilt = only(&graph);
    assert_eq!(rebuilt.id, note.id);
    assert_ne!(rebuilt.note_page_id, note.note_page_id);
    assert_eq!(rebuilt.status, ReadingNoteStatus::Recovered);
    let id = rebuilt.source.page_id.as_deref().unwrap();
    let source = graph.reading_source(&*graph.db.conn()?, id)?;
    assert_eq!(
        rebuilt.target_block_id.as_deref(),
        Some(source.blocks[2].id.as_str())
    );
    assert_ne!(rebuilt.target_block_id, note.target_block_id);
    assert_eq!(fs::read_to_string(&source_path)?, raw);
    // A copied ordinary file tree and a brand-new DB, not just in-memory state.
    let fresh = tempfile::tempdir_in(".")?;
    let fresh_graph = Graph::open(fresh.path())?;
    for relative in [page.file_path.as_ref().unwrap(), &note.file_path] {
        let dest = fresh_graph.root_dir.join(relative);
        fs::create_dir_all(dest.parent().unwrap())?;
        fs::copy(graph.root_dir.join(relative), dest)?;
    }
    fresh_graph.reindex_all()?;
    let restored = only(&fresh_graph);
    assert_eq!(restored.id, note.id);
    assert_eq!(restored.status, ReadingNoteStatus::Recovered);
    assert_eq!(restored.body, "A note");
    Ok(())
}

#[test]
fn reading_notes_body_is_literal_markdown_and_ordinary_edits_round_trip() -> Result<()> {
    let (directory, graph, page) = fixture("- Original\n")?;
    let body = "\n# Heading\n\nowner:: NOT metadata\nid:: NOT an ID\n- bullet\n  - nested\n\n```rust\nlet x = \"id:: body\";\n\n```\n\n[[A link]] and 😀\n  trailing spaces  \n\n";
    let note = create(&graph, &page, None, body)?;
    assert_eq!(only(&graph).body, body);
    let blocks = graph.db.list_blocks_for_page(&note.note_page_id)?;
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].content, body);
    let file = graph.root_dir.join(&note.file_path);
    assert!(fs::read_to_string(&file)?.ends_with(body));
    assert!(fs::read_to_string(&file)?.starts_with("reading-note:: {"));
    let changed = format!("{body}external-property:: still body\n");
    graph.update_block(&blocks[0].id, &changed, None)?;
    assert_eq!(only(&graph).body, changed);
    // A canonical fast-path rewrite must not insert outline syntax or an ID
    // line into the literal Markdown on the second and following edits.
    graph.update_block(&blocks[0].id, "second\nowner:: body\n", None)?;
    assert_eq!(only(&graph).body, "second\nowner:: body\n");
    graph.update_block(&blocks[0].id, &changed, None)?;
    let document = graph.reading_document(&file)?;
    let mut properties = document.properties;
    properties["category"] = "Synthetic reading".into();
    let changed = format!("{changed}# Full-page source edit\n\ncategory:: body, not metadata\n");
    graph.update_page_source_guarded(
        &note.note_page_id,
        &graph.get_page_source(&note.note_page_id)?,
        &encode(&properties, &changed),
    )?;
    assert_eq!(only(&graph).body, changed);
    assert_eq!(
        graph.db.get_page_by_id(&note.note_page_id)?.properties["category"],
        "Synthetic reading"
    );
    graph.reindex_all()?;
    assert_eq!(only(&graph).body, changed);
    drop(graph);
    let reopened = Graph::open(directory.path())?;
    let restored = only(&reopened);
    assert_eq!(restored.body, changed);
    assert_eq!(
        reopened
            .db
            .get_page_by_id(&restored.note_page_id)?
            .properties["category"],
        "Synthetic reading"
    );
    Ok(())
}

#[test]
fn reading_notes_hooks_require_a_valid_versioned_marker() -> Result<()> {
    for marker in [
        "Personal reading",
        "{}",
        "{\"version\":1}",
        "{\"version\":999,\"bodyBlockId\":\"some-id\"}",
    ] {
        let content =
            format!("reading-note:: {marker}\n\n- First\n  id:: one\n- Second\n  id:: two\n");
        let parsed = parser::parse_page(&content, "Ordinary.md");
        assert_eq!(parsed.blocks.len(), 2, "{marker}");
        assert_eq!(parsed.blocks[0].id.as_deref(), Some("one"));
        assert_eq!(parsed.blocks[1].id.as_deref(), Some("two"));
        assert!(parse_note_page(&content, "Ordinary.md").is_none());
        assert!(serialize_note_page(&parsed.properties, &[]).is_none());
        assert!(!is_note_page(&parsed.properties));
    }
    let (_directory, graph, page) = fixture("- Source\n")?;
    let note = create(&graph, &page, None, "literal\nowner:: body")?;
    let valid = graph.reading_document(&graph.root_dir.join(&note.file_path))?;
    assert!(is_note_page(&valid.properties));
    assert_eq!(
        parse_note_page(&valid.content, "Valid.md").unwrap().blocks[0].content,
        valid.body
    );
    Ok(())
}

#[test]
fn reading_notes_multiblock_utf16_and_rendered_selection_validation() -> Result<()> {
    let (_directory, graph, page) = fixture("- Before 😀 café after\n- Other **bold** words\n")?;
    let mut selection = select(&graph, &page, 0, "😀 café");
    assert_eq!(selection.parts[0].to - selection.parts[0].from, 7);
    let second = select(&graph, &page, 1, "bold");
    selection.parts.extend(second.parts);
    selection.block_ids.extend(second.block_ids);
    selection.text.push_str("\nbold");
    let note = create(&graph, &page, Some(&selection), "both")?;
    assert_eq!(note.quote, "😀 café\nbold");
    assert_eq!(note.status, ReadingNoteStatus::Attached);
    let mut wrong = selection.clone();
    wrong.parts[0].to -= 1;
    assert!(create(&graph, &page, Some(&wrong), "bad").is_err());
    wrong = selection.clone();
    wrong.page_id = "another page".into();
    assert!(create(&graph, &page, Some(&wrong), "bad").is_err());
    wrong = selection.clone();
    wrong.parts[1].block_id = "foreign".into();
    assert!(create(&graph, &page, Some(&wrong), "bad").is_err());
    let mut rendered = select(&graph, &page, 1, "bold");
    rendered.kind = SelectionKind::Rendered;
    rendered.parts[0].from = 6;
    rendered.parts[0].to = 10;
    rendered.parts[0].prefix = "Other ".into();
    rendered.parts[0].suffix = " words".into();
    assert_eq!(
        create(&graph, &page, Some(&rendered), "rendered")?.status,
        ReadingNoteStatus::Attached
    );
    rendered.parts[0].from = 5;
    rendered.parts[0].to = 11;
    rendered.parts[0].text = " bold ".into();
    rendered.parts[0].prefix = "Other".into();
    rendered.parts[0].suffix = "words".into();
    rendered.text = " bold ".into();
    assert_eq!(
        create(&graph, &page, Some(&rendered), "rendered with spaces")?.status,
        ReadingNoteStatus::Attached
    );
    let mut document = select(&graph, &page, 0, "😀 café");
    let source = graph.get_page_source(&page.id)?;
    let start = source.find("😀").unwrap();
    let from = source[..start].encode_utf16().count();
    document.document_range = Some(ReadingDocumentRange {
        from,
        to: from + 7,
        text: "😀 café".into(),
    });
    create(&graph, &page, Some(&document), "continuous document")?;
    document.document_range.as_mut().unwrap().from += 1;
    document.document_range.as_mut().unwrap().to += 1;
    assert!(create(&graph, &page, Some(&document), "stale document").is_err());
    Ok(())
}

#[test]
fn reading_notes_unicode_context_windows_use_utf16_not_bytes() -> Result<()> {
    let prefix = "a".repeat(63);
    let suffix = "b".repeat(63);
    let raw = format!("- 😀{prefix}😀選択{suffix}😀\n");
    let (_directory, graph, page) = fixture(&raw)?;
    let mut selection = select(&graph, &page, 0, "😀選択");
    // These are the UI's repaired 64-unit windows: exclude the split
    // surrogate at each outer boundary without changing selection offsets.
    selection.parts[0].prefix = prefix;
    selection.parts[0].suffix = suffix;
    assert_eq!(selection.parts[0].from, 65);
    assert_eq!(selection.parts[0].to, 69);
    let selection: ReadingSelection = serde_json::from_str(&serde_json::to_string(&selection)?)?;
    let note = create(&graph, &page, Some(&selection), "Unicode note 😀")?;
    assert_eq!(note.status, ReadingNoteStatus::Attached);
    let saved = graph.reading_document(&graph.root_dir.join(&note.file_path))?;
    assert_eq!(saved.metadata.anchor.offset_unit, "utf-16");
    assert_eq!(saved.metadata.anchor.parts[0].from, 65);
    assert_eq!(saved.metadata.anchor.parts[0].to, 69);

    let mut byte_offsets = selection.clone();
    byte_offsets.parts[0].from = 67;
    byte_offsets.parts[0].to = 71;
    assert!(create(&graph, &page, Some(&byte_offsets), "wrong offset unit").is_err());
    let mut split_surrogate = selection;
    split_surrogate.text = "😀".into();
    split_surrogate.parts[0].text = "😀".into();
    split_surrogate.parts[0].from = 1;
    split_surrogate.parts[0].to = 3;
    split_surrogate.parts[0].prefix.clear();
    split_surrogate.parts[0].suffix.clear();
    assert!(create(&graph, &page, Some(&split_surrogate), "split surrogate").is_err());

    graph.reindex_all()?;
    let recovered = only(&graph);
    assert_eq!(recovered.status, ReadingNoteStatus::Recovered);
    assert_eq!(recovered.quote, "😀選択");
    assert_eq!(recovered.body, "Unicode note 😀");
    assert_eq!(
        graph.get_page_source(recovered.source.page_id.as_deref().unwrap())?,
        raw
    );
    Ok(())
}

#[test]
fn reading_notes_external_edits_conflict_and_reattach_preserves_body_and_sources() -> Result<()> {
    let (_directory, graph, page) = fixture("- Before selected after\n  id:: stable-block\n")?;
    let selection = select(&graph, &page, 0, "selected");
    let note = create(&graph, &page, Some(&selection), "original body")?;
    let source = graph.root_dir.join(page.file_path.as_ref().unwrap());
    let original = fs::read(&source)?;
    let file = graph.root_dir.join(&note.file_path);
    let external = fs::read_to_string(&file)?.replace("original body", "external\nowner:: body");
    fs::write(&file, &external)?;
    assert!(graph
        .reading_note_update(&note.id, &note.revision, "stale overwrite")
        .unwrap_err()
        .to_string()
        .contains("revision conflict"));
    let edited = only(&graph);
    assert_eq!(edited.body, "external\nowner:: body");
    assert_ne!(edited.revision, note.revision);
    let updated = graph.reading_note_update(
        &note.id,
        &edited.revision,
        "saved\n\n```\nid:: literal\n```",
    )?;
    assert_eq!(updated.status, ReadingNoteStatus::Attached);
    let other = graph.create_page_with_content("2026-09-16", true, "- Journal target\n")?;
    let other_file = graph.root_dir.join(other.file_path.as_ref().unwrap());
    let other_original = fs::read(&other_file)?;
    let new_selection = select(&graph, &other, 0, "Journal");
    let reattached = graph.reading_note_reattach(
        &note.id,
        &updated.revision,
        &other.id,
        Some(&new_selection),
    )?;
    assert_eq!(reattached.body, updated.body);
    assert_eq!(reattached.quote, "Journal");
    assert_eq!(
        reattached.source.page_id.as_deref(),
        Some(other.id.as_str())
    );
    assert_eq!(graph.reading_notes_list(Some(&page.id))?.notes.len(), 0);
    assert_eq!(graph.reading_notes_list(Some(&other.id))?.notes.len(), 1);
    assert_eq!(fs::read(&source)?, original);
    assert_eq!(fs::read(&other_file)?, other_original);
    Ok(())
}

#[test]
fn reading_notes_recovery_never_guesses_after_source_changes() -> Result<()> {
    let (_directory, graph, page) = fixture("- Before selected after\n- Tail\n")?;
    let selection = select(&graph, &page, 0, "selected");
    create(&graph, &page, Some(&selection), "preserved")?;
    let file = graph.root_dir.join(page.file_path.as_ref().unwrap());
    fs::write(
        &file,
        "- New before\n- Before selected after\n- Tail\n- New after\n",
    )?;
    graph.reindex_all()?;
    let recovered = only(&graph);
    assert_eq!(recovered.status, ReadingNoteStatus::Recovered);
    let block = graph
        .db
        .get_block_by_id(recovered.target_block_id.as_ref().unwrap())?;
    assert_eq!(block.content, "Before selected after");
    fs::write(&file, "- Before selected after\n- Before selected after\n")?;
    graph.reindex_all()?;
    let ambiguous = only(&graph);
    assert_eq!(ambiguous.status, ReadingNoteStatus::Ambiguous);
    assert!(ambiguous.target_block_id.is_none());
    fs::write(&file, "- Selected text has been deleted\n")?;
    graph.reindex_all()?;
    let missing = only(&graph);
    assert_eq!(missing.status, ReadingNoteStatus::Orphaned);
    assert!(missing.source.page_id.is_some());
    assert_eq!(missing.quote, "selected");
    fs::remove_file(file)?;
    graph.reindex_all()?;
    let orphan = only(&graph);
    assert!(orphan.source.page_id.is_none());
    assert!(orphan.target_block_id.is_none());
    assert_eq!(orphan.body, "preserved");
    assert_eq!(orphan.status, ReadingNoteStatus::Orphaned);
    Ok(())
}

#[test]
fn reading_notes_stable_ids_verify_quotes_not_reused_index_slots() -> Result<()> {
    let (_directory, graph, page) = fixture("- Before selected after\n  id:: stable\n- Tail\n")?;
    let selection = select(&graph, &page, 0, "selected");
    create(&graph, &page, Some(&selection), "stable")?;
    let file = graph.root_dir.join(page.file_path.as_ref().unwrap());
    fs::write(
        &file,
        "- Changed before selected changed after\n  id:: stable\n- Tail\n",
    )?;
    assert_eq!(only(&graph).status, ReadingNoteStatus::Attached);
    fs::write(
        &file,
        "- Same ID but unrelated text\n  id:: stable\n- Tail\n",
    )?;
    let orphan = only(&graph);
    assert_eq!(orphan.status, ReadingNoteStatus::Orphaned);
    assert!(orphan.target_block_id.is_none());
    Ok(())
}

#[test]
fn reading_notes_rename_requires_verified_identity_not_a_title() -> Result<()> {
    let (_directory, graph, page) = fixture("- Unique selected passage\n")?;
    create(
        &graph,
        &page,
        Some(&select(&graph, &page, 0, "selected")),
        "rename",
    )?;
    let old = graph.root_dir.join(page.file_path.as_ref().unwrap());
    let new = graph.pages_dir.join("Renamed.md");
    fs::rename(&old, &new)?;
    graph.reindex_all()?;
    let recovered = only(&graph);
    assert_eq!(recovered.status, ReadingNoteStatus::Recovered);
    assert_eq!(recovered.source.file_path, "pages/Renamed.md");
    fs::copy(&new, graph.pages_dir.join("Copy.md"))?;
    graph.reindex_all()?;
    assert_eq!(only(&graph).status, ReadingNoteStatus::Ambiguous);
    fs::remove_file(&new)?;
    fs::write(graph.pages_dir.join("Copy.md"), "- Unrelated content\n")?;
    graph.reindex_all()?;
    assert_eq!(only(&graph).status, ReadingNoteStatus::Orphaned);
    Ok(())
}

#[test]
fn reading_notes_idempotency_never_overwrites_collisions() -> Result<()> {
    let (_directory, graph, page) = fixture("- Source\n")?;
    let id = Uuid::new_v4().to_string();
    let note = graph.create_file_reading_note(&id, &page.id, None, "original")?;
    let retry = graph.create_file_reading_note(&id, &page.id, None, "original")?;
    assert_eq!(note.revision, retry.revision);
    assert!(graph
        .create_file_reading_note(&id, &page.id, None, "different")
        .is_err());
    assert_eq!(only(&graph).body, "original");
    let collision = Uuid::new_v4().to_string();
    let path = graph.root_dir.join(format!("{DIRECTORY}/{collision}.md"));
    fs::write(&path, "Unrelated user file")?;
    assert!(graph
        .create_file_reading_note(&collision, &page.id, None, "overwrite")
        .is_err());
    assert_eq!(fs::read_to_string(path)?, "Unrelated user file");
    assert!(graph
        .create_file_reading_note("../outside", &page.id, None, "bad")
        .is_err());
    let updated = graph.reading_note_update(&id, &note.revision, "updated")?;
    assert!(graph
        .create_file_reading_note(&id, &page.id, None, "original")
        .is_err());
    assert_eq!(updated.body, "updated");
    Ok(())
}

#[test]
fn reading_notes_index_failure_keeps_authoritative_file_and_retry_recovers() -> Result<()> {
    let (_directory, graph, page) = fixture("- Source\n")?;
    graph.db.conn()?.execute_batch(
        "CREATE TRIGGER note_test_failure BEFORE INSERT ON blocks WHEN NEW.content = 'fail index'
         BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
    )?;
    let id = Uuid::new_v4().to_string();
    let failure = graph
        .create_file_reading_note(&id, &page.id, None, "fail index")
        .unwrap_err();
    assert!(failure.to_string().contains("file saved at"), "{failure}");
    let path = graph.root_dir.join(format!("{DIRECTORY}/{id}.md"));
    assert!(fs::read_to_string(&path)?.ends_with("fail index"));
    graph
        .db
        .conn()?
        .execute_batch("DROP TRIGGER note_test_failure;")?;
    let note = graph.create_file_reading_note(&id, &page.id, None, "fail index")?;
    assert_eq!(note.id, id);
    assert_eq!(only(&graph).body, "fail index");
    graph.db.conn()?.execute_batch(
        "CREATE TRIGGER note_update_failure BEFORE UPDATE ON blocks WHEN NEW.content = 'failed update'
         BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;"
    )?;
    let original = fs::read_to_string(&path)?;
    let failure = graph
        .reading_note_update(&note.id, &note.revision, "failed update")
        .unwrap_err();
    assert!(failure.to_string().contains("file saved at"), "{failure}");
    assert!(fs::read_to_string(&path)?.ends_with("failed update"));
    let backups = fs::read_dir(path.parent().unwrap())?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|v| v == "pending" || v == "displaced")
        })
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read_to_string(backups[0].path())?, original);
    graph
        .db
        .conn()?
        .execute_batch("DROP TRIGGER note_update_failure;")?;
    graph.reindex_all()?;
    assert_eq!(only(&graph).body, "failed update");
    Ok(())
}

#[test]
fn reading_notes_atomic_publication_refuses_a_racing_collision() -> Result<()> {
    let (_directory, graph, page) = fixture("- Source\n")?;
    let note = create(&graph, &page, None, "first")?;
    let path = graph.root_dir.join(&note.file_path);
    let mut conn = graph.db.conn()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    assert!(graph
        .persist_reading_note(tx, &path, None, "overwrite")
        .is_err());
    assert_eq!(only(&graph).body, "first");
    Ok(())
}

#[test]
fn reading_notes_can_be_moved_edited_and_split_as_ordinary_pages() -> Result<()> {
    let (_directory, graph, page) = fixture("- Source\n")?;
    let note = create(&graph, &page, None, "Original")?;
    let blocks = graph.db.list_blocks_for_page(&note.note_page_id)?;
    graph.insert_block_after(
        &note.note_page_id,
        &blocks[0].id,
        "Additional\nowner:: literal",
    )?;
    let body = only(&graph).body;
    assert!(body.contains("Original"));
    assert!(body.contains("Additional"));
    assert!(body.contains("owner:: literal"));
    let old = graph.root_dir.join(&note.file_path);
    let new = graph.knowledge_dir.join("Moved annotation.md");
    fs::rename(&old, &new)?;
    graph.reindex_all()?;
    let moved = only(&graph);
    assert_eq!(moved.body, body);
    assert_eq!(moved.file_path, "knowledge/Moved annotation.md");
    let updated = graph.reading_note_update(&note.id, &moved.revision, "Moved and edited")?;
    assert_eq!(updated.body, "Moved and edited");
    assert_eq!(
        graph.db.get_page_by_id(&updated.note_page_id)?.title,
        "Knowledge/Moved annotation"
    );
    Ok(())
}

#[test]
fn reading_notes_parse_errors_versions_and_paths_are_visible() -> Result<()> {
    let (_directory, graph, page) = fixture("- Source\n")?;
    let note = create(&graph, &page, None, "kept")?;
    let file = graph.root_dir.join(&note.file_path);
    let original = fs::read_to_string(&file)?;
    for content in [
        original.replace("\"version\":1", "\"version\":999"),
        original.replace("\"offsetUnit\":\"utf-16\"", "\"offsetUnit\":\"bytes\""),
        original.replace("pages/Books/Synthetic/Chapter.md", "../outside.md"),
        "reading-note:: {broken json}\n\nStill readable".into(),
    ] {
        fs::write(&file, &content)?;
        let listed = graph.reading_notes_list(None)?;
        assert!(listed.notes.is_empty());
        assert!(!listed.warnings.is_empty());
        assert_eq!(fs::read_to_string(&file)?, content);
        assert!(graph
            .reading_note_update(&note.id, &Graph::content_hash(&content), "overwrite")
            .is_err());
    }
    fs::write(&file, original)?;
    assert!(graph
        .reading_note_update(&note.id, "invalid revision", "overwrite")
        .is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn reading_notes_reject_symlink_sources_note_files_and_destination_directories() -> Result<()> {
    use std::os::unix::fs::symlink;
    let (_directory, graph, page) = fixture("- Source\n")?;
    let outside = tempfile::tempdir_in(".")?;
    let outside_file = outside.path().join("Outside.md");
    fs::write(&outside_file, "- Private synthetic data\n")?;
    let source = graph.root_dir.join(page.file_path.as_ref().unwrap());
    fs::remove_file(&source)?;
    symlink(outside_file.canonicalize()?, &source)?;
    assert!(create(&graph, &page, None, "bad").is_err());
    fs::remove_file(&source)?;
    fs::write(&source, "- Source\n")?;
    let folder = graph.root_dir.join(DIRECTORY);
    symlink(outside.path().canonicalize()?, &folder)?;
    assert!(create(&graph, &page, None, "bad").is_err());
    fs::remove_file(&folder)?;
    let note = create(&graph, &page, None, "safe")?;
    let file = graph.root_dir.join(&note.file_path);
    fs::remove_file(&file)?;
    symlink(outside_file.canonicalize()?, &file)?;
    assert!(graph
        .reading_note_update(&note.id, &note.revision, "bad")
        .is_err());
    assert!(!graph.reading_notes_list(None)?.warnings.is_empty());
    assert_eq!(
        fs::read_to_string(outside_file)?,
        "- Private synthetic data\n"
    );
    Ok(())
}

#[test]
fn reading_notes_are_in_existing_sync_tree() -> Result<()> {
    use crate::sync::backend::SyncBackend;
    let (_directory, graph, page) = fixture("- Source\n")?;
    let note = create(&graph, &page, None, "portable")?;
    let backend =
        crate::sync::filesystem::FilesystemBackend::new(graph.root_dir.clone(), "synthetic".into());
    assert!(backend
        .list_files()?
        .iter()
        .any(|f| f.rel_path == note.file_path));
    Ok(())
}
