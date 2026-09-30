use super::*;

fn import_book(graph: &Graph, source: &Path, name: &str, content: &str) -> books::BookInfo {
    let file = source.join(format!("{name}.fb2"));
    fs::write(
        &file,
        format!("<FictionBook><body><section><p>{content}</p></section></body></FictionBook>"),
    )
    .unwrap();
    graph.import_original_book(&file).unwrap()
}

#[test]
fn reindex_includes_every_graph_book_without_the_external_originals_or_index_rows() {
    let root = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let graph = Graph::open(root.path()).unwrap();
    let first = import_book(
        &graph,
        external.path(),
        "First",
        "Cobalt author evidence #science.",
    );
    let second = import_book(
        &graph,
        external.path(),
        "Second",
        "Neodymium author evidence.",
    );
    let converted = graph
        .create_page_with_content("Books/Converted", false, "- Zirconium converted text.")
        .unwrap();
    let note = graph
        .book_note_save(
            &first.id,
            &Uuid::new_v4().to_string(),
            None,
            "My own reading note.",
            "",
            None,
            &first.source_sha256,
        )
        .unwrap();
    let first_bytes = fs::read(root.path().join(&first.file_path)).unwrap();
    let note_bytes = fs::read(root.path().join(&note.file_path)).unwrap();
    fs::remove_file(external.path().join("First.fb2")).unwrap();
    fs::remove_file(external.path().join("Second.fb2")).unwrap();
    graph.db.delete_blocks_for_page(&first.page_id).unwrap();
    graph
        .db
        .conn()
        .unwrap()
        .execute_batch("DELETE FROM fts_blocks; DELETE FROM fts_block_rowid;")
        .unwrap();

    graph.reindex_all().unwrap();

    for term in ["cobalt", "neodymium", "zirconium"] {
        assert!(!graph.db.search_fts(term, 10).unwrap().is_empty(), "{term}");
    }
    assert_eq!(
        graph.db.get_page_by_title(&first.title).unwrap().id,
        first.page_id
    );
    assert_eq!(
        graph.db.get_page_by_title(&second.title).unwrap().id,
        second.page_id
    );
    assert_eq!(
        graph.db.get_page_by_title(&converted.title).unwrap().id,
        converted.id
    );
    assert_eq!(
        fs::read(root.path().join(&first.file_path)).unwrap(),
        first_bytes
    );
    assert_eq!(
        fs::read(root.path().join(&note.file_path)).unwrap(),
        note_bytes
    );
    assert_eq!(graph.book_notes_list(&first.id).unwrap()[0].id, note.id);
    let pending = graph.db.list_pending_vector_cleanup(100).unwrap();
    assert!(pending.iter().any(|(id, _)| id == &first.page_id));
    assert!(pending.iter().any(|(id, _)| id == &second.page_id));
}

#[test]
fn explicit_reindex_preserves_favorites_reviews_ink_and_page_identity() {
    let root = tempfile::tempdir().unwrap();
    let graph = Graph::open(root.path()).unwrap();
    let page = graph
        .create_page_with_content(
            "Learning",
            false,
            "- Question :: Answer\n- Handwriting source\n- TODO Keep this task #study\n",
        )
        .unwrap();
    let blocks = graph.db.list_blocks_for_page(&page.id).unwrap();
    graph.db.add_favorite(&page.id).unwrap();
    let card = graph.db.list_flashcards(10, 0).unwrap().remove(0);
    graph.db.grade_flashcard(&card.id, 5).unwrap();
    let reviewed = graph.db.list_flashcards(10, 0).unwrap().remove(0);
    let ink = graph
        .db
        .register_ink_page(&blocks[1].id, "assets/handwriting.svg")
        .unwrap();
    graph
        .db
        .confirm_ink_recognition(&ink.id, "Irreplaceable recognized writing")
        .unwrap();
    let source_bytes = fs::read(graph.page_filesystem_path(&page.id).unwrap()).unwrap();
    graph.db.conn().unwrap().execute_batch(
        "DELETE FROM fts_blocks; DELETE FROM fts_block_rowid; DELETE FROM fts_ink; DELETE FROM links;",
    ).unwrap();

    graph.reindex_all().unwrap();

    assert_eq!(graph.db.get_page_by_title(&page.title).unwrap().id, page.id);
    assert_eq!(graph.db.list_favorites().unwrap()[0].id, page.id);
    let after = graph.db.list_flashcards(10, 0).unwrap().remove(0);
    assert_eq!(after.id, reviewed.id);
    assert_eq!(after.review_count, reviewed.review_count);
    assert_eq!(after.next_review_at, reviewed.next_review_at);
    assert_eq!(after.ease_factor, reviewed.ease_factor);
    assert_eq!(
        graph
            .db
            .get_ink_page(&ink.id)
            .unwrap()
            .unwrap()
            .recognized_text,
        "Irreplaceable recognized writing"
    );
    assert!(!graph
        .db
        .search_ink_fts("Irreplaceable", 10)
        .unwrap()
        .is_empty());
    assert!(!graph.db.get_links_from_page(&page.id).unwrap().is_empty());
    assert_eq!(
        fs::read(graph.page_filesystem_path(&page.id).unwrap()).unwrap(),
        source_bytes
    );
}

