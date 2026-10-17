use grafium_core::graph::BlockMove;
use grafium_core::{models::BlockType, Graph};
use serde_json::json;

#[test]
fn numbered_siblings_and_empty_item_exit_survive_storage_and_reindexing() {
    for delimiter in [".", ")"] {
        let temp = tempfile::tempdir().unwrap();
        let graph = Graph::open(temp.path()).unwrap();
        let page = graph
            .create_page_with_content("2026-10-16", true, "")
            .unwrap();
        let heading = graph
            .insert_block_at(&page.id, None, 0, "Steps", BlockType::Text, json!({}))
            .unwrap();
        let first = graph
            .insert_block_at(
                &page.id,
                Some(&heading.id),
                0,
                &format!("7{delimiter} First"),
                BlockType::Text,
                json!({}),
            )
            .unwrap();
        let second = graph
            .insert_block_at(
                &page.id,
                Some(&heading.id),
                1,
                &format!("8{delimiter} "),
                BlockType::Text,
                json!({}),
            )
            .unwrap();
        graph
            .update_block(&second.id, &format!("8{delimiter} Second"), None)
            .unwrap();
        let exited = graph
            .insert_block_at(
                &page.id,
                Some(&heading.id),
                2,
                &format!("9{delimiter} "),
                BlockType::Text,
                json!({}),
            )
            .unwrap();
        graph.update_block(&exited.id, "", None).unwrap();
        graph
            .update_block(&exited.id, "Ordinary writing", None)
            .unwrap();
        let source = graph.get_page_source(&page.id).unwrap();
        graph.update_page_source(&page.id, &source).unwrap();
        assert_eq!(graph.get_page_source(&page.id).unwrap(), source);
        drop(graph);

        let graph = Graph::open(temp.path()).unwrap();
        assert_eq!(graph.db.list_blocks_for_page(&page.id).unwrap().len(), 4);
        for (position, (id, content)) in [
            (&first.id, format!("7{delimiter} First")),
            (&second.id, format!("8{delimiter} Second")),
            (&exited.id, "Ordinary writing".to_owned()),
        ]
        .into_iter()
        .enumerate()
        {
            let block = graph.db.get_block_by_id(id).unwrap();
            assert_eq!(block.parent_id.as_ref(), Some(&heading.id));
            assert_eq!(block.order_index, position as i32);
            assert_eq!(block.content, content);
        }
    }
}

#[test]
fn hierarchical_numbered_branch_moves_keep_ids_text_and_parents_after_reopen() {
    for delimiter in [".", ")"] {
        let temp = tempfile::tempdir().unwrap();
        let graph = Graph::open(temp.path()).unwrap();
        let page = graph
            .create_page_with_content("Nested steps", false, "")
            .unwrap();
        let insert = |parent: Option<&str>, position, text: &str| {
            graph
                .insert_block_at(&page.id, parent, position, text, BlockType::Text, json!({}))
                .unwrap()
        };
        let first = insert(None, 0, &format!("1{delimiter} First"));
        let second = insert(None, 1, &format!("2{delimiter} Second"));
        let third = insert(None, 2, &format!("3{delimiter} Keep text\n  and spacing\n"));
        let child = insert(Some(&third.id), 0, &format!("1{delimiter} Existing child"));
        graph
            .restructure_blocks(
                &page.id,
                &[BlockMove {
                    id: third.id.clone(),
                    new_parent_id: Some(second.id.clone()),
                    order_index: 0,
                }],
                &[],
            )
            .unwrap();
        graph
            .update_block(
                &third.id,
                &format!("2.1{delimiter} Keep text\n  and spacing\n"),
                None,
            )
            .unwrap();
        graph
            .update_block(&child.id, &format!("2.1.1{delimiter} Existing child"), None)
            .unwrap();
        let continued = insert(Some(&second.id), 1, &format!("2.2{delimiter} Continued"));
        let exited = insert(Some(&second.id), 2, &format!("2.3{delimiter} "));
        graph.update_block(&exited.id, "", None).unwrap();
        graph
            .restructure_blocks(
                &page.id,
                &[BlockMove {
                    id: continued.id.clone(),
                    new_parent_id: None,
                    order_index: 2,
                }],
                &[],
            )
            .unwrap();
        graph
            .update_block(&continued.id, &format!("3{delimiter} Continued"), None)
            .unwrap();
        let source = graph.get_page_source(&page.id).unwrap();
        graph.update_page_source(&page.id, &source).unwrap();
        assert_eq!(graph.get_page_source(&page.id).unwrap(), source);
        drop(graph);

        let graph = Graph::open(temp.path()).unwrap();
        assert_eq!(graph.db.list_blocks_for_page(&page.id).unwrap().len(), 6);
        for (id, parent, content) in [
            (&first.id, None, format!("1{delimiter} First")),
            (&second.id, None, format!("2{delimiter} Second")),
            (
                &third.id,
                Some(second.id.as_str()),
                format!("2.1{delimiter} Keep text\n  and spacing\n"),
            ),
            (
                &child.id,
                Some(third.id.as_str()),
                format!("2.1.1{delimiter} Existing child"),
            ),
            (&continued.id, None, format!("3{delimiter} Continued")),
            (&exited.id, Some(second.id.as_str()), String::new()),
        ] {
            let stored = graph.db.get_block_by_id(id).unwrap();
            assert_eq!(stored.parent_id.as_deref(), parent);
            assert_eq!(stored.content, content);
        }
    }
}

