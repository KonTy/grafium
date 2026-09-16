use super::*;

fn fixture(raw: &str) -> Result<(tempfile::TempDir, Graph, Page)> {
    let directory = tempfile::tempdir_in(".")?;
    let graph = Graph::open(directory.path())?;
    let page = graph.create_page_with_content("Books/Synthetic", false, raw)?;
    Ok((directory, graph, page))
}

fn select(graph: &Graph, page: &Page, ordinal: usize, quote: &str) -> ReadingSelection {
    let source = graph
        .reading_source(&graph.db.conn().unwrap(), &page.id)
        .unwrap();
    let block = &source.blocks[ordinal];
    let byte = block.content.find(quote).unwrap();
    let from = block.content[..byte].encode_utf16().count();
    ReadingSelection {
        page_id: page.id.clone(),
        block_ids: vec![block.id.clone()],
        text: quote.into(),
        kind: SelectionKind::Source,
        document_range: None,
        parts: vec![ReadingSelectionPart {
            block_id: block.id.clone(),
            text: quote.into(),
            from,
            to: from + quote.encode_utf16().count(),
            prefix: block.content[..byte].into(),
            suffix: block.content[byte + quote.len()..].into(),
        }],
    }
}

fn create(
    graph: &Graph,
    page: &Page,
    selection: Option<&ReadingSelection>,
    body: &str,
) -> Result<ReadingNote> {
    graph.reading_note_create(&Uuid::new_v4().to_string(), &page.id, selection, body)
}

fn notes(graph: &Graph) -> Vec<ReadingNote> {
    let result = graph.reading_notes_list(None).unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    result.notes
}

#[test]
fn inline_footnotes_are_common_markdown_and_preserve_raw_source_and_block_ids() -> Result<()> {
    let raw = "- # Chapter\n  - Before selected sentence after.\n  - Source tail\n";
    let (_directory, graph, page) = fixture(raw)?;
    let before = graph.db.list_blocks_for_page(&page.id)?;
    let selection = select(&graph, &page, 1, "selected sentence");
    let note = create(
        &graph,
        &page,
        Some(&selection),
        "My **annotation**\n\nowner:: literal\n```rust\nlet x = 1;\n```\n",
    )?;
    assert_eq!(note.storage, ReadingNoteStorage::Inline);
    assert_eq!(note.note_page_id, page.id);
    assert_eq!(note.file_path, page.file_path.clone().unwrap());
    assert_eq!(note.footnote_label.as_deref(), Some("grafium-note-1"));
    assert_eq!(note.status, ReadingNoteStatus::Attached);
    let content = graph.get_page_source(&page.id)?;
    assert!(content.contains("after. [^grafium-note-1]"));
    assert!(content.contains(
        "\n[^grafium-note-1]: My **annotation**\n    \n    owner:: literal\n    ```rust\n"
    ));
    assert_eq!(source_evidence(&content), raw.trim_end_matches('\n'));
    let after = graph.db.list_blocks_for_page(&page.id)?;
    for block in before {
        assert!(after.iter().any(|b| b.id == block.id));
    }
    let annotation = graph
        .db
        .get_block_by_id(note.note_block_id.as_ref().unwrap())?;
    assert!(parser::is_reading_note_block(&annotation));
    assert_eq!(annotation.content, note.body);
    assert_eq!(annotation.properties["reading-note-storage"], "inline");
    assert!(annotation.parent_id.is_none());
    let indexed: i64 = graph.db.conn()?.query_row(
        "SELECT COUNT(*) FROM fts_blocks WHERE fts_blocks MATCH 'annotation' AND block_id = ?1",
        [&annotation.id],
        |row| row.get(0),
    )?;
    assert_eq!(indexed, 1);
    let events: Vec<_> =
        pulldown_cmark::Parser::new_ext(&content, pulldown_cmark::Options::all()).collect();
    assert!(events.iter().any(|e| matches!(e, pulldown_cmark::Event::Start(pulldown_cmark::Tag::FootnoteDefinition(label)) if label.as_ref() == "grafium-note-1")));
    assert!(!graph.root_dir.join(DIRECTORY).exists());
    Ok(())
}

