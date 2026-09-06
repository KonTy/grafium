//! Tauri commands for the Knowledge Engine — AI, references, vector search, schemas.

use grafium_core::ai::config::{
    AiConfig, AiMode, CloudConfig, LocalConfig, LocalEmbeddingSettings, LocalLlmSettings,
    ProviderType,
};
use grafium_core::ai::references::PageReferencesMeta;
use grafium_core::ai::traits::SearchResult;
use grafium_core::cancel::CancellationToken;
use grafium_core::knowledge::engine::HealthStatus;
use grafium_core::knowledge::registry::{GraphType, RegisteredGraph};
use grafium_core::knowledge::schemas::Schema;
use grafium_core::knowledge::KnowledgeEngine;
use grafium_core::model_library::LocalModelRef;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, State};
use tokio::sync::RwLock;

/// Shared state for the knowledge engine.
pub struct KnowledgeState {
    pub engine: Arc<RwLock<Option<KnowledgeEngine>>>,
    /// Registry of cancellation tokens for in-flight AI operations, keyed
    /// by the caller-supplied `operationId`. Populated when
    /// `ai_generate_references` / `ai_summarize_selection` /
    /// `ai_research_web` starts, cleared when it finishes. Drained by
    /// `ai_cancel_operation` to signal the running command it should
    /// stop and by [`LlmProvider::abort_in_flight`] to hard-kill the
    /// local worker child (the only way to interrupt a llama.cpp
    /// generation in progress).
    ///
    /// A plain `std::sync::Mutex` (not `tokio::sync::Mutex`) is
    /// intentional: the critical section is a one-op HashMap update, so
    /// blocking briefly is fine and *not* holding it across `.await`
    /// avoids the risk of deadlocking with the engine's own RwLock. All
    /// callers `.lock()` and immediately release before doing any await
    /// work.
    pub cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

const AI_INDEX_BATCH_SIZE: i64 = 100;

/// Error message for commands that need the chat LLM ready (Ask, Analyze
/// Selection, Web Research, ...) when it isn't. Includes the *actual*
/// reason from `KnowledgeEngine::llm_load_error` when there is one (e.g.
/// "doesn't fit in available RAM... try one of: ...") — the embedded local
/// provider deliberately never silently substitutes a different model
/// than the one configured (see `LocalLlm::from_settings`'s docs), so
/// surfacing why here, instead of a bare generic message, is how the user
/// finds out what to actually do about it instead of guessing.
fn llm_not_ready_error(engine: &KnowledgeEngine) -> String {
    match engine.llm_load_error() {
        Some(reason) => format!(
            "AI engine not ready: {reason}\n\n(Settings \u{2192} AI / Knowledge Engine)"
        ),
        None => "AI engine not ready — check configuration in Settings \u{2192} AI / Knowledge \
                  Engine."
            .to_string(),
    }
}

/// Error message for commands that need semantic search (indexing, vector
/// search, "research this page" references) when the engine's LLM is fine
/// but no embedder is configured. Distinct from the plain "not ready at
/// all" case so the user gets an actionable explanation instead of a
/// generic "not ready" that reads like a bug even when the provider is
/// loaded and working fine for chat.
fn semantic_search_unavailable_error(engine: &KnowledgeEngine) -> String {
    if !engine.is_llm_ready() {
        llm_not_ready_error(engine)
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
    /// Context window size in tokens for the embedded (llama.cpp) local
    /// LLM. `None` uses the model's own trained context length. See
    /// `LocalLlmSettings::context_size`.
    pub local_context_size: Option<u32>,
    /// Number of transformer layers to offload to the GPU for the
    /// embedded (llama.cpp) local LLM. `None` offloads every layer.
    /// See `LocalLlmSettings::gpu_layers`.
    pub local_gpu_layers: Option<u32>,
    /// Whether llama.cpp is allowed to memory-map the model file for
    /// the embedded local LLM. `None` uses the built-in heuristic
    /// (mmap OFF for CPU-only, ON for GPU); `Some(false)` disables
    /// mmap outright as a durable fix for repeated SIGBUS crashes on
    /// unreliable storage. See `LocalLlmSettings::use_mmap`.
    pub local_use_mmap: Option<bool>,
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
            context_size: payload.local_context_size,
            gpu_layers: payload.local_gpu_layers,
            use_mmap: payload.local_use_mmap,
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
            llm_load_error: None,
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

#[tauri::command]
pub async fn ai_index_all_pages(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
) -> Result<usize, String> {
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
    let mut cursor = PageBatchCursor::new(AI_INDEX_BATCH_SIZE);
    let mut total = 0;

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
                Ok(count) => total += count,
                Err(e) => {
                    eprintln!("Failed to index page '{}': {}", page.title, e);
                }
            }
        }
    }

