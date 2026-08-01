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
