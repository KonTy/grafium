use super::*;
use grafium_core::graph::Graph;

#[test]
fn sync_reconciliation_preserves_unrelated_favorites_and_reviews() {
    let local = tempfile::tempdir_in(".").unwrap();
    let remote = tempfile::tempdir_in(".").unwrap();
    let graph = Graph::open(local.path()).unwrap();
    let keep = graph
        .create_page_with_content("Keep", false, "- Capital of France :: Paris #flashcard\n")
        .unwrap();
    graph.db.add_favorite(&keep.id).unwrap();
    let card = graph.db.list_flashcards(10, 0).unwrap().remove(0);
    graph
        .db
        .update_flashcard_review(&card.id, 2.7, 5, 123456)
        .unwrap();
    let gone = graph.create_page("Gone", false).unwrap();
    let backend = FilesystemBackend::new(remote.path().to_path_buf(), "synthetic".into());
    let engine = SyncEngine::new(local.path().to_path_buf());
    assert!(engine.sync(&backend).unwrap().errors.is_empty());
    std::fs::remove_file(remote.path().join("pages/Gone.md")).unwrap();
    std::fs::write(remote.path().join("pages/Added.md"), "- Remote addition\n").unwrap();
    let result = engine.sync(&backend).unwrap();
    assert!(changes_local_sources(&result));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    reconcile_after_sync(&graph).unwrap();
    assert_eq!(graph.db.list_favorites().unwrap().len(), 1);
    let reviewed = graph.db.list_flashcards(10, 0).unwrap().remove(0);
    assert_eq!(reviewed.block_id, card.block_id);
    assert_eq!(reviewed.review_count, 1);
    assert_eq!(reviewed.next_review_at, Some(123456));
    assert!(graph.db.get_page_by_title("Added").is_ok());
    assert!(graph.db.get_page_by_id(&gone.id).is_err());
    assert!(graph.db.get_page_by_title("Gone.conflict_").is_err());
}

#[test]
fn completion_payload_never_hides_errors_or_deletions() {
    let mut result = SyncResult::failed("Reconcile failed");
    result.deleted_local.push("pages/deleted.md".into());
    result.deleted_remote.push("books/deleted.pdf".into());
    result
        .annotation_conflicts
        .push("books/book/original.jsonld".into());
    result.merged.push("books/book/original.jsonld".into());
    let payload = completion_payload("synthetic", &result);
    assert_eq!(payload["errors"], 1);
    assert_eq!(payload["deleted_local"], 1);
    assert_eq!(payload["deleted_remote"], 1);
    assert_eq!(payload["annotation_conflicts"], 1);
    assert_eq!(payload["merged"], 1);
}