    Ok(total)
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

/// RAII helper for the [`KnowledgeState::cancellations`] map: registers a
/// token on construction and de-registers on drop. Guarantees the map
/// stays clean even if the command errors out or is cancelled itself,
/// because Rust runs `Drop` unconditionally at scope exit — so we don't
/// leak dead tokens the next `cancel_operation` would then target.
struct CancelGuard {
    id: Option<String>,
    slot: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl CancelGuard {
    fn install(
        slot: Arc<Mutex<HashMap<String, CancellationToken>>>,
        id: Option<String>,
    ) -> (Self, CancellationToken) {
        let token = CancellationToken::new();
        if let Some(ref id) = id {
            slot.lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(id.clone(), token.clone());
        }
        (Self { id, slot }, token)
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if let Some(id) = &self.id {
            self.slot
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(id);
        }
    }
}

/// Signal cancellation of a running AI operation the frontend previously
/// launched with an `operationId` — currently one of
/// [`ai_generate_references`], [`ai_summarize_selection`],
/// [`ai_research_web`], which all accept an optional `operationId` param.
/// This looks the id up in [`KnowledgeState::cancellations`], flips its
/// atomic latch, and hard-kills any in-flight local LLM worker so the
/// C++ inference loop actually stops (it checks nothing between tokens
/// otherwise). A no-op if the id doesn't match a live operation — safe
/// to call from a slow "did the user press it twice" render.
#[tauri::command(rename_all = "camelCase")]
pub async fn ai_cancel_operation(
    state: State<'_, KnowledgeState>,
    operation_id: String,
) -> Result<(), String> {
    let token = state
        .cancellations
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&operation_id)
        .cloned();
    if let Some(token) = token {
        token.cancel();
    }
    // Also proactively hard-kill any current LLM worker. Cancelling the
    // token alone is co-operative — the running command will honor it at
    // its next `select!` branch — but the worker process's llama.cpp
    // generation loop checks nothing between tokens, so without the
    // kill the user's "cancel" would only take effect after the current
    // generation finished naturally (i.e. never, for the hung-model
    // case). See `LlmProvider::abort_in_flight` for the full rationale.
    let engine_guard = state.engine.read().await;
    if let Some(engine) = engine_guard.as_ref() {
        if let Some(llm) = engine.llm_provider() {
            llm.abort_in_flight();
        }
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn ai_generate_references(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    page_id: String,
    operation_id: Option<String>,
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

    // RAII: this registers the cancel token in the shared map now and
    // removes it when the guard drops (either at the end of this fn or
    // on any early return). Prevents the map from accumulating dead
    // entries that would then confuse the next `ai_cancel_operation`.
    let (_guard, cancel) = CancelGuard::install(state.cancellations.clone(), operation_id);

    engine
        .generate_references(
            &page_id,
            &page_title,
            &blocks_data,
            &graph_id,
            &mut emit_progress,
            &cancel,
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
    operation_id: Option<String>,
) -> Result<grafium_core::ai::references::PageSummary, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(llm_not_ready_error(engine));
    }

    let mut emit_progress = move |message: &str| {
        let _ = app.emit("ai-selection-summary-progress", message);
    };
    let title = title.unwrap_or_else(|| "Selected text".to_string());

    let (_guard, cancel) = CancelGuard::install(state.cancellations.clone(), operation_id);

    engine
        .summarize_text(&title, &text, &mut emit_progress, &cancel)
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
    operation_id: Option<String>,
) -> Result<grafium_core::ai::web_research::WebResearchResult, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(llm_not_ready_error(engine));
    }

