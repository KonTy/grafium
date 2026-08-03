use crate::AppState;
use grafium_core::models::{Block, BlockType};
use tauri::State;

#[tauri::command(rename_all = "camelCase")]
pub fn list_blocks(state: State<AppState>, page_id: String) -> Result<Vec<Block>, String> {
    let start = std::time::Instant::now();
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let result = graph.db.list_blocks_for_page(&page_id).map_err(|e| e.to_string());
    match &result {
        Ok(blocks) => tracing::info!(
            page_id = %page_id,
            block_count = blocks.len(),
            elapsed_ms = start.elapsed().as_millis(),
            "list_blocks completed"
        ),
        Err(e) => tracing::warn!(page_id = %page_id, error = %e, "list_blocks failed"),
    }
    result
}

#[tauri::command(rename_all = "camelCase")]
pub fn create_block(
    state: State<AppState>,
    page_id: String,
    parent_id: Option<String>,
    order_index: i32,
    content: String,
    block_type: Option<String>,
    properties: Option<serde_json::Value>,
) -> Result<Block, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let bt = block_type
        .map(|s| BlockType::from_str(&s))
        .unwrap_or(BlockType::Text);
    let props = properties.unwrap_or(serde_json::json!({}));
    graph
        .create_block(
            &page_id,
            parent_id.as_deref(),
            order_index,
            &content,
            bt,
            props,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn update_block(
    state: State<AppState>,
    id: String,
    content: String,
    properties: Option<serde_json::Value>,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .update_block(&id, &content, properties.as_ref())
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_block(state: State<AppState>, id: String) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.delete_block(&id).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn move_block(
    state: State<AppState>,
    id: String,
    new_parent_id: Option<String>,
    order_index: i32,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .move_block(&id, new_parent_id.as_deref(), order_index)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn reorder_blocks(
    state: State<AppState>,
    page_id: String,
    block_ids: Vec<String>,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .reorder_blocks(&page_id, &block_ids)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_block_page_title(state: State<AppState>, block_id: String) -> Result<String, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .get_block_page_title(&block_id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn search_fts(
    state: State<AppState>,
    query: String,
    limit: Option<i64>,
) -> Result<Vec<Block>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .search_fts(&query, limit.unwrap_or(50))
        .map_err(|e| e.to_string())
}