#[test]
fn inline_notes_survive_single_file_copy_reopen_reindex_and_note_only_edits() -> Result<()> {
    let raw = "- # Chapter\n  - Identical repeated quote.\n  - Identical repeated quote.\n- Tail\n";
    let (directory, graph, page) = fixture(raw)?;
    let first = create(
        &graph,
        &page,
        Some(&select(&graph, &page, 1, "repeated")),
        "first",
    )?;
    let second = create(
        &graph,
        &page,
        Some(&select(&graph, &page, 2, "repeated")),
        "second",
    )?;
    assert_eq!(second.footnote_label.as_deref(), Some("grafium-note-2"));
    assert!(notes(&graph)
        .iter()
        .all(|n| n.status == ReadingNoteStatus::Attached));
    let current_first = notes(&graph)
        .into_iter()
        .find(|n| n.id == first.id)
        .unwrap();
    graph.reading_note_update(&first.id, &current_first.revision, "changed first")?;
    assert!(notes(&graph)
        .iter()
        .all(|n| n.status == ReadingNoteStatus::Attached));
    let content = graph.get_page_source(&page.id)?;
    assert_eq!(source_evidence(&content), raw.trim_end_matches('\n'));
    drop(graph);
    let graph = Graph::open(directory.path())?;
    assert_eq!(notes(&graph).len(), 2);
    graph.reindex_all()?;
    let restored = notes(&graph);
    assert!(restored
        .iter()
        .all(|n| n.status == ReadingNoteStatus::Recovered));
    assert_ne!(restored[0].target_block_id, restored[1].target_block_id);
    let fresh = tempfile::tempdir_in(".")?;
    let other = Graph::open(fresh.path())?;
    fs::write(other.pages_dir.join("Portable book.md"), &content)?;
    other.reindex_all()?;
    let copied = notes(&other);
    assert_eq!(copied.len(), 2);
    assert!(copied.iter().all(
        |n| n.status == ReadingNoteStatus::Recovered && n.file_path == "pages/Portable book.md"
    ));
    assert!(copied
        .iter()
        .any(|n| n.id == first.id && n.body == "changed first"));
    Ok(())
}

#[test]
fn inline_update_and_normal_edit_roundtrip_body_metadata_and_author_footnotes() -> Result<()> {
    let raw = "- Author paragraph [^author]\n\n[^author]: The author's definition.\n";
    let (directory, graph, page) = fixture(raw)?;
    let body = "\n# Heading\nid:: literal\nowner:: literal\n\n```md\n[^author]: example\n<!-- /grafium-reading-note -->\n```\n\nCRLF\r\nunicode 😀\n";
    let note = create(
        &graph,
        &page,
        Some(&select(&graph, &page, 0, "Author")),
        body,
    )?;
    assert_eq!(note.body, body);
    let before = graph.get_page_source(&page.id)?;
    let changed =
        graph.reading_note_update(&note.id, &note.revision, &format!("{body}More\n\n"))?;
    assert_eq!(
        source_evidence(&graph.get_page_source(&page.id)?),
        source_evidence(&before)
    );
    assert!(graph
        .get_page_source(&page.id)?
        .contains("[^author]: The author's definition."));
    let block_id = changed.note_block_id.clone().unwrap();
    let before_block_edit = graph.get_page_source(&page.id)?;
    graph.update_block(&block_id, "ordinary block edit\nowner:: still body\n", None)?;
    assert_eq!(
        source_evidence(&graph.get_page_source(&page.id)?),
        source_evidence(&before_block_edit)
    );
    assert_eq!(
        notes(&graph)[0].body,
        "ordinary block edit\nowner:: still body\n"
    );
    let source = graph
        .get_page_source(&page.id)?
        .replace("ordinary block edit", "whole source edit");
    graph.update_page_source_guarded(&page.id, &graph.get_page_source(&page.id)?, &source)?;
    graph.reindex_all()?;
    assert_eq!(
        notes(&graph)[0].body,
        "whole source edit\nowner:: still body\n"
    );
    drop(graph);
    let reopened = Graph::open(directory.path())?;
    assert_eq!(
        notes(&reopened)[0].body,
        "whole source edit\nowner:: still body\n"
    );
    Ok(())
}