    let mut emit_progress = move |message: &str| {
        let _ = app.emit("ai-web-research-progress", message);
    };

    let (_guard, cancel) = CancelGuard::install(state.cancellations.clone(), operation_id);

    engine
        .research_web(&title, &seed_text, &mut emit_progress, &cancel)
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
pub fn text_wrap_known_terms(
    content: String,
    terms: Vec<grafium_core::parser::TagTerm>,
) -> String {
    grafium_core::parser::wrap_known_terms_as_links(&content, &terms)
}

/// Details of a single "wrap known tags as `[[wiki-link]]`s" change made
/// during a summary insert — enough to reverse or reapply it without
/// re-parsing anything, so Ctrl-Z after "Insert into page" can restore
/// each block's exact previous content and Ctrl-Y can put the wrapped
/// version back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryWrapChange {
    pub block_id: String,
    pub previous_content: String,
    pub new_content: String,
}

/// What `ai_insert_page_summary` did — returned so the frontend can push
/// a matching undo action onto its stack (see `undoStack.ts`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiInsertSummaryResult {
    /// The id of the freshly-created block that holds the summary text.
    pub inserted_block_id: String,
    /// The exact markdown that went into that block, kept so redo can
    /// recreate it verbatim (topic ordering, formatting, tag-wrapping).
    pub inserted_content: String,
    /// The anchor block id the summary was placed after, or `None` when
    /// it landed at the top of the page. Used by redo to put the block
    /// back in roughly the same spot.
    pub inserted_after_block_id: Option<String>,
    /// Blocks whose existing content was rewritten to embed the new
    /// `[[wiki-link]]`s. Both previous and new content are captured so
    /// undo can restore, and redo can reapply.
    pub wrap_changes: Vec<SummaryWrapChange>,
}

/// Inserts an AI-generated page summary (title answer + one paragraph per
/// topic) as a new block on the page, and wraps each topic's `tags` in
/// place — as `[[wiki-link]]`s, substituting any `qualified` disambiguation
/// phrase — across the page's existing block content wherever those terms
/// already appear verbatim. Used by the "Insert into page" button in
/// `ReferencePanel.svelte`, which only fires on explicit user action so
/// repeated "Research this page" runs never duplicate content.
///
/// When `after_block_id` is provided (the id of the block the user last
/// had a caret in), the summary is inserted immediately after that block
/// as its next sibling — so the summary lands "at the cursor" wherever
/// the user was reading. When it's `None` or an empty string, the summary
/// falls back to the top of the page.
///
/// Returns an [`AiInsertSummaryResult`] describing everything that
/// changed so the frontend can push a corresponding undo action —
/// Ctrl-Z should reverse the whole operation, including the tag-wrap
/// rewrites of unrelated pre-existing blocks.
#[tauri::command(rename_all = "camelCase")]
pub fn ai_insert_page_summary(
    app_state: State<'_, crate::AppState>,
    page_id: String,
    title_answer: Option<String>,
    topics: Vec<grafium_core::ai::references::TopicSummary>,
    after_block_id: Option<String>,
) -> Result<AiInsertSummaryResult, String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;

    let mut summary_text = String::new();
    if let Some(answer) = title_answer {
        if !answer.trim().is_empty() {
            summary_text.push_str(answer.trim());
            summary_text.push_str("\n\n");
        }
    }
    // One heading + paragraph per topic, same shape as the media-import
    // transcript notes, so a multi-subject "Research this page" summary
    // (e.g. a long podcast transcript) keeps every topic distinguishable.
    let mut all_tags: Vec<grafium_core::parser::TagTerm> = Vec::new();
    for (i, topic) in topics.iter().enumerate() {
        if i > 0 {
            summary_text.push_str("\n\n");
        }
        summary_text.push_str(&format!("### {}\n\n", topic.topic.trim()));
        summary_text.push_str(topic.summary.trim());
        for tag in &topic.tags {
            if !all_tags
                .iter()
                .any(|t| t.term.eq_ignore_ascii_case(&tag.term))
            {
                all_tags.push(tag.clone());
            }
        }
    }

    // Normalize an empty-string anchor id to None so callers that always
    // pass a value (e.g. TypeScript `lastFocusedBlockId ?? ""`) still get
    // the top-of-page fallback when there was no caret.
    let anchor = after_block_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let anchor_used_str = anchor.map(String::from);
    let mut inserted_after_block_id = anchor_used_str.clone();

    let inserted_block = match anchor {
        Some(anchor_id) => match graph.insert_block_after(&page_id, anchor_id, &summary_text) {
            Ok(b) => b,
            Err(e) => {
                // Stale anchor (deleted, cross-page). Preserve the
                // (expensive) summary by falling back to top-of-page,
                // and clear `inserted_after_block_id` in the return so
                // redo doesn't try to re-anchor to the stale id.
                tracing::warn!(
                    "ai_insert_page_summary: insert_block_after failed ({}); falling back to top of page",
                    e
                );
                inserted_after_block_id = None;
                graph
                    .insert_block_at_top(&page_id, &summary_text)
                    .map_err(|e| e.to_string())?
            }
        },
        None => graph
            .insert_block_at_top(&page_id, &summary_text)
            .map_err(|e| e.to_string())?,
    };

    let mut wrap_changes: Vec<SummaryWrapChange> = Vec::new();
    if !all_tags.is_empty() {
        let blocks = graph
            .db
            .list_blocks_for_page(&page_id)
            .map_err(|e| e.to_string())?;
        for block in blocks {
            // Never rewrap the block we just created — its content is
            // already the finished summary text.
            if block.id == inserted_block.id {
                continue;
            }
            let wrapped =
                grafium_core::parser::wrap_known_terms_as_links(&block.content, &all_tags);
            if wrapped != block.content {
                graph
                    .update_block(&block.id, &wrapped, None)
                    .map_err(|e| e.to_string())?;
                wrap_changes.push(SummaryWrapChange {
                    block_id: block.id,
                    previous_content: block.content,
                    new_content: wrapped,
                });
            }
        }
    }

    Ok(AiInsertSummaryResult {
        inserted_block_id: inserted_block.id,
        inserted_content: summary_text,
        inserted_after_block_id,
        wrap_changes,
    })
}

