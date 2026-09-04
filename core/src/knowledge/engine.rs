//! Knowledge Engine — the orchestrator that ties AI, vectors, and graphs together.
//!
//! This is the main entry point for all knowledge-OS operations.
//! It manages provider lifecycle, coordinates embeddings, and dispatches queries.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::ai::config::{AiConfig, AiMode, ProviderType};
use crate::ai::embeddings::EmbeddingPipeline;
use crate::ai::providers::anthropic::AnthropicLlm;
use crate::ai::providers::ollama::{OllamaEmbedder, OllamaLlm};
use crate::ai::providers::openai::{OpenAiEmbedder, OpenAiLlm};
use crate::ai::providers::openai_compatible::{OpenAiCompatibleEmbedder, OpenAiCompatibleLlm};
use crate::ai::references::{PageReferencesMeta, PageSummary, ReferenceEngine};
use crate::ai::traits::{Embedder, LlmProvider, SearchResult, VectorStore};
use crate::error::{CoreError, Result};
use crate::knowledge::registry::GraphRegistry;
use crate::knowledge::vector_store::SqliteVectorStore;
use crate::models::{Block, Page};

/// A fused retrieval hit: a primary block plus the page/date metadata and
/// expansion context needed to build a dated, cited prompt.
#[derive(Debug, Clone)]
pub struct RetrievedHit {
    pub block_id: String,
    pub page_id: String,
    pub page_title: String,
    pub content: String,
    /// Best-known date for the hit, in epoch-ms. For journal pages this is
    /// parsed from the page title; otherwise it falls back to the block's
    /// `created_at`.
    pub date_ms: Option<i64>,
    pub is_journal: bool,
    /// Fused RRF score.
    pub score: f64,
    /// Ancestor blocks (outermost first) for small-to-big context.
    pub parents: Vec<crate::knowledge::retrieval::ContextItem>,
    /// Immediate children for small-to-big detail.
    pub children: Vec<crate::knowledge::retrieval::ContextItem>,
}

/// The Knowledge Engine — main orchestrator for all AI/knowledge operations.
pub struct KnowledgeEngine {
    config: AiConfig,
    llm: Option<Box<dyn LlmProvider>>,
    embedder: Option<Box<dyn Embedder>>,
    vector_store: Option<Arc<dyn VectorStore>>,
    pipeline: RwLock<EmbeddingPipeline>,
    reference_engine: ReferenceEngine,
    registry: RwLock<GraphRegistry>,
    data_dir: PathBuf,
    /// Where `model_library::default_models_dir` looks for locally-managed
    /// model files (embedded LLM GGUFs) when `LocalConfig::models_dir` is
    /// unset. Defaults to `data_dir` (old behaviour), but callers that keep
    /// `data_dir` scoped to a feature-specific subfolder (e.g.
    /// `<app_data_dir>/knowledge`, so vectors.db/graph_registry.json don't
    /// share a folder with other data) should override this via
    /// [`Self::with_models_root`] to the actual app data root, so "leave
    /// Models Directory blank" resolves to the *same* shared folder the
    /// Whisper settings default to as well — one shared models folder
    /// instead of two different feature-namespaced ones a user would never
    /// guess at.
    models_root: PathBuf,
}

impl KnowledgeEngine {
    /// Create a new Knowledge Engine.
    /// `data_dir` is the app's data directory where vector store and registry live.
    pub fn new(data_dir: &Path, config: AiConfig) -> Result<Self> {
        let registry_path = data_dir.join("graph_registry.json");
        let registry = GraphRegistry::load(&registry_path)?;

        let pipeline = EmbeddingPipeline::new(config.embedding.clone());
        let reference_engine = ReferenceEngine::new(config.references.clone());

        let mut engine = Self {
            config: config.clone(),
            llm: None,
            embedder: None,
            vector_store: None,
            pipeline: RwLock::new(pipeline),
            reference_engine,
            registry: RwLock::new(registry),
            data_dir: data_dir.to_path_buf(),
            models_root: data_dir.to_path_buf(),
        };

        if config.enabled {
            engine.initialize_providers()?;
        }

        Ok(engine)
    }

    /// Overrides where the default (unconfigured) local models directory is
    /// resolved from — see the `models_root` field doc for why a caller
    /// would want this to differ from `data_dir`. Chainable so it reads
    /// naturally right after `new(...)` at the call site.
    pub fn with_models_root(mut self, root: PathBuf) -> Self {
        self.models_root = root;
        self
    }