#[test]
fn inline_real_passage_changes_are_ambiguous_not_nearest_position() -> Result<()> {
    let (_directory, graph, page) = fixture("- Before selected after\n- Tail\n")?;
    let note = create(
        &graph,
        &page,
        Some(&select(&graph, &page, 0, "selected")),
        "keep",
    )?;
    let file = graph.root_dir.join(&note.file_path);
    let content = fs::read_to_string(&file)?.replace("- Tail", "- Before selected after\n- Tail");
    fs::write(&file, content)?;
    graph.reindex_all()?;
    let ambiguous = notes(&graph).remove(0);
    assert_eq!(ambiguous.status, ReadingNoteStatus::Ambiguous);
    assert!(ambiguous.target_block_id.is_none());
    assert_eq!(ambiguous.body, "keep");
    Ok(())
}

#[test]
fn inline_revision_conflicts_preserve_external_bytes_before_and_at_exchange() -> Result<()> {
    let (_directory, graph, page) = fixture("- Source selected text\n")?;
    let note = create(&graph, &page, None, "original")?;
    let file = graph.root_dir.join(&note.file_path);
    let original = fs::read_to_string(&file)?;
    let external = original
        .replace("Source selected text", "External source edit")
        .replace(
            "[^grafium-note-1]: original",
            "[^grafium-note-1]: external body",
        );
    fs::write(&file, &external)?;
    assert!(graph
        .reading_note_update(&note.id, &note.revision, "stale")
        .unwrap_err()
        .to_string()
        .contains("revision conflict"));
    assert_eq!(fs::read_to_string(&file)?, external);
    graph.index_file(&file)?;
    let draft = external.replace(
        "[^grafium-note-1]: external body",
        "[^grafium-note-1]: new draft",
    );
    let concurrent = external.replace("External source edit", "Latest racing external data");
    let mut conn = graph.db.conn()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let failure = graph
        .persist_reading_note_with_hook(tx, &file, Some(&external), &draft, |path| {
            fs::write(path, &concurrent)?;
            Ok(())
        })
        .unwrap_err();
    assert!(failure.to_string().contains("AT replacement"), "{failure}");
    assert_eq!(fs::read_to_string(&file)?, draft);
    let backups = fs::read_dir(file.parent().unwrap())?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".reading-note-")
        })
        .collect::<Vec<_>>();
    assert!(backups
        .iter()
        .any(|entry| fs::read_to_string(entry.path()).ok().as_deref() == Some(&concurrent)));
    assert_eq!(
        graph
            .db
            .get_block_by_id(note.note_block_id.as_ref().unwrap())?
            .content,
        "external body"
    );
    Ok(())
}

#[test]
fn inline_and_legacy_duplicate_files_are_quarantined_before_any_indexing() -> Result<()> {
    for legacy in [false, true] {
        let (_directory, graph, page) = fixture("- Source\n")?;
        let id = Uuid::new_v4().to_string();
        let note = if legacy {
            graph.create_file_reading_note(&id, &page.id, None, "Never lose this body")?
        } else {
            graph.reading_note_create(&id, &page.id, None, "Never lose this body")?
        };
        let file = graph.root_dir.join(&note.file_path);
        let original = fs::read_to_string(&file)?;
        let copy = graph.pages_dir.join("Copied annotation.md");
        fs::copy(&file, &copy)?;
        assert!(graph.index_file(&copy).is_err());
        let owner = graph
            .db
            .get_block_by_id(note.note_block_id.as_ref().unwrap())?;
        assert_eq!(owner.page_id, note.note_page_id);
        assert_eq!(owner.content, "Never lose this body");
        let listed = graph.reading_notes_list(None)?;
        assert!(listed.notes.is_empty());
        assert!(!listed.warnings.is_empty());
        assert!(graph
            .reading_note_update(&note.id, &note.revision, "overwrite")
            .is_err());
        assert!(graph.reindex_all().is_err());
        assert_eq!(fs::read_to_string(&file)?, original);
        assert_eq!(fs::read_to_string(&copy)?, original);
    }
    Ok(())
}

