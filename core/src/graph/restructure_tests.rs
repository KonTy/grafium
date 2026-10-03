use super::*;

fn graph() -> (tempfile::TempDir, Graph) {
    let directory = tempfile::tempdir_in(".").unwrap();
    let graph = Graph::open(directory.path()).unwrap();
    (directory, graph)
}

fn outline(graph: &Graph, page: &Page) -> Vec<(String, usize)> {
    let blocks = graph.db.list_blocks_for_page(&page.id).unwrap();
    let parent: HashMap<_, _> = blocks
        .iter()
        .map(|block| (block.id.clone(), block.parent_id.clone()))
        .collect();
    blocks
        .iter()
        .map(|block| {
            let mut depth = 0;
            let mut ancestor = block.parent_id.clone();
            while let Some(id) = ancestor {
                depth += 1;
                ancestor = parent.get(&id).cloned().flatten();
            }
            (block.content.clone(), depth)
        })
        .collect()
}

fn id_of(graph: &Graph, page: &Page, content: &str) -> String {
    graph
        .db
        .list_blocks_for_page(&page.id)
        .unwrap()
        .into_iter()
        .find(|block| block.content == content)
        .unwrap()
        .id
}

fn moved(id: String, parent: Option<String>, order_index: i32) -> BlockMove {
    BlockMove { id, new_parent_id: parent, order_index }
}

fn file(graph: &Graph, page: &Page) -> String {
    fs::read_to_string(graph.root_dir.join(page.file_path.as_ref().unwrap())).unwrap()
}

fn fixture() -> (tempfile::TempDir, Graph, Page) {
    let (directory, graph) = graph();
    let page = graph
        .create_page_with_content(
            "Outline",
            false,
            "- A\n- Parent\n  - Child 1\n    - Grandchild\n  - Child 2\n- B\n",
        )
        .unwrap();
    (directory, graph, page)
}

#[test]
fn deleting_a_parent_keeps_its_children_in_its_place() -> Result<()> {
    let (_directory, graph, page) = fixture();
    let parent = id_of(&graph, &page, "Parent");
    let deleted = graph.restructure_blocks(
        &page.id,
        &[
            moved(id_of(&graph, &page, "Child 1"), None, 1),
            moved(id_of(&graph, &page, "Child 2"), None, 2),
            moved(id_of(&graph, &page, "B"), None, 3),
        ],
        &[parent.clone()],
    )?;

    assert_eq!(deleted.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(), [parent.as_str()]);
    assert_eq!(
        outline(&graph, &page),
        [("A", 0), ("Child 1", 0), ("Grandchild", 1), ("Child 2", 0), ("B", 0)]
            .map(|(content, depth)| (content.to_string(), depth))
    );
    let saved = file(&graph, &page);
    assert!(!saved.contains("Parent"), "{saved}");
    let order: Vec<_> = ["- A", "- Child 1", "  - Grandchild", "- Child 2", "- B"]
        .iter()
        .map(|line| saved.find(&format!("{line}\n")).unwrap_or_else(|| panic!("{line} in {saved}")))
        .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{saved}");
    Ok(())
}

