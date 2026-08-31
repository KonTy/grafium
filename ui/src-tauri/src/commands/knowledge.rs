//! Tauri commands for the Knowledge Engine — AI, references, vector search, schemas.

use grafium_core::ai::config::{
    AiConfig, AiMode, CloudConfig, LocalConfig, LocalLlmSettings, ProviderType,
};
use grafium_core::ai::traits::SearchResult;
use grafium_core::knowledge::engine::HealthStatus;
use grafium_core::knowledge::registry::{GraphType, RegisteredGraph};
use grafium_core::knowledge::schemas::Schema;
use grafium_core::knowledge::KnowledgeEngine;
use grafium_core::model_library::LocalModelRef;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use tokio::sync::RwLock;

/// Shared state for the knowledge engine.
///
/// The engine lives behind an `Arc` *inside* the lock so callers can clone a
/// handle out and release the lock immediately. Holding a read guard for the
/// duration of a long AI call is what used to freeze the app: `tokio`'s
/// `RwLock` is write-preferring, so a single queued writer (saving AI
/// settings) blocks every subsequent reader until the long reader finishes.
pub struct KnowledgeState {
    pub engine: Arc<RwLock<Option<Arc<KnowledgeEngine>>>>,
}

impl KnowledgeState {
    /// Clone a handle to the engine, holding the lock only long enough to copy
    /// an `Arc`. Never hold the guard across an `.await` that does real work.
    async fn handle(&self) -> Result<Arc<KnowledgeEngine>, String> {
        self.engine
            .read()
            .await
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(|| "Knowledge engine not initialized".to_string())
    }

    /// As [`KnowledgeState::handle`], but rejects an engine whose providers
    /// aren't configured, so callers fail fast with an actionable message.
    async fn ready_handle(&self) -> Result<Arc<KnowledgeEngine>, String> {
        let engine = self.handle().await?;
        if !engine.is_ready() {
            return Err("AI engine not ready — check your AI settings".to_string());
        }
        Ok(engine)
    }
}

const AI_INDEX_BATCH_SIZE: i64 = 100;

struct PageBatchCursor {
    batch_size: i64,
    offset: i64,
    finished: bool,
}

impl PageBatchCursor {
    fn new(batch_size: i64) -> Self {
        Self {
            batch_size,
            offset: 0,
            finished: false,
        }
    }

    /// Advance the cursor over a batch the caller already fetched.
    ///
    /// Split out from [`PageBatchCursor::next_batch`] because the indexing job
    /// fetches through `spawn_blocking`, which can't be expressed as the
    /// synchronous `fetch` closure.
    fn accept_batch<T>(&mut self, batch: Vec<T>) -> Option<Vec<T>> {
        if self.finished {
            return None;
        }
        if batch.is_empty() {
            self.finished = true;
            return None;
        }

        self.offset += batch.len() as i64;
        Some(batch)
    }
}

// ─── Configuration ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct AiConfigPayload {
    pub enabled: bool,
    pub mode: String,
    pub local_provider: Option<String>,
    pub local_base_url: Option<String>,
    pub local_api_key: Option<String>,
    pub local_model_path: Option<String>,
    /// Shared directory to search for local model files (embedded LLM
    /// GGUF, and in future Whisper) instead of Grafium's own managed
    /// `<data_dir>/models` folder — lets a user point at e.g.
    /// `~/Documents/models` shared with other apps. `None`/empty keeps the
    /// default.
    pub local_models_dir: Option<String>,
    pub llm_model: Option<String>,
    pub embedding_model: Option<String>,
    pub cloud_provider: Option<String>,
    pub cloud_base_url: Option<String>,
    pub cloud_llm_model: Option<String>,
    pub cloud_api_key: Option<String>,
    pub cloud_embedding_provider: Option<String>,
    pub cloud_embedding_base_url: Option<String>,
    pub cloud_embedding_api_key: Option<String>,
    pub cloud_embedding_model: Option<String>,
}

#[tauri::command]
pub async fn ai_get_config(state: State<'_, KnowledgeState>) -> Result<serde_json::Value, String> {
    let engine = state.engine.read().await.as_ref().map(Arc::clone);
    if let Some(engine) = engine {
        serde_json::to_value(engine.config()).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::to_value(AiConfig::default()).unwrap())
    }
}

