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

/// A page whose own blocks all link back to it must not capture the graph view.
///
/// Regression: seeding ranked hubs by raw inbound link count, so a tag-like
/// page carrying its own backlink in every block scored thousands while having
/// no neighbours at all. The traversal seeded there, found nothing to expand
/// to, and rendered one isolated node for a graph with 141 pages and 134 real
/// edges — a failure that looked like an empty database from the UI.
#[test]
fn graph_data_is_not_captured_by_a_self_linking_hub() {
    let temp = tempfile::tempdir().unwrap();
    let graph = Graph::open(temp.path()).unwrap();

    // A "tag" page with many blocks that each link to the tag page itself.
    let tag = graph.create_page("Tag", false).unwrap();
    for i in 0..50 {
        graph
            .create_block(
                &tag.id,
                None,
                i,
                "[[Tag]]",
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
    }

    // A small genuinely-connected cluster elsewhere: alpha → beta → gamma.
    let alpha = graph.create_page("Alpha", false).unwrap();
    graph
        .create_block(
            &alpha.id,
            None,
            0,
            "[[Beta]]",
            BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();
    let beta = graph.db.get_page_by_title_ci("BETA").unwrap();
    graph
        .create_block(
            &beta.id,
            None,
            0,
            "[[Gamma]]",
            BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    let (nodes, edges) = graph.db.graph_data(None, 200).unwrap();

    assert!(
        nodes.len() >= 3,
        "expected the real cluster to be reachable, got {} node(s)",
        nodes.len()
    );
    assert!(
        !edges.is_empty(),
        "a graph with real links must render edges"
    );
}

/// Notes cluster by topic, so a knowledge graph is rarely one component.
/// Exploring only the seed's component left most of the graph invisible.
#[test]
fn graph_data_spans_disconnected_clusters() {
    let temp = tempfile::tempdir().unwrap();
    let graph = Graph::open(temp.path()).unwrap();

    for (from, to) in [("A1", "A2"), ("B1", "B2"), ("C1", "C2")] {
        let page = graph.create_page(from, false).unwrap();
        graph
            .create_block(
                &page.id,
                None,
                0,
                &format!("[[{to}]]"),
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
    }

    let (nodes, _edges) = graph.db.graph_data(None, 200).unwrap();
    assert!(
        nodes.len() >= 6,
        "all three disconnected pairs should be represented, got {}",
        nodes.len()
    );
}

/// A collection marking must survive the markdown round trip.
///
/// Regression: the marker was stored as a nested JSON object, but the
/// serializer only emits string properties and indexing a file *replaces* a
/// page's properties with whatever the parser read back. So the mark was
/// written to the database and then silently erased by the next reindex,
/// file-watcher event or sync pull — `pages_list_collections` returned an
/// empty list with no error, and the marking never travelled between devices.
#[test]
fn collection_marking_survives_a_disk_round_trip() {
    use grafium_core::knowledge::collections::{collection_of, mark_collection};

    let temp = tempfile::tempdir().unwrap();
    let graph = Graph::open(temp.path()).unwrap();
    let page = graph.create_page("My Books/Dune", false).unwrap();

    // Give the page a block so it has real content alongside the marker, and
    // an unrelated property that must not be collateral damage.
    graph
        .create_block(
            &page.id,
            None,
            0,
            "[[Chapter 1]]",
            BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    let mut props = serde_json::json!({ "icon": "book" });
    mark_collection(&mut props, "book");
    graph.update_page_properties(&page.id, props).unwrap();

    let stored = graph.db.get_page_by_id(&page.id).unwrap();
    assert_eq!(collection_of(&stored).map(|c| c.kind), Some("book".into()));

    // Re-index the file from disk, which is what a watcher event or sync pull
    // does, and is where the marker used to disappear.
    // `file_path` is stored relative to the graph root.
    let file_path = temp
        .path()
        .join(stored.file_path.clone().expect("page should have a file"));
    assert!(
        std::fs::read_to_string(&file_path)
            .expect("page file should exist on disk")
            .contains("collection:: book"),
        "the marker must actually be written into the markdown"
    );
    graph.index_file(&file_path).unwrap();

    let reindexed = graph.db.get_page_by_id(&page.id).unwrap();
    assert_eq!(
        collection_of(&reindexed).map(|c| c.kind),
        Some("book".into()),
        "collection marking must survive reindexing from disk"
    );
    assert_eq!(
        reindexed.properties["icon"],
        serde_json::json!("book"),
        "unrelated properties must survive too"
    );
}
