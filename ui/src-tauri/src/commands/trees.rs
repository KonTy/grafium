//! Tauri commands for the page-tree sidebar and collections.
//!
//! These are thin: the real work (reconstructing a tree, reading/writing a
//! collection marker) lives in `grafium_core` so it can be unit-tested without
//! Tauri. Each command fetches the already-computed page set and hands it to a
//! pure builder.
//!
//! ## Casing
//!
//! Every command here is a plain `#[tauri::command]` — no `rename_all` — exactly
//! like `ai_ask_stream`. Tauri v2 already maps camelCase JS argument names onto
//! snake_case Rust parameters by default (`pageId` in JS → `page_id` here), and
//! the returned DTOs derive plain `Serialize`, so they go over the wire in
//! snake_case (`page_id`, `descendant_count`, `member_count`) — the field names
//! the frontend contract is frozen against.

use crate::AppState;
use grafium_core::knowledge::{
    build_namespace_tree, build_tag_tree, clear_collection, collection_of, mark_collection,
    TreeNode,
};
use serde::Serialize;
use tauri::State;

/// One row in the collections list: enough to render "My Novel · book · 12
/// entries" without fetching each page.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionSummary {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub member_count: i64,
}

/// The namespace tree: every non-journal page nested by its title path.
///
/// Pulls the whole page set in one shot (`limit = -1` means "everything" to
/// `list_pages`, which already drops journals and case-duplicate titles) and
/// nests it in memory, rather than doing any per-node recursion in SQL.
#[tauri::command]
pub fn pages_namespace_tree(state: State<AppState>) -> Result<Vec<TreeNode>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let pages = graph.db.list_pages(-1, 0).map_err(|e| e.to_string())?;
    Ok(build_namespace_tree(&pages))
}

/// The tag tree: the pages used as tags, nested by their tag path.
#[tauri::command]
pub fn pages_tag_tree(state: State<AppState>) -> Result<Vec<TreeNode>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let tag_pages = graph.db.list_tag_pages().map_err(|e| e.to_string())?;
    Ok(build_tag_tree(&tag_pages))
}

/// Mark a page as a collection of `kind`, or clear its marker when `kind` is
/// `None`.
///
/// Reads the page's current properties, edits only the `collection` key, and
/// writes the whole blob back through the same `update_page` path the page
/// editor uses — so unrelated properties survive and the normalized
/// `page_properties` mirror stays in sync.
#[tauri::command]
pub fn page_set_collection(
    state: State<AppState>,
    page_id: String,
    kind: Option<String>,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let page = graph
        .db
        .get_page_by_id(&page_id)
        .map_err(|e| e.to_string())?;

    let mut properties = page.properties.clone();
    match kind {
        Some(kind) => mark_collection(&mut properties, &kind),
        None => clear_collection(&mut properties),
    }

    graph
        .update_page_properties(&page_id, properties)
        .map_err(|e| e.to_string())
}

/// List every collection page with its kind and member count.
///
/// `list_collections` already filters to marked pages, so `collection_of` is
/// expected to be `Some` for each; `filter_map` just means a page whose marker
/// was hand-corrupted between the SQL filter and here is quietly skipped rather
/// than crashing the whole list.
#[tauri::command]
pub fn pages_list_collections(state: State<AppState>) -> Result<Vec<CollectionSummary>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let collections = graph.db.list_collections().map_err(|e| e.to_string())?;

    Ok(collections
        .into_iter()
        .filter_map(|(page, member_count)| {
            collection_of(&page).map(|info| CollectionSummary {
                id: page.id,
                title: page.title,
                kind: info.kind,
                member_count,
            })
        })
        .collect())
}
