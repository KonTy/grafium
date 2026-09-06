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

// ─── A book link inside a journal ────────────────────────────────────────────

/// Typing `[[mybooks/coolbook/toc]]` into a journal, then more journal text
/// underneath it, must not mix the two: the words you type in the journal
/// belong to the journal file, and the book page stays empty until you
/// actually open it and write there.
#[test]
fn test_book_link_in_journal_keeps_journal_text_in_the_journal() {
    let (tmp, graph) = open_graph();

    let journal = graph.create_page("2025_01_15", true).unwrap();
    graph
        .create_block(
            &journal.id,
            None,
            0,
            "Starting on [[mybooks/coolbook/toc]]",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();
    graph
        .create_block(
            &journal.id,
            None,
            1,
            "thought about chapter ordering today",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    let toc = graph
        .db
        .get_page_by_title_ci("mybooks/coolbook/toc")
        .expect("linking must create the book page");

    // Linking alone must not carve out folders on disk.
    assert!(
        toc.file_path.is_none(),
        "a link alone must not create a file, got {:?}",
        toc.file_path
    );
    assert!(
        !tmp.path().join("pages/mybooks").exists(),
        "a link alone must not create the book folder"
    );

    // The journal text stayed in the journal, and none of it leaked into the book.
    let journal_text = std::fs::read_to_string(tmp.path().join("journals/2025_01_15.md")).unwrap();
    assert!(journal_text.contains("chapter ordering"));
    assert!(graph.db.list_blocks_for_page(&toc.id).unwrap().is_empty());

    // Writing *in* the book page is what creates the folder and the file.
    graph
        .create_block(
            &toc.id,
            None,
            0,
            "1. Openings",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    let toc_file = tmp.path().join("pages/mybooks/coolbook/toc.md");
    assert!(toc_file.exists(), "writing in the page creates the folder");
    assert!(std::fs::read_to_string(&toc_file).unwrap().contains("Openings"));
    assert!(
        !std::fs::read_to_string(&toc_file).unwrap().contains("chapter ordering"),
        "journal text must never land in the book file"
    );
}


// ─── Task completion history ─────────────────────────────────────────────────

/// Completing a task must leave a record in the markdown, not just the database.
///
/// This is the whole point: a completion time held only in SQLite is lost the
/// moment the graph is re-indexed, copied to another machine, or opened against
/// a fresh database — and "when did I finish that?" is not recoverable.
#[test]
fn test_completion_time_is_written_to_the_file_and_survives_reindex() {
    let (tmp, graph) = open_graph();

    let page = graph.create_page("work", false).unwrap();
    let block = graph
        .create_block(
            &page.id,
            None,
            0,
            "TODO Write the report",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    graph
        .update_task_state(&block.id, &grafium_core::models::TaskState::Doing)
        .unwrap();
    graph
        .update_task_state(&block.id, &grafium_core::models::TaskState::Done)
        .unwrap();

    let path = tmp.path().join("pages/work.md");
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(on_disk.contains("DONE Write the report"), "{on_disk}");
    assert!(on_disk.contains("CLOSED: ["), "completion time must be in the file:\n{on_disk}");
    assert!(
        on_disk.contains(r#"* State "DOING" from "TODO""#),
        "the start must be recorded too, or duration is unanswerable:\n{on_disk}"
    );
    assert!(on_disk.contains(r#"* State "DONE" from "DOING""#), "{on_disk}");

    // Re-index from disk, the way a fresh machine or a rebuilt database would.
    graph.index_file(&path).unwrap();

    let reloaded = graph.db.list_blocks_for_page(&page.id).unwrap();
    let task_block = reloaded
        .iter()
        .find(|b| b.content.starts_with("DONE"))
        .expect("the task must still be there");
    let fields = grafium_core::parser::task::parse_fields(&task_block.content);
    assert!(
        fields.closed_at.is_some(),
        "the completion time must survive a re-index: {:?}",
        task_block.content
    );
}

/// Re-opening a finished task clears its completion time everywhere.
#[test]
fn test_reopening_a_task_clears_the_completion_time() {
    let (tmp, graph) = open_graph();
    let page = graph.create_page("work", false).unwrap();
    let block = graph
        .create_block(
            &page.id,
            None,
            0,
            "TODO Fix the bug",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    graph
        .update_task_state(&block.id, &grafium_core::models::TaskState::Done)
        .unwrap();
    graph
        .update_task_state(&block.id, &grafium_core::models::TaskState::Todo)
        .unwrap();

    let on_disk = std::fs::read_to_string(tmp.path().join("pages/work.md")).unwrap();
    assert!(!on_disk.contains("CLOSED:"), "a reopened task is not closed:\n{on_disk}");
    assert!(
        on_disk.contains(r#"* State "TODO" from "DONE""#),
        "reopening is itself part of the history:\n{on_disk}"
    );
}

/// A task nobody has touched in months must still appear.
///
/// The open-task query used to drop anything older than 182 days, which is
/// backwards for a task list: the thing you have been avoiding longest is the
/// one that most needs to be seen, and instead it vanished silently.
#[test]
fn test_a_long_neglected_task_is_still_listed() {
    let (_tmp, graph) = open_graph();
    let page = graph.create_page("work", false).unwrap();
    let block = graph
        .create_block(
            &page.id,
            None,
            0,
            "TODO Renew the domain",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    // Backdate it well past the old cutoff.
    let ancient = chrono::Utc::now().timestamp_millis() - 400 * 24 * 60 * 60 * 1000;
    graph.db.backdate_task_for_test(&block.id, ancient).unwrap();

    let open = graph.db.get_open_tasks(182).unwrap();
    assert!(
        open.iter().any(|(_, content, _, _, _)| content.contains("Renew the domain")),
        "a neglected task must not disappear from the list: {open:?}"
    );
}

/// Completing a repeating task rolls it forward instead of closing it.
#[test]
fn test_a_repeating_task_reopens_on_its_next_date() {
    let (tmp, graph) = open_graph();
    let page = graph.create_page("chores", false).unwrap();
    let block = graph
        .create_block(
            &page.id,
            None,
            0,
            "TODO Water the plants\nSCHEDULED: <2026-09-07 Mon .+3d>",
            grafium_core::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();

    graph
        .update_task_state(&block.id, &grafium_core::models::TaskState::Done)
        .unwrap();

    let on_disk = std::fs::read_to_string(tmp.path().join("pages/chores.md")).unwrap();
    assert!(
        on_disk.contains("TODO Water the plants"),
        "a repeating task comes back open, not DONE:\n{on_disk}"
    );
    assert!(!on_disk.contains("CLOSED:"), "and it is not closed:\n{on_disk}");
    assert!(
        !on_disk.contains("<2026-09-07"),
        "its date must have moved on:\n{on_disk}"
    );
}
