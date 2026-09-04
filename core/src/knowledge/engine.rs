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
use crate::knowledge::retrieval::{self, ContextEntry, RetrievedHit};
use crate::knowledge::vector_store::SqliteVectorStore;
use crate::models::{Block, Page};

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

    /// Report indexing status for a graph so the UI can show the empty-index
    /// banner and post-index confirmation. `total_blocks` is the number of
    /// blocks in the graph's own DB; `indexed_chunks` is how many vectors are
    /// stored for this graph.
    pub async fn index_status(
        &self,
        db: &crate::db::Database,
        graph_id: &str,
    ) -> Result<IndexStatus> {
        let indexed_chunks = if let Some(store) = &self.vector_store {
            store.count_for_graph(graph_id).await.unwrap_or(0)
        } else {
            0
        };
        let total_blocks = db.count_blocks().unwrap_or(0);
        Ok(IndexStatus {
            indexed_chunks,
            total_blocks,
            embedder_ready: self.embedder.is_some() && self.vector_store.is_some(),
            llm_ready: self.is_llm_ready(),
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

        let query_vec = embedder.embed_query(query).await?;

        store.search(&query_vec, top_k, graph_id).await
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

        let fused = retrieval::reciprocal_rank_fusion(&rankings, retrieval::RRF_K);

        // Temporal queries ("when did I…", "lately", "in 2025") need date-aware
        // reordering. Hydrate a wider slice so the reorder has candidates to
        // pull forward, then truncate back to `top_k`.
        let intent = retrieval::detect_temporal_intent(query);
        let hydrate_count = if intent.is_temporal {
            fused.len().min(fetch)
        } else {
            top_k
        };

        let top_ids: Vec<String> = fused
            .iter()
            .take(hydrate_count)
            .map(|f| f.id.clone())
            .collect();
        let metas = db.get_blocks_with_page_meta(&top_ids)?;
        let score_by_id: std::collections::HashMap<&str, f64> =
            fused.iter().map(|f| (f.id.as_str(), f.score)).collect();
        let meta_by_id: std::collections::HashMap<&str, &crate::db::BlockPageMeta> =
            metas.iter().map(|m| (m.block_id.as_str(), m)).collect();

        let mut hits: Vec<RetrievedHit> = top_ids
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

        if intent.is_temporal {
            hits = retrieval::order_hits_temporally(hits, &intent);
            hits.truncate(top_k);
        }

        Ok(hits)
    }

    /// Best-known date for a hit: journal-title date if the page is a journal
    /// and its title parses as a date, otherwise the block's `created_at`.
    fn resolve_hit_date(meta: &crate::db::BlockPageMeta) -> Option<i64> {
        if meta.is_journal {
            if let Some(ms) = retrieval::journal_title_to_ms(&meta.page_title) {
                return Some(ms);
            }
        }
        Some(meta.created_at)
    }

    /// Fill each hit's `parents` (ancestor chain, outermost first) and
    /// `children` (immediate children) from the DB, for small-to-big
    /// context expansion. Best-effort per hit — a failed lookup just leaves
    /// that hit's expansion empty rather than failing the whole query.
    pub fn expand_hits(&self, db: &crate::db::Database, hits: &mut [RetrievedHit]) {
        for hit in hits.iter_mut() {
            if let Ok(parents) = db.get_ancestor_chain(&hit.block_id) {
                hit.parents = parents
                    .into_iter()
                    .map(|b| retrieval::ContextItem {
                        block_id: b.id,
                        content: b.content,
                    })
                    .collect();
            }
            if let Ok(children) = db.list_child_blocks(&hit.block_id) {
                hit.children = children
                    .into_iter()
                    .map(|b| retrieval::ContextItem {
                        block_id: b.id,
                        content: b.content,
                    })
                    .collect();
            }
        }
    }

    /// Retrieve (hybrid), expand (small-to-big), and assemble a dated, cited
    /// context block bounded by `budget_tokens`.
    pub async fn retrieve_context(
        &self,
        db: &crate::db::Database,
        query: &str,
        top_k: usize,
        budget_tokens: usize,
        graph_id: Option<&str>,
    ) -> Result<Vec<ContextEntry>> {
        let mut hits = self.hybrid_search(db, query, top_k, graph_id).await?;
        self.expand_hits(db, &mut hits);
        Ok(retrieval::assemble_within_budget(&hits, budget_tokens))
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
    pub async fn ask(
        &self,
        db: &crate::db::Database,
        question: &str,
        graph_id: Option<&str>,
    ) -> Result<AskResponse> {
        let llm = self
            .llm
            .as_ref()
            .ok_or_else(|| CoreError::Other("LLM not initialized".to_string()))?;

        // Hybrid retrieval degrades gracefully: works with an empty vector
        // index (pure BM25) and even with no embedder at all. The context
        // budget is sized to the model's own window so the assembled prompt
        // can't overflow it (embedded llama.cpp hard-errors on overflow).
        let budget = ask_context_budget(llm.context_window());
        let entries = self
            .retrieve_context(db, question, ASK_TOP_K, budget, graph_id)
            .await
            .unwrap_or_default();

        let sources: Vec<Source> = entries
            .iter()
            .map(|e| Source {
                index: e.index,
                page_id: e.page_id.clone(),
                page_title: e.page_title.clone(),
                block_id: e.block_id.clone(),
                date: e.date_ms.map(retrieval::format_date_ms),
            })
            .collect();

        let context_block = build_context_block(&entries);
        let system_prompt = build_system_prompt(&context_block, !entries.is_empty());

        let messages = vec![
            crate::ai::traits::ChatMessage {
                role: crate::ai::traits::MessageRole::System,
                content: system_prompt,
            },
            crate::ai::traits::ChatMessage {
                role: crate::ai::traits::MessageRole::User,
                content: question.to_string(),
            },
        ];

        let answer = llm
            .complete(
                &messages,
                &crate::ai::traits::CompletionOptions {
                    // Cap output so prompt + generation stays within n_ctx.
                    max_tokens: Some(ASK_RESERVED_OUTPUT_TOKENS as u32),
                    ..Default::default()
                },
            )
            .await?;

        Ok(AskResponse { answer, sources })
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

/// Indexing coverage for a graph, for the Chat empty-index banner and
/// post-index confirmation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexStatus {
    /// Vectors stored for this graph.
    pub indexed_chunks: usize,
    /// Total blocks in the graph's DB.
    pub total_blocks: usize,
    /// Whether an embedder + vector store are available (semantic indexing
    /// possible).
    pub embedder_ready: bool,
    /// Whether the LLM is ready for chat.
    pub llm_ready: bool,
}

/// How many hits `ask` retrieves before context assembly.
const ASK_TOP_K: usize = 10;
/// Tokens reserved for the model's answer. Also used as the `ask`
/// completion's `max_tokens`, so prompt + output can never exceed `n_ctx`.
const ASK_RESERVED_OUTPUT_TOKENS: usize = 1024;
/// Tokens reserved for the fixed prompt scaffolding (system-prompt template,
/// the user's question, chat-template control tokens, and per-line
/// formatting) that sits outside the retrieved-context budget. Generous on
/// purpose — `estimate_tokens` (chars/4) under-counts real tokenization.
const ASK_PROMPT_OVERHEAD_TOKENS: usize = 1024;
/// Floor for the retrieved-context budget so a tiny window still gets *some*
/// context rather than none.
const ASK_MIN_CONTEXT_BUDGET_TOKENS: usize = 512;
/// Retrieved-context budget used when the LLM can't report its context
/// window (most remote providers). Deliberately conservative — the embedded
/// llama.cpp default window is only 4096 and it hard-errors on overflow.
const ASK_DEFAULT_CONTEXT_BUDGET_TOKENS: usize = 2048;

/// Model-aware retrieved-context budget: `n_ctx − prompt_overhead −
/// reserved_output`, floored, or a conservative default when the window is
/// unknown. Keeps the assembled prompt from overflowing the model's context
/// window (embedded llama.cpp hard-errors once prompt tokens reach `n_ctx`).
fn ask_context_budget(context_window: Option<usize>) -> usize {
    match context_window {
        Some(n_ctx) => n_ctx
            .saturating_sub(ASK_PROMPT_OVERHEAD_TOKENS)
            .saturating_sub(ASK_RESERVED_OUTPUT_TOKENS)
            .max(ASK_MIN_CONTEXT_BUDGET_TOKENS),
        None => ASK_DEFAULT_CONTEXT_BUDGET_TOKENS,
    }
}

/// A structured citation returned alongside an answer so the UI can render
/// which notes were used and navigate to them.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Source {
    /// The `[N]` citation marker used in the context and answer.
    pub index: usize,
    pub page_id: String,
    pub page_title: String,
    pub block_id: String,
    /// ISO `YYYY-MM-DD` date when known (journal date or block created_at).
    pub date: Option<String>,
}

/// A blended answer plus the structured sources that informed it.
#[derive(Debug, Clone)]
pub struct AskResponse {
    pub answer: String,
    pub sources: Vec<Source>,
}

/// Render assembled context entries into numbered, dated prompt lines, e.g.
/// `[3] 2026-03-14 (journal) — Kirsten's project ...`. Empty when there is no
/// context.
fn build_context_block(entries: &[ContextEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        let date = e
            .date_ms
            .map(retrieval::format_date_ms)
            .unwrap_or_else(|| "undated".to_string());
        let kind = if e.is_journal { " (journal)" } else { "" };
        out.push_str(&format!(
            "[{}] {}{} — from \"{}\":\n{}\n\n",
            e.index, date, kind, e.page_title, e.text
        ));
    }
    out.trim_end().to_string()
}

/// Build the blended system prompt. It supports three regimes without ever
/// forcing "answer only from context" (which would break general questions)
/// and without letting the model pass off general knowledge as the user's
/// notes.
fn build_system_prompt(context_block: &str, has_context: bool) -> String {
    let base = "You are Grafium's assistant. You help the user with BOTH questions about their \
personal knowledge graph (their notes) AND general questions using your own knowledge.\n\n\
Rules:\n\
- When the retrieved notes below are relevant, answer from them and cite each claim with its \
[N] marker. The notes are the user's own writing and journal entries.\n\
- For \"when\" / temporal questions, state the explicit date(s) from the cited notes. Journal \
entries are dated; use those dates. If you cannot find a date, say so plainly.\n\
- If the notes do not contain the answer, you may answer from your own general knowledge — but \
say clearly that this is general knowledge and not from their notes.\n\
- If an answer blends both, keep them separate: cite the notes with [N], and label the general \
part as general knowledge.\n\
- Never invent citations or dates, and never present general knowledge as if it came from their \
notes. If you don't know, say you don't know.";

    if has_context {
        format!(
            "{base}\n\nRetrieved notes (each prefixed with its [N] citation marker and date):\n\n{context_block}"
        )
    } else {
        format!(
            "{base}\n\nNo relevant notes were retrieved from the user's graph for this question, \
so answer from your general knowledge and make clear it is not from their notes."
        )
    }
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

        fn count_for_graph<'a>(&'a self, graph_id: &'a str) -> BoxFuture<'a, Result<usize>> {
            Box::pin(async move {
                let state = self.state.lock().unwrap();
                Ok(state
                    .chunks
                    .values()
                    .filter(|c| c.graph_id == graph_id)
                    .count())
            })
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

    #[tokio::test]
    async fn index_status_reports_empty_index_but_counts_blocks() -> Result<()> {
        use crate::db::Database;
        use crate::models::BlockType;

        let mut config = AiConfig::default();
        config.enabled = false;
        let engine = KnowledgeEngine::new(Path::new(env!("CARGO_MANIFEST_DIR")), config)?;

        let db = Database::in_memory()?;
        let page = db.create_page("Notes", false)?;
        db.create_block(&page.id, None, 0, "one", BlockType::Text, json!({}))?;
        db.create_block(&page.id, None, 1, "two", BlockType::Text, json!({}))?;

        let status = engine.index_status(&db, "graph-x").await?;
        assert_eq!(status.total_blocks, 2);
        assert_eq!(status.indexed_chunks, 0, "vector index is empty");
        assert!(!status.embedder_ready);
        assert!(!status.llm_ready);

        Ok(())
    }

    #[test]
    fn context_block_renders_dated_cited_lines() {
        let entries = vec![
            ContextEntry {
                index: 1,
                block_id: "b1".to_string(),
                page_id: "p1".to_string(),
                page_title: "2026-03-14".to_string(),
                date_ms: retrieval::journal_title_to_ms("2026-03-14"),
                is_journal: true,
                text: "felt upset about the deadline".to_string(),
            },
            ContextEntry {
                index: 2,
                block_id: "b2".to_string(),
                page_id: "p2".to_string(),
                page_title: "Rust".to_string(),
                date_ms: None,
                is_journal: false,
                text: "ownership and borrowing".to_string(),
            },
        ];
        let block = build_context_block(&entries);
        assert!(block.contains("[1] 2026-03-14 (journal) — from \"2026-03-14\":"));
        assert!(block.contains("felt upset about the deadline"));
        assert!(block.contains("[2] undated — from \"Rust\":"));
        assert!(!block.contains("(journal) — from \"Rust\""));
    }

    #[test]
    fn context_block_is_empty_without_entries() {
        assert!(build_context_block(&[]).is_empty());
    }

    #[test]
    fn system_prompt_supports_blended_answering() {
        let with_ctx = build_system_prompt("[1] 2026-03-14 — from \"x\":\nhi", true);
        // Must not force "only answer from context".
        assert!(!with_ctx.to_lowercase().contains("only answer from"));
        assert!(with_ctx.contains("Retrieved notes"));
        assert!(with_ctx.contains("general knowledge"));
        assert!(with_ctx.contains("[1] 2026-03-14"));

        let no_ctx = build_system_prompt("", false);
        assert!(no_ctx.contains("No relevant notes"));
        assert!(no_ctx.contains("general knowledge"));
    }

    #[test]
    fn ask_context_budget_is_model_aware_and_never_overflows() {
        // Embedded llama.cpp default window (4096): budget must leave room for
        // prompt overhead + reserved output, and stay well under 6000.
        let b4096 = ask_context_budget(Some(4096));
        assert_eq!(
            b4096,
            4096 - ASK_PROMPT_OVERHEAD_TOKENS - ASK_RESERVED_OUTPUT_TOKENS
        );
        assert!(
            b4096 + ASK_PROMPT_OVERHEAD_TOKENS + ASK_RESERVED_OUTPUT_TOKENS <= 4096,
            "prompt + output must fit the window"
        );
        assert!(
            b4096 < 6000,
            "must not use the old unconditional 6000 budget"
        );

        // Larger window scales the budget up.
        assert!(ask_context_budget(Some(8192)) > b4096);

        // Unknown window (most remote providers) falls back conservatively,
        // never the old 6000.
        assert_eq!(ask_context_budget(None), ASK_DEFAULT_CONTEXT_BUDGET_TOKENS);
        assert!(ask_context_budget(None) <= 2048);

        // A tiny window still yields the floor rather than zero.
        assert_eq!(ask_context_budget(Some(256)), ASK_MIN_CONTEXT_BUDGET_TOKENS);
    }
}
