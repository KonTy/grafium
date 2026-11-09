use crate::AppState;
use grafium_core::graph::{BlockCreateParent, BlockCreateSpec, BlockMove};
use grafium_core::models::{Block, BlockType};
use serde::Deserialize;
use tauri::State;

#[cfg(all(test, target_os = "linux"))]
#[path = "blocks_native_tests.rs"]
mod native_tests;

// Both waiting for the graph lock and SQLite/file work must stay off the UI thread.
async fn with_graph<T: Send + 'static>(
    state: State<'_, AppState>,
    work: impl FnOnce(&grafium_core::Graph) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let graph = state.graph.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let graph = graph.lock().map_err(|e| e.to_string())?;
        work(&graph)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBlockBatchItem {
    id: Option<String>,
    parent_id: Option<String>,
    parent_index: Option<usize>,
    order_index: i32,
    content: String,
    block_type: Option<String>,
    properties: Option<serde_json::Value>,
}

#[tauri::command(rename_all = "camelCase")]
pub async fn list_blocks(
    state: State<'_, AppState>,
    page_id: String,
) -> Result<Vec<Block>, String> {
    let start = std::time::Instant::now();
    with_graph(state, move |graph| {
        let result = graph
            .db
            .list_blocks_for_page(&page_id)
            .map_err(|e| e.to_string());
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
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_block(state: State<'_, AppState>, block_id: String) -> Result<Block, String> {
    with_graph(state, move |graph| {
        graph
            .db
            .get_block_by_id(&block_id)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn create_block(
    state: State<'_, AppState>,
    page_id: String,
    parent_id: Option<String>,
    order_index: i32,
    content: String,
    block_type: Option<String>,
    properties: Option<serde_json::Value>,
) -> Result<Block, String> {
    with_graph(state, move |graph| {
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
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn insert_block(
    state: State<'_, AppState>,
    page_id: String,
    parent_id: Option<String>,
    position: usize,
    content: String,
) -> Result<Block, String> {
    with_graph(state, move |graph| {
        graph
            .insert_block_at(
                &page_id,
                parent_id.as_deref(),
                position,
                &content,
                BlockType::Text,
                serde_json::json!({}),
            )
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn create_blocks(
    state: State<'_, AppState>,
    page_id: String,
    blocks: Vec<CreateBlockBatchItem>,
) -> Result<Vec<Block>, String> {
    let specs = blocks
        .into_iter()
        .map(|block| {
            if block.parent_id.is_some() && block.parent_index.is_some() {
                return Err("batch block cannot specify both parentId and parentIndex".to_string());
            }

            let parent = if let Some(index) = block.parent_index {
                BlockCreateParent::NewBlock(index)
            } else if let Some(parent_id) = block.parent_id {
                BlockCreateParent::Existing(parent_id)
            } else {
                BlockCreateParent::Root
            };

            Ok(BlockCreateSpec {
                id: block.id,
                parent,
                order_index: block.order_index,
                content: block.content,
                block_type: block
                    .block_type
                    .map(|s| BlockType::from_str(&s))
                    .unwrap_or(BlockType::Text),
                properties: block.properties.unwrap_or_else(|| serde_json::json!({})),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    with_graph(state, move |graph| {
        graph
            .create_blocks(&page_id, specs)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_block(
    state: State<'_, AppState>,
    id: String,
    content: String,
    properties: Option<serde_json::Value>,
) -> Result<(), String> {
    with_graph(state, move |graph| {
        graph
            .update_block(&id, &content, properties.as_ref())
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_block(state: State<'_, AppState>, id: String) -> Result<(), String> {
    with_graph(state, move |graph| {
        graph.delete_block(&id).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_blocks(
    state: State<'_, AppState>,
    page_id: String,
    ids: Vec<String>,
) -> Result<Vec<Block>, String> {
    with_graph(state, move |graph| {
        graph
            .delete_blocks(&page_id, &ids)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn move_block(
    state: State<'_, AppState>,
    id: String,
    new_parent_id: Option<String>,
    order_index: i32,
) -> Result<(), String> {
    with_graph(state, move |graph| {
        graph
            .move_block(&id, new_parent_id.as_deref(), order_index)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn restructure_blocks(
    state: State<'_, AppState>,
    page_id: String,
    moves: Vec<BlockMove>,
    delete_ids: Vec<String>,
) -> Result<Vec<Block>, String> {
    with_graph(state, move |graph| {
        graph
            .restructure_blocks(&page_id, &moves, &delete_ids)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reorder_blocks(
    state: State<'_, AppState>,
    page_id: String,
    block_ids: Vec<String>,
) -> Result<(), String> {
    with_graph(state, move |graph| {
        graph
            .reorder_blocks(&page_id, &block_ids)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_block_page_title(
    state: State<'_, AppState>,
    block_id: String,
) -> Result<String, String> {
    with_graph(state, move |graph| {
        graph
            .db
            .get_block_page_title(&block_id)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn search_fts(
    state: State<'_, AppState>,
    query: String,
    limit: Option<i64>,
) -> Result<Vec<Block>, String> {
    with_graph(state, move |graph| {
        graph
            .db
            .search_fts(&query, limit.unwrap_or(50))
            .map_err(|e| e.to_string())
    })
    .await
}
