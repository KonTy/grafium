use pkm_core::models::BlockType;
use pkm_core::Graph;

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

    graph.move_block(&linked.id, Some(&container.id), 0).unwrap();

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