    /// Initialize AI providers based on config.
    fn initialize_providers(&mut self) -> Result<()> {
        match &self.config.mode {
            AiMode::Local => {
                if let Some(local) = &self.config.local {
                    match local.provider {
                        ProviderType::Ollama => {
                            self.llm =
                                Some(Box::new(OllamaLlm::new(&local.base_url, &local.llm_model)));
                            self.embedder = Some(Box::new(OllamaEmbedder::new(
                                &local.base_url,
                                &local.embedding_model,
                                768,
                            )));
                        }
                        ProviderType::OpenAiCompatible => {
                            self.llm = Some(Box::new(OpenAiCompatibleLlm::new(
                                &local.base_url,
                                &local.llm_model,
                                local.api_key.clone(),
                            )));
                            self.embedder = Some(Box::new(OpenAiCompatibleEmbedder::new(
                                &local.base_url,
                                &local.embedding_model,
                                1024,
                                local.api_key.clone(),
                            )));
                        }
                        ProviderType::HuggingFace => {
                            #[cfg(feature = "llm-local")]
                            {
                                // Best-effort, like the embedder below: a
                                // local GGUF chat model can fail to load
                                // for reasons entirely orthogonal to
                                // whether the *config itself* is valid —
                                // out-of-VRAM being the most common (see
                                // `LocalLlm::load`'s own CPU-fallback
                                // retry, which already covers the typical
                                // case, but e.g. a corrupt/incompatible
                                // GGUF file could still fail even that).
                                // Previously this used `?`, which made
                                // `initialize_providers` — and therefore
                                // `KnowledgeEngine::new`/`reconfigure` —
                                // fail outright, leaving the whole engine
                                // as `None` (app startup) or the config
                                // change rejected (Settings save) even
                                // though the user's config was fine; the
                                // UI then misleadingly reported "AI is not
                                // configured" instead of "model failed to
                                // load". Keeping the engine alive lets
                                // health checks/logs surface the real
                                // error and lets the user retry (e.g. after
                                // freeing VRAM or picking a smaller model)
                                // without restarting the app.
                                match crate::ai::providers::local_llm::LocalLlm::from_config(
                                    &self.config,
                                    &self.models_root,
                                ) {
                                    Ok(llm) => self.llm = Some(Box::new(llm)),
                                    Err(e) => {
                                        tracing::warn!(
                                            "Embedded local chat model failed to load (chat / \
                                             \"Ask\" will be unavailable until this is \
                                             resolved — check the model file, available VRAM, \
                                             and the \"GPU layers\" setting): {e}"
                                        );
                                    }
                                }
                                // Best-effort: an embedding model is a
                                // separate download from the chat LLM, so a
                                // user who's only set up the latter should
                                // still get a working "Embedded" chat
                                // provider — just without semantic search /
                                // "Research this page" until they also point
                                // Settings at a GGUF embedding model. Mirrors
                                // the cloud branches' "only set what's
                                // configured" partial-init pattern below.
                                match crate::ai::providers::local_embedder::LocalEmbedder::from_config(
                                    &self.config,
                                    &self.models_root,
                                ) {
                                    Ok(embedder) => self.embedder = Some(Box::new(embedder)),
                                    Err(e) => {
                                        tracing::warn!(
                                            "Embedded local embedding model not available yet \
                                             (semantic search / \"Research this page\" will be \
                                             disabled until one is configured): {e}"
                                        );
                                    }
                                }
                            }
                            #[cfg(not(feature = "llm-local"))]
                            {
                                return Err(CoreError::Other(
                                    "Embedded Hugging Face local runtime requires building \
                                     grafium-core with the `llm-local` (or `llm-local-vulkan`) \
                                     Cargo feature enabled."
                                        .to_string(),
                                ));
                            }
                        }
                        _ => {
                            return Err(CoreError::Other("Unsupported local provider".to_string()));
                        }
                    }
                }
            }
            AiMode::Cloud => {
                if let Some(cloud) = &self.config.cloud {
                    match cloud.llm_provider {
                        ProviderType::OpenAi => {
                            let key = cloud.llm_api_key.as_deref().ok_or_else(|| {
                                CoreError::Other("Missing OpenAI API key".to_string())
                            })?;
                            self.llm = Some(Box::new(OpenAiLlm::new(key, &cloud.llm_model)));
                        }
                        ProviderType::Anthropic => {
                            let key = cloud.llm_api_key.as_deref().ok_or_else(|| {
                                CoreError::Other("Missing Anthropic API key".to_string())
                            })?;
                            self.llm = Some(Box::new(AnthropicLlm::new(key, &cloud.llm_model)));
                        }
                        ProviderType::OpenAiCompatible => {
                            let base_url = cloud
                                .llm_base_url
                                .clone()
                                .unwrap_or_else(|| "http://localhost:8000/v1".to_string());
                            self.llm = Some(Box::new(OpenAiCompatibleLlm::new(
                                &base_url,
                                &cloud.llm_model,
                                cloud.llm_api_key.clone(),
                            )));
                        }
                        _ => {}
                    }

                    let embed_key = cloud
                        .embedding_api_key
                        .clone()
                        .or_else(|| cloud.llm_api_key.clone());

                    match cloud.embedding_provider {
                        ProviderType::OpenAi => {
                            let key = embed_key.as_deref().ok_or_else(|| {
                                CoreError::Other("Missing OpenAI embedding API key".to_string())
                            })?;
                            self.embedder = Some(Box::new(OpenAiEmbedder::new(
                                key,
                                &cloud.embedding_model,
                                1536,
                            )));
                        }
                        ProviderType::OpenAiCompatible => {
                            let base_url = cloud
                                .embedding_base_url
                                .clone()
                                .or_else(|| cloud.llm_base_url.clone())
                                .unwrap_or_else(|| "http://localhost:8000/v1".to_string());
                            self.embedder = Some(Box::new(OpenAiCompatibleEmbedder::new(
                                &base_url,
                                &cloud.embedding_model,
                                1024,
                                embed_key,
                            )));
                        }
                        _ => {}
                    }
                }
            }
            AiMode::Hybrid => {
                // Embeddings from local provider.
                if let Some(local) = &self.config.local {
                    match local.provider {
                        ProviderType::Ollama => {
                            self.embedder = Some(Box::new(OllamaEmbedder::new(
                                &local.base_url,
                                &local.embedding_model,
                                768,
                            )));
                        }
                        ProviderType::OpenAiCompatible => {
                            self.embedder = Some(Box::new(OpenAiCompatibleEmbedder::new(
                                &local.base_url,
                                &local.embedding_model,
                                1024,
                                local.api_key.clone(),
                            )));
                        }
                        _ => {}
                    }
                }

                // LLM from cloud provider.
                if let Some(cloud) = &self.config.cloud {
                    match cloud.llm_provider {
                        ProviderType::OpenAi => {
                            let key = cloud.llm_api_key.as_deref().ok_or_else(|| {
                                CoreError::Other("Missing OpenAI API key".to_string())
                            })?;
                            self.llm = Some(Box::new(OpenAiLlm::new(key, &cloud.llm_model)));
                        }
                        ProviderType::Anthropic => {
                            let key = cloud.llm_api_key.as_deref().ok_or_else(|| {
                                CoreError::Other("Missing Anthropic API key".to_string())
                            })?;
                            self.llm = Some(Box::new(AnthropicLlm::new(key, &cloud.llm_model)));
                        }
                        ProviderType::OpenAiCompatible => {
                            let base_url = cloud
                                .llm_base_url
                                .clone()
                                .unwrap_or_else(|| "http://localhost:8000/v1".to_string());
                            self.llm = Some(Box::new(OpenAiCompatibleLlm::new(
                                &base_url,
                                &cloud.llm_model,
                                cloud.llm_api_key.clone(),
                            )));
                        }
                        _ => {}
                    }
                }
            }
        }

        // Initialize vector store.
        let vs_path = self
            .config
            .embedding
            .vector_store_path
            .clone()
            .unwrap_or_else(|| self.data_dir.join("vectors.db"));
        self.vector_store = Some(Arc::new(SqliteVectorStore::open(&vs_path)?));

        Ok(())
    }

