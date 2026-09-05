//! Tauri commands for the Knowledge Engine — AI, references, vector search, schemas.

use grafium_core::ai::config::{
    AiConfig, AiMode, CloudConfig, LocalConfig, LocalEmbeddingSettings, LocalLlmSettings,
    ProviderType,
};
use grafium_core::ai::references::PageReferencesMeta;
use grafium_core::ai::traits::SearchResult;
use grafium_core::knowledge::engine::{
    AskPhase, AskStreamEvent, HealthStatus, IndexStatus, Source,
};
use grafium_core::knowledge::registry::{GraphType, RegisteredGraph};
use grafium_core::knowledge::schemas::Schema;
use grafium_core::knowledge::KnowledgeEngine;
use grafium_core::model_library::LocalModelRef;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, State};
use tokio::sync::RwLock;

/// Shared state for the knowledge engine.
pub struct KnowledgeState {
    pub engine: Arc<RwLock<Option<KnowledgeEngine>>>,
    /// Per-request cancellation flags for in-flight streamed answers, so
    /// `ai_cancel_stream` can abort a slow local generation. Keyed by the
    /// UI-supplied `request_id`; entries are inserted when a stream starts and
    /// removed when it ends.
    pub cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

const AI_INDEX_BATCH_SIZE: i64 = 100;

/// Error message for commands that need semantic search (indexing, vector
/// search, "research this page" references) when the engine's LLM is fine
/// but no embedder is configured. Distinct from the plain "not ready at
/// all" case so the user gets an actionable explanation instead of a
/// generic "not ready" that reads like a bug even when the provider is
/// loaded and working fine for chat.
fn semantic_search_unavailable_error(engine: &KnowledgeEngine) -> String {
    if !engine.is_llm_ready() {
        "AI engine not ready — check configuration in Settings \u{2192} AI / Knowledge Engine."
            .to_string()
    } else {
        "This needs a search embedding model, but none is configured yet. If you're using the \
         Embedded (llama.cpp) provider, download a GGUF embedding model (e.g. \
         nomic-embed-text-v1.5-GGUF or bge-small-en-v1.5-gguf from Hugging Face) and select it \
         under \"Embedding Model File\" in Settings \u{2192} AI / Knowledge Engine — or switch \
         the local provider to Ollama / vLLM / OpenAI-compatible, or configure a cloud embedding \
         provider."
            .to_string()
    }
}

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