#[test]
fn undo_recreates_the_parent_and_moves_children_back() -> Result<()> {
    let (_directory, graph, page) = fixture();
    let before_file = file(&graph, &page);
    let before_outline = outline(&graph, &page);
    let blocks = graph.db.list_blocks_for_page(&page.id)?;
    let original = |content: &str| blocks.iter().find(|b| b.content == content).unwrap().clone();
    let parent = original("Parent");
    let promoted = ["Child 1", "Child 2", "B"].map(original);
    graph.restructure_blocks(
        &page.id,
        &promoted
            .iter()
            .enumerate()
            .map(|(index, block)| moved(block.id.clone(), None, index as i32 + 1))
            .collect::<Vec<_>>(),
        &[parent.id.clone()],
    )?;

    graph.create_blocks(
        &page.id,
        vec![BlockCreateSpec {
            id: Some(parent.id.clone()),
            parent: BlockCreateParent::Root,
            order_index: parent.order_index,
            content: parent.content.clone(),
            block_type: parent.block_type.clone(),
            properties: parent.properties.clone(),
        }],
    )?;
    graph.restructure_blocks(
        &page.id,
        &promoted
            .iter()
            .map(|block| moved(block.id.clone(), block.parent_id.clone(), block.order_index))
            .collect::<Vec<_>>(),
        &[],
    )?;

    assert_eq!(outline(&graph, &page), before_outline);
    // Any structural write records block ids in the file; the text and the
    // outline must otherwise be exactly what they were.
    let without_ids = |text: String| {
        text.lines()
            .filter(|line| !line.trim_start().starts_with("id:: "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(without_ids(file(&graph, &page)), without_ids(before_file));
    assert_eq!(id_of(&graph, &page, "Parent"), parent.id, "undo restores the same block identity");
    Ok(())
}

#[test]
fn unsafe_batches_change_nothing() -> Result<()> {
    let (_directory, graph, page) = fixture();
    let other = graph.create_page_with_content("Other", false, "- Elsewhere\n")?;
    let before_file = file(&graph, &page);
    let before_outline = outline(&graph, &page);
    let parent = id_of(&graph, &page, "Parent");
    let child = id_of(&graph, &page, "Child 1");
    let grandchild = id_of(&graph, &page, "Grandchild");

    let rejected = [
        // Children left under a deleted parent would silently vanish.
        graph.restructure_blocks(&page.id, &[], &[parent.clone()]),
        // A block cannot end up inside its own subtree.
        graph.restructure_blocks(&page.id, &[moved(parent.clone(), Some(grandchild), 0)], &[]),
        graph.restructure_blocks(&page.id, &[moved(child.clone(), Some(parent.clone()), 0)], &[child.clone()]),
        graph.restructure_blocks(
            &page.id,
            &[moved(child.clone(), None, 0), moved(child.clone(), None, 1)],
            &[],
        ),
        graph.restructure_blocks(
            &page.id,
            &[moved(id_of(&graph, &other, "Elsewhere"), None, 9)],
            &[],
        ),
        graph.restructure_blocks(
            &page.id,
            &[moved(child, Some(id_of(&graph, &other, "Elsewhere")), 0)],
            &[],
        ),
    ];
    for result in rejected {
        assert!(result.is_err(), "unsafe batch was accepted: {result:?}");
    }
    assert_eq!(outline(&graph, &page), before_outline);
    assert_eq!(file(&graph, &page), before_file);
    assert_eq!(outline(&graph, &other), [("Elsewhere".to_string(), 0)]);
    Ok(())
}

#[test]
fn indent_and_outdent_rewrite_the_page_once_in_tree_order() -> Result<()> {
    let (_directory, graph, page) = fixture();
    let parent = id_of(&graph, &page, "Parent");
    // Outdent "Child 1" past its parent (logical outdent): it lands after the
    // parent's remaining children, and the later root sibling is renumbered.
    graph.restructure_blocks(
        &page.id,
        &[
            moved(id_of(&graph, &page, "Child 1"), None, 2),
            moved(id_of(&graph, &page, "B"), None, 3),
        ],
        &[],
    )?;
    assert_eq!(
        outline(&graph, &page),
        [("A", 0), ("Parent", 0), ("Child 2", 1), ("Child 1", 0), ("Grandchild", 1), ("B", 0)]
            .map(|(content, depth)| (content.to_string(), depth))
    );
    graph.restructure_blocks(
        &page.id,
        &[moved(id_of(&graph, &page, "B"), Some(parent), 2)],
        &[],
    )?;
    assert_eq!(
        outline(&graph, &page),
        [("A", 0), ("Parent", 0), ("Child 2", 1), ("B", 1), ("Child 1", 0), ("Grandchild", 1)]
            .map(|(content, depth)| (content.to_string(), depth))
    );
    Ok(())
}

#[test]
fn inserted_blocks_are_saved_where_the_editor_shows_them() -> Result<()> {
    let (_directory, graph, page) = fixture();
    let parent = id_of(&graph, &page, "Parent");
    let insert = |parent: Option<&str>, position: usize, content: &str| {
        graph.insert_block_at(&page.id, parent, position, content, BlockType::Text, serde_json::json!({}))
    };
    // A new sibling right after "Child 1".
    insert(Some(&parent), 1, "Typed between")?;
    // Enter at the end of "Parent", which shows children: its new first child.
    insert(Some(&parent), 0, "New first child")?;
    // Enter at the very start of "A": an empty sibling above it.
    insert(None, 0, "")?;

    let expected = [
        ("", 0), ("A", 0), ("Parent", 0), ("New first child", 1), ("Child 1", 1), ("Grandchild", 2),
        ("Typed between", 1), ("Child 2", 1), ("B", 0),
    ]
    .map(|(content, depth)| (content.to_string(), depth));
    assert_eq!(outline(&graph, &page), expected);
    // A newer block used to lose a tie on order to its older next sibling,
    // so it moved down one place once the page was loaded again.
    let reopened = Graph::open(&graph.root_dir)?;
    assert_eq!(outline(&reopened, &page), expected);
    assert_dense_sibling_order(&graph, &page);
    Ok(())
}

#[test]
fn inserting_respects_siblings_that_already_share_an_order_number() -> Result<()> {
    let (_directory, graph) = graph();
    let page = graph.create_page_with_content("Ties", false, "- X\n- Y\n")?;
    std::thread::sleep(std::time::Duration::from_millis(5));
    // Older versions saved a new block with its next sibling's order number;
    // creation time then decides, so "N" is shown after "Y".
    graph.create_block(&page.id, None, 1, "N", BlockType::Text, serde_json::json!({}))?;
    let contents = |graph: &Graph| -> Vec<String> {
        outline(graph, &page).into_iter().map(|(content, _)| content).collect()
    };
    assert_eq!(contents(&graph), ["X", "Y", "N"]);

    // Enter at the end of "Y": right after it, not after "N".
    graph.insert_block_at(&page.id, None, 2, "After Y", BlockType::Text, serde_json::json!({}))?;
    assert_eq!(contents(&graph), ["X", "Y", "After Y", "N"]);
    // Enter at the start of "N": right above it, not above "Y".
    graph.insert_block_at(&page.id, None, 3, "Above N", BlockType::Text, serde_json::json!({}))?;
    assert_eq!(contents(&graph), ["X", "Y", "After Y", "Above N", "N"]);
    assert_dense_sibling_order(&graph, &page);
    Ok(())
}

fn assert_dense_sibling_order(graph: &Graph, page: &Page) {
    let blocks = graph.db.list_blocks_for_page(&page.id).unwrap();
    for parent in blocks.iter().map(|block| &block.parent_id).collect::<HashSet<_>>() {
        let mut orders: Vec<_> = blocks.iter().filter(|b| &b.parent_id == parent).map(|b| b.order_index).collect();
        let count = orders.len();
        orders.dedup();
        assert_eq!(orders.len(), count, "no two siblings share an order number");
    }
}
