use grafium_core::Graph;
use tempfile::TempDir;

// ─── Helper ───────────────────────────────────────────────────────────────────

fn open_graph() -> (TempDir, Graph) {
    let tmp = TempDir::new().expect("Failed to create temp dir");
    let graph = Graph::open(tmp.path()).expect("Failed to open graph");
    (tmp, graph)
}

// ─── Parent / child creation via link ────────────────────────────────────────

#[test]
fn test_hierarchy_parent_child_creation() {
    let (_tmp, graph) = open_graph();

    let root = graph.create_page("root", false).unwrap();
    graph
        .create_block(
            &root.id,
            None,
            0,
            "Check [[test/page]]",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    let all = graph.db.list_pages(100, 0).unwrap();
    let titles: Vec<&str> = all.iter().map(|p| p.title.as_str()).collect();

    assert!(
        titles.contains(&"test"),
        "parent 'test' must be auto-created"
    );
    assert!(
        titles.contains(&"test/page"),
        "child 'test/page' must be auto-created"
    );

    let parent = graph.db.get_parent_page("test/page").unwrap();
    assert!(
        parent.is_some(),
        "get_parent_page must return Some for test/page"
    );
    assert_eq!(parent.unwrap().title.to_lowercase(), "test");
}

#[test]
fn test_hierarchy_parent_is_none_for_top_level() {
    let (_tmp, graph) = open_graph();
    let parent = graph.db.get_parent_page("toplevel").unwrap();
    assert!(parent.is_none(), "top-level page must have no parent");
}

// ─── Children lookup ──────────────────────────────────────────────────────────

#[test]
fn test_hierarchy_children_lookup() {
    let (_tmp, graph) = open_graph();

    graph.create_page("project/web/frontend", false).unwrap();
    graph.create_page("project/web/backend", false).unwrap();
    graph.create_page("project/mobile", false).unwrap();

    let mut web_children = graph.db.get_child_pages("project/web").unwrap();
    web_children.sort_by(|a, b| a.title.cmp(&b.title));
    let web_titles: Vec<&str> = web_children.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(
        web_titles,
        vec!["project/web/backend", "project/web/frontend"],
        "unexpected children of project/web: {:?}",
        web_titles
    );

    let proj_children = graph.db.get_child_pages("project").unwrap();
    let proj_titles: Vec<&str> = proj_children.iter().map(|p| p.title.as_str()).collect();
    assert!(
        proj_titles.contains(&"project/mobile"),
        "project/mobile must appear as child of project; got {:?}",
        proj_titles
    );
}

#[test]
fn test_hierarchy_leaf_has_no_children() {
    let (_tmp, graph) = open_graph();
    graph.create_page("leaf", false).unwrap();
    let children = graph.db.get_child_pages("leaf").unwrap();
    assert!(children.is_empty(), "leaf page must have no children");
}

// ─── Backslash normalisation ──────────────────────────────────────────────────

#[test]
fn test_hierarchy_backslash_normalization() {
    let (_tmp, graph) = open_graph();

    let page = graph.create_page("notes", false).unwrap();
    graph
        .create_block(
            &page.id,
            None,
            0,
            "Check out [[test\\page]] for more info",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    // Link must be indexed against the forward-slash normalised page
    let links = graph.db.get_links_from_page(&page.id).unwrap();
    assert_eq!(links.len(), 1, "expected exactly one link");
    let target = graph.db.get_page_by_title_ci("test/page").unwrap();
    assert_eq!(
        links[0].to_page_id, target.id,
        "link must point to test/page (not test\\page)"
    );
}

#[test]
fn test_hierarchy_mixed_slash_normalisation() {
    let (_tmp, graph) = open_graph();

    let page = graph.create_page("notes", false).unwrap();
    graph
        .create_block(
            &page.id,
            None,
            0,
            "[[a\\b/c\\d]]",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    // All ancestor pages must exist with forward slashes
    graph
        .db
        .get_page_by_title_ci("a")
        .expect("ancestor 'a' must exist");
    graph
        .db
        .get_page_by_title_ci("a/b")
        .expect("ancestor 'a/b' must exist");
    graph
        .db
        .get_page_by_title_ci("a/b/c")
        .expect("ancestor 'a/b/c' must exist");
    graph
        .db
        .get_page_by_title_ci("a/b/c/d")
        .expect("leaf 'a/b/c/d' must exist");
}

// ─── Deep auto-create chain ───────────────────────────────────────────────────

#[test]
fn test_hierarchy_auto_create_chain() {
    let (_tmp, graph) = open_graph();

    let page = graph.create_page("root", false).unwrap();
    graph
        .create_block(
            &page.id,
            None,
            0,
            "See [[a/b/c/d]] for details",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    for title in &["a", "a/b", "a/b/c", "a/b/c/d"] {
        let p = graph.db.get_page_by_title_ci(title);
        assert!(
            p.is_ok() && !p.unwrap().id.is_empty(),
            "ancestor '{}' must have been auto-created",
            title
        );
    }
}

#[test]
fn test_hierarchy_duplicate_link_does_not_create_duplicate_pages() {
    let (_tmp, graph) = open_graph();

    let page = graph.create_page("root", false).unwrap();
    graph
        .create_block(
            &page.id,
            None,
            0,
            "[[x/y]]",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();
    graph
        .create_block(
            &page.id,
            None,
            1,
            "again [[x/y]]",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    let all = graph.db.list_pages(100, 0).unwrap();
    let xy_count = all
        .iter()
        .filter(|p| p.title.to_lowercase() == "x/y")
        .count();
    assert_eq!(xy_count, 1, "x/y must appear exactly once, not duplicated");
}

// ─── create_page directly also creates parents ───────────────────────────────

#[test]
fn test_hierarchy_create_page_via_api_creates_parents() {
    // When the user navigates to [[test/page]] which calls get_or_create_page,
    // the parent should already exist (auto-created from the link).
    // This test verifies the DB query logic is correct end-to-end.
    let (_tmp, graph) = open_graph();

    let page = graph.create_page("root", false).unwrap();
    // Referencing multi-level path via block triggers ensure_parent_hierarchy
    graph
        .create_block(
            &page.id,
            None,
            0,
            "[[parent/child/grandchild]]",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    let parent = graph.db.get_parent_page("parent/child/grandchild").unwrap();
    assert!(parent.is_some());
    assert_eq!(parent.unwrap().title.to_lowercase(), "parent/child");

    let grandparent = graph.db.get_parent_page("parent/child").unwrap();
    assert!(grandparent.is_some());
    assert_eq!(grandparent.unwrap().title.to_lowercase(), "parent");
}

// ─── Multi-block indent / outdent (block move) ───────────────────────────────

use grafium_core::models::{Block, BlockType};

fn text_block(graph: &Graph, page_id: &str, parent: Option<&str>, order: i32, body: &str) -> Block {
    graph
        .create_block(
            page_id,
            parent,
            order,
            body,
            BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap()
}

/// Return (id, parent_id, order_index) triples in tree order for a page.
fn tree_shape(graph: &Graph, page_id: &str) -> Vec<(String, Option<String>, i32)> {
    graph
        .db
        .list_blocks_for_page(page_id)
        .unwrap()
        .into_iter()
        .map(|b| (b.id, b.parent_id, b.order_index))
        .collect()
}

/// A freshly-created page seeds a single empty `- ` bullet; drop it so tests
/// control the whole block tree.
fn clear_seed_blocks(graph: &Graph, page_id: &str) {
    for b in graph.db.list_blocks_for_page(page_id).unwrap() {
        graph.delete_block(&b.id).unwrap();
    }
}

#[test]
fn test_multiblock_indent_parents_contiguous_selection_under_predecessor() {
    let (_tmp, graph) = open_graph();
    let page = graph.create_page("indent-in", false).unwrap();
    clear_seed_blocks(&graph, &page.id);

    let a = text_block(&graph, &page.id, None, 0, "a");
    let b = text_block(&graph, &page.id, None, 1, "b");
    let c = text_block(&graph, &page.id, None, 2, "c");
    let d = text_block(&graph, &page.id, None, 3, "d");

    // Simulate planIndentSelection([b,c,d], "in"): all become children of a.
    graph.move_block(&b.id, Some(&a.id), 0).unwrap();
    graph.move_block(&c.id, Some(&a.id), 1).unwrap();
    graph.move_block(&d.id, Some(&a.id), 2).unwrap();

    let shape = tree_shape(&graph, &page.id);
    assert_eq!(
        shape,
        vec![
            (a.id.clone(), None, 0),
            (b.id.clone(), Some(a.id.clone()), 0),
            (c.id.clone(), Some(a.id.clone()), 1),
            (d.id.clone(), Some(a.id.clone()), 2),
        ]
    );
}

#[test]
fn test_multiblock_outdent_moves_group_to_grandparent_after_parent() {
    let (_tmp, graph) = open_graph();
    let page = graph.create_page("indent-out", false).unwrap();
    clear_seed_blocks(&graph, &page.id);

    // a -> {b, c, d}
    let a = text_block(&graph, &page.id, None, 0, "a");
    let b = text_block(&graph, &page.id, Some(&a.id), 0, "b");
    let c = text_block(&graph, &page.id, Some(&a.id), 1, "c");
    let d = text_block(&graph, &page.id, Some(&a.id), 2, "d");

    // Simulate planIndentSelection([b,c,d], "out"): all become roots after a.
    graph.move_block(&b.id, None, 1).unwrap();
    graph.move_block(&c.id, None, 2).unwrap();
    graph.move_block(&d.id, None, 3).unwrap();

    let shape = tree_shape(&graph, &page.id);
    assert_eq!(
        shape,
        vec![
            (a.id.clone(), None, 0),
            (b.id.clone(), None, 1),
            (c.id.clone(), None, 2),
            (d.id.clone(), None, 3),
        ]
    );
}

#[test]
fn test_multiblock_indent_non_contiguous_runs_reparent_independently() {
    let (_tmp, graph) = open_graph();
    let page = graph.create_page("indent-noncontig", false).unwrap();
    clear_seed_blocks(&graph, &page.id);

    // a, b(sel), c, d(sel), e(sel)  -> b under a; d,e under c
    let a = text_block(&graph, &page.id, None, 0, "a");
    let b = text_block(&graph, &page.id, None, 1, "b");
    let c = text_block(&graph, &page.id, None, 2, "c");
    let d = text_block(&graph, &page.id, None, 3, "d");
    let e = text_block(&graph, &page.id, None, 4, "e");

    graph.move_block(&b.id, Some(&a.id), 0).unwrap();
    graph.move_block(&d.id, Some(&c.id), 0).unwrap();
    graph.move_block(&e.id, Some(&c.id), 1).unwrap();

    let shape = tree_shape(&graph, &page.id);
    assert_eq!(
        shape,
        vec![
            (a.id.clone(), None, 0),
            (b.id.clone(), Some(a.id.clone()), 0),
            (c.id.clone(), None, 2),
            (d.id.clone(), Some(c.id.clone()), 0),
            (e.id.clone(), Some(c.id.clone()), 1),
        ]
    );
}