#[test]
fn inline_reattach_same_file_preserves_other_notes_and_cross_file_rejects_cleanly() -> Result<()> {
    let (_directory, graph, page) = fixture("- First selected\n- Second destination\n")?;
    let note = create(
        &graph,
        &page,
        Some(&select(&graph, &page, 0, "selected")),
        "body",
    )?;
    let other_note = create(
        &graph,
        &page,
        None,
        "Keep this reference [^grafium-note-1] in another note",
    )?;
    let current = notes(&graph).into_iter().find(|n| n.id == note.id).unwrap();
    let changed = graph.reading_note_reattach(
        &note.id,
        &current.revision,
        &page.id,
        Some(&select(&graph, &page, 1, "destination")),
    )?;
    assert_eq!(changed.quote, "destination");
    let content = graph.get_page_source(&page.id)?;
    assert!(content.contains("- First selected\n"));
    assert!(content.contains("[^grafium-note-1]"));
    assert_eq!(
        notes(&graph)
            .into_iter()
            .find(|n| n.id == other_note.id)
            .unwrap()
            .body,
        other_note.body
    );
    let other = graph.create_page_with_content("Other", false, "- Other\n")?;
    let other_original = graph.get_page_source(&other.id)?;
    assert!(graph
        .reading_note_reattach(&note.id, &changed.revision, &other.id, None)
        .unwrap_err()
        .to_string()
        .contains("neither source file was changed"));
    assert_eq!(graph.get_page_source(&page.id)?, content);
    assert_eq!(graph.get_page_source(&other.id)?, other_original);
    Ok(())
}

#[test]
fn inline_code_math_urls_and_escaped_author_comments_are_not_rewritten() -> Result<()> {
    for raw in [
        "- ```rust\n  let x = 1;\n  ```\n",
        "- $a+b$ and https://example.invalid/path\n",
        "- [[Page|label]] and [label](https://example.invalid)\n",
    ] {
        let (_directory, graph, page) = fixture(raw)?;
        let note = create(&graph, &page, None, "safe")?;
        assert_eq!(
            source_evidence(&graph.get_page_source(&page.id)?),
            raw.trim_end_matches('\n')
        );
        assert_eq!(note.status, ReadingNoteStatus::Attached);
    }
    let author = "- `[^grafium-note-1]` and [^author]\n\n[^author]: author\n\n\\<!-- grafium-reading-note {\"version\":2} -->\n";
    assert_eq!(parser::strip_reading_note_references(author), author);
    assert!(parse_inline_reading_notes(author).warnings.is_empty());
    Ok(())
}

#[test]
fn inline_table_markers_preserve_rendered_table_ast_cells_and_visibility() -> Result<()> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    for raw in [
        "- | A | B |\n  | --- | --- |\n  | C | D |",
        "- | A | B |\n  | :--- | ---: |\n  | C | D |\n",
        "- Intro\n\n  | A | B |\n  | --- | --- |\n  | C | D |\n",
    ] {
        let (_directory, graph, page) = fixture(raw)?;
        let original_ast = table_ast(raw);
        assert!(
            !original_ast.is_empty(),
            "fixture must actually render a table"
        );
        let ordinal = graph
            .reading_source(&*graph.db.conn()?, &page.id)?
            .blocks
            .iter()
            .position(|block| block.content.contains("| C | D |"))
            .unwrap();
        let note = create(
            &graph,
            &page,
            Some(&select(&graph, &page, ordinal, "C")),
            "Table note",
        )?;
        let saved = graph.get_page_source(&page.id)?;
        assert_eq!(table_ast(&saved), original_ast);
        assert!(saved.contains("| --- | --- |") || saved.contains("| :--- | ---: |"));
        let mut cells = Vec::new();
        let mut cell = None;
        let mut tables = 0;
        let mut refs = 0;
        for event in Parser::new_ext(&saved, Options::all()) {
            match event {
                Event::Start(Tag::Table(_)) => tables += 1,
                Event::Start(Tag::TableCell) => cell = Some(String::new()),
                Event::Text(text) if cell.is_some() => cell.as_mut().unwrap().push_str(&text),
                Event::End(TagEnd::TableCell) => cells.push(cell.take().unwrap()),
                Event::FootnoteReference(label)
                    if Some(label.as_ref()) == note.footnote_label.as_deref() =>
                {
                    assert!(cell.is_none(), "reference belongs outside the table");
                    refs += 1;
                }
                _ => {}
            }
        }
        assert_eq!(tables, 1);
        assert_eq!(cells, ["A", "B", "C", "D"]);
        assert_eq!(
            refs, 1,
            "reference must be rendered, not discarded as an extra cell"
        );
        assert_eq!(source_evidence(&saved), raw.trim_end_matches('\n'));
    }
    Ok(())
}

