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

#[test]
fn inserting_and_editing_a_following_journal_block_keeps_saved_code_intact() {
    for fence in ["```", "~~~~", "````"] {
        let temp = tempfile::tempdir().unwrap();
        let graph = Graph::open(temp.path()).unwrap();
        let page = graph
            .create_page_with_content("2026-10-08", true, "")
            .unwrap();
        let time = graph
            .insert_block_at(
                &page.id,
                None,
                0,
                "11:53",
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
        let heading = graph
            .insert_block_at(
                &page.id,
                Some(&time.id),
                0,
                "Figuring out comfyui for exercise videos exmples",
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
        let code = graph
            .insert_block_at(
                &page.id,
                Some(&heading.id),
                0,
                "",
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
        let content = format!(
                "{fence}\n  can we crate examples foder in workflows,  \n\n\t- literal bullet\nid:: literal property\n \n{fence}"
            );
        graph
            .update_block(&code.id, &format!("{fence}\n\n{fence}"), None)
            .unwrap();
        graph.update_block(&code.id, &content, None).unwrap();
        let before = graph.db.get_block_by_id(&code.id).unwrap();

        // The first Enter may be saved while the caret is on the exit line;
        // the second turns only that final newline into a block boundary.
        graph
            .update_block(&code.id, &format!("{content}\n"), None)
            .unwrap();
        graph.update_block(&code.id, &content, None).unwrap();
        let following = graph
            .insert_block_at(
                &page.id,
                Some(&heading.id),
                1,
                "",
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
        graph
            .update_block(&following.id, "Continue writing below the code", None)
            .unwrap();
        let source = graph.get_page_source(&page.id).unwrap();
        graph.update_page_source(&page.id, &source).unwrap();
        let stored = graph.db.get_block_by_id(&code.id).unwrap();
        assert_eq!(stored.content, before.content);
        assert_eq!(stored.parent_id, before.parent_id);
        assert_eq!(stored.order_index, before.order_index);
        assert_eq!(stored.properties, before.properties);
        assert_eq!(graph.db.list_blocks_for_page(&page.id).unwrap().len(), 4);
        assert_eq!(
            graph.db.get_block_by_id(&following.id).unwrap().parent_id,
            Some(heading.id.clone())
        );
        assert_eq!(graph.get_page_source(&page.id).unwrap(), source);
        drop(graph);
        let reopened = Graph::open(temp.path()).unwrap();
        assert_eq!(
            reopened.db.get_block_by_id(&code.id).unwrap().content,
            content
        );
        assert_eq!(
            reopened.db.get_block_by_id(&following.id).unwrap().content,
            "Continue writing below the code"
        );
    }
}