    /// Reconfigure the engine with new settings.
    pub fn reconfigure(&mut self, config: AiConfig) -> Result<()> {
        self.config = config.clone();
        self.llm = None;
        self.embedder = None;
        self.vector_store = None;
        self.pipeline = RwLock::new(EmbeddingPipeline::new(config.embedding.clone()));
        self.reference_engine = ReferenceEngine::new(config.references.clone());

        if config.enabled {
            self.initialize_providers()?;
        }

        Ok(())
    }

    /// Check if the engine is ready for operations that need semantic
    /// search (indexing, vector search, "research this page" references) —
    /// these fundamentally require an embedding model, so the Embedded
    /// (llama.cpp) local provider — which is chat-only, see the
    /// `ProviderType::HuggingFace` branch in `initialize_providers` — never
    /// satisfies this, by design.
    pub fn is_ready(&self) -> bool {
        self.config.enabled
            && self.llm.is_some()
            && self.embedder.is_some()
            && self.vector_store.is_some()
    }

    /// Check if the engine is ready for LLM-only operations (chat/"Ask") —
    /// deliberately does *not* require an embedder, unlike [`Self::is_ready`],
    /// since [`Self::ask`] degrades gracefully to a non-RAG direct answer
    /// when no embedder is configured (e.g. the Embedded local provider).
    pub fn is_llm_ready(&self) -> bool {
        self.config.enabled && self.llm.is_some()
    }

