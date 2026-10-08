use grafium_core::{models::BlockType, Graph};

#[test]
fn pasted_fenced_code_round_trips_through_real_graph_storage() {
    for fence in ["```", "~~~~", "````"] {
        let temp = tempfile::tempdir().unwrap();
        let graph = Graph::open(temp.path()).unwrap();
        let page = graph
            .create_page_with_content("Fenced paste", false, "")
            .unwrap();
        let heading = graph
            .create_block(
                &page.id,
                None,
                0,
                "Topic",
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
        let content = format!(
            "Heading\n{fence}text\n  code with trailing spaces  \n\n\t- literal bullet\n-\nid:: literal property\n{fence} not a closer\n{fence}\nAfter fence"
        );
        let code = graph
            .create_block(
                &page.id,
                Some(&heading.id),
                0,
                &content,
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
        let sibling = graph
            .create_block(
                &page.id,
                Some(&heading.id),
                1,
                "Unrelated note",
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
        let source = graph.get_page_source(&page.id).unwrap();
        assert!(source.contains("    id:: literal property"));
        graph.update_page_source(&page.id, &source).unwrap();
        let blocks = graph.db.list_blocks_for_page(&page.id).unwrap();
        assert_eq!(blocks.len(), 3);
        let stored = graph.db.get_block_by_id(&code.id).unwrap();
        assert_eq!(stored.content, content);
        assert_eq!(stored.parent_id, Some(heading.id.clone()));
        assert_eq!(stored.order_index, 0);
        assert_eq!(
            graph.db.get_block_by_id(&sibling.id).unwrap().content,
            "Unrelated note"
        );
        assert_eq!(graph.get_page_source(&page.id).unwrap(), source);
        drop(graph);
        let reopened = Graph::open(temp.path()).unwrap();
        assert_eq!(
            reopened.db.get_block_by_id(&code.id).unwrap().content,
            content
        );
    }
}

#[test]
fn unfinished_fences_never_merge_unrelated_identified_graph_blocks() {
    let temp = tempfile::tempdir().unwrap();
    let graph = Graph::open(temp.path()).unwrap();
    let page = graph
        .create_page_with_content("Separate outline blocks", false, "")
        .unwrap();
    let mut ids = Vec::new();
    for (order, content) in ["```", "Unrelated note", "```"].into_iter().enumerate() {
        ids.push(
            graph
                .create_block(
                    &page.id,
                    None,
                    order as i32,
                    content,
                    BlockType::Text,
                    serde_json::json!({}),
                )
                .unwrap()
                .id,
        );
    }
    let source = graph.get_page_source(&page.id).unwrap();
    graph.update_page_source(&page.id, &source).unwrap();
    assert_eq!(graph.db.list_blocks_for_page(&page.id).unwrap().len(), 3);
    for (id, content) in ids.iter().zip(["```", "Unrelated note", "```"]) {
        assert_eq!(graph.db.get_block_by_id(id).unwrap().content, content);
    }
}