#[tauri::command]
pub async fn ai_set_config(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    payload: AiConfigPayload,
) -> Result<(), String> {
    fn parse_provider(name: &str) -> ProviderType {
        match name {
            "anthropic" => ProviderType::Anthropic,
            "openai_compatible" | "openaicompatible" | "vllm" => ProviderType::OpenAiCompatible,
            "ollama" => ProviderType::Ollama,
            "huggingface" | "huggingface_local" => ProviderType::HuggingFace,
            _ => ProviderType::OpenAi,
        }
    }

    let mode = match payload.mode.as_str() {
        "cloud" => AiMode::Cloud,
        "hybrid" => AiMode::Hybrid,
        _ => AiMode::Local,
    };

    let local_provider = payload
        .local_provider
        .as_deref()
        .map(parse_provider)
        .unwrap_or(ProviderType::OpenAiCompatible);

    let local_base_url_default = match local_provider {
        ProviderType::Ollama => "http://localhost:11434",
        _ => "http://localhost:8000/v1",
    };

    let local = Some(LocalConfig {
        provider: local_provider,
        base_url: payload
            .local_base_url
            .unwrap_or_else(|| local_base_url_default.to_string()),
        api_key: payload.local_api_key,
        models_dir: payload
            .local_models_dir
            .filter(|s| !s.trim().is_empty())
            .map(std::path::PathBuf::from),
        local_llm: LocalLlmSettings {
            model_ref: LocalModelRef {
                model: payload.local_model_path,
            },
            ..Default::default()
        },
        llm_model: payload.llm_model.unwrap_or_else(|| "llama3.2".to_string()),
        embedding_model: payload
            .embedding_model
            .unwrap_or_else(|| "nomic-embed-text".to_string()),
    });

    let cloud = if let (Some(provider), Some(model)) =
        (&payload.cloud_provider, &payload.cloud_llm_model)
    {
        let provider_type = parse_provider(provider);
        let embedding_provider = payload
            .cloud_embedding_provider
            .as_deref()
            .map(parse_provider)
            .unwrap_or_else(|| {
                if provider_type == ProviderType::OpenAi {
                    ProviderType::OpenAi
                } else {
                    ProviderType::OpenAiCompatible
                }
            });

        Some(CloudConfig {
            llm_provider: provider_type.clone(),
            llm_model: model.clone(),
            llm_api_key: payload.cloud_api_key.clone(),
            llm_base_url: payload.cloud_base_url,
            embedding_provider,
            embedding_model: payload
                .cloud_embedding_model
                .unwrap_or_else(|| "text-embedding-3-small".to_string()),
            embedding_api_key: payload.cloud_embedding_api_key.or(payload.cloud_api_key),
            embedding_base_url: payload.cloud_embedding_base_url,
        })
    } else {
        None
    };

    let config = AiConfig {
        enabled: payload.enabled,
        mode,
        local,
        cloud,
        ..AiConfig::default()
    };

    // Save config to disk.
    let config_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("knowledge");
    std::fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    let config_path = config_dir.join("ai_config.json");
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&config_path, json).map_err(|e| e.to_string())?;

    // Reconfigure by swapping in a freshly built engine rather than mutating
    // the shared one. Any job still running keeps the engine it started with,
    // so changing the model mid-index can't swap providers underneath it. The
    // graph registry is persisted on every write, so a new engine reloads it.
    let rebuilt = KnowledgeEngine::new(&config_dir, config).map_err(|e| e.to_string())?;
    let mut guard = state.engine.write().await;
    *guard = Some(Arc::new(rebuilt));
    drop(guard);

    Ok(())
}

// ─── Health ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_health_check(state: State<'_, KnowledgeState>) -> Result<HealthStatus, String> {
    // Health checks hit the provider over the network; cloning the handle
    // first means a slow or unreachable provider can't hold the engine lock.
    let engine = state.engine.read().await.as_ref().map(Arc::clone);
    if let Some(engine) = engine {
        engine.health_check().await.map_err(|e| e.to_string())
    } else {
        Ok(HealthStatus {
            enabled: false,
            llm_available: false,
            embedder_available: false,
            vector_store_available: false,
            vector_count: 0,
            mode: AiMode::Local,
        })
    }
}

