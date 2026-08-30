use grafium_core::models::BlockType;
use grafium_core::Graph;

#[test]
fn create_block_indexes_links_immediately() {
    let temp = tempfile::tempdir().unwrap();
    let graph = Graph::open(temp.path()).unwrap();

    let source_page = graph.create_page("2026-05-01", true).unwrap();
    let created = graph
        .create_block(
            &source_page.id,
            None,
            0,
            "[[test]]",
            BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    let target_page = graph.db.get_page_by_title_ci("TEST").unwrap();
    let backlinks = graph.db.get_backlinks(&target_page.id).unwrap();

    assert!(
        backlinks.iter().any(|(_, b)| b.id == created.id),
        "newly created block with [[test]] should appear in backlinks immediately"
    );
}

#[test]
fn move_block_preserves_content_and_backlinks() {
    let temp = tempfile::tempdir().unwrap();
    let graph = Graph::open(temp.path()).unwrap();

    let page = graph.create_page("2026-05-01", true).unwrap();

    let container = graph
        .create_block(
            &page.id,
            None,
            0,
            "container",
            BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    let linked = graph
        .create_block(
            &page.id,
            None,
            1,
            "[[test]]",
            BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    graph
        .move_block(&linked.id, Some(&container.id), 0)
        .unwrap();

    let moved = graph.db.get_block_by_id(&linked.id).unwrap();
    assert_eq!(moved.content, "[[test]]");
    assert_eq!(moved.parent_id.as_deref(), Some(container.id.as_str()));

    let target_page = graph.db.get_page_by_title_ci("test").unwrap();
    let backlinks = graph.db.get_backlinks(&target_page.id).unwrap();

    assert!(
        backlinks.iter().any(|(_, b)| b.id == linked.id),
        "backlink should still resolve after moving linked block"
    );
}

#[test]
fn deindex_file_removes_a_note_deleted_outside_the_app() {
    let temp = tempfile::tempdir().unwrap();
    let graph = Graph::open(temp.path()).unwrap();

    // A note that another machine will later delete, and one that links to it.
    let doomed = temp.path().join("pages").join("Doomed.md");
    std::fs::create_dir_all(doomed.parent().unwrap()).unwrap();
    std::fs::write(&doomed, "- doomed note about elephants\n").unwrap();
    graph.index_file(&doomed).unwrap();

    assert!(
        !graph.db.search_fts("elephants", 10).unwrap().is_empty(),
        "note should be searchable once indexed"
    );

    // A sync removes the file from disk; the watcher sees a path that no
    // longer exists.
    std::fs::remove_file(&doomed).unwrap();
    assert!(graph.deindex_file(&doomed).unwrap());

    assert!(
        graph.db.search_fts("elephants", 10).unwrap().is_empty(),
        "deleted note is still in the search index"
    );
    assert!(
        graph.db.find_page_by_file_path("pages/Doomed.md").unwrap().is_none(),
        "deleted note still has a page row"
    );

    // Re-running on an unknown path is a no-op rather than an error.
    assert!(!graph.deindex_file(&doomed).unwrap());
}