#[test]
fn failed_book_or_markdown_source_does_not_erase_other_books_on_reindex() {
    let root = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let graph = Graph::open(root.path()).unwrap();
    let damaged = import_book(&graph, external.path(), "Damaged", "Obsolete cobalt text.");
    let kept = import_book(&graph, external.path(), "Kept", "Retained neodymium text.");
    let page = graph
        .create_page_with_content("Keep me", false, "- Valuable standalone note.")
        .unwrap();
    graph.db.add_favorite(&page.id).unwrap();
    fs::write(
        root.path().join(format!("books/{}/book.json", damaged.id)),
        "{}",
    )
    .unwrap();
    graph
        .db
        .conn()
        .unwrap()
        .execute_batch("DELETE FROM fts_blocks; DELETE FROM fts_block_rowid;")
        .unwrap();

    assert!(graph.reindex_all().is_err());

    assert!(graph.db.search_fts("cobalt", 10).unwrap().is_empty());
    assert!(!graph.db.search_fts("neodymium", 10).unwrap().is_empty());
    assert_eq!(graph.db.get_page_by_title(&kept.title).unwrap().id, kept.id);
    assert_eq!(graph.db.list_favorites().unwrap()[0].id, page.id);
    assert!(root.path().join(damaged.file_path).is_file());
}

#[test]
fn reindex_retries_unavailable_extraction_but_never_restores_a_removed_graph_copy() {
    let root = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let graph = Graph::open(root.path()).unwrap();
    let info = import_book(
        &graph,
        external.path(),
        "Retry",
        "Restorable cobalt evidence.",
    );
    let page = graph.db.get_page_by_id(&info.id).unwrap();
    let mut properties = page.properties;
    properties["book-indexing-warning"] =
        "Text is not indexed: extractor previously unavailable".into();
    graph
        .db
        .update_page(&info.id, None, Some(&properties))
        .unwrap();
    graph.db.delete_blocks_for_page(&info.id).unwrap();
    graph.reindex_all().unwrap();
    assert_eq!(
        graph.book_open(&info.id).unwrap().indexing_warning,
        info.indexing_warning
    );
    assert!(!graph.db.search_fts("cobalt", 10).unwrap().is_empty());

    fs::remove_file(root.path().join(&info.file_path)).unwrap();
    graph.reindex_all().unwrap();
    assert!(graph.db.search_fts("cobalt", 10).unwrap().is_empty());
    assert!(!root.path().join(&info.file_path).exists());
    assert!(external.path().join("Retry.fb2").exists());
}

#[test]
fn a_fresh_database_rebuilds_original_books_and_companion_notes_from_graph_files() {
    let root = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let graph = Graph::open(root.path()).unwrap();
    let info = import_book(
        &graph,
        external.path(),
        "Portable",
        "Portable cobalt evidence.",
    );
    let note = graph
        .book_note_save(
            &info.id,
            &Uuid::new_v4().to_string(),
            None,
            "Portable personal annotation.",
            "",
            None,
            &info.source_sha256,
        )
        .unwrap();
    let rebuilt = Graph::open_with_db_path_and_metadata_dir(
        root.path(),
        &root.path().join(".grafium/fresh-index.db"),
        ".grafium",
    )
    .unwrap();
    rebuilt.reindex_all().unwrap();
    assert_eq!(rebuilt.book_open(&info.id).unwrap().id, info.id);
    assert!(!rebuilt.db.search_fts("cobalt", 10).unwrap().is_empty());
    let notes = rebuilt.book_notes_list(&info.id).unwrap();
    assert_eq!(notes[0].id, note.id);
    assert_eq!(notes[0].body, note.body);
}

#[test]
fn inserting_unlabelled_text_before_a_persisted_card_cannot_steal_its_identity() {
    let root = tempfile::tempdir().unwrap();
    let graph = Graph::open(root.path()).unwrap();
    let card_block_id = Uuid::new_v4().to_string();
    let page = graph
        .create_page_with_content(
            "Reviewed",
            false,
            &format!("- Question :: Answer\n  id:: {card_block_id}\n"),
        )
        .unwrap();
    let card = graph.db.list_flashcards(10, 0).unwrap().remove(0);
    graph.db.grade_flashcard(&card.id, 5).unwrap();
    let path = graph.page_filesystem_path(&page.id).unwrap();
    let content = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("- New paragraph without an ID\n{content}")).unwrap();
    graph.index_file(&path).unwrap();
    graph.reindex_all().unwrap();
    let after = graph.db.list_flashcards(10, 0).unwrap().remove(0);
    assert_eq!(after.id, card.id);
    assert_eq!(after.block_id, card_block_id);
    assert_eq!(after.review_count, 1);
    assert_eq!(
        graph.db.get_block_by_id(&card_block_id).unwrap().content,
        "Question :: Answer"
    );
}