#[test]
fn inline_structural_only_sources_use_visible_separate_paragraphs() -> Result<()> {
    use pulldown_cmark::{Event, Options, Parser};
    for raw in [
        "- # Heading ##\n",
        "- Setext heading\n  ---\n",
        "- ```rust\n  let a = 1;\n  ```\n",
        "- ---\n",
    ] {
        let (_directory, graph, page) = fixture(raw)?;
        let note = create(&graph, &page, None, "Structural note")?;
        let saved = graph.get_page_source(&page.id)?;
        assert!(saved.starts_with(raw), "{saved}");
        assert!(saved.contains("\n\n[^grafium-note-1]\n"));
        assert!(Parser::new_ext(&saved, Options::all()).any(|event| {
            matches!(event, Event::FootnoteReference(label) if Some(label.as_ref()) == note.footnote_label.as_deref())
        }));
    }
    Ok(())
}

#[test]
fn inline_guarded_source_save_preserves_actual_external_bytes_at_exchange() -> Result<()> {
    let raw = "- Original source\n";
    let (_directory, graph, page) = fixture(raw)?;
    let path = graph.root_dir.join(page.file_path.as_ref().unwrap());
    let draft = "- Editor's source edit\n";
    let concurrent = "- Latest external source, not the expected base\n";
    let failure = graph
        .update_reading_source_file_with_hook(&path, raw, draft, |path| {
            fs::write(path, concurrent)?;
            Ok(())
        })
        .unwrap_err();
    assert!(failure.to_string().contains("AT replacement"), "{failure}");
    assert_eq!(fs::read_to_string(&path)?, draft);
    assert!(fs::read_dir(path.parent().unwrap())?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with(".reading-note-"))
        .any(|entry| fs::read_to_string(entry.path()).ok().as_deref() == Some(concurrent)));
    assert_eq!(
        graph.db.list_blocks_for_page(&page.id)?[0].content,
        "Original source"
    );
    Ok(())
}

#[test]
fn inline_utf16_and_idempotent_retry_are_independent_of_other_note_edits() -> Result<()> {
    let raw = "- 😀 prefix 😀選択 suffix\n- Other paragraph\n";
    let (_directory, graph, page) = fixture(raw)?;
    let selection = select(&graph, &page, 0, "😀選択");
    assert_eq!(selection.parts[0].from, 10);
    assert_eq!(selection.parts[0].to, 14);
    let id = Uuid::new_v4().to_string();
    let first = graph.reading_note_create(&id, &page.id, Some(&selection), "Unicode 😀")?;
    create(&graph, &page, None, "Other note")?;
    let retried = graph.reading_note_create(&id, &page.id, Some(&selection), "Unicode 😀")?;
    assert_eq!(retried.id, first.id);
    assert_eq!(retried.revision, first.revision);
    assert_eq!(notes(&graph).len(), 2);
    let mut wrong = selection.clone();
    wrong.parts[0].from += 1;
    wrong.parts[0].to += 1;
    assert!(create(&graph, &page, Some(&wrong), "wrong").is_err());
    let document = graph
        .reading_documents(&graph.root_dir.join(&first.file_path), &mut Vec::new())?
        .into_iter()
        .find(|d| d.metadata.id == id)
        .unwrap();
    assert_eq!(document.metadata.anchor.parts[0].from, 10);
    assert_eq!(document.metadata.anchor.offset_unit, "utf-16");
    assert!(graph
        .reading_note_create(&id, &page.id, None, "different")
        .is_err());
    assert_eq!(
        source_evidence(&graph.get_page_source(&page.id)?),
        raw.trim_end_matches('\n')
    );
    Ok(())
}