    fn next_batch<T, E>(
        &mut self,
        fetch: impl FnOnce(i64, i64) -> Result<Vec<T>, E>,
    ) -> Result<Option<Vec<T>>, E> {
        if self.finished {
            return Ok(None);
        }

        let batch = fetch(self.batch_size, self.offset)?;
        if batch.is_empty() {
            self.finished = true;
            return Ok(None);
        }

        self.offset += batch.len() as i64;
        Ok(Some(batch))
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
    /// GGUF embedding model file for the Embedded (llama.cpp) local
    /// provider — resolved against `ModelKind::Embedding` rather than
    /// `ModelKind::Llm`, so an "Embedded" provider can do semantic search /
    /// "Research this page" on its own. See `LocalEmbeddingSettings`.
    pub local_embedding_model_path: Option<String>,
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
    let guard = state.engine.read().await;
    if let Some(engine) = guard.as_ref() {
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
        local_embedding: LocalEmbeddingSettings {
            model_ref: LocalModelRef {
                model: payload.local_embedding_model_path,
            },
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

    // Reconfigure the engine.
    let mut guard = state.engine.write().await;
    if let Some(engine) = guard.as_mut() {
        engine.reconfigure(config).map_err(|e| e.to_string())?;
    } else {
        let app_data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let engine = KnowledgeEngine::new(&config_dir, config)
            .map_err(|e| e.to_string())?
            .with_models_root(app_data_dir);
        *guard = Some(engine);
    }

    Ok(())
}

// ─── Health ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_health_check(state: State<'_, KnowledgeState>) -> Result<HealthStatus, String> {
    let guard = state.engine.read().await;
    if let Some(engine) = guard.as_ref() {
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
pub async fn ai_index_status(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
) -> Result<IndexStatus, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    let snapshot = crate::current_graph_snapshot(&app, app_state.graph.as_ref())?;
    let graph_id = snapshot.root_dir.to_string_lossy().to_string();
    let graph = crate::open_graph_snapshot(&snapshot)?;

    engine
        .index_status(&graph.db, &graph_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ai_index_page(
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    page_id: String,
) -> Result<usize, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_ready() {
        return Err(semantic_search_unavailable_error(engine));
    }

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

#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexAllResult {
    /// Chunks embedded/updated across all pages.
    pub indexed_chunks: usize,
    /// Pages successfully processed (indexed or already up to date).
    pub pages_processed: usize,
    /// Pages that failed to index (errors were logged, not fatal).
    pub pages_failed: usize,
}

#[tauri::command]
pub async fn ai_index_all_pages(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
) -> Result<IndexAllResult, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_ready() {
        return Err(semantic_search_unavailable_error(engine));
    }

    let snapshot = crate::current_graph_snapshot(&app, app_state.graph.as_ref())?;
    let graph_id = snapshot.root_dir.to_string_lossy().to_string();
    let graph = crate::open_graph_snapshot(&snapshot)?;

    // Recover the hash cache from already-stored vectors so a restart doesn't
    // needlessly re-embed unchanged content.
    if let Err(e) = engine.restore_hash_cache(&graph_id).await {
        eprintln!("Failed to restore embedding hash cache: {e}");
    }

    let mut cursor = PageBatchCursor::new(AI_INDEX_BATCH_SIZE);
    let mut indexed_chunks = 0;
    let mut pages_processed = 0;
    let mut pages_failed = 0;

    while let Some(pages) = cursor.next_batch(|limit, offset| {
        graph
            .db
            .list_pages_window(limit, offset, false)
            .map_err(|e| e.to_string())
    })? {
        let mut pages_and_blocks = Vec::with_capacity(pages.len());
        for page in pages {
            let blocks = graph
                .db
                .list_blocks_for_page(&page.id)
                .map_err(|e| e.to_string())?;
            pages_and_blocks.push((page, blocks));
        }

        for (page, blocks) in &pages_and_blocks {
            match engine.index_page(page, blocks, &graph_id).await {
                Ok(count) => {
                    indexed_chunks += count;
                    pages_processed += 1;
                }
                Err(e) => {
                    pages_failed += 1;
                    eprintln!("Failed to index page '{}': {}", page.title, e);
                }
            }
        }
    }

    Ok(IndexAllResult {
        indexed_chunks,
        pages_processed,
        pages_failed,
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
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_ready() {
        return Err(semantic_search_unavailable_error(engine));
    }

    engine
        .search(&query, top_k.unwrap_or(10), graph_id.as_deref())
        .await
        .map_err(|e| e.to_string())
}

// ─── References ──────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_generate_references(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    page_id: String,
) -> Result<PageReferencesMeta, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_ready() {
        return Err(semantic_search_unavailable_error(engine));
    }

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

    // Mirrors `media_import_video`'s `media-import-progress` convention —
    // "Research this page" can take minutes on a local CPU-bound model, so
    // the UI needs live status instead of an unexplained "Analyzing..."
    // that reads like a hang.
    let mut emit_progress = move |message: &str| {
        let _ = app.emit("ai-reference-progress", message);
    };

    engine
        .generate_references(
            &page_id,
            &page_title,
            &blocks_data,
            &graph_id,
            &mut emit_progress,
        )
        .await
        .map_err(|e| e.to_string())
}

/// Analyzes arbitrary selected text (one or more selected blocks'
/// concatenated content) and returns a short AI summary + hashtag-style
/// topic tags — the same shape/prompt used for "Analyze this Page" and
/// media-import summaries, just applied to a text selection instead of a
/// whole page. The caller (`PageContent.svelte`'s "Analyze Selected"
/// action) inserts the result as a new block right after the selection.
#[tauri::command(rename_all = "camelCase")]
pub async fn ai_summarize_selection(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    text: String,
    title: Option<String>,
) -> Result<grafium_core::ai::references::PageSummary, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(
            "AI engine not ready — check configuration in Settings \u{2192} AI / Knowledge Engine."
                .to_string(),
        );
    }

    let mut emit_progress = move |message: &str| {
        let _ = app.emit("ai-selection-summary-progress", message);
    };
    let title = title.unwrap_or_else(|| "Selected text".to_string());

    engine
        .summarize_text(&title, &text, &mut emit_progress)
        .await
        .map_err(|e| e.to_string())
}

/// Actually researches `title`/`seed_text` on the open internet: plans
/// search queries, searches the web (a plain HTML scrape of Brave's
/// results page — no paid search API/keys involved), reads the most
/// relevant results, and synthesizes a topic-by-topic summary with inline
/// `[n]` citation markers pointing at real, clickable source URLs. Unlike
/// `ai_generate_references`/`ai_summarize_selection`, this can surface
/// information not already present anywhere in the user's graph, so the
/// result always carries its sources for the user to verify. Works with
/// whatever LLM provider is configured — local (embedded llama.cpp,
/// Ollama) or a remote OpenAI-compatible endpoint (e.g. vLLM reachable
/// over Tailscale/LAN) — since it only needs `engine.is_llm_ready()`, not
/// an embedder/vector store.
#[tauri::command(rename_all = "camelCase")]
pub async fn ai_research_web(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    title: String,
    seed_text: String,
) -> Result<grafium_core::ai::web_research::WebResearchResult, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(
            "AI engine not ready — check configuration in Settings \u{2192} AI / Knowledge Engine."
                .to_string(),
        );
    }

    let mut emit_progress = move |message: &str| {
        let _ = app.emit("ai-web-research-progress", message);
    };

    engine
        .research_web(&title, &seed_text, &mut emit_progress)
        .await
        .map_err(|e| e.to_string())
}