#[test]
fn empty_number_exit_and_child_preserving_deletion_survive_journal_reopen() {
    for delimiter in [".", ")"] {
        let temp = tempfile::tempdir().unwrap();
        let graph = Graph::open(temp.path()).unwrap();
        let page = graph
            .create_page_with_content("2026-10-17", true, "")
            .unwrap();
        let insert = |parent: Option<&str>, position, text: &str| {
            graph
                .insert_block_at(&page.id, parent, position, text, BlockType::Text, json!({}))
                .unwrap()
        };
        let heading = insert(None, 0, "Steps");
        let previous = insert(Some(&heading.id), 0, &format!("2{delimiter} Previous"));
        let empty = insert(Some(&heading.id), 1, &format!("2.1{delimiter}\u{a0}"));
        let child = insert(Some(&empty.id), 0, "Written child");
        let grandchild = insert(Some(&child.id), 0, "  Keep whitespace\nand lines\n");
        let exited = insert(Some(&heading.id), 2, &format!("3{delimiter} "));
        let last = insert(Some(&heading.id), 3, "");

        graph.update_block(&empty.id, "", None).unwrap();
        graph
            .restructure_blocks(
                &page.id,
                &[BlockMove {
                    id: child.id.clone(),
                    new_parent_id: Some(heading.id.clone()),
                    order_index: 1,
                }],
                std::slice::from_ref(&empty.id),
            )
            .unwrap();
        graph
            .update_block(&previous.id, &format!("2{delimiter} Previou"), None)
            .unwrap();
        graph.update_block(&exited.id, "", None).unwrap();
        graph.update_block(&exited.id, "Plain writing", None).unwrap();
        let source = graph.get_page_source(&page.id).unwrap();
        graph.update_page_source(&page.id, &source).unwrap();
        assert_eq!(graph.get_page_source(&page.id).unwrap(), source);
        drop(graph);

        let graph = Graph::open(temp.path()).unwrap();
        assert_eq!(graph.db.list_blocks_for_page(&page.id).unwrap().len(), 6);
        assert!(graph.db.get_block_by_id(&empty.id).is_err());
        for (id, parent, content) in [
            (&previous.id, &heading.id, format!("2{delimiter} Previou")),
            (&child.id, &heading.id, "Written child".to_owned()),
            (
                &grandchild.id,
                &child.id,
                "  Keep whitespace\nand lines\n".to_owned(),
            ),
            (&exited.id, &heading.id, "Plain writing".to_owned()),
            (&last.id, &heading.id, String::new()),
        ] {
            let stored = graph.db.get_block_by_id(id).unwrap();
            assert_eq!(stored.parent_id.as_ref(), Some(parent));
            assert_eq!(stored.content, content);
        }
    }
}