#[test]
fn inline_external_note_edits_and_index_failures_keep_every_file_version() -> Result<()> {
    let (_directory, graph, page) = fixture("- Source\n")?;
    let note = create(&graph, &page, None, "initial")?;
    let file = graph.root_dir.join(&note.file_path);
    let changed = fs::read_to_string(&file)?.replace(
        "[^grafium-note-1]: initial",
        "[^grafium-note-1]: External\n    owner:: literal",
    );
    fs::write(&file, &changed)?;
    let read = notes(&graph).remove(0);
    assert_eq!(read.body, "External\nowner:: literal");
    assert!(graph
        .reading_note_update(&note.id, &note.revision, "stale")
        .is_err());
    graph.db.conn()?.execute_batch(
        "CREATE TRIGGER inline_note_failure BEFORE UPDATE ON blocks WHEN NEW.content = 'failure body'
         BEGIN SELECT RAISE(ABORT, 'synthetic index failure'); END;"
    )?;
    let failure = graph
        .reading_note_update(&note.id, &read.revision, "failure body")
        .unwrap_err();
    assert!(failure.to_string().contains("file saved at"), "{failure}");
    assert!(fs::read_to_string(&file)?.contains("[^grafium-note-1]: failure body"));
    let backups = fs::read_dir(file.parent().unwrap())?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".reading-note-")
        });
    assert!(backups
        .into_iter()
        .any(|entry| fs::read_to_string(entry.path()).ok().as_deref() == Some(&changed)));
    graph
        .db
        .conn()?
        .execute_batch("DROP TRIGGER inline_note_failure;")?;
    graph.reindex_all()?;
    assert_eq!(notes(&graph)[0].body, "failure body");
    Ok(())
}

#[test]
fn inline_general_index_rejects_plain_block_trying_to_take_annotation_id() -> Result<()> {
    let (_directory, graph, page) = fixture("- Source\n")?;
    let note = create(&graph, &page, None, "body retained")?;
    let intruder = graph.pages_dir.join("Ordinary block copy.md");
    fs::write(
        &intruder,
        format!(
            "- Unrelated text\n  id:: {}\n",
            note.note_block_id.as_ref().unwrap()
        ),
    )?;
    assert!(graph.index_file(&intruder).is_err());
    assert_eq!(
        graph
            .db
            .get_block_by_id(note.note_block_id.as_ref().unwrap())?
            .content,
        "body retained"
    );
    assert_eq!(notes(&graph)[0].body, "body retained");
    Ok(())
}

#[test]
fn inline_source_block_edits_preserve_annotations_and_reject_unsynced_external_notes() -> Result<()>
{
    let (_directory, graph, page) =
        fixture("- Source paragraph\n  id:: source\n- Other paragraph\n  id:: other\n")?;
    let note = create(
        &graph,
        &page,
        Some(&select(&graph, &page, 0, "Source")),
        "Annotation",
    )?;
    graph.update_block("other", "A deliberate source edit", None)?;
    assert_eq!(notes(&graph)[0].body, "Annotation");
    let file = graph.root_dir.join(&note.file_path);
    let external = fs::read_to_string(&file)?.replace(
        "[^grafium-note-1]: Annotation",
        "[^grafium-note-1]: External annotation edit",
    );
    fs::write(&file, &external)?;
    assert!(graph
        .update_block("other", "Do not lose external annotation", None)
        .is_err());
    assert_eq!(fs::read_to_string(&file)?, external);
    assert_eq!(
        graph.db.get_block_by_id("other")?.content,
        "A deliberate source edit"
    );
    graph.index_file(&file)?;
    graph.update_block("other", "After refresh", None)?;
    assert_eq!(notes(&graph)[0].body, "External annotation edit");
    graph.reindex_all()?;
    assert_eq!(notes(&graph)[0].body, "External annotation edit");
    Ok(())
}