    /// Health check — verify all providers are reachable.
    pub async fn health_check(&self) -> Result<HealthStatus> {
        let llm_ok = if let Some(llm) = &self.llm {
            llm.health_check().await.unwrap_or(false)
        } else {
            false
        };

        let vector_count = if let Some(store) = &self.vector_store {
            store.count().await.unwrap_or(0)
        } else {
            0
        };

        Ok(HealthStatus {
            enabled: self.config.enabled,
            llm_available: llm_ok,
            embedder_available: self.embedder.is_some(),
            vector_store_available: self.vector_store.is_some(),
            vector_count,
            mode: self.config.mode.clone(),
        })
    }

    /// Index a single page — embed its blocks and store vectors.
    pub async fn index_page(&self, page: &Page, blocks: &[Block], graph_id: &str) -> Result<usize> {
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| CoreError::Other("Embedder not initialized".to_string()))?;
        let store = self
            .vector_store
            .as_ref()
            .ok_or_else(|| CoreError::Other("Vector store not initialized".to_string()))?;

        let update_plan = {
            let pipeline = self.pipeline.read().await;
            let chunks = pipeline.chunk_page(page, blocks);
            pipeline.diff_page_chunks(&page.id, chunks)
        };

        if update_plan.dirty_chunks.is_empty() && update_plan.removed_chunk_ids.is_empty() {
            return Ok(0);
        }

        let count = if update_plan.dirty_chunks.is_empty() {
            0
        } else {
            let pipeline = self.pipeline.read().await;
            pipeline
                .embed_and_store(
                    &update_plan.dirty_chunks,
                    graph_id,
                    embedder.as_ref(),
                    store.as_ref(),
                )
                .await?
        };

        if !update_plan.removed_chunk_ids.is_empty() {
            store
                .delete_chunks(graph_id, &update_plan.removed_chunk_ids)
                .await?;
        }

        let mut pipeline = self.pipeline.write().await;
        pipeline.mark_chunks_clean(&update_plan.dirty_chunks);
        pipeline.remove_chunks(&update_plan.removed_chunk_ids);

