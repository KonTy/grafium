use pkm_core::Database;
use pkm_core::models::*;

#[test]
fn test_full_workflow() {
    let db = Database::in_memory().unwrap();

    // Create a page
    let page = db.create_page("My Test Page", false).unwrap();
    assert_eq!(page.title, "My Test Page");
    assert!(!page.is_journal);

    // Create blocks
    let block1 = db.create_block(
        &page.id, None, 0, "Hello [[World]]",
        BlockType::Text, serde_json::json!({})
    ).unwrap();

    let block2 = db.create_block(
        &page.id, None, 1, "TODO Buy groceries",
        BlockType::Text, serde_json::json!({})
    ).unwrap();

    let _child = db.create_block(
        &page.id, Some(&block1.id), 0, "Child block with #rust tag",
        BlockType::Text, serde_json::json!({})
    ).unwrap();

    // List blocks
    let blocks = db.list_blocks_for_page(&page.id).unwrap();
    assert_eq!(blocks.len(), 3);

    // List children
    let children = db.list_child_blocks(&block1.id).unwrap();
    assert_eq!(children.len(), 1);

    // Update block
    db.update_block(&block1.id, "Updated content", None).unwrap();
    let updated = db.get_block(&block1.id).unwrap();
    assert_eq!(updated.content, "Updated content");

    // FTS search
    let results = db.search_fts("groceries", 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, block2.id);

    // Tasks
    let task = db.upsert_task(&block2.id, &TaskState::Todo, Some("2024-01-15"), None).unwrap();
    assert_eq!(task.state, TaskState::Todo);

    let tasks = db.list_tasks(Some(&TaskState::Todo), None, None).unwrap();
    assert_eq!(tasks.len(), 1);

    db.update_task_state(&block2.id, &TaskState::Done).unwrap();
    let done_tasks = db.list_tasks(Some(&TaskState::Done), None, None).unwrap();
    assert_eq!(done_tasks.len(), 1);

    // Favorites
    db.add_favorite(&page.id).unwrap();
    let favs = db.list_favorites().unwrap();
    assert_eq!(favs.len(), 1);
    assert_eq!(favs[0].title, "My Test Page");

    db.remove_favorite(&page.id).unwrap();
    let favs = db.list_favorites().unwrap();
    assert_eq!(favs.len(), 0);

    // Recent pages
    db.record_page_open(&page.id).unwrap();
    let recent = db.list_recent_pages(10).unwrap();
    assert_eq!(recent.len(), 1);

    // Links
    db.insert_link(&block1.id, &page.id, LinkType::Page).unwrap();
    let backlinks = db.get_backlinks(&page.id).unwrap();
    assert_eq!(backlinks.len(), 1);

    // Delete
    db.delete_block(&block1.id).unwrap();
    let blocks = db.list_blocks_for_page(&page.id).unwrap();
    assert_eq!(blocks.len(), 1); // tree-order listing only returns reachable root/subtree blocks

    // Page count
    let count = db.count_pages().unwrap();
    assert_eq!(count, 1);

    db.delete_page(&page.id).unwrap();
    let count = db.count_pages().unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_journal_pages() {
    let db = Database::in_memory().unwrap();

    db.create_page("2024_01_15", true).unwrap();
    db.create_page("2024_01_16", true).unwrap();
    db.create_page("Regular Page", false).unwrap();

    let journals = db.list_journal_pages(10, 0).unwrap();
    assert_eq!(journals.len(), 2);
}

#[test]
fn test_flashcards() {
    let db = Database::in_memory().unwrap();

    let page = db.create_page("Flashcard Page", false).unwrap();
    let block = db.create_block(
        &page.id, None, 0, "Capital of France :: Paris #flashcard",
        BlockType::Flashcard, serde_json::json!({})
    ).unwrap();

    let card = db.upsert_flashcard(
        &block.id, "Capital of France", "Paris", &["geography".to_string()]
    ).unwrap();
    assert_eq!(card.ease_factor, 2.5);
    assert_eq!(card.interval_days, 0);

    // List due cards (should include new cards)
    let due = db.list_flashcards_due(10).unwrap();
    assert_eq!(due.len(), 1);

    // Review
    let next_review = chrono::Utc::now().timestamp_millis() + 86400000;
    db.update_flashcard_review(&card.id, 2.6, 1, next_review).unwrap();

    // After review with future date, should not be due
    let due = db.list_flashcards_due(10).unwrap();
    assert_eq!(due.len(), 0);
}

#[test]
fn test_get_or_create_page() {
    let db = Database::in_memory().unwrap();

    let page1 = db.get_or_create_page("Test", false).unwrap();
    let page2 = db.get_or_create_page("Test", false).unwrap();

    assert_eq!(page1.id, page2.id);
    assert_eq!(db.count_pages().unwrap(), 1);
}

#[test]
fn test_block_reorder() {
    let db = Database::in_memory().unwrap();

    let page = db.create_page("Reorder Test", false).unwrap();
    let b1 = db.create_block(&page.id, None, 0, "First", BlockType::Text, serde_json::json!({})).unwrap();
    let b2 = db.create_block(&page.id, None, 1, "Second", BlockType::Text, serde_json::json!({})).unwrap();
    let b3 = db.create_block(&page.id, None, 2, "Third", BlockType::Text, serde_json::json!({})).unwrap();

    // Reverse order
    db.reorder_blocks(&page.id, &[b3.id.clone(), b2.id.clone(), b1.id.clone()]).unwrap();

    let blocks = db.list_blocks_for_page(&page.id).unwrap();
    assert_eq!(blocks[0].content, "Third");
    assert_eq!(blocks[1].content, "Second");
    assert_eq!(blocks[2].content, "First");
}

#[test]
fn test_recent_pages_case_insensitive_dedup() {
    let db = Database::in_memory().unwrap();

    let lower = db.create_page("test", false).unwrap();
    let upper = db.create_page("Test", false).unwrap();

    db.record_page_open(&lower.id).unwrap();
    db.record_page_open(&upper.id).unwrap();

    let recents = db.list_recent_pages(10).unwrap();
    assert_eq!(recents.len(), 1);
    assert_eq!(recents[0].title.to_lowercase(), "test");
}

#[test]
fn test_list_pages_case_insensitive_dedup() {
    let db = Database::in_memory().unwrap();

    db.create_page("test", false).unwrap();
    db.create_page("Test", false).unwrap();
    db.create_page("TEST", false).unwrap();
    // journal pages are excluded from list_pages
    db.create_page("2026-01-01", true).unwrap();

    let pages = db.list_pages(100, 0).unwrap();
    let lower_titles: Vec<String> = pages.iter().map(|p| p.title.to_lowercase()).collect();

    // Should contain "test" exactly once
    assert_eq!(lower_titles.iter().filter(|t| *t == "test").count(), 1,
        "expected exactly one 'test' entry, got: {:?}", pages.iter().map(|p| &p.title).collect::<Vec<_>>());
    // Journal pages must not appear
    assert!(!lower_titles.contains(&"2026-01-01".to_string()),
        "journal pages must not appear in list_pages");
}