// ─── Indexing ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_index_page(
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    page_id: String,
) -> Result<usize, String> {
    let engine = state.ready_handle().await?;

    let (page, blocks, graph_id) = {
        let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
        let page = graph
            .db
            .get_page_by_id(&page_id)
            .map_err(|e| e.to_string())?;
        let blocks = graph
            .db
            .list_blocks_for_page(&page_id)
            .map_err(|e| e.to_string())?;
        let graph_id = graph.root_dir.to_string_lossy().to_string();
        (page, blocks, graph_id)
    };

    engine
        .index_page(&page, &blocks, &graph_id)
        .await
        .map_err(|e| e.to_string())
}

/// Start a full-graph reindex as a background job.
///
/// Returns a job id immediately rather than the final chunk count. Indexing a
/// large graph takes minutes; making the UI await it meant the work belonged
/// to whichever settings panel was open, and closing that panel lost all
/// visibility into it. Progress and completion arrive on `job://update`.
#[tauri::command]
pub async fn ai_index_all_pages(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    jobs: State<'_, crate::commands::jobs::JobsState>,
) -> Result<String, String> {
    let engine = state.ready_handle().await?;
    let snapshot = crate::current_graph_snapshot(&app, app_state.graph.as_ref())?;

    let job = jobs
        .registry
        .start(app.clone(), "ai_index_all", "Indexing graph for AI search", true);
    let job_id = job.id().to_string();

    tokio::spawn(async move {
        match index_all_pages_job(&job, engine, snapshot).await {
            Ok(Outcome::Finished { chunks, pages }) => {
                job.succeeded(
                    format!("Indexed {chunks} chunks across {pages} pages"),
                    None,
                );
            }
            Ok(Outcome::Cancelled) => job.cancelled(),
            Err(e) => job.failed(e),
        }
    });

    Ok(job_id)
}

enum Outcome {
    Finished { chunks: usize, pages: usize },
    Cancelled,
}

/// The actual indexing loop, off the command path.
///
/// SQLite access here is synchronous, so every database touch goes through
/// `spawn_blocking` — running it inline would park a tokio worker thread for
/// the whole query and starve unrelated IPC commands.
async fn index_all_pages_job(
    job: &crate::commands::jobs::JobHandle,
    engine: Arc<KnowledgeEngine>,
    snapshot: crate::GraphRuntimeSnapshot,
) -> Result<Outcome, String> {
    let graph = Arc::new(
        tokio::task::spawn_blocking({
            let snapshot = snapshot.clone();
            move || crate::open_graph_snapshot(&snapshot)
        })
        .await
        .map_err(|e| e.to_string())??,
    );
    let graph_id = snapshot.root_dir.to_string_lossy().to_string();

    let total_pages = {
        let graph = Arc::clone(&graph);
        tokio::task::spawn_blocking(move || graph.db.count_pages())
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())? as usize
    };

    job.progress(0, total_pages, format!("Indexing 0 of {total_pages} pages"));

    let mut cursor = PageBatchCursor::new(AI_INDEX_BATCH_SIZE);
    let mut chunks = 0usize;
    let mut done = 0usize;
    let mut failures = 0usize;

    loop {
        if job.is_cancelled() {
            return Ok(Outcome::Cancelled);
        }

        let (limit, offset) = (cursor.batch_size, cursor.offset);
        let batch = {
            let graph = Arc::clone(&graph);
            tokio::task::spawn_blocking(move || graph.db.list_pages_window(limit, offset, false))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?
        };
        let Some(pages) = cursor.accept_batch(batch) else {
            break;
        };

        for page in pages {
            if job.is_cancelled() {
                return Ok(Outcome::Cancelled);
            }

            let blocks = {
                let graph = Arc::clone(&graph);
                let page_id = page.id.clone();
                tokio::task::spawn_blocking(move || graph.db.list_blocks_for_page(&page_id))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?
            };

            match engine.index_page(&page, &blocks, &graph_id).await {
                Ok(count) => chunks += count,
                Err(e) => {
                    // One unindexable page must not abort the whole run.
                    failures += 1;
                    eprintln!("Failed to index page '{}': {}", page.title, e);
                }
            }

            done += 1;
            let label = if failures > 0 {
                format!("Indexing {done} of {total_pages} pages · {failures} skipped")
            } else {
                format!("Indexing {done} of {total_pages} pages")
            };
            job.progress(done, total_pages, label);
        }
    }

    Ok(Outcome::Finished {
        chunks,
        pages: done,
    })
}