        Ok(count)
    }

    /// Semantic search across all (or specific) graphs.
    pub async fn search(
        &self,
        query: &str,
        top_k: usize,
        graph_id: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| CoreError::Other("Embedder not initialized".to_string()))?;
        let store = self
            .vector_store
            .as_ref()
            .ok_or_else(|| CoreError::Other("Vector store not initialized".to_string()))?;

        let query_texts = vec![query.to_string()];
        let embeddings = embedder.embed(&query_texts).await?;

        if embeddings.is_empty() {
            return Ok(vec![]);
        }

        store.search(&embeddings[0], top_k, graph_id).await
    }

    /// Hybrid retrieval: fuse dense (vector) and sparse (BM25/FTS) rankings
    /// with Reciprocal Rank Fusion, then hydrate the top block IDs with
    /// page/date metadata from `db`.
    ///
    /// Degrades gracefully:
    /// - no embedder / empty or unavailable vector index → pure BM25,
    /// - FTS unavailable → pure vector,
    /// - both unavailable → empty result (never an error).
    ///
    /// `db` is the graph's own SQLite database, so FTS is inherently scoped
    /// to that graph; `graph_id` only filters the (shared) vector store.
    pub async fn hybrid_search(
        &self,
        db: &crate::db::Database,
        query: &str,
        top_k: usize,
        graph_id: Option<&str>,
    ) -> Result<Vec<RetrievedHit>> {
        // Over-fetch each arm so fusion has room to reorder.
        let fetch = (top_k * 3).max(top_k);

        // Dense arm — best-effort. An empty index or a transient embedder
        // failure must not sink the whole query.
        let vector_ids: Vec<String> = if self.embedder.is_some() && self.vector_store.is_some() {
            match self.search(query, fetch, graph_id).await {
                Ok(results) => results.into_iter().filter_map(|r| r.block_id).collect(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        // Sparse arm — best-effort BM25 over the graph's FTS index.
        let fts_ids: Vec<String> = match db.search_fts(query, fetch as i64) {
            Ok(blocks) => blocks.into_iter().map(|b| b.id).collect(),
            Err(_) => Vec::new(),
        };

        let mut rankings: Vec<Vec<String>> = Vec::new();
        if !vector_ids.is_empty() {
            rankings.push(vector_ids);
        }
        if !fts_ids.is_empty() {
            rankings.push(fts_ids);
        }
        if rankings.is_empty() {
            return Ok(Vec::new());
        }

        let fused = crate::knowledge::retrieval::reciprocal_rank_fusion(
            &rankings,
            crate::knowledge::retrieval::RRF_K,
        );

        let top_ids: Vec<String> = fused.iter().take(top_k).map(|f| f.id.clone()).collect();
        let metas = db.get_blocks_with_page_meta(&top_ids)?;
        let score_by_id: std::collections::HashMap<&str, f64> =
            fused.iter().map(|f| (f.id.as_str(), f.score)).collect();
        let meta_by_id: std::collections::HashMap<&str, &crate::db::BlockPageMeta> =
            metas.iter().map(|m| (m.block_id.as_str(), m)).collect();

        let hits = top_ids
            .iter()
            .filter_map(|id| {
                let meta = meta_by_id.get(id.as_str())?;
                let date_ms = Self::resolve_hit_date(meta);
                Some(RetrievedHit {
                    block_id: meta.block_id.clone(),
                    page_id: meta.page_id.clone(),
                    page_title: meta.page_title.clone(),
                    content: meta.content.clone(),
                    date_ms,
                    is_journal: meta.is_journal,
                    score: score_by_id.get(id.as_str()).copied().unwrap_or(0.0),
                    parents: Vec::new(),
                    children: Vec::new(),
                })
            })
            .collect();

        Ok(hits)
    }

    /// Best-known date for a hit: journal-title date if the page is a journal
    /// and its title parses as a date, otherwise the block's `created_at`.
    fn resolve_hit_date(meta: &crate::db::BlockPageMeta) -> Option<i64> {
        if meta.is_journal {
            if let Some(ms) = crate::knowledge::retrieval::journal_title_to_ms(&meta.page_title) {
                return Some(ms);
            }
        }
        Some(meta.created_at)
    }

    /// Generate references for a page using AI.
    pub async fn generate_references(
        &self,
        page_id: &str,
        page_title: &str,
        blocks: &[(String, String)], // (block_id, content)
        graph_id: &str,
        on_progress: &mut (dyn FnMut(&str) + Send),
    ) -> Result<PageReferencesMeta> {
        let llm = self
            .llm
            .as_ref()
            .ok_or_else(|| CoreError::Other("LLM not initialized".to_string()))?;
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| CoreError::Other("Embedder not initialized".to_string()))?;
        let store = self
            .vector_store
            .as_ref()
            .ok_or_else(|| CoreError::Other("Vector store not initialized".to_string()))?;

        self.reference_engine
            .generate_references(
                page_id,
                page_title,
                blocks,
                graph_id,
                llm.as_ref(),
                embedder.as_ref(),
                store.as_ref(),
                on_progress,
            )
            .await
    }

    /// Summarize an arbitrary piece of text (title-answer, prose summary,
    /// hashtag-style topic tags) using the configured LLM — the same shape
    /// `generate_references` produces for "Research this page", reused here
    /// so callers like the media-import pipeline (video/audio transcripts)
    /// don't need their own copy of the summarization prompt/parsing.
    /// Only needs `self.llm`, not the embedder/vector store, so it works
    /// even with the Embedded (llama.cpp) provider, which is chat-only.
    pub async fn summarize_text(
        &self,
        title: &str,
        full_text: &str,
        on_progress: &mut (dyn FnMut(&str) + Send),
    ) -> Result<PageSummary> {
        let llm = self
            .llm
            .as_ref()
            .ok_or_else(|| CoreError::Other("LLM not initialized".to_string()))?;

        crate::ai::references::generate_page_summary(title, full_text, llm.as_ref(), on_progress)
            .await
    }

    /// Actually researches `title`/`seed_text` on the open internet — plans
    /// search queries, searches, picks sources, fetches them, and
    /// synthesizes a cited summary — as opposed to `summarize_text`, which
    /// only ever reflects content already given to it. Only needs
    /// `self.llm` (a plain HTTP fetch is used for both search and page
    /// fetching, requiring no separate configuration), so it works with
    /// any configured provider, local or cloud.
    pub async fn research_web(
        &self,
        title: &str,
        seed_text: &str,
        on_progress: &mut (dyn FnMut(&str) + Send),
    ) -> Result<crate::ai::web_research::WebResearchResult> {
        let llm = self
            .llm
            .as_ref()
            .ok_or_else(|| CoreError::Other("LLM not initialized".to_string()))?;

        let browser = crate::scraping::HttpBrowserDriver::new();
        crate::ai::web_research::WebResearchEngine::new(llm.as_ref(), &browser)
            .research(title, seed_text, on_progress)
            .await
    }

    /// Ask a question against the knowledge base (RAG).
    ///
    /// Falls back to a direct (non-RAG) answer when no embedder is
    /// configured — the Embedded (llama.cpp) local provider is chat-only,
    /// so requiring semantic search here would make "Ask" completely
    /// unusable for it even though the LLM itself works fine.
    pub async fn ask(&self, question: &str, graph_id: Option<&str>) -> Result<String> {
        let llm = self
            .llm
            .as_ref()
            .ok_or_else(|| CoreError::Other("LLM not initialized".to_string()))?;

        // Search for relevant context, if semantic search is available.
        let results = if self.embedder.is_some() {
            self.search(question, 10, graph_id).await?
        } else {
            Vec::new()
        };

        if results.is_empty() {
            // No context found, answer directly.
            let messages = vec![crate::ai::traits::ChatMessage {
                role: crate::ai::traits::MessageRole::User,
                content: question.to_string(),
            }];
            return llm
                .complete(&messages, &crate::ai::traits::CompletionOptions::default())
                .await;
        }

        // Build RAG prompt with context.
        let context: String = results
            .iter()
            .enumerate()
            .map(|(i, r)| format!("[{}] From \"{}\":\n{}\n", i + 1, r.page_title, r.content))
            .collect();

        let messages = vec![
            crate::ai::traits::ChatMessage {
                role: crate::ai::traits::MessageRole::System,
                content: format!(
                    "You are a knowledge assistant. Answer based on the following context from the user's notes. \
                     Cite sources using [N] notation. If the context doesn't contain enough information, say so.\n\n\
                     Context:\n{}",
                    context
                ),
            },
            crate::ai::traits::ChatMessage {
                role: crate::ai::traits::MessageRole::User,
                content: question.to_string(),
            },
        ];

        llm.complete(&messages, &crate::ai::traits::CompletionOptions::default())
            .await
    }

    /// Get the graph registry (read access).
    pub async fn registry(&self) -> tokio::sync::RwLockReadGuard<'_, GraphRegistry> {
        self.registry.read().await
    }

    /// Get the graph registry (write access).
    pub async fn registry_mut(&self) -> tokio::sync::RwLockWriteGuard<'_, GraphRegistry> {
        self.registry.write().await
    }

    /// Get current config.
    pub fn config(&self) -> &AiConfig {
        &self.config
    }
}