/// Undoes a previous `ai_insert_page_summary` call: deletes the summary
/// block and restores each block whose content was rewrapped back to its
/// pre-wrap text. Silent no-op for a block that has since been deleted
/// (so a Ctrl-Z after a user manually deletes the summary block and
/// then some wrapped block still restores the survivors instead of
/// erroring out).
#[tauri::command(rename_all = "camelCase")]
pub fn ai_undo_summary_insert(
    app_state: State<'_, crate::AppState>,
    inserted_block_id: String,
    wrap_changes: Vec<SummaryWrapChange>,
) -> Result<(), String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;

    // Best-effort delete: if the user already deleted the summary block
    // themselves, don't fail the whole undo — just move on to the wrap
    // restorations.
    if let Err(e) = graph.delete_block(&inserted_block_id) {
        tracing::warn!(
            "ai_undo_summary_insert: delete_block({}) failed ({}); continuing with wrap restore",
            inserted_block_id,
            e
        );
    }

    for change in &wrap_changes {
        // Same best-effort logic per wrap change: skip blocks that no
        // longer exist rather than aborting the whole undo.
        if let Err(e) = graph.update_block(&change.block_id, &change.previous_content, None) {
            tracing::warn!(
                "ai_undo_summary_insert: restore of {} failed ({}); skipping",
                change.block_id,
                e
            );
        }
    }

    Ok(())
}