// ─── Search ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_search(
    state: State<'_, KnowledgeState>,
    query: String,
    top_k: Option<usize>,
    graph_id: Option<String>,
) -> Result<Vec<SearchResult>, String> {
    let engine = state.ready_handle().await?;

    engine
        .search(&query, top_k.unwrap_or(10), graph_id.as_deref())
        .await
        .map_err(|e| e.to_string())
}

// ─── References ──────────────────────────────────────────────────────────────

/// Channel carrying finished reference payloads.
pub const REFERENCES_EVENT: &str = "ai://references";

#[derive(Debug, Clone, Serialize)]
pub struct ReferencesReady {
    pub page_id: String,
    pub meta: grafium_core::ai::references::PageReferencesMeta,
}

/// Generate AI references for a page as a background job.
///
/// Returns a job id immediately. The old version made the reference panel own
/// the request, so navigating away mid-generation threw the result away with
/// no indication anything had happened. The job survives the panel and the
/// completion notification links straight back to the page.
#[tauri::command]
pub async fn ai_generate_references(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    jobs: State<'_, crate::commands::jobs::JobsState>,
    page_id: String,
) -> Result<String, String> {
    let engine = state.ready_handle().await?;

    let (page_title, blocks_data, graph_id) = {
        let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
        let page = graph
            .db
            .get_page_by_id(&page_id)
            .map_err(|e| e.to_string())?;
        let blocks = graph
            .db
            .list_blocks_for_page(&page_id)
            .map_err(|e| e.to_string())?;
        let graph_id = graph.root_dir.to_string_lossy().to_string();

        let blocks_data: Vec<(String, String)> =
            blocks.into_iter().map(|b| (b.id, b.content)).collect();

        (page.title, blocks_data, graph_id)
    };

    let job = jobs.registry.start(
        app.clone(),
        "ai_references",
        format!("Finding references for “{page_title}”"),
        false,
    );
    let job_id = job.id().to_string();

    tokio::spawn(async move {
        match engine
            .generate_references(&page_id, &page_title, &blocks_data, &graph_id)
            .await
        {
            Ok(meta) => {
                let count = meta.references.len();
                // References are computed, not persisted, so the payload has
                // to reach the UI on its own channel. Any mounted reference
                // panel for this page picks it up, whether or not it was the
                // thing that started the run.
                let _ = app.emit(
                    REFERENCES_EVENT,
                    ReferencesReady {
                        page_id: page_id.clone(),
                        meta,
                    },
                );
                let summary = if count == 1 {
                    format!("Found 1 reference for “{page_title}”")
                } else {
                    format!("Found {count} references for “{page_title}”")
                };
                job.succeeded(
                    summary,
                    Some(crate::commands::jobs::JobLink {
                        page_id,
                        label: page_title,
                    }),
                );
            }
            Err(e) => job.failed(e.to_string()),
        }
    });

    Ok(job_id)
}