/// Health status of the knowledge engine.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HealthStatus {
    pub enabled: bool,
    pub llm_available: bool,
    pub embedder_available: bool,
    pub vector_store_available: bool,
    pub vector_count: usize,
    pub mode: AiMode,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::traits::{BoxFuture, ChunkEmbedding};
    use serde_json::json;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockEmbedderState {
        calls: usize,
    }

    struct MockEmbedder {
        state: Arc<Mutex<MockEmbedderState>>,
        dimension: usize,
    }

    impl MockEmbedder {
        fn new(dimension: usize) -> (Self, Arc<Mutex<MockEmbedderState>>) {
            let state = Arc::new(Mutex::new(MockEmbedderState::default()));
            (
                Self {
                    state: state.clone(),
                    dimension,
                },
                state,
            )
        }
    }

    impl Embedder for MockEmbedder {
        fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
            Box::pin(async move {
                self.state.lock().unwrap().calls += 1;
                Ok(texts
                    .iter()
                    .map(|_| vec![0.25; self.dimension])
                    .collect::<Vec<_>>())
            })
        }

        fn dimension(&self) -> usize {
            self.dimension
        }

        fn model_name(&self) -> &str {
            "mock-embedder"
        }
    }

    #[derive(Clone, Default)]
    struct MockVectorStoreSnapshot {
        upsert_calls: usize,
        delete_by_page_calls: usize,
        delete_chunks_calls: usize,
        stored_chunks: usize,
    }

    #[derive(Default)]
    struct MockVectorStoreState {
        upsert_calls: usize,
        delete_by_page_calls: usize,
        delete_chunks_calls: usize,
        fail_next_upsert: bool,
        chunks: HashMap<String, ChunkEmbedding>,
    }

    struct MockVectorStore {
        state: Mutex<MockVectorStoreState>,
    }

    impl MockVectorStore {
        fn new(fail_next_upsert: bool) -> Self {
            Self {
                state: Mutex::new(MockVectorStoreState {
                    fail_next_upsert,
                    ..Default::default()
                }),
            }
        }

        fn snapshot(&self) -> MockVectorStoreSnapshot {
            let state = self.state.lock().unwrap();
            MockVectorStoreSnapshot {
                upsert_calls: state.upsert_calls,
                delete_by_page_calls: state.delete_by_page_calls,
                delete_chunks_calls: state.delete_chunks_calls,
                stored_chunks: state.chunks.len(),
            }
        }
    }

    impl VectorStore for MockVectorStore {
        fn upsert<'a>(&'a self, chunks: &'a [ChunkEmbedding]) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                let mut state = self.state.lock().unwrap();
                state.upsert_calls += 1;
                if state.fail_next_upsert {
                    state.fail_next_upsert = false;
                    return Err(CoreError::Other("simulated upsert failure".to_string()));
                }

                for chunk in chunks {
                    state.chunks.insert(chunk.chunk_id.clone(), chunk.clone());
                }
                Ok(())
            })
        }

        fn search<'a>(
            &'a self,
            _query_embedding: &'a [f32],
            _top_k: usize,
            _filter_graph_id: Option<&'a str>,
        ) -> BoxFuture<'a, Result<Vec<SearchResult>>> {
            Box::pin(async move { Ok(vec![]) })
        }

        fn delete_by_page<'a>(
            &'a self,
            graph_id: &'a str,
            page_id: &'a str,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                let mut state = self.state.lock().unwrap();
                state.delete_by_page_calls += 1;
                state
                    .chunks
                    .retain(|_, chunk| !(chunk.graph_id == graph_id && chunk.page_id == page_id));
                Ok(())
            })
        }

        fn delete_chunks<'a>(
            &'a self,
            graph_id: &'a str,
            chunk_ids: &'a [String],
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                let mut state = self.state.lock().unwrap();
                state.delete_chunks_calls += 1;
                for chunk_id in chunk_ids {
                    let should_remove = state
                        .chunks
                        .get(chunk_id)
                        .map(|chunk| chunk.graph_id == graph_id)
                        .unwrap_or(false);
                    if should_remove {
                        state.chunks.remove(chunk_id);
                    }
                }
                Ok(())
            })
        }

        fn delete_by_graph<'a>(&'a self, graph_id: &'a str) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                let mut state = self.state.lock().unwrap();
                state.chunks.retain(|_, chunk| chunk.graph_id != graph_id);
                Ok(())
            })
        }

        fn count<'a>(&'a self) -> BoxFuture<'a, Result<usize>> {
            Box::pin(async move { Ok(self.state.lock().unwrap().chunks.len()) })
        }
    }

    fn test_engine(
        embedder: Box<dyn Embedder>,
        vector_store: Arc<dyn VectorStore>,
    ) -> Result<KnowledgeEngine> {
        let config = AiConfig::default();
        let registry_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("graph_registry.engine-test.json");
        Ok(KnowledgeEngine {
            config: config.clone(),
            llm: None,
            embedder: Some(embedder),
            vector_store: Some(vector_store),
            pipeline: RwLock::new(EmbeddingPipeline::new(config.embedding.clone())),
            reference_engine: ReferenceEngine::new(config.references.clone()),
            registry: RwLock::new(GraphRegistry::load(&registry_path)?),
            data_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            models_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        })
    }

    fn test_page() -> Page {
        Page {
            id: "page-1".to_string(),
            title: "Test Page".to_string(),
            file_path: None,
            created_at: 0,
            updated_at: 0,
            is_journal: false,
            properties: json!({}),
        }
    }

    fn test_block(content: &str) -> Block {
        Block {
            id: "block-1".to_string(),
            page_id: "page-1".to_string(),
            parent_id: None,
            order_index: 0,
            content: content.to_string(),
            block_type: crate::models::BlockType::Text,
            properties: json!({}),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn reindexing_unchanged_page_skips_store_mutations() -> Result<()> {
        let (mock_embedder, embedder_state) = MockEmbedder::new(4);
        let store = Arc::new(MockVectorStore::new(false));
        let engine = test_engine(Box::new(mock_embedder), store.clone())?;
        let page = test_page();
        let blocks = vec![test_block(
            "This block content is long enough to be indexed.",
        )];

        assert_eq!(engine.index_page(&page, &blocks, "graph-1").await?, 1);

        let first_snapshot = store.snapshot();
        assert_eq!(first_snapshot.upsert_calls, 1);
        assert_eq!(first_snapshot.stored_chunks, 1);

        assert_eq!(engine.index_page(&page, &blocks, "graph-1").await?, 0);

        let second_snapshot = store.snapshot();
        assert_eq!(second_snapshot.upsert_calls, 1);
        assert_eq!(second_snapshot.delete_by_page_calls, 0);
        assert_eq!(second_snapshot.delete_chunks_calls, 0);
        assert_eq!(second_snapshot.stored_chunks, 1);
        assert_eq!(embedder_state.lock().unwrap().calls, 1);

        Ok(())
    }

    #[tokio::test]
    async fn failed_upsert_does_not_poison_hash_cache() -> Result<()> {
        let (mock_embedder, embedder_state) = MockEmbedder::new(4);
        let store = Arc::new(MockVectorStore::new(true));
        let engine = test_engine(Box::new(mock_embedder), store.clone())?;
        let page = test_page();
        let blocks = vec![test_block(
            "This block content is long enough to be indexed.",
        )];

        let error = engine
            .index_page(&page, &blocks, "graph-1")
            .await
            .expect_err("first index should fail");
        assert!(error.to_string().contains("simulated upsert failure"));

        let failed_snapshot = store.snapshot();
        assert_eq!(failed_snapshot.upsert_calls, 1);
        assert_eq!(failed_snapshot.stored_chunks, 0);

        assert_eq!(engine.index_page(&page, &blocks, "graph-1").await?, 1);

        let retry_snapshot = store.snapshot();
        assert_eq!(retry_snapshot.upsert_calls, 2);
        assert_eq!(retry_snapshot.stored_chunks, 1);
        assert_eq!(embedder_state.lock().unwrap().calls, 2);

        Ok(())
    }

    #[tokio::test]
    async fn reconfigure_rebuilds_embedding_pipeline() -> Result<()> {
        let mut config = AiConfig::default();
        config.enabled = false;
        config.embedding.chunk_max_tokens = 10;
        config.embedding.chunk_overlap_tokens = 0;

        let mut engine =
            KnowledgeEngine::new(Path::new(env!("CARGO_MANIFEST_DIR")), config.clone())?;
        let page = test_page();
        let blocks = vec![test_block(
            "This is sentence one. This is sentence two. This is sentence three.",
        )];

        let before = {
            let pipeline = engine.pipeline.read().await;
            pipeline.chunk_page(&page, &blocks).len()
        };

        let mut reconfigured = config.clone();
        reconfigured.embedding.chunk_max_tokens = 256;
        engine.reconfigure(reconfigured)?;

        let after = {
            let pipeline = engine.pipeline.read().await;
            pipeline.chunk_page(&page, &blocks).len()
        };

        assert!(before > 1, "expected initial config to split the block");
        assert_eq!(after, 1, "expected reconfigured pipeline to stop splitting");

        Ok(())
    }

    #[tokio::test]
    async fn hybrid_search_degrades_to_bm25_when_vector_index_empty() -> Result<()> {
        use crate::db::Database;
        use crate::models::BlockType;

        // A disabled engine has no embedder / no vector store — exactly the
        // real-world "index never built" situation.
        let mut config = AiConfig::default();
        config.enabled = false;
        let engine = KnowledgeEngine::new(Path::new(env!("CARGO_MANIFEST_DIR")), config)?;
        assert!(engine.embedder.is_none() && engine.vector_store.is_none());

        let db = Database::in_memory()?;
        let journal = db.create_page("2026-03-14", true)?;
        db.create_block(
            &journal.id,
            None,
            0,
            "Felt really upset about the deadline slipping again",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        let other = db.create_page("Recipes", false)?;
        db.create_block(
            &other.id,
            None,
            0,
            "Pasta with garlic and olive oil",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let hits = engine
            .hybrid_search(&db, "upset deadline", 10, None)
            .await?;
        assert_eq!(hits.len(), 1, "BM25 should still find the journal block");
        let hit = &hits[0];
        assert!(hit.content.contains("upset"));
        assert!(hit.is_journal);
        // Journal date is derived from the page title, not created_at.
        assert_eq!(
            crate::knowledge::retrieval::format_date_ms(hit.date_ms.unwrap()),
            "2026-03-14"
        );

        Ok(())
    }
}