#[test]
fn inline_malformed_versions_labels_and_anchors_produce_visible_warnings_without_rewrites(
) -> Result<()> {
    let (_directory, graph, page) = fixture("- Source\n")?;
    let note = create(&graph, &page, None, "Body preserved")?;
    let file = graph.root_dir.join(&note.file_path);
    let original = fs::read_to_string(&file)?;
    for changed in [
        original.replace("\"version\":2", "\"version\":99"),
        original.replace("[^grafium-note-1]: Body", "[^grafium-note-9]: Body"),
        original.replace("pages/Books/Synthetic.md", "../outside.md"),
        original.replace(parser::reading_notes::CLOSE, "<!-- missing terminator -->"),
    ] {
        fs::write(&file, &changed)?;
        let result = graph.reading_notes_list(Some(&page.id))?;
        assert!(result.notes.is_empty());
        assert!(!result.warnings.is_empty());
        assert!(graph.index_file(&file).is_err());
        assert!(graph
            .reading_note_update(&note.id, &Graph::content_hash(&changed), "overwrite")
            .is_err());
        assert_eq!(fs::read_to_string(&file)?, changed);
    }
    Ok(())
}

#[test]
fn inline_rendered_selection_ignores_existing_managed_references() -> Result<()> {
    let (_directory, graph, page) = fixture("- Before **selected** after\n")?;
    create(&graph, &page, None, "Existing annotation")?;
    let mut selection = select(&graph, &page, 0, "selected");
    selection.kind = SelectionKind::Rendered;
    selection.parts[0].from = 7;
    selection.parts[0].to = 15;
    selection.parts[0].prefix = "Before ".into();
    // Ignoring the reference chrome can leave its preceding DOM whitespace.
    selection.parts[0].suffix = " after ".into();
    let note = create(&graph, &page, Some(&selection), "Rendered quote")?;
    assert_eq!(note.quote, "selected");
    assert_eq!(note.status, ReadingNoteStatus::Attached);
    assert!(notes(&graph)
        .iter()
        .all(|note| note.status == ReadingNoteStatus::Attached));
    graph.reindex_all()?;
    assert!(notes(&graph)
        .iter()
        .all(|note| note.status == ReadingNoteStatus::Recovered));
    Ok(())
}

#[test]
fn inline_definition_revisions_ignore_other_notes_and_preserve_new_source_edits() -> Result<()> {
    let raw = "- First selected passage.\n- Other paragraph.\n";
    let (_directory, graph, page) = fixture(raw)?;
    let a = create(
        &graph,
        &page,
        Some(&select(&graph, &page, 0, "selected")),
        "Body A",
    )?;
    let b = create(&graph, &page, None, "Body B")?;
    assert_eq!(
        notes(&graph)
            .into_iter()
            .find(|n| n.id == a.id)
            .unwrap()
            .revision,
        a.revision
    );
    let b = graph.reading_note_update(&b.id, &b.revision, "Body B edited")?;
    let a = graph.reading_note_update(&a.id, &a.revision, "Body A edited")?;
    let preserved_b = notes(&graph).into_iter().find(|n| n.id == b.id).unwrap();
    assert_eq!(preserved_b.body, "Body B edited");
    assert_eq!(preserved_b.revision, b.revision);
    let file = graph.root_dir.join(&a.file_path);
    assert_eq!(
        source_evidence(&fs::read_to_string(&file)?),
        raw.trim_end_matches('\n')
    );

    let external_a = fs::read_to_string(&file)?.replace(
        "[^grafium-note-1]: Body A edited",
        "[^grafium-note-1]: External A",
    );
    fs::write(&file, &external_a)?;
    assert!(graph
        .reading_note_update(&a.id, &a.revision, "stale A")
        .unwrap_err()
        .to_string()
        .contains("revision conflict"));
    assert_eq!(fs::read_to_string(&file)?, external_a);
    let current_a = notes(&graph).into_iter().find(|n| n.id == a.id).unwrap();

    let external_source =
        fs::read_to_string(&file)?.replace("First selected passage.", "Completely revised source.");
    fs::write(&file, &external_source)?;
    // Do not reindex first: a freshly read file snapshot must preserve this
    // source edit even though the SQLite rows still describe the older book.
    let a = graph.reading_note_update(&a.id, &current_a.revision, "My followup")?;
    assert_eq!(a.status, ReadingNoteStatus::Orphaned);
    assert_eq!(a.quote, "selected");
    assert!(a.target_block_id.is_none());
    let current = fs::read_to_string(&file)?;
    assert!(current.starts_with("- Completely revised source."));
    assert!(current.contains("[^grafium-note-2]: Body B edited"));
    let b = graph.reading_note_update(&b.id, &b.revision, "Body B edited again")?;
    let repaired = graph.reading_note_reattach(&a.id, &a.revision, &page.id, None)?;
    assert_eq!(repaired.body, "My followup");
    assert_eq!(repaired.status, ReadingNoteStatus::Attached);
    assert!(repaired.quote.is_empty());
    assert_eq!(
        notes(&graph)
            .into_iter()
            .find(|n| n.id == b.id)
            .unwrap()
            .revision,
        b.revision
    );
    Ok(())
}