/// Wraps the first verbatim, whole-word occurrence of each term found in
/// `content` with `[[wiki-link]]` syntax (optionally substituting a
/// `qualified` disambiguation phrase — see
/// [`grafium_core::parser::TagTerm`]). Thin synchronous wrapper around
/// [`grafium_core::parser::wrap_known_terms_as_links`] — kept as a single
/// shared entry point so "Analyze Selected" and any other AI-tagging
/// caller wrap terms identically instead of re-implementing matching
/// logic per call site.
#[tauri::command(rename_all = "camelCase")]
pub fn text_wrap_known_terms(content: String, terms: Vec<grafium_core::parser::TagTerm>) -> String {
    grafium_core::parser::wrap_known_terms_as_links(&content, &terms)
}

/// Inserts an AI-generated page summary (title answer + one paragraph per
/// topic) as a new block at the very top of the page (right after the
/// title), and wraps each topic's `tags` in place — as `[[wiki-link]]`s,
/// substituting any `qualified` disambiguation phrase — across the page's
/// existing block content wherever those terms already appear verbatim.
/// Used by the "Insert into page" button in `ReferencePanel.svelte`, which
/// only fires on explicit user action so repeated "Research this page"
/// runs never duplicate content.
#[tauri::command(rename_all = "camelCase")]
pub fn ai_insert_page_summary(
    app_state: State<'_, crate::AppState>,
    page_id: String,
    title_answer: Option<String>,
    topics: Vec<grafium_core::ai::references::TopicSummary>,
) -> Result<(), String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;

    // Collect every topic's tags up front so the in-place wiki-linking pass
    // below sees them regardless of how the blocks get nested.
    let mut all_tags: Vec<grafium_core::parser::TagTerm> = Vec::new();
    for topic in &topics {
        for tag in &topic.tags {
            if !all_tags
                .iter()
                .any(|t| t.term.eq_ignore_ascii_case(&tag.term))
            {
                all_tags.push(tag.clone());
            }
        }
    }

    // Build a real block tree rather than one block holding headings and
    // prose as flat text. Grafium is an outliner: a heading only "owns" the
    // paragraphs beneath it when they are its children, so a flat summary
    // leaves every topic heading structurally unrelated to its own body —
    // backlinks, block refs, and collapsing all treat them as unconnected
    // siblings.
    let root = graph
        .insert_block_at_top(
            &page_id,
            title_answer.as_deref().unwrap_or("Summary").trim(),
        )
        .map_err(|e| e.to_string())?;

    for (index, topic) in topics.iter().enumerate() {
        let heading = graph
            .create_block(
                &page_id,
                Some(&root.id),
                index as i32,
                &format!("### {}", topic.topic.trim()),
                grafium_core::models::BlockType::Text,
                serde_json::json!({}),
            )
            .map_err(|e| e.to_string())?;

        let body = topic.summary.trim();
        if !body.is_empty() {
            graph
                .create_block(
                    &page_id,
                    Some(&heading.id),
                    0,
                    body,
                    grafium_core::models::BlockType::Text,
                    serde_json::json!({}),
                )
                .map_err(|e| e.to_string())?;
        }
    }

    if !all_tags.is_empty() {
        let blocks = graph
            .db
            .list_blocks_for_page(&page_id)
            .map_err(|e| e.to_string())?;
        for block in blocks {
            let wrapped =
                grafium_core::parser::wrap_known_terms_as_links(&block.content, &all_tags);
            if wrapped != block.content {
                graph
                    .update_block(&block.id, &wrapped, None)
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(())
}

// ─── RAG / Ask ───────────────────────────────────────────────────────────────

/// A structured citation surfaced to the UI so it can render source chips
/// and navigate to the originating page/block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceDto {
    pub index: usize,
    pub page_id: String,
    pub page_title: String,
    pub block_id: String,
    pub date: Option<String>,
}

impl From<Source> for SourceDto {
    fn from(s: Source) -> Self {
        SourceDto {
            index: s.index,
            page_id: s.page_id,
            page_title: s.page_title,
            block_id: s.block_id,
            date: s.date,
        }
    }
}

/// Answer plus structured sources returned by `ai_ask`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskResult {
    pub answer: String,
    pub sources: Vec<SourceDto>,
}

