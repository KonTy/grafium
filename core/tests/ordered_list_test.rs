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