/// Redoes a previously-undone summary insert: recreates the summary
/// block (after its original anchor if the anchor still exists, else at
/// the top of the page) and reapplies each wrap change so the on-page
/// `[[wiki-link]]`s come back. Returns a fresh
/// [`AiInsertSummaryResult`] carrying the new summary block's id, so
/// the undo stack can flip it back into an undo entry.
#[tauri::command(rename_all = "camelCase")]
pub fn ai_reapply_summary_insert(
    app_state: State<'_, crate::AppState>,
    page_id: String,
    inserted_content: String,
    inserted_after_block_id: Option<String>,
    wrap_changes: Vec<SummaryWrapChange>,
) -> Result<AiInsertSummaryResult, String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;

    let anchor = inserted_after_block_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let mut effective_anchor = anchor.map(String::from);
    let inserted_block = match anchor {
        Some(anchor_id) => match graph.insert_block_after(&page_id, anchor_id, &inserted_content) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    "ai_reapply_summary_insert: insert_block_after failed ({}); falling back to top of page",
                    e
                );
                effective_anchor = None;
                graph
                    .insert_block_at_top(&page_id, &inserted_content)
                    .map_err(|e| e.to_string())?
            }
        },
        None => graph
            .insert_block_at_top(&page_id, &inserted_content)
            .map_err(|e| e.to_string())?,
    };

    for change in &wrap_changes {
        if let Err(e) = graph.update_block(&change.block_id, &change.new_content, None) {
            tracing::warn!(
                "ai_reapply_summary_insert: reapply of {} failed ({}); skipping",
                change.block_id,
                e
            );
        }
    }

    Ok(AiInsertSummaryResult {
        inserted_block_id: inserted_block.id,
        inserted_content,
        inserted_after_block_id: effective_anchor,
        wrap_changes,
    })
}

// ─── RAG / Ask ───────────────────────────────────────────────────────────────

/// Gathers a short block of graph-structure facts (page/link counts, the
/// most-connected page titles) from the currently open graph's live
/// database. The knowledge engine's RAG context is chunk-similarity only
/// (from the separate vectors.db), so without this the assistant has no way
/// to answer graph-shaped questions like "how many notes do I have" or
/// "what's my most connected topic".
fn build_graph_context(app_state: &crate::AppState) -> Option<String> {
    let graph = app_state.graph.lock().ok()?;
    let page_count = graph.db.count_pages().ok()?;
    let link_count = graph.db.count_links().ok()?;
    let (mut nodes, _edges) = graph.db.graph_data(None, 8).ok()?;
    nodes.sort_by(|a, b| b.2.cmp(&a.2));

    let mut out = format!(
        "The user's current graph has {} page(s) and {} link(s) between them.",
        page_count, link_count
    );
    if !nodes.is_empty() {
        out.push_str(" Most-connected pages: ");
        let top: Vec<String> = nodes
            .iter()
            .take(8)
            .map(|(_, title, degree)| format!("\"{}\" ({} connections)", title, degree))
            .collect();
        out.push_str(&top.join(", "));
        out.push('.');
    }
    Some(out)
}

#[tauri::command]
pub async fn ai_ask(
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    question: String,
    graph_id: Option<String>,
) -> Result<String, String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(llm_not_ready_error(engine));
    }

    let graph_context = build_graph_context(&app_state);
    engine
        .ask(&question, graph_id.as_deref(), graph_context.as_deref())
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

#[tauri::command]
pub async fn ai_ask_stream(
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    app: tauri::AppHandle,
    question: String,
    graph_id: Option<String>,
    request_id: String,
) -> Result<(), String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(llm_not_ready_error(engine));
    }

    let graph_context = build_graph_context(&app_state);
    let answer = engine
        .ask(&question, graph_id.as_deref(), graph_context.as_deref())
        .await
        .map_err(|e| e.to_string())?;

    // Stream by small chunks to give responsive UI updates across all providers.
    const CHUNK_SIZE: usize = 24;
    if answer.is_empty() {
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
        return Ok(());
    }

    let mut buf = String::new();
    for ch in answer.chars() {
        buf.push(ch);
        if buf.chars().count() >= CHUNK_SIZE {
            app.emit(
                "ai://chat_stream",
                AskStreamChunk {
                    request_id: request_id.clone(),
                    delta: std::mem::take(&mut buf),
                    done: false,
                    error: None,
                },
            )
            .map_err(|e| e.to_string())?;
            tokio::time::sleep(std::time::Duration::from_millis(12)).await;
        }
    }

    if !buf.is_empty() {
        app.emit(
            "ai://chat_stream",
            AskStreamChunk {
                request_id: request_id.clone(),
                delta: buf,
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