#[tauri::command]
pub async fn ai_ask(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    question: String,
    graph_id: Option<String>,
) -> Result<AskResult, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(
            "AI chat isn't ready — configure and save a Local or Cloud provider in Settings \
             \u{2192} AI / Knowledge Engine first."
                .to_string(),
        );
    }

    let snapshot = crate::current_graph_snapshot(&app, app_state.graph.as_ref())?;
    let resolved_graph_id =
        graph_id.unwrap_or_else(|| snapshot.root_dir.to_string_lossy().to_string());
    let graph = crate::open_graph_snapshot(&snapshot)?;

    let response = engine
        .ask(&graph.db, &question, Some(resolved_graph_id.as_str()))
        .await
        .map_err(|e| e.to_string())?;

    Ok(AskResult {
        answer: response.answer,
        sources: response.sources.into_iter().map(SourceDto::from).collect(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskStreamChunk {
    pub request_id: String,
    pub delta: String,
    /// The current answering phase (`retrieving`, `processing_prompt`,
    /// `thinking`, `generating`), when this chunk reports a phase transition.
    /// `None` for a pure text delta or the terminal `done` event. Drives the
    /// UI's evidence-based status indicator; reasoning is never sent as
    /// `delta`, only reflected as the `thinking` phase.
    pub phase: Option<String>,
    pub done: bool,
    pub error: Option<String>,
}

/// Emitted once per request on `ai://chat_sources`, carrying the structured
/// citations for the answer. Kept as a separate event so the existing
/// `AskStreamChunk` shape on `ai://chat_stream` is unchanged and backward
/// compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskSourcesPayload {
    pub request_id: String,
    pub sources: Vec<SourceDto>,
}

#[tauri::command]
pub async fn ai_ask_stream(
    state: State<'_, KnowledgeState>,
    app: tauri::AppHandle,
    app_state: State<'_, crate::AppState>,
    question: String,
    graph_id: Option<String>,
    request_id: String,
) -> Result<(), String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(
            "AI chat isn't ready — configure and save a Local or Cloud provider in Settings \
             \u{2192} AI / Knowledge Engine first."
                .to_string(),
        );
    }

    let snapshot = crate::current_graph_snapshot(&app, app_state.graph.as_ref())?;
    let resolved_graph_id =
        graph_id.unwrap_or_else(|| snapshot.root_dir.to_string_lossy().to_string());
    let graph = crate::open_graph_snapshot(&snapshot)?;

    // Register a cancellation flag so `ai_cancel_stream` can stop a slow local
    // generation instead of leaving the user staring at a frozen pane.
    let cancel = Arc::new(AtomicBool::new(false));
    if let Ok(mut map) = state.cancels.lock() {
        map.insert(request_id.clone(), cancel.clone());
    }

    // Forward real token deltas and phase transitions as they happen. Phase
    // events let the UI show *what* the model is doing (and prove it's alive);
    // reasoning is surfaced only as the `thinking` phase, never as `delta`.
    let app_for_events = app.clone();
    let rid = request_id.clone();
    let mut on_event = move |ev: AskStreamEvent<'_>| {
        let (delta, phase) = match ev {
            AskStreamEvent::Delta(d) => (d.to_string(), None),
            AskStreamEvent::Phase(p) => (String::new(), Some(p.as_str().to_string())),
        };
        let _ = app_for_events.emit(
            "ai://chat_stream",
            AskStreamChunk {
                request_id: rid.clone(),
                delta,
                phase,
                done: false,
                error: None,
            },
        );
    };

    let outcome = engine
        .ask_stream(
            &graph.db,
            &question,
            Some(resolved_graph_id.as_str()),
            Some(cancel),
            &mut on_event,
        )
        .await;

    // Deregister the cancel flag regardless of outcome.
    if let Ok(mut map) = state.cancels.lock() {
        map.remove(&request_id);
    }

    let outcome = outcome.map_err(|e| e.to_string())?;

    // Emit the structured citations now that the answer is complete.
    app.emit(
        "ai://chat_sources",
        AskSourcesPayload {
            request_id: request_id.clone(),
            sources: outcome.sources.into_iter().map(SourceDto::from).collect(),
        },
    )
    .map_err(|e| e.to_string())?;

    // If the model produced only reasoning (budget exhausted with no answer),
    // show the explanatory message in place of an answer rather than an empty
    // pane or raw chain-of-thought.
    if let Some(message) = outcome.trailing_message {
        app.emit(
            "ai://chat_stream",
            AskStreamChunk {
                request_id: request_id.clone(),
                delta: message,
                phase: None,
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
            phase: None,
            done: true,
            error: None,
        },
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Cancel an in-flight streamed answer started by `ai_ask_stream`. Flips the
/// request's cancellation flag; the local generation loop checks it and stops,
/// returning what it has so far. A no-op if the request already finished.
#[tauri::command]
pub async fn ai_cancel_stream(
    state: State<'_, KnowledgeState>,
    request_id: String,
) -> Result<(), String> {
    if let Ok(map) = state.cancels.lock() {
        if let Some(flag) = map.get(&request_id) {
            flag.store(true, Ordering::Relaxed);
        }
    }
    Ok(())
}

// ─── Graph Registry ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn ai_list_registered_graphs(
    state: State<'_, KnowledgeState>,
) -> Result<Vec<RegisteredGraph>, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

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
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

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

        while let Some(batch) = cursor
            .next_batch(|limit, offset| {
                requested_windows.push((limit, offset));
                let start = offset as usize;
                if start >= total_pages {
                    return Ok::<Vec<usize>, &'static str>(Vec::new());
                }
                let end = (start + limit as usize).min(total_pages);
                Ok((start..end).collect())
            })
            .expect("cursor should paginate cleanly")
        {
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