// ─── RAG / Ask ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_ask(
    state: State<'_, KnowledgeState>,
    question: String,
    graph_id: Option<String>,
) -> Result<String, String> {
    let engine = state.ready_handle().await?;

    engine
        .ask(&question, graph_id.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskStreamChunk {
    pub request_id: String,
    pub delta: String,
    pub done: bool,
    pub error: Option<String>,
}

/// Answer a question and deliver it over the `ai://chat_stream` channel.
///
/// The provider trait exposes only `complete()`, so the answer necessarily
/// arrives whole. This previously dribbled it out 24 characters at a time with
/// a 12 ms sleep between chunks — a typewriter effect faked *after* the answer
/// was already known, which added over a second of pure invented latency to a
/// long reply. The answer is now emitted as soon as it exists; any reveal
/// animation belongs in the UI, where it costs nothing.
#[tauri::command]
pub async fn ai_ask_stream(
    state: State<'_, KnowledgeState>,
    app: tauri::AppHandle,
    question: String,
    graph_id: Option<String>,
    request_id: String,
) -> Result<(), String> {
    let engine = state.ready_handle().await?;

    let answer = match engine.ask(&question, graph_id.as_deref()).await {
        Ok(answer) => answer,
        Err(e) => {
            // Report the failure on the same channel so a listening chat view
            // resolves instead of waiting forever on a reply that never comes.
            let _ = app.emit(
                "ai://chat_stream",
                AskStreamChunk {
                    request_id,
                    delta: String::new(),
                    done: true,
                    error: Some(e.to_string()),
                },
            );
            return Err(e.to_string());
        }
    };

    if !answer.is_empty() {
        app.emit(
            "ai://chat_stream",
            AskStreamChunk {
                request_id: request_id.clone(),
                delta: answer,
                done: false,
                error: None,
            },
        )
        .map_err(|e| e.to_string())?;
    }

    app.emit(
        "ai://chat_stream",
        AskStreamChunk {
            request_id,
            delta: String::new(),
            done: true,
            error: None,
        },
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

// ─── Graph Registry ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_list_registered_graphs(
    state: State<'_, KnowledgeState>,
) -> Result<Vec<RegisteredGraph>, String> {
    let engine = state.handle().await?;

    let registry = engine.registry().await;
    Ok(registry.list().into_iter().cloned().collect())
}

#[tauri::command]
pub async fn ai_register_graph(
    state: State<'_, KnowledgeState>,
    name: String,
    path: String,
    graph_type: String,
) -> Result<(), String> {
    let engine = state.handle().await?;

    let gtype = match graph_type.as_str() {
        "reference" => GraphType::Reference,
        "ingested" => GraphType::Ingested,
        "archive" => GraphType::Archive,
        _ => GraphType::Primary,
    };

    let graph = RegisteredGraph {
        id: grafium_core::knowledge::GraphRegistry::generate_id(),
        name,
        path: PathBuf::from(path),
        graph_type: gtype,
        last_indexed: None,
        page_count: None,
        vector_count: None,
        cross_searchable: true,
        description: None,
    };

    let mut registry = engine.registry_mut().await;
    registry.register(graph).map_err(|e| e.to_string())
}

// ─── Schemas ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_list_schemas(app_state: State<'_, crate::AppState>) -> Result<Vec<Schema>, String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
    let manager =
        grafium_core::knowledge::SchemaManager::load(&graph.root_dir).map_err(|e| e.to_string())?;
    Ok(manager.list().into_iter().cloned().collect())
}

#[tauri::command]
pub async fn ai_save_schema(
    app_state: State<'_, crate::AppState>,
    schema: Schema,
) -> Result<(), String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
    let mut manager =
        grafium_core::knowledge::SchemaManager::load(&graph.root_dir).map_err(|e| e.to_string())?;
    manager.save_schema(schema).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ai_create_default_schemas(
    app_state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
    let mut manager =
        grafium_core::knowledge::SchemaManager::load(&graph.root_dir).map_err(|e| e.to_string())?;
    manager.create_defaults().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::PageBatchCursor;

    #[test]
    fn page_batch_cursor_streams_past_legacy_ten_thousand_cap() {
        let total_pages = 10_050usize;
        let batch_size = 128i64;
        let mut cursor = PageBatchCursor::new(batch_size);
        let mut requested_windows = Vec::new();
        let mut processed = 0usize;

        loop {
            let (limit, offset) = (cursor.batch_size, cursor.offset);
            requested_windows.push((limit, offset));
            let start = offset as usize;
            let fetched: Vec<usize> = if start >= total_pages {
                Vec::new()
            } else {
                let end = (start + limit as usize).min(total_pages);
                (start..end).collect()
            };

            let Some(batch) = cursor.accept_batch(fetched) else {
                break;
            };
            assert!(
                batch.len() <= batch_size as usize,
                "batch should stay memory-bounded"
            );
            processed += batch.len();
        }

        assert_eq!(processed, total_pages);
        assert!(
            requested_windows
                .iter()
                .any(|(_, offset)| *offset >= 10_000),
            "cursor should continue beyond the old 10k preload cap"
        );
    }

    #[test]
    fn page_batch_cursor_stays_finished_after_an_empty_batch() {
        // Once exhausted the cursor must not resume, or a cancelled or
        // completed index could be restarted by a late batch.
        let mut cursor = PageBatchCursor::new(10);
        assert!(cursor.accept_batch(vec![1, 2, 3]).is_some());
        assert!(cursor.accept_batch(Vec::<i32>::new()).is_none());
        assert!(
            cursor.accept_batch(vec![4, 5]).is_none(),
            "cursor resumed after reporting exhaustion"
        );
    }

}