#[test]
fn inline_revision_hashes_exact_envelope_and_legacy_revision_hashes_whole_file() -> Result<()> {
    let (_directory, graph, page) = fixture("- Source\n")?;
    let inline = create(&graph, &page, None, "Inline body")?;
    let content = graph.get_page_source(&page.id)?;
    let parsed = parse_inline_reading_notes(&content);
    let envelope = &content[parsed.notes[0].range.clone()];
    assert_eq!(inline.revision, Graph::content_hash(envelope));
    assert_ne!(inline.revision, Graph::content_hash(&content));
    let legacy = graph.create_file_reading_note(
        &Uuid::new_v4().to_string(),
        &page.id,
        None,
        "Legacy body",
    )?;
    assert_eq!(
        legacy.revision,
        Graph::content_hash(&fs::read_to_string(graph.root_dir.join(&legacy.file_path))?)
    );
    Ok(())
}

#[test]
fn inline_native_cross_language_fixture() -> Result<()> {
    let raw = concat!(
        "category:: synthetic-fixture\n\n",
        "- # A synthetic chapter\n",
        "  - A **short passage** about café and 😀.\n",
        "  - A second paragraph preserves the author's [^author] reference.\n",
        "- Final source paragraph.\n\n",
        "[^author]: An author footnote, not a managed annotation.\n",
    );
    let (directory, graph, page) = fixture(raw)?;
    create(
        &graph,
        &page,
        Some(&select(&graph, &page, 1, "short passage")),
        concat!(
            "# Why this matters\n\n",
            "owner:: reader, not metadata\n",
            "id:: literal body text\n\n",
            "```rust\nlet label = \"body:: not metadata\";\n```\n\n",
            "Keep **bold**, café, and 😀.\n",
        ),
    )?;
    create(
        &graph,
        &page,
        Some(&select(&graph, &page, 2, "second paragraph")),
        "A second note.\n\n- An ordinary list item\n- Unicode: café 😀\n\n",
    )?;
    let relative = page.file_path.clone().unwrap();
    drop(graph);
    let graph = Graph::open(directory.path())?;
    graph.reindex_all()?;
    let source_file = graph.root_dir.join(relative);
    let page_id = graph.reading_page_id(&source_file)?.unwrap();
    let listed = graph.reading_notes_list(Some(&page_id))?;
    assert!(listed.warnings.is_empty());
    assert_eq!(listed.notes.len(), 2);
    let artifact = serde_json::json!({
        "fixtureVersion": 1,
        "sourceMarkdown": fs::read_to_string(&source_file)?,
        "page": graph.db.get_page_by_id(&page_id)?,
        "blocks": graph.db.list_blocks_for_page(&page_id)?,
        "notes": listed.notes,
        "warnings": listed.warnings,
    });
    assert_eq!(artifact["notes"][0]["storage"], "inline");
    assert!(artifact["blocks"].as_array().unwrap().len() > 2);
    if let Ok(destination) = std::env::var("GRAFIUM_READING_NOTES_FIXTURE_PATH") {
        let destination = Path::new(&destination);
        if destination.is_absolute() {
            return Err(error(
                "fixture output must be relative to the test working directory",
            ));
        }
        Graph::atomic_write(destination, &serde_json::to_string_pretty(&artifact)?)?;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(destination)?)?,
            artifact
        );
        println!(
            "Native cross-language fixture written to {}",
            destination.display()
        );
    }
    Ok(())
}
