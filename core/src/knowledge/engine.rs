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
use crate::knowledge::conversation::{self, ChatTurn};
use crate::knowledge::registry::GraphRegistry;
use crate::knowledge::research_intent;
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

        // Tell the chunk pipeline which embedding scheme (document prefix) is
        // active, so content hashes change when the prefix scheme changes and
        // stale, differently-prefixed vectors get re-embedded on reindex.
        let scheme = self
            .embedder
            .as_ref()
            .map(|e| e.embedding_scheme_id())
            .unwrap_or_default();
        self.pipeline.get_mut().set_embedding_scheme(scheme);

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

    /// Reload the local chat model forcing full GPU offload, bypassing the
    /// free-VRAM heuristic. Backs Chat's "Retry on GPU" action: the heuristic
    /// may have landed on CPU because VRAM was *transiently* busy at startup
    /// (the embedder mid-index, a previous instance shutting down); once that
    /// clears, this moves inference onto the GPU without a restart or a
    /// Settings edit. Only meaningful for the embedded local provider; other
    /// provider modes return an error the UI can surface. On failure the
    /// previous provider is left untouched (a fresh instance is only swapped
    /// in on success), so a failed retry can't leave chat unavailable.
    pub fn retry_llm_on_gpu(&mut self) -> Result<()> {
        #[cfg(feature = "llm-local")]
        {
            if !matches!(self.config.mode, crate::ai::config::AiMode::Local)
                || !self
                    .config
                    .local
                    .as_ref()
                    .is_some_and(|l| l.provider == crate::ai::config::ProviderType::HuggingFace)
            {
                return Err(CoreError::Other(
                    "Retry on GPU only applies to the embedded local chat model.".to_string(),
                ));
            }
            let llm = crate::ai::providers::local_llm::LocalLlm::from_config_forcing_gpu(
                &self.config,
                &self.models_root,
            )?;
            self.llm = Some(Box::new(llm));
            Ok(())
        }
        #[cfg(not(feature = "llm-local"))]
        {
            Err(CoreError::Other(
                "This build has no embedded local LLM support.".to_string(),
            ))
        }
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

    /// Check if the engine can index/embed content — requires an embedder and
    /// a vector store, but deliberately *not* an LLM. Auto-indexing and manual
    /// "Index my notes" only need to embed, so a user who configured an
    /// embedder but no chat model still gets a fresh index.
    pub fn can_index(&self) -> bool {
        self.config.enabled && self.embedder.is_some() && self.vector_store.is_some()
    }

    /// Check if the engine is ready for LLM-only operations (chat/"Ask") —
    /// deliberately does *not* require an embedder, unlike [`Self::is_ready`],
    /// since [`Self::ask`] degrades gracefully to a non-RAG direct answer
    /// when no embedder is configured (e.g. the Embedded local provider).
    pub fn is_llm_ready(&self) -> bool {
        self.config.enabled && self.llm.is_some()
    }

    /// Asks the model whether a question needs live information from the web.
    /// Returns `false` when no LLM is configured, so the caller degrades to
    /// the rule-based trigger alone rather than erroring.
    pub async fn classify_needs_web(&self, question: &str) -> bool {
        match self.llm.as_ref() {
            Some(llm) => research_intent::classify_needs_web(llm.as_ref(), question).await,
            None => false,
        }
    }

    /// GPU/CPU status of the loaded local LLM, if it reports one. Lets the
    /// command layer return fresh accelerator status after a "Retry on GPU"
    /// reload without going through a full [`Self::index_status`] (which needs
    /// a graph DB handle).
    pub fn llm_accelerator_status(&self) -> Option<crate::ai::traits::AcceleratorStatus> {
        self.llm.as_ref().and_then(|l| l.accelerator_status())
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
        let pending_pages = db.count_pending_reindex().unwrap_or(0);
        Ok(IndexStatus {
            indexed_chunks,
            total_blocks,
            pending_pages,
            embedder_ready: self.embedder.is_some() && self.vector_store.is_some(),
            llm_ready: self.is_llm_ready(),
            accelerator: self.llm.as_ref().and_then(|l| l.accelerator_status()),
        })
    }

    /// Rebuild the in-memory hash cache from vectors already stored for a
    /// graph, so a fresh process doesn't re-embed unchanged content on the
    /// next reindex. No-op once the cache is populated, or when there's no
    /// vector store. Best-effort — a failure just means some content may be
    /// needlessly re-embedded, never a wrong result.
    pub async fn restore_hash_cache(&self, graph_id: &str) -> Result<()> {
        let store = match self.vector_store.as_ref() {
            Some(s) => s,
            None => return Ok(()),
        };
        if !self.pipeline.read().await.hash_cache_is_empty() {
            return Ok(());
        }
        let pairs = store.list_content_hashes(graph_id).await?;
        if pairs.is_empty() {
            return Ok(());
        }
        let mut pipeline = self.pipeline.write().await;
        pipeline.preload_hashes(pairs);
        Ok(())
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

    /// Remove a page's vectors from the index — used when a page is deleted so
    /// stale vectors don't surface phantom citations in Chat. Deletion needs no
    /// embedder (nothing to embed), so this works even in degraded states as
    /// long as a vector store is present; a missing store is a silent no-op.
    /// Also drops the page's chunk hashes so a later re-create re-embeds it.
    pub async fn remove_page(&self, graph_id: &str, page_id: &str) -> Result<()> {
        if let Some(store) = &self.vector_store {
            store.delete_by_page(graph_id, page_id).await?;
        }
        self.pipeline.write().await.invalidate_page(page_id);
        Ok(())
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
        // Over-fetch each arm generously so fusion has a deep pool to work
        // with and semantic-only synonym matches aren't crowded out by
        // two-arm overlaps. Trivial cost at graph scale (a few thousand
        // blocks); the win is recall for queries like "upset" → "frustrated".
        let fetch = HYBRID_CANDIDATE_POOL.max(top_k);

        // Dense arm — best-effort. An empty index or a transient embedder
        // failure must not sink the whole query. Keep cosine scores so the
        // relevance gate and prompt-mode selection can use them.
        let dense_results: Vec<SearchResult> =
            if self.embedder.is_some() && self.vector_store.is_some() {
                match self.search(query, fetch, graph_id).await {
                    Ok(results) => results,
                    Err(_) => Vec::new(),
                }
            } else {
                Vec::new()
            };
        let cosine_by_id: std::collections::HashMap<String, f32> = dense_results
            .iter()
            .filter_map(|r| r.block_id.clone().map(|id| (id, r.score)))
            .collect();
        let vector_ids: Vec<String> = dense_results
            .into_iter()
            .filter_map(|r| r.block_id)
            .collect();

        // Sparse arm — best-effort BM25 over the graph's FTS index, using the
        // chat-query path (stopword-stripped, OR-joined) so a full question
        // retrieves results instead of ANDing every word and matching nothing.
        let fts_ids: Vec<String> = match db.search_fts_chat(query, fetch as i64) {
            Ok(blocks) => blocks.into_iter().map(|b| b.id).collect(),
            Err(_) => Vec::new(),
        };

        // Reserve slots for the top dense-only candidates (in the vector
        // ranking but not the FTS ranking) so a semantic synonym match that
        // only one arm found survives fusion's overlap bias.
        let fts_set: std::collections::HashSet<String> = fts_ids.iter().cloned().collect();
        let mut protected: std::collections::HashSet<String> = vector_ids
            .iter()
            .filter(|id| !fts_set.contains(id.as_str()))
            .take(DENSE_ONLY_RESERVED)
            .cloned()
            .collect();

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

        // Also protect the very top fused hits so a rank-#1 non-journal can
        // never be dropped by temporal reordering + truncation.
        for f in fused.iter().take(TOP_RRF_RESERVED) {
            protected.insert(f.id.clone());
        }

        // Hydrate the whole candidate pool so ordering + reservation have
        // material beyond `top_k` to work with; expansion happens later on
        // just the final hits.
        let intent = retrieval::detect_temporal_intent(query);
        let hydrate_count = fused.len().min(fetch);

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

        // Salient content terms drive both the sparse arm (above) and the
        // lexical branch of the relevance gate (below), so a BM25 hit counts
        // as evidence only when it shares a real content word with the query.
        let salient_terms = crate::db::chat_salient_terms(query);

        // Adaptive dense-relevance discriminator: judge the dense arm from the
        // spread of ALL its candidate cosines, not a fixed absolute floor. When
        // the answer isn't in the graph every candidate bunches at the model's
        // baseline similarity and the arm carries no signal, so its hits are
        // gated out en masse. Computed once over the whole candidate pool.
        let dense_cosines: Vec<f32> = cosine_by_id.values().copied().collect();
        let dense_has_signal = dense_arm_has_signal(&dense_cosines);

        if std::env::var_os("GRAFIUM_LOG_COSINES").is_some() {
            let mut sorted = dense_cosines.clone();
            sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            let top = sorted.first().copied().unwrap_or(0.0);
            let median = if sorted.is_empty() {
                0.0
            } else {
                sorted[sorted.len() / 2]
            };
            eprintln!(
                "cosine-log: query={query:?} candidates={} top={top:.3} median~={median:.3} \
                 margin={:.3} dense_has_signal={dense_has_signal}",
                sorted.len(),
                top - median,
            );
        }

        let hits: Vec<RetrievedHit> = top_ids
            .iter()
            .filter_map(|id| {
                let meta = meta_by_id.get(id.as_str())?;
                let (date_ms, note_created_ms) = Self::resolve_hit_dates(meta);
                let cosine = cosine_by_id.get(id.as_str()).copied();
                let lexical = fts_set.contains(id.as_str());
                Some(RetrievedHit {
                    block_id: meta.block_id.clone(),
                    page_id: meta.page_id.clone(),
                    page_title: meta.page_title.clone(),
                    content: meta.content.clone(),
                    date_ms,
                    note_created_ms,
                    is_journal: meta.is_journal,
                    score: score_by_id.get(id.as_str()).copied().unwrap_or(0.0),
                    cosine,
                    lexical,
                    parents: Vec::new(),
                    children: Vec::new(),
                })
            })
            // Calibration hook (opt-in): `GRAFIUM_LOG_COSINES=1` prints the
            // observed cosine + gate outcome for every candidate so the floor
            // and margin can be tuned against real data (see the summary line
            // logged above, and ASK_DENSE_RELATIVE_MARGIN's calibration table).
            .inspect(|h| {
                if std::env::var_os("GRAFIUM_LOG_COSINES").is_some() {
                    eprintln!(
                        "cosine-log:   pass={} lexical={} cosine={:?} title={:?} :: {}",
                        passes_relevance_gate(h, &salient_terms, dense_has_signal),
                        h.lexical,
                        h.cosine,
                        h.page_title,
                        h.content.chars().take(80).collect::<String>(),
                    );
                }
            })
            // Relevance gate (HIGH 5 + re-audit): drop candidates that aren't
            // genuine evidence. A dense hit must clear the absolute floor AND
            // belong to a dense arm that carries signal (top cosine stands clear
            // of the candidate median); a lexical hit must share a salient
            // content term (BM25 matches "work"/"how"/"explain" happily). This
            // keeps a general question ("explain how mutexes work") from
            // dragging irrelevant notes into the prompt once the index is
            // populated, while the empty-index BM25-only path is unaffected
            // (those hits match a salient term by construction).
            .filter(|h| passes_relevance_gate(h, &salient_terms, dense_has_signal))
            .collect();

        // Establish relevance ordering (temporal reorder is a soft boost, not
        // a partition), then truncate to top_k while guaranteeing reserved
        // (top-RRF and dense-only) hits survive.
        let ordered = if intent.is_temporal {
            retrieval::order_hits_temporally(hits, &intent)
        } else {
            hits
        };
        let hits = retrieval::finalize_hits(ordered, top_k, &protected);

        Ok(hits)
    }

    /// Resolve a hit's defensible *event* date and its "note saved"
    /// timestamp, kept separate so a note's creation/import time is never
    /// presented as when the described event actually happened (HIGH 4).
    ///
    /// - Journal page → the journal date is the event date.
    /// - Otherwise → an explicit ISO date written in the block's own text is
    ///   the event date, if present.
    /// - The block's `created_at` is always returned as `note_created`, used
    ///   only as a "note saved" hint, never for event ordering.
    fn resolve_hit_dates(meta: &crate::db::BlockPageMeta) -> (Option<i64>, Option<i64>) {
        let note_created = Some(meta.created_at);
        if meta.is_journal {
            if let Some(ms) = retrieval::journal_title_to_ms(&meta.page_title) {
                return (Some(ms), note_created);
            }
        }
        let event = retrieval::extract_content_date(&meta.content);
        (event, note_created)
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

        let request = self
            .build_ask_request(db, llm.as_ref(), question, graph_id, &[])
            .await?;

        let raw = llm
            .complete(
                &request.messages,
                &crate::ai::traits::CompletionOptions {
                    // Cap output so prompt + generation stays within n_ctx.
                    max_tokens: Some(request.output_tokens as u32),
                    ..Default::default()
                },
            )
            .await?;

        // Reasoning models wrap a chain-of-thought in <think>…</think>; strip
        // it, and if the model only reasoned (budget exhausted with no answer)
        // surface a clear message instead of raw chain-of-thought.
        let answer = match crate::ai::reasoning::strip_think_blocks(&raw) {
            crate::ai::reasoning::ThinkStripResult::Answer(a) => a,
            crate::ai::reasoning::ThinkStripResult::ReasoningOnly => {
                crate::ai::reasoning::REASONING_ONLY_MESSAGE.to_string()
            }
        };

        let sources = build_sources(&request.entries, &answer);
        Ok(AskResponse { answer, sources })
    }

    /// Streaming counterpart to [`Self::ask`]: runs the same retrieval,
    /// gating, budgeting and prompt assembly, then streams the model's answer
    /// token-by-token through `on_event`, hiding `<think>` reasoning behind a
    /// `Thinking` signal. `cancel` (if provided) can be flipped from the UI to
    /// abort a slow local generation. Returns the cited sources and, when the
    /// model produced only reasoning, a trailing message to show in place of
    /// an answer.
    pub async fn ask_stream(
        &self,
        db: &crate::db::Database,
        question: &str,
        graph_id: Option<&str>,
        history: &[ChatTurn],
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        on_event: &mut (dyn FnMut(AskStreamEvent<'_>) + Send),
    ) -> Result<AskStreamOutcome> {
        let llm = self
            .llm
            .as_ref()
            .ok_or_else(|| CoreError::Other("LLM not initialized".to_string()))?;

        // Retrieval runs first; announce it so the UI shows "Searching your
        // notes…" instead of a blank pause. This is fast, but it's honest.
        on_event(AskStreamEvent::Phase(AskPhase::Retrieving));

        let request = self
            .build_ask_request(db, llm.as_ref(), question, graph_id, history)
            .await?;

        let options = crate::ai::traits::CompletionOptions {
            max_tokens: Some(request.output_tokens as u32),
            cancel,
            ..Default::default()
        };

        // The prompt is built and about to be handed to the model. Until the
        // first token arrives the model is processing the prompt — the slow,
        // previously-silent phase — so label it explicitly.
        on_event(AskStreamEvent::Phase(AskPhase::ProcessingPrompt));

        let mut filter = crate::ai::reasoning::ThinkStreamFilter::new();
        // Track the last phase we announced so token-level updates only emit a
        // Phase event on a real transition (into Thinking, into Generating),
        // not once per token.
        let mut phase = AskPhase::ProcessingPrompt;
        {
            let mut on_token = |piece: &str| match filter.push(piece) {
                crate::ai::reasoning::StreamStep::Answer(delta) => {
                    if phase != AskPhase::Generating {
                        phase = AskPhase::Generating;
                        on_event(AskStreamEvent::Phase(AskPhase::Generating));
                    }
                    on_event(AskStreamEvent::Delta(&delta));
                }
                crate::ai::reasoning::StreamStep::Thinking => {
                    if phase != AskPhase::Thinking {
                        phase = AskPhase::Thinking;
                        on_event(AskStreamEvent::Phase(AskPhase::Thinking));
                    }
                }
                crate::ai::reasoning::StreamStep::Idle => {}
            };
            llm.complete_stream(&request.messages, &options, &mut on_token)
                .await?;
        }

        match filter.finish() {
            crate::ai::reasoning::ThinkStripResult::Answer(answer) => Ok(AskStreamOutcome {
                sources: build_sources(&request.entries, &answer),
                trailing_message: None,
                web_citations: Vec::new(),
            }),
            crate::ai::reasoning::ThinkStripResult::ReasoningOnly => Ok(AskStreamOutcome {
                sources: Vec::new(),
                trailing_message: Some(crate::ai::reasoning::REASONING_ONLY_MESSAGE.to_string()),
                web_citations: Vec::new(),
            }),
        }
    }

    /// Two-part "research on the web" answer: streams the ordinary
    /// notes-grounded answer under a `## From your notes` header, then runs a
    /// live [`crate::ai::web_research`] pass and streams its cited summary
    /// under `## From the web`. Used when
    /// [`crate::knowledge::detect_research_intent`] fires on the user's
    /// question — the deliberate, explicit gesture that authorises actually
    /// leaving the graph and hitting the internet.
    ///
    /// The two arms are isolated so a weakness in one never eats the other: if
    /// the notes arm finds nothing relevant it *says so* (rather than
    /// fabricating from general knowledge) and the web arm still runs; if the
    /// web arm fails — offline, blocked, no results — the notes answer above it
    /// still stands and a calm one-line note explains the web part didn't
    /// finish. `cancel` is honoured in *both* arms (the local generation and
    /// the web fetches), so a single Stop halts the whole run.
    pub async fn ask_stream_with_web(
        &self,
        db: &crate::db::Database,
        question: &str,
        graph_id: Option<&str>,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        on_event: &mut (dyn FnMut(AskStreamEvent<'_>) + Send),
    ) -> Result<AskStreamOutcome> {
        let browser = crate::scraping::HttpBrowserDriver::new();
        self.ask_stream_with_web_using(db, question, graph_id, &browser, cancel, on_event)
            .await
    }

    /// [`Self::ask_stream_with_web`] with an injected browser, so tests can
    /// drive the web arm from canned search results and pages instead of the
    /// live network. Production callers use the public wrapper above, which
    /// supplies the real [`crate::scraping::HttpBrowserDriver`].
    pub async fn ask_stream_with_web_using(
        &self,
        db: &crate::db::Database,
        question: &str,
        graph_id: Option<&str>,
        browser: &dyn crate::scraping::browser::BrowserDriver,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        on_event: &mut (dyn FnMut(AskStreamEvent<'_>) + Send),
    ) -> Result<AskStreamOutcome> {
        let llm = self
            .llm
            .as_ref()
            .ok_or_else(|| CoreError::Other("LLM not initialized".to_string()))?;

        // ── Part 1: From your notes ─────────────────────────────────────────
        // Shared with the Deep Research flow so both produce a byte-identical
        // notes section (see [`Self::stream_notes_arm`]).
        let notes_sources = self
            .stream_notes_arm(db, llm.as_ref(), question, graph_id, &cancel, on_event)
            .await?;

        // A Stop during the notes arm means the user doesn't want the web pass
        // either — return what we have without starting it.
        if cancel_requested(&cancel) {
            return Ok(AskStreamOutcome {
                sources: notes_sources,
                trailing_message: None,
                web_citations: Vec::new(),
            });
        }

        // ── Part 2: From the web ────────────────────────────────────────────
        on_event(AskStreamEvent::Delta("\n\n## From the web\n\n"));
        on_event(AskStreamEvent::Phase(AskPhase::SearchingWeb));

        let mut web_citations = Vec::new();
        let research = {
            // Translate the research engine's textual progress into UI signals:
            // the first "Reading source …" line flips the phase to
            // ReadingSources, and every line is forwarded verbatim as a Note so
            // the status label can show "Reading source 2/5: …".
            let mut announced_reading = false;
            let mut on_progress = |msg: &str| {
                if !announced_reading && msg.starts_with("Reading source") {
                    announced_reading = true;
                    on_event(AskStreamEvent::Phase(AskPhase::ReadingSources));
                }
                on_event(AskStreamEvent::Note(msg));
            };
            let engine = crate::ai::web_research::WebResearchEngine::new(llm.as_ref(), browser);
            // The cleaned question is both the "title" (what to answer) and the
            // seed text (what to search around) here — a Chat question has no
            // separate page body to draw on.
            engine
                .research_cancellable(question, question, cancel.as_deref(), &mut on_progress)
                .await
        };

        match research {
            Ok(result) => {
                on_event(AskStreamEvent::Phase(AskPhase::Generating));
                on_event(AskStreamEvent::Delta(&render_web_section(&result)));
                web_citations = result.citations;
            }
            // A cancelled web arm is a deliberate Stop, not a failure — stay
            // silent rather than scaring the user with an error they caused.
            Err(_) if cancel_requested(&cancel) => {}
            Err(e) => on_event(AskStreamEvent::Delta(&describe_web_failure(&e.to_string()))),
        }

        Ok(AskStreamOutcome {
            sources: notes_sources,
            trailing_message: None,
            web_citations,
        })
    }

    /// Stream the shared "From your notes" arm (Part 1 of the two-part flows):
    /// emit the header, run gated hybrid retrieval, stream the notes-only answer
    /// with `<think>` filtering, and return the sources the model cited.
    ///
    /// Extracted so [`Self::ask_stream_with_web_using`] and
    /// [`Self::ask_stream_with_deep_research_using`] produce an identical notes
    /// section — the only thing that differs between "research on the web" and
    /// "deep research" is the Part 2 engine, and duplicating ~60 lines of
    /// streaming/`<think>`-handling to say that would be a maintenance trap.
    async fn stream_notes_arm(
        &self,
        db: &crate::db::Database,
        llm: &dyn LlmProvider,
        question: &str,
        graph_id: Option<&str>,
        cancel: &Option<Arc<std::sync::atomic::AtomicBool>>,
        on_event: &mut (dyn FnMut(AskStreamEvent<'_>) + Send),
    ) -> Result<Vec<Source>> {
        // The header is answer text, streamed first so the reader immediately
        // sees the two-part structure forming.
        on_event(AskStreamEvent::Delta("## From your notes\n\n"));
        on_event(AskStreamEvent::Phase(AskPhase::Retrieving));

        let notes_request = self
            .build_notes_only_request(db, llm, question, graph_id)
            .await?;

        let mut notes_sources = Vec::new();
        match notes_request {
            // Gate rejected everything: be honest about the empty result rather
            // than letting the model paper over it with general knowledge —
            // that's what Part 2 is for.
            None => on_event(AskStreamEvent::Delta(
                "I couldn't find anything about this in your notes.",
            )),
            Some(request) => {
                on_event(AskStreamEvent::Phase(AskPhase::ProcessingPrompt));
                let options = crate::ai::traits::CompletionOptions {
                    max_tokens: Some(request.output_tokens as u32),
                    cancel: cancel.clone(),
                    ..Default::default()
                };
                let mut filter = crate::ai::reasoning::ThinkStreamFilter::new();
                let mut phase = AskPhase::ProcessingPrompt;
                {
                    let mut on_token = |piece: &str| match filter.push(piece) {
                        crate::ai::reasoning::StreamStep::Answer(delta) => {
                            if phase != AskPhase::Generating {
                                phase = AskPhase::Generating;
                                on_event(AskStreamEvent::Phase(AskPhase::Generating));
                            }
                            on_event(AskStreamEvent::Delta(&delta));
                        }
                        crate::ai::reasoning::StreamStep::Thinking => {
                            if phase != AskPhase::Thinking {
                                phase = AskPhase::Thinking;
                                on_event(AskStreamEvent::Phase(AskPhase::Thinking));
                            }
                        }
                        crate::ai::reasoning::StreamStep::Idle => {}
                    };
                    llm.complete_stream(&request.messages, &options, &mut on_token)
                        .await?;
                }
                match filter.finish() {
                    crate::ai::reasoning::ThinkStripResult::Answer(answer) => {
                        notes_sources = build_sources(&request.entries, &answer);
                    }
                    // Reasoning models can burn their whole budget thinking; show
                    // the standard placeholder instead of a blank notes section.
                    crate::ai::reasoning::ThinkStripResult::ReasoningOnly => on_event(
                        AskStreamEvent::Delta(crate::ai::reasoning::REASONING_ONLY_MESSAGE),
                    ),
                }
            }
        }
        Ok(notes_sources)
    }

    /// The two-part "notes + Deep Research" answer: the same "From your notes"
    /// section as [`Self::ask_stream_with_web`], followed by a "From the web"
    /// section produced by the multi-round [`crate::research::DeepResearchEngine`]
    /// instead of the single-round web-research pass.
    ///
    /// It intentionally shares the notes arm, the `AskStreamOutcome` shape, and
    /// the `render_web_section`/`describe_web_failure` rendering with the
    /// single-round flow, so the UI renders a Deep Research answer with no new
    /// rendering path. The one real difference — the multi-round loop — is
    /// confined to Part 2, and its finer-grained phases (planning, assessing,
    /// refining, synthesizing) are surfaced through the extra [`AskPhase`]
    /// variants.
    pub async fn ask_stream_with_deep_research(
        &self,
        db: &crate::db::Database,
        question: &str,
        graph_id: Option<&str>,
        config: &crate::research::ResearchConfig,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        on_event: &mut (dyn FnMut(AskStreamEvent<'_>) + Send),
    ) -> Result<AskStreamOutcome> {
        let browser = crate::scraping::HttpBrowserDriver::new();
        self.ask_stream_with_deep_research_using(
            db, question, graph_id, config, &browser, cancel, on_event,
        )
        .await
    }

    /// [`Self::ask_stream_with_deep_research`] with an injected browser, so tests
    /// can drive the Deep Research arm from canned engine responses and pages
    /// instead of the live network.
    #[allow(clippy::too_many_arguments)]
    pub async fn ask_stream_with_deep_research_using(
        &self,
        db: &crate::db::Database,
        question: &str,
        graph_id: Option<&str>,
        config: &crate::research::ResearchConfig,
        browser: &dyn crate::scraping::browser::BrowserDriver,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        on_event: &mut (dyn FnMut(AskStreamEvent<'_>) + Send),
    ) -> Result<AskStreamOutcome> {
        let llm = self
            .llm
            .as_ref()
            .ok_or_else(|| CoreError::Other("LLM not initialized".to_string()))?;

        // ── Part 1: From your notes (shared) ────────────────────────────────
        let notes_sources = self
            .stream_notes_arm(db, llm.as_ref(), question, graph_id, &cancel, on_event)
            .await?;

        // A Stop during the notes arm means the user doesn't want the web pass
        // either — return what we have without starting it.
        if cancel_requested(&cancel) {
            return Ok(AskStreamOutcome {
                sources: notes_sources,
                trailing_message: None,
                web_citations: Vec::new(),
            });
        }

        // ── Part 2: From the web (multi-round Deep Research) ─────────────────
        on_event(AskStreamEvent::Delta("\n\n## From the web\n\n"));

        let mut web_citations = Vec::new();
        let research = {
            // The agent emits structured phase/note progress; map its phases
            // onto the ask flow's phases and forward its notes verbatim so the
            // status label shows the live per-source / per-round play-by-play.
            let mut on_progress = |progress: crate::research::ResearchProgress| match progress {
                crate::research::ResearchProgress::Phase(phase) => {
                    on_event(AskStreamEvent::Phase(map_research_phase(phase)));
                }
                crate::research::ResearchProgress::Note(note) => {
                    on_event(AskStreamEvent::Note(note));
                }
            };
            let engine = crate::research::DeepResearchEngine::new(llm.as_ref(), browser, config);
            engine
                .research_cancellable(question, cancel.clone(), &mut on_progress)
                .await
        };

        match research {
            Ok(result) => {
                // Rendering the finished result is the answer-text phase, same
                // as the single-round flow.
                on_event(AskStreamEvent::Phase(AskPhase::Generating));
                on_event(AskStreamEvent::Delta(&render_web_section(&result)));
                web_citations = result.citations;
            }
            // A cancelled arm is a deliberate Stop, not a failure — stay silent.
            Err(_) if cancel_requested(&cancel) => {}
            Err(e) => on_event(AskStreamEvent::Delta(&describe_web_failure(&e.to_string()))),
        }

        Ok(AskStreamOutcome {
            sources: notes_sources,
            trailing_message: None,
            web_citations,
        })
    }
    /// prompt assembly — kept in one place so the blocking and streaming paths
    /// can never drift apart.
    async fn build_ask_request(
        &self,
        db: &crate::db::Database,
        llm: &dyn LlmProvider,
        question: &str,
        graph_id: Option<&str>,
        history: &[ChatTurn],
    ) -> Result<AskRequest> {
        // Reasoning models need more room: they spend tokens thinking *before*
        // answering, so reserve a larger output budget (which also shrinks the
        // retrieved-context budget symmetrically, keeping prompt + output
        // inside n_ctx).
        let reserved_output = if llm.supports_thinking() {
            ASK_THINKING_OUTPUT_TOKENS
        } else {
            ASK_RESERVED_OUTPUT_TOKENS
        };
        let budget = ask_context_budget_with(llm.context_window(), reserved_output);

        // Retrieval can't resolve "it"/"that", so a follow-up is searched
        // under the topic it refers back to rather than under its own
        // (contentless) words.
        let retrieval_query = conversation::resolve_followup(question, history);

        let mut hits = self
            .hybrid_search(db, &retrieval_query, ASK_TOP_K, graph_id)
            .await
            .unwrap_or_default();

        // Choose the prompt regime outside the model (HIGH 5) from the gated
        // hits, so it answers a single clear instruction instead of branching.
        // An explicit "answer from your own knowledge" in the question
        // overrides the retrieval-derived regime.
        let mode = choose_answer_mode_for(question, &hits);

        // The transcript and the retrieved notes share one context window, so
        // the notes budget is reduced by whatever the transcript will occupy.
        // Filling notes to the whole budget and *then* prepending history
        // overspends `n_ctx` by exactly the size of the conversation — which
        // grows silently as a thread gets longer, so it would surface as a
        // model that inexplicably degrades the more you talk to it.
        let history_tokens = conversation::history_budget(budget);
        let notes_budget = budget.saturating_sub(history_tokens);

        // In General mode we deliberately include no context at all — nothing
        // relevant was retrieved, so any notes would only contaminate a
        // general answer.
        let entries = if mode == AnswerMode::General {
            Vec::new()
        } else {
            self.expand_hits(db, &mut hits);
            retrieval::assemble_within_budget(&hits, notes_budget)
        };

        let context_block = build_context_block(&entries);
        let system_prompt = build_system_prompt(&context_block, mode);

        // Calibration hook (opt-in): `GRAFIUM_LOG_PROMPT_TOKENS=1` logs the
        // assembled prompt's estimated token count, so an over-large RAG
        // context (transcript pages) is measurable rather than guessed at.
        if std::env::var_os("GRAFIUM_LOG_PROMPT_TOKENS").is_some() {
            let est =
                retrieval::estimate_tokens(&system_prompt) + retrieval::estimate_tokens(question);
            eprintln!(
                "[grafium] ask prompt ~{est} tokens — {} context entries, mode {mode:?}, \
                 reserved_output {reserved_output}, context_budget {budget}",
                entries.len()
            );
        }

        // Replay the conversation so the model can resolve references itself
        // when it writes the answer — the rewrite above only fixes retrieval.
        // The thread is never truncated outright: turns too old to replay
        // verbatim are folded into a recap so a long conversation keeps its
        // continuity instead of silently forgetting its own beginning.
        let mut messages = vec![crate::ai::traits::ChatMessage {
            role: crate::ai::traits::MessageRole::System,
            content: system_prompt,
        }];
        let fitted = conversation::fit_history(history, history_tokens);
        if fitted.needs_compaction() {
            messages.push(crate::ai::traits::ChatMessage {
                role: crate::ai::traits::MessageRole::System,
                content: conversation::render_compaction(fitted.to_compact),
            });
        }
        for turn in fitted.verbatim {
            messages.push(crate::ai::traits::ChatMessage {
                role: if turn.is_user() {
                    crate::ai::traits::MessageRole::User
                } else {
                    crate::ai::traits::MessageRole::Assistant
                },
                content: turn.content.clone(),
            });
        }
        messages.push(crate::ai::traits::ChatMessage {
            role: crate::ai::traits::MessageRole::User,
            content: question.to_string(),
        });

        Ok(AskRequest {
            messages,
            entries,
            output_tokens: reserved_output,
        })
    }

    /// Planning for the "From your notes" arm of the two-part research flow:
    /// the same hybrid retrieval and gating as [`Self::build_ask_request`], but
    /// it returns `Ok(None)` when the relevance gate rejected everything,
    /// instead of falling back to a general-knowledge answer. That difference
    /// is the whole point of the split — Part 1 must speak *only* to what the
    /// user's own notes contain (and honestly admit when that's nothing),
    /// because here it is Part 2's live web pass, not the model's training
    /// data, that covers the outside world.
    async fn build_notes_only_request(
        &self,
        db: &crate::db::Database,
        llm: &dyn LlmProvider,
        question: &str,
        graph_id: Option<&str>,
    ) -> Result<Option<AskRequest>> {
        let reserved_output = if llm.supports_thinking() {
            ASK_THINKING_OUTPUT_TOKENS
        } else {
            ASK_RESERVED_OUTPUT_TOKENS
        };
        let budget = ask_context_budget_with(llm.context_window(), reserved_output);

        let mut hits = self
            .hybrid_search(db, question, ASK_TOP_K, graph_id)
            .await
            .unwrap_or_default();

        // General mode == nothing relevant survived the gate. Signal "no notes"
        // to the caller rather than assembling an empty, general-knowledge
        // prompt.
        if choose_answer_mode(&hits) == AnswerMode::General {
            return Ok(None);
        }

        self.expand_hits(db, &mut hits);
        let entries = retrieval::assemble_within_budget(&hits, budget);
        let context_block = build_context_block(&entries);
        let system_prompt = build_notes_only_system_prompt(&context_block);

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

        Ok(Some(AskRequest {
            messages,
            entries,
            output_tokens: reserved_output,
        }))
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
    /// Pages edited since their last index that are awaiting a background
    /// vector refresh — lets Chat show "N pages pending" instead of implying
    /// the index is perfectly current.
    pub pending_pages: usize,
    /// Whether an embedder + vector store are available (semantic indexing
    /// possible).
    pub embedder_ready: bool,
    /// Whether the LLM is ready for chat.
    pub llm_ready: bool,
    /// GPU/CPU status of the local LLM, when it can report one (embedded
    /// local provider only). Lets Chat warn the user when inference silently
    /// fell back to CPU — a 5–10× slowdown that otherwise looks like a hang.
    /// `None` for remote providers or when no LLM is loaded.
    pub accelerator: Option<crate::ai::traits::AcceleratorStatus>,
}

/// How many hits `ask` retrieves before context assembly.
const ASK_TOP_K: usize = 10;
/// Candidate pool fetched per retrieval arm before fusion. Deep enough that
/// semantic-only synonym matches aren't crowded out by two-arm overlaps;
/// trivial cost at graph scale.
const HYBRID_CANDIDATE_POOL: usize = 80;
/// Reserved final-result slots for top dense-only (semantic synonym)
/// candidates that only the vector arm found, so fusion's overlap bias can't
/// evict them.
const DENSE_ONLY_RESERVED: usize = 3;
/// Reserved final-result slots for the very top fused (RRF) hits, so a
/// rank-#1 non-journal is never dropped by temporal reordering + truncation.
const TOP_RRF_RESERVED: usize = 3;
/// Tokens reserved for the model's answer. Also used as the `ask`
/// completion's `max_tokens`, so prompt + output can never exceed `n_ctx`.
const ASK_RESERVED_OUTPUT_TOKENS: usize = 1024;
/// Larger output reservation for reasoning ("thinking") models: they spend
/// tokens on a `<think>` chain-of-thought *before* the answer, so a 1024-token
/// budget can be entirely consumed reasoning (observed: an 8B Qwen3 model used
/// its whole budget thinking and never answered). Reserving more here also
/// shrinks the retrieved-context budget symmetrically, so prompt + output
/// still fit inside `n_ctx`.
const ASK_THINKING_OUTPUT_TOKENS: usize = 2048;
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
/// The explicit `reserved_output` lets a reasoning model (which needs a larger
/// output allowance) still leave the retrieved context small enough that
/// prompt + output fit inside `n_ctx`.
fn ask_context_budget_with(context_window: Option<usize>, reserved_output: usize) -> usize {
    match context_window {
        Some(n_ctx) => n_ctx
            .saturating_sub(ASK_PROMPT_OVERHEAD_TOKENS)
            .saturating_sub(reserved_output)
            .max(ASK_MIN_CONTEXT_BUDGET_TOKENS),
        None => ASK_DEFAULT_CONTEXT_BUDGET_TOKENS,
    }
}

/// Minimum cosine similarity for a *dense-only* hit to be considered relevant
/// enough to enter the prompt. Below this, a nearest-neighbour is just the
/// closest of an irrelevant bunch and would only contaminate a general answer
/// (HIGH 5).
///
/// This is retained purely as an absolute *sanity backstop* — clearing it is
/// necessary but NOT sufficient. The primary discriminator is the relative
/// margin below, because every embedding model has a different baseline
/// similarity and the user can switch models freely, so no fixed absolute
/// floor is correct across models. Set `GRAFIUM_LOG_COSINES=1` to log observed
/// cosines (see [`KnowledgeEngine::hybrid_search`]) for re-calibration.
const ASK_SIMILARITY_FLOOR: f32 = 0.25;
/// Cosine at/above which a dense hit is treated as strong evidence, enough to
/// answer purely from notes rather than a cautious blend.
const ASK_STRONG_SIMILARITY: f32 = 0.6;

/// Primary dense-relevance discriminator: how far the best candidate cosine
/// must stand above the candidate *median* for the dense arm to be considered
/// to carry real signal. When a query's answer is genuinely present, the top
/// hit stands well clear of the pack; when it isn't, every candidate bunches
/// at the model's baseline similarity and the margin collapses. Measuring the
/// spread relative to the model's own distribution makes this model-agnostic
/// where a fixed absolute floor is brittle.
///
/// Calibrated 2026-09 against the user's real 8,288-chunk index with
/// **Qwen3-Embedding-0.6B** (via `GRAFIUM_LOG_COSINES`):
///
/// | query                        | top   | ~median | margin | verdict |
/// |------------------------------|-------|---------|--------|---------|
/// | what is a fresco             | 0.777 | ~0.51   | 0.27   | signal  |
/// | what does meticulous mean    | 0.791 | ~0.56   | 0.23   | signal  |
/// | explain how mutexes work     | 0.421 | ~0.40   | 0.02   | noise   |
///
/// 0.10 sits an order of magnitude below observed real signal (0.23–0.27) and
/// well above pure noise (0.02). Re-derive when the embedding model changes:
///   `GRAFIUM_LOG_COSINES=1 CHAT_E2E_QUERIES="q1|q2" cargo run -p grafium-core \
///     --release --features llm-local --example chat_e2e -- <graph> <data>`
const ASK_DENSE_RELATIVE_MARGIN: f32 = 0.10;

/// Decide whether the dense arm carries real signal for this query, from the
/// distribution of *all* its candidate cosines. Returns true only when the
/// best candidate both clears the absolute sanity floor and stands at least
/// [`ASK_DENSE_RELATIVE_MARGIN`] above the candidate median. On a query whose
/// answer isn't in the graph, the candidates bunch at the model's baseline,
/// the margin collapses, and this returns false → the dense arm contributes no
/// evidence and the answer falls back to general knowledge.
///
/// Degenerate cases are handled without panicking or dividing by zero: an
/// empty candidate set is "no signal"; a single candidate (or an all-identical
/// pack) has a zero margin and is likewise treated as no signal, since nothing
/// stands out from the pack.
fn dense_arm_has_signal(cosines: &[f32]) -> bool {
    let mut sorted: Vec<f32> = cosines.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let Some(&top) = sorted.last() else {
        return false;
    };
    if top < ASK_SIMILARITY_FLOOR {
        return false;
    }
    let n = sorted.len();
    let median = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    };
    top - median >= ASK_DENSE_RELATIVE_MARGIN
}

/// Relevance gate for a single hit. A hit is admissible evidence only if it is
/// genuinely on-topic:
///
/// - A **dense** hit must clear the cosine similarity floor *and* the dense arm
///   as a whole must carry signal (`dense_has_signal`, computed once per query
///   from the candidate distribution). This is what keeps a general question
///   whose answer isn't in the graph — where every candidate clusters at the
///   model's baseline similarity — from admitting the least-irrelevant note.
/// - A **lexical** (BM25) hit must match at least one *salient* query term —
///   a non-stopword content word. This is the fix for HIGH 5: BM25 happily
///   matches common filler ("work", "how", "explain"), so a bare `hit.lexical`
///   is NOT evidence. Requiring a salient-term overlap means a general
///   question like "explain how mutexes work" — whose only salient term is
///   "mutexes" — retrieves nothing from a graph that never mentions mutexes.
///
/// The empty-index BM25-only path is preserved: those hits are still lexical
/// and still pass as long as they match a salient term (they do, because the
/// sparse arm is itself built from the salient terms).
fn passes_relevance_gate(
    hit: &RetrievedHit,
    salient_terms: &[String],
    dense_has_signal: bool,
) -> bool {
    if hit.lexical {
        return content_matches_salient_term(&hit.content, salient_terms);
    }
    dense_has_signal && matches!(hit.cosine, Some(c) if c >= ASK_SIMILARITY_FLOOR)
}

/// True if `content` contains at least one of the `salient_terms`, matched
/// prefix-first to approximate the FTS `porter`/prefix (`*`) tokenizer (so
/// "painted" satisfies the salient term "paint"). An empty salient set never
/// matches — a question with no content terms has no lexical evidence.
fn content_matches_salient_term(content: &str, salient_terms: &[String]) -> bool {
    if salient_terms.is_empty() {
        return false;
    }
    let lower = content.to_lowercase();
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|tok| !tok.is_empty())
        .any(|tok| {
            salient_terms.iter().any(|term| {
                // Prefix match in either direction covers light stemming:
                // note "painted" vs query "paint", note "run" vs query
                // "running". Guard the reverse direction against trivially
                // short tokens so a two-letter word can't match everything.
                tok.starts_with(term.as_str()) || (term.starts_with(tok) && tok.len() >= 3)
            })
        })
}

/// Prompt regime chosen *outside* the model, so a small local model follows a
/// single unambiguous instruction instead of branching through a conditional
/// tree it tends to blur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnswerMode {
    /// No relevant notes — answer purely from general knowledge.
    General,
    /// Strong note evidence — answer from the notes and cite them.
    Notes,
    /// Only weak/partial note evidence — allow blending, kept clearly
    /// separated and labelled.
    Blend,
}

/// Decide the answering regime for a real user question, letting an explicit
/// instruction from the user outrank the retrieval scores.
///
/// [`choose_answer_mode`] looks only at what was retrieved, which is right
/// when the question is a genuine query about the graph. But it made Chat
/// refuse a direct request: because some note matched, "based on your
/// knowledge that you don't have in my notes" was answered under the notes
/// regime, which instructs the model to answer from the notes and say plainly
/// when they don't cover the question — so it replied "I do not have knowledge
/// outside of the notes provided." The user asked for general knowledge and
/// was told it didn't exist.
///
/// Retrieval confidence is a guess about intent; the words the user typed are
/// a statement of it, so the statement wins.
fn choose_answer_mode_for(question: &str, hits: &[RetrievedHit]) -> AnswerMode {
    if research_intent::wants_general_knowledge(question) {
        return AnswerMode::General;
    }
    choose_answer_mode(hits)
}

/// Decide the prompt regime from the gated hits. Empty → General. Any strong
/// hit (a term match or a high-cosine semantic match) → Notes. Otherwise only
/// weak semantic matches survived the gate → Blend.
fn choose_answer_mode(hits: &[RetrievedHit]) -> AnswerMode {
    if hits.is_empty() {
        return AnswerMode::General;
    }
    let strong = hits
        .iter()
        .any(|h| h.lexical || matches!(h.cosine, Some(c) if c >= ASK_STRONG_SIMILARITY));
    if strong {
        AnswerMode::Notes
    } else {
        AnswerMode::Blend
    }
}

/// Parse the set of `[N]` citation markers the model actually used, so the UI
/// shows only sources that informed the answer rather than everything
/// retrieved (HIGH 5). Pure manual scan — no regex dependency.
fn parse_cited_indices(answer: &str) -> std::collections::HashSet<usize> {
    let mut out = std::collections::HashSet::new();
    let bytes = answer.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            while j < n && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < n && bytes[j] == b']' {
                if let Ok(num) = answer[i + 1..j].parse::<usize>() {
                    out.insert(num);
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
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

/// Fully-planned inputs for a single `ask`, shared by the blocking and
/// streaming paths (see [`KnowledgeEngine::build_ask_request`]).
struct AskRequest {
    messages: Vec<crate::ai::traits::ChatMessage>,
    entries: Vec<ContextEntry>,
    /// Output-token allowance for this request (larger for reasoning models).
    output_tokens: usize,
}

/// A coarse phase of answering a question, emitted so the UI can show *what*
/// the model is doing right now (and, crucially, whether it's genuinely
/// working) instead of an undifferentiated spinner. Every phase is driven by a
/// real backend transition — never a timer — so a stalled or failed generation
/// stops reporting progress rather than lying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskPhase {
    /// Hybrid retrieval (BM25 + vector) is running. Fast (~250ms).
    Retrieving,
    /// The prompt has been assembled and handed to the model; it's processing
    /// the prompt (tokenize + eval) but hasn't emitted any output yet. This is
    /// the phase that grows slow with a large retrieved context and, without a
    /// label, is indistinguishable from a hang.
    ProcessingPrompt,
    /// The model is reasoning inside a `<think>` block — no answer text will
    /// appear yet, and the raw chain-of-thought is never forwarded.
    Thinking,
    /// Real answer tokens are streaming.
    Generating,
    /// A live web-research pass is running its planned searches — only in the
    /// two-part "research on the web" flow ([`KnowledgeEngine::ask_stream_with_web`]).
    /// Distinct from `Retrieving`, which is the *local* notes search; this one
    /// is hitting the open internet.
    SearchingWeb,
    /// The web-research pass has chosen its sources and is fetching and reading
    /// them (the slowest part of a research run, gated by network latency).
    ReadingSources,
    /// Deep Research is planning its search queries from the question (the
    /// opening LLM step of the multi-round agentic loop,
    /// [`KnowledgeEngine::ask_stream_with_deep_research`]).
    Planning,
    /// Deep Research is judging whether the sources gathered so far are enough
    /// to answer the question — the step that decides whether to loop again.
    Assessing,
    /// Deep Research found the material insufficient and is proposing better
    /// queries to fill the gap before searching again.
    Refining,
    /// Deep Research has enough and is writing the final cited answer.
    Synthesizing,
}

impl AskPhase {
    /// Stable lowercase identifier for the UI/event wire format.
    pub fn as_str(self) -> &'static str {
        match self {
            AskPhase::Retrieving => "retrieving",
            AskPhase::ProcessingPrompt => "processing_prompt",
            AskPhase::Thinking => "thinking",
            AskPhase::Generating => "generating",
            AskPhase::SearchingWeb => "searching_web",
            AskPhase::ReadingSources => "reading_sources",
            AskPhase::Planning => "planning",
            AskPhase::Assessing => "assessing",
            AskPhase::Refining => "refining",
            AskPhase::Synthesizing => "synthesizing",
        }
    }
}

/// An event emitted while streaming an answer via
/// [`KnowledgeEngine::ask_stream`].
pub enum AskStreamEvent<'a> {
    /// The answering phase changed — the UI should update its status label.
    /// Emitted only on genuine transitions (deduplicated), so its arrival is
    /// itself evidence the backend is alive.
    Phase(AskPhase),
    /// A chunk of answer text to append in the UI.
    Delta(&'a str),
    /// A human-readable progress note for the *current* phase — e.g.
    /// "Reading source 2/5: …" during a web-research pass. Unlike `Phase` (a
    /// coarse, machine-readable state) a `Note` carries transient detail the UI
    /// can show verbatim under the status label and then discard. It is never
    /// part of the answer text, so dropping it loses nothing but the live
    /// play-by-play.
    Note(&'a str),
}

/// Result of a streamed answer once generation finishes.
pub struct AskStreamOutcome {
    /// Sources the model actually cited (empty for a general answer).
    pub sources: Vec<Source>,
    /// A message to display *in place of* an answer when the model produced
    /// only reasoning and never answered — `None` for a normal answer.
    pub trailing_message: Option<String>,
    /// Web sources cited by the "From the web" section, when the two-part
    /// research flow ran ([`KnowledgeEngine::ask_stream_with_web`]). Empty for
    /// an ordinary graph-only answer, so existing callers are unaffected.
    pub web_citations: Vec<crate::ai::web_research::Citation>,
}

/// Build the cited-sources list for an answer: only entries whose `[N]` marker
/// the model actually referenced (HIGH 5). General/reasoning-only answers cite
/// nothing and so carry no sources.
fn build_sources(entries: &[ContextEntry], answer: &str) -> Vec<Source> {
    let cited = parse_cited_indices(answer);
    entries
        .iter()
        .filter(|e| cited.contains(&e.index))
        .map(|e| Source {
            index: e.index,
            page_id: e.page_id.clone(),
            page_title: e.page_title.clone(),
            block_id: e.block_id.clone(),
            // Only a defensible *event* date is surfaced to the UI chip; a
            // note's saved/imported timestamp is never presented as when the
            // event happened (HIGH 4).
            date: e.date_ms.map(retrieval::format_date_ms),
        })
        .collect()
}

/// Render assembled context entries into numbered, dated prompt lines. The
/// date label distinguishes a defensible *event* date (a journal date or an
/// explicit date written in the note) from a mere "note saved" timestamp, so
/// the model never reports an import time as when something happened (HIGH 4):
/// - journal event date → `[N] 2026-03-14 (journal) — from "Title":`
/// - explicit in-note date → `[N] 2026-03-14 (dated in note) — from "Title":`
/// - only a saved timestamp → `[N] note saved 2026-03-14; event date unknown — from "Title":`
/// - nothing → `[N] undated — from "Title":`
fn build_context_block(entries: &[ContextEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        let label = match (e.date_ms, e.note_created_ms) {
            (Some(ms), _) if e.is_journal => {
                format!("{} (journal)", retrieval::format_date_ms(ms))
            }
            (Some(ms), _) => format!("{} (dated in note)", retrieval::format_date_ms(ms)),
            (None, Some(created)) => format!(
                "note saved {}; event date unknown",
                retrieval::format_date_ms(created)
            ),
            (None, None) => "undated".to_string(),
        };
        out.push_str(&format!(
            "[{}] {} — from \"{}\":\n{}\n\n",
            e.index, label, e.page_title, e.text
        ));
    }
    out.trim_end().to_string()
}

/// Whether a cancellation flag (if any) has been tripped. Mirrors the check
/// the web-research engine makes internally, so the two-part flow can decide
/// between arms whether the user has asked to stop.
fn cancel_requested(cancel: &Option<Arc<std::sync::atomic::AtomicBool>>) -> bool {
    cancel
        .as_ref()
        .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
}

/// System prompt for the "From your notes" arm of the two-part research flow.
/// Unlike [`build_system_prompt`]'s Blend/General regimes — which are allowed
/// to reach for general knowledge — this one forbids it outright: Part 1
/// answers *only* from the retrieved notes and admits when they don't cover the
/// question, because the companion "From the web" section is what supplies
/// outside information in this flow. Keeping the two strictly separate is what
/// makes the split honest rather than two overlapping general answers.
fn build_notes_only_system_prompt(context_block: &str) -> String {
    format!(
        "You are Grafium's assistant answering STRICTLY from the user's own notes below — their \
personal writing and journal entries. Use ONLY these notes; do NOT add anything from your general \
knowledge in this section (a separate web-research section handles outside information). Cite each \
claim with its [N] marker.\n\
- For \"when\" / temporal questions, use only explicit dates from the cited notes. A line that \
says \"note saved …; event date unknown\" is not an event date — don't treat it as one.\n\
- Never invent citations or dates. If these notes don't contain the answer, say so plainly in one \
sentence and stop — do not guess.\n\n\
The user's notes (each prefixed with its [N] citation marker and date):\n\n{context_block}"
    )
}

/// Turns a web-research failure into something the user can act on.
///
/// A raw error was being shown verbatim, so a routine throttle appeared as a
/// wall of URL and HTTP status. Rate limiting in particular is both the most
/// common failure and the most recoverable — a research run fires several
/// queries back to back, which is the exact burst engines throttle — so it is
/// worth saying plainly that waiting will fix it, rather than implying
/// something is broken.
/// Map a [`crate::research::ResearchPhase`] onto the ask flow's [`AskPhase`] so
/// a Deep Research run drives the same status label as an ordinary answer. The
/// three loop-specific phases (assessing, refining, and the planning step) have
/// dedicated `AskPhase` variants; search and read reuse the existing web-flow
/// phases so the two flows look identical when they overlap.
fn map_research_phase(phase: crate::research::ResearchPhase) -> AskPhase {
    match phase {
        crate::research::ResearchPhase::Planning => AskPhase::Planning,
        crate::research::ResearchPhase::Searching => AskPhase::SearchingWeb,
        crate::research::ResearchPhase::Reading => AskPhase::ReadingSources,
        crate::research::ResearchPhase::Assessing => AskPhase::Assessing,
        crate::research::ResearchPhase::Refining => AskPhase::Refining,
        crate::research::ResearchPhase::Synthesizing => AskPhase::Synthesizing,
    }
}

fn describe_web_failure(error: &str) -> String {
    let rate_limited = error.contains("429") || error.to_lowercase().contains("too many requests");
    if rate_limited {
        "I couldn't search the web just now — the search engine is rate-limiting requests \
         (too many searches in a short time). This usually clears within a minute or two; \
         your notes answer above is unaffected."
            .to_string()
    } else {
        format!(
            "I couldn't complete the web research just now ({error}). Your notes answer above is \
             unaffected — you can try again."
        )
    }
}

/// Render a completed [`crate::ai::web_research::WebResearchResult`] into the
/// Markdown body of the "From the web" section: the direct answer (when the
/// research posed one) followed by each cited topic paragraph, whose inline
/// `[n]` markers point at the numbered web sources the UI renders as clickable
/// chips. Returns a short honest line when the run produced no prose at all, so
/// the section is never left blank under its header.
fn render_web_section(result: &crate::ai::web_research::WebResearchResult) -> String {
    let mut out = String::new();
    if let Some(answer) = result.title_answer.as_deref() {
        let answer = answer.trim();
        if !answer.is_empty() {
            out.push_str(answer);
            out.push_str("\n\n");
        }
    }
    for topic in &result.topics {
        let summary = topic.summary.trim();
        if summary.is_empty() {
            continue;
        }
        let heading = topic.topic.trim();
        if !heading.is_empty() {
            out.push_str("**");
            out.push_str(heading);
            out.push_str("**\n\n");
        }
        out.push_str(summary);
        out.push_str("\n\n");
    }
    let out = out.trim_end().to_string();
    if out.is_empty() {
        "I searched the web but couldn't extract a useful summary from the sources.".to_string()
    } else {
        out
    }
}

/// Build the system prompt for a single chosen `AnswerMode`. Each regime is a
/// single unambiguous instruction (rather than a conditional tree a small
/// local model would blur): never "answer only from context" in a way that
/// breaks general questions, and never letting general knowledge be passed off
/// as the user's notes.
fn build_system_prompt(context_block: &str, mode: AnswerMode) -> String {
    let base = "You are Grafium's assistant. You help the user with BOTH questions about their \
personal knowledge graph (their notes) AND general questions using your own knowledge.";
    match mode {
        AnswerMode::General => format!(
            "{base}\n\n\
No relevant notes were retrieved from the user's graph for this question. Answer from your \
own general knowledge, and make clear this is general knowledge and not from their notes. If \
you don't know, say you don't know."
        ),
        AnswerMode::Notes => format!(
            "{base}\n\n\
Answer using the retrieved notes below, which are the user's own writing and journal entries. \
Cite each claim with its [N] marker.\n\
- For \"when\" / temporal questions, state the explicit date(s) from the cited notes. Journal \
entries are dated — use those dates. A line that says \"note saved …; event date unknown\" \
means you only know when the note was saved, NOT when the event happened: say you found the \
note but can't establish when it happened. Never present a \"note saved\" date as the event date.\n\
- Never invent citations or dates.\n\
- If these notes don't actually answer the question, say so in one short sentence, then answer \
from your own general knowledge and label that part clearly as general knowledge rather than \
something from their notes. Never refuse a question just because the notes don't cover it, and \
never claim you have no knowledge outside their notes — you do.\n\n\
Retrieved notes (each prefixed with its [N] citation marker and date):\n\n{context_block}"
        ),
        AnswerMode::Blend => format!(
            "{base}\n\n\
Some possibly-related notes were retrieved, but they may only partially answer the question. \
Use them where genuinely relevant and cite those parts with their [N] marker; answer the rest \
from your general knowledge and clearly label which parts are general knowledge versus from \
their notes. Keep the two separate.\n\
- For \"when\" / temporal questions, only use explicit dates from the cited notes. A line that \
says \"note saved …; event date unknown\" is not an event date — don't treat it as one.\n\
- Never invent citations or dates, and never present general knowledge as if it came from their \
notes. If you don't know, say you don't know.\n\n\
Retrieved notes (each prefixed with its [N] citation marker and date):\n\n{context_block}"
        ),
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

    fn block_with(id: &str, content: &str) -> Block {
        Block {
            id: id.to_string(),
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

    fn test_engine_without_providers() -> Result<KnowledgeEngine> {
        let config = AiConfig::default();
        let registry_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("graph_registry.engine-test.json");
        Ok(KnowledgeEngine {
            config: config.clone(),
            llm: None,
            embedder: None,
            vector_store: None,
            pipeline: RwLock::new(EmbeddingPipeline::new(config.embedding.clone())),
            reference_engine: ReferenceEngine::new(config.references.clone()),
            registry: RwLock::new(GraphRegistry::load(&registry_path)?),
            data_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            models_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        })
    }

    fn mk_hit(id: &str) -> RetrievedHit {
        RetrievedHit {
            block_id: id.to_string(),
            page_id: "pg".to_string(),
            page_title: "Page".to_string(),
            content: "content".to_string(),
            date_ms: None,
            note_created_ms: None,
            is_journal: false,
            score: 1.0,
            cosine: None,
            lexical: false,
            parents: Vec::new(),
            children: Vec::new(),
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
    async fn one_char_edit_reembeds_only_the_changed_chunk() -> Result<()> {
        // Auto-index unit-of-work is the whole page, but the content-hash diff
        // must keep the actual embedding cost proportional to the edit: a
        // one-character change to a single leaf block re-embeds exactly one
        // chunk, not the whole page.
        let (mock_embedder, embedder_state) = MockEmbedder::new(4);
        let store = Arc::new(MockVectorStore::new(false));
        let engine = test_engine(Box::new(mock_embedder), store.clone())?;
        let page = test_page();
        let mut blocks = vec![
            block_with("b1", "The first block has enough content to index."),
            block_with("b2", "The second block also has plenty of content here."),
            block_with("b3", "A third block, likewise long enough to be embedded."),
        ];

        assert_eq!(engine.index_page(&page, &blocks, "graph-1").await?, 3);
        assert_eq!(embedder_state.lock().unwrap().calls, 1);

        blocks[1].content.push('!');
        assert_eq!(
            engine.index_page(&page, &blocks, "graph-1").await?,
            1,
            "only the edited block's chunk should be re-embedded"
        );
        assert_eq!(store.snapshot().stored_chunks, 3);
        Ok(())
    }

    #[tokio::test]
    async fn removing_a_block_deletes_its_vector_on_reindex() -> Result<()> {
        let (mock_embedder, _) = MockEmbedder::new(4);
        let store = Arc::new(MockVectorStore::new(false));
        let engine = test_engine(Box::new(mock_embedder), store.clone())?;
        let page = test_page();
        let blocks = vec![
            block_with("b1", "The first block has enough content to index."),
            block_with("b2", "The second block also has plenty of content here."),
        ];
        assert_eq!(engine.index_page(&page, &blocks, "graph-1").await?, 2);
        assert_eq!(store.snapshot().stored_chunks, 2);

        // b2 deleted from the page → its stored vector must go away.
        let remaining = vec![blocks[0].clone()];
        engine.index_page(&page, &remaining, "graph-1").await?;
        let snap = store.snapshot();
        assert_eq!(snap.stored_chunks, 1);
        assert!(snap.delete_chunks_calls >= 1);
        Ok(())
    }

    #[tokio::test]
    async fn remove_page_deletes_all_of_its_vectors() -> Result<()> {
        let (mock_embedder, _) = MockEmbedder::new(4);
        let store = Arc::new(MockVectorStore::new(false));
        let engine = test_engine(Box::new(mock_embedder), store.clone())?;
        let page = test_page();
        let blocks = vec![
            block_with("b1", "The first block has enough content to index."),
            block_with("b2", "The second block also has plenty of content here."),
        ];
        engine.index_page(&page, &blocks, "graph-1").await?;
        assert_eq!(store.snapshot().stored_chunks, 2);

        engine.remove_page("graph-1", &page.id).await?;
        let snap = store.snapshot();
        assert_eq!(snap.delete_by_page_calls, 1);
        assert_eq!(snap.stored_chunks, 0);
        Ok(())
    }

    #[tokio::test]
    async fn auto_index_is_a_silent_noop_without_an_embedder() -> Result<()> {
        // The drainer gates on can_index(); with no providers it must stay
        // idle, and the engine's removal path must be a no-op (not an error) so
        // a delete while AI is disabled never surfaces a toast or a retry storm.
        let engine = test_engine_without_providers()?;
        assert!(!engine.is_ready());
        assert!(!engine.can_index());

        engine.remove_page("graph-1", "page-1").await?;

        // Reindex without an embedder fails cleanly (drainer logs + leaves it
        // pending), never panics.
        let page = test_page();
        let blocks = vec![test_block(
            "This block content is long enough to be indexed.",
        )];
        assert!(engine.index_page(&page, &blocks, "graph-1").await.is_err());
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

    /// A deterministic, concept-based embedder for retrieval-pipeline tests.
    ///
    /// It maps each text to a sparse vector over a fixed set of *concepts*
    /// (synonym clusters), so semantically-related wording — "upset" in a
    /// query vs "frustrated"/"rough day" in a note — lands on the same
    /// dimension and yields high cosine similarity, while unrelated topics stay
    /// orthogonal. A tiny always-on baseline dimension avoids zero-norm
    /// vectors. This simulates a real embedding model's semantic similarity
    /// closely enough to exercise fusion, the relevance gate, and ordering; it
    /// is NOT a real embedder.
    struct ConceptEmbedder {
        concepts: Vec<Vec<&'static str>>,
    }

    impl ConceptEmbedder {
        fn new() -> Self {
            Self {
                concepts: vec![
                    // 0: distress / "upset"
                    vec![
                        "upset",
                        "frustrated",
                        "rough",
                        "angry",
                        "annoyed",
                        "mad",
                        "cross",
                    ],
                    // 1: painting / decorating
                    vec![
                        "paint",
                        "painted",
                        "painting",
                        "bedroom",
                        "room",
                        "wall",
                        "renovation",
                    ],
                    // 2: running / exercise
                    vec!["run", "running", "ran", "jog", "exercise"],
                    // 3: food / cooking
                    vec!["pasta", "dinner", "cooked", "garlic", "recipe", "food"],
                    // 4: concurrency (present in NO fixture note on purpose)
                    vec!["mutex", "mutexes", "thread", "lock", "concurrency"],
                ],
            }
        }

        fn vectorize(&self, text: &str) -> Vec<f32> {
            let lower = text.to_lowercase();
            let mut v = vec![0.0f32; self.concepts.len() + 1];
            for (i, keywords) in self.concepts.iter().enumerate() {
                if keywords.iter().any(|kw| lower.contains(kw)) {
                    v[i] = 1.0;
                }
            }
            // Baseline dimension so unrelated texts never have a zero norm.
            *v.last_mut().unwrap() = 0.01;
            v
        }
    }

    impl Embedder for ConceptEmbedder {
        fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
            Box::pin(async move { Ok(texts.iter().map(|t| self.vectorize(t)).collect()) })
        }
        fn dimension(&self) -> usize {
            self.concepts.len() + 1
        }
        fn model_name(&self) -> &str {
            "concept-test-embedder"
        }
    }

    /// End-to-end retrieval verification against the reviewers' required
    /// fixture and the user's two literal queries, plus the general-question
    /// negative case. Uses a real SQLite FTS index and a real vector store;
    /// only the embedder is simulated (see [`ConceptEmbedder`]).
    #[tokio::test]
    async fn literal_user_queries_retrieve_the_right_block() -> Result<()> {
        use crate::db::Database;
        use crate::models::BlockType;

        let graph_id = "fixture-graph";
        let store = Arc::new(SqliteVectorStore::in_memory()?);
        let engine = test_engine(Box::new(ConceptEmbedder::new()), store)?;

        let db = Database::in_memory()?;

        // Journal decoy (recent, unrelated) — must not win the "upset" query.
        let j1 = db.create_page("2026-03-20", true)?;
        db.create_block(
            &j1.id,
            None,
            0,
            "cooked a nice pasta dinner with garlic",
            BlockType::Text,
            json!({}),
        )?;
        // The "upset" answer, phrased ONLY with a synonym — no literal "upset".
        let j2 = db.create_page("2026-03-14", true)?;
        db.create_block(
            &j2.id,
            None,
            0,
            "had a really rough day, felt frustrated about everything",
            BlockType::Text,
            json!({}),
        )?;
        // Older journal decoy.
        let j3 = db.create_page("2026-02-01", true)?;
        db.create_block(
            &j3.id,
            None,
            0,
            "went for a long run in the park this morning",
            BlockType::Text,
            json!({}),
        )?;
        // Single-mention non-journal block: the "paint" answer, on a
        // home/renovation page (NOT a journal).
        let reno = db.create_page("home/renovation", false)?;
        db.create_block(
            &reno.id,
            None,
            0,
            "painted the bedroom today, finally finished it",
            BlockType::Text,
            json!({}),
        )?;
        // Unrelated general content.
        let recipes = db.create_page("Recipes", false)?;
        db.create_block(
            &recipes.id,
            None,
            0,
            "pasta with garlic and olive oil is a quick meal",
            BlockType::Text,
            json!({}),
        )?;

        // Index every page into the vector store (block ids match the DB).
        for page in [&j1, &j2, &j3, &reno, &recipes] {
            let blocks = db.list_blocks_for_page(&page.id)?;
            engine.index_page(page, &blocks, graph_id).await?;
        }

        // Query 1: "when was the last time I was upset" — only the synonym
        // block answers it, and only semantic retrieval can find it (FTS for
        // "upset" matches nothing here).
        let upset_hits = engine
            .hybrid_search(
                &db,
                "when was the last time I was upset",
                10,
                Some(graph_id),
            )
            .await?;
        let upset_block = db.list_blocks_for_page(&j2.id)?[0].id.clone();
        assert!(
            upset_hits.iter().any(|h| h.block_id == upset_block),
            "the 'frustrated / rough day' block must be retrieved for the upset query; got {:?}",
            upset_hits.iter().map(|h| &h.content).collect::<Vec<_>>()
        );
        assert_ne!(choose_answer_mode(&upset_hits), AnswerMode::General);

        // Query 2: "when did I paint my room" — the single-mention
        // non-journal block must survive even amid journal hits (no hard
        // journal partition).
        let paint_hits = engine
            .hybrid_search(&db, "when did I paint my room", 10, Some(graph_id))
            .await?;
        let paint_block = db.list_blocks_for_page(&reno.id)?[0].id.clone();
        assert!(
            paint_hits.iter().any(|h| h.block_id == paint_block),
            "the 'painted the bedroom' block must be retrieved for the paint query; got {:?}",
            paint_hits.iter().map(|h| &h.content).collect::<Vec<_>>()
        );
        assert_ne!(choose_answer_mode(&paint_hits), AnswerMode::General);

        // Negative case: a general question with no relevant notes must clear
        // the relevance gate to nothing, so Chat answers from general
        // knowledge rather than dragging in irrelevant notes.
        let mutex_hits = engine
            .hybrid_search(&db, "explain how mutexes work", 10, Some(graph_id))
            .await?;
        assert!(
            mutex_hits.is_empty(),
            "a general question must retrieve no context past the gate; got {:?}",
            mutex_hits.iter().map(|h| &h.content).collect::<Vec<_>>()
        );
        assert_eq!(choose_answer_mode(&mutex_hits), AnswerMode::General);

        Ok(())
    }

    /// Real-data-shaped regression for the relevance gate (HIGH 5, re-audit):
    /// the user's real graph is study/PACER notes + GRE vocabulary, none of
    /// which mention concurrency. A general question ("explain how mutexes
    /// work") must retrieve *nothing* even though the notes are full of common
    /// words BM25 could latch onto ("work", "read", "store", "concept") — the
    /// exact case that leaked 8 irrelevant hits on the live graph. A salient
    /// question must still retrieve its note, proving the gate isn't just
    /// rejecting everything.
    #[tokio::test]
    async fn general_question_over_a_study_graph_retrieves_no_context() -> Result<()> {
        use crate::db::Database;
        use crate::models::BlockType;

        let graph_id = "study-graph";
        let store = Arc::new(SqliteVectorStore::in_memory()?);
        let engine = test_engine(Box::new(ConceptEmbedder::new()), store)?;
        let db = Database::in_memory()?;

        // A study/PACER page, deliberately dense with generic words that BM25
        // would match against filler in a question ("work", "read", "store").
        let pacer = db.create_page("PACER - Tag What You Read", false)?;
        for line in [
            "Digest by: store it under its concept and link the pages",
            "Map it: create or link a concept page for what you read",
            "rehearse needs active-recall practice to make the work stick",
            "inbox: captured, not yet digested",
        ] {
            db.create_block(&pacer.id, None, 0, line, BlockType::Text, json!({}))?;
        }
        // A GRE vocabulary page — the salient-question positive control.
        let vocab = db.create_page("GRE Vocab", false)?;
        db.create_block(
            &vocab.id,
            None,
            0,
            "fresco (wall painting): a mural done on fresh plaster",
            BlockType::Text,
            json!({}),
        )?;

        for page in [&pacer, &vocab] {
            let blocks = db.list_blocks_for_page(&page.id)?;
            engine.index_page(page, &blocks, graph_id).await?;
        }

        // The reported failure: a general concurrency question against a graph
        // that never mentions concurrency must yield zero context + General,
        // NOT a pile of study notes that merely share the word "work".
        let mutex_hits = engine
            .hybrid_search(&db, "explain how mutexes work", 10, Some(graph_id))
            .await?;
        assert!(
            mutex_hits.is_empty(),
            "study notes must not leak into a general concurrency question; got {:?}",
            mutex_hits.iter().map(|h| &h.content).collect::<Vec<_>>()
        );
        assert_eq!(choose_answer_mode(&mutex_hits), AnswerMode::General);

        // Positive control: a salient question still retrieves its note, so the
        // gate isn't simply suppressing everything.
        let fresco_hits = engine
            .hybrid_search(&db, "what is a fresco", 10, Some(graph_id))
            .await?;
        assert!(
            fresco_hits.iter().any(|h| h.content.contains("fresco")),
            "a salient vocab question must still retrieve its note; got {:?}",
            fresco_hits.iter().map(|h| &h.content).collect::<Vec<_>>()
        );
        assert_ne!(choose_answer_mode(&fresco_hits), AnswerMode::General);

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
                note_created_ms: Some(0),
                is_journal: true,
                text: "felt upset about the deadline".to_string(),
            },
            ContextEntry {
                index: 2,
                block_id: "b2".to_string(),
                page_id: "p2".to_string(),
                page_title: "Rust".to_string(),
                date_ms: None,
                note_created_ms: None,
                is_journal: false,
                text: "ownership and borrowing".to_string(),
            },
            // Non-journal with only a saved timestamp: must NOT be presented
            // as an event date (HIGH 4).
            ContextEntry {
                index: 3,
                block_id: "b3".to_string(),
                page_id: "p3".to_string(),
                page_title: "home/renovation".to_string(),
                date_ms: None,
                note_created_ms: retrieval::journal_title_to_ms("2026-01-02"),
                is_journal: false,
                text: "painted the bedroom".to_string(),
            },
        ];
        let block = build_context_block(&entries);
        assert!(block.contains("[1] 2026-03-14 (journal) — from \"2026-03-14\":"));
        assert!(block.contains("felt upset about the deadline"));
        assert!(block.contains("[2] undated — from \"Rust\":"));
        assert!(!block.contains("(journal) — from \"Rust\""));
        assert!(block
            .contains("[3] note saved 2026-01-02; event date unknown — from \"home/renovation\":"));
    }

    #[test]
    fn context_block_is_empty_without_entries() {
        assert!(build_context_block(&[]).is_empty());
    }

    #[test]
    fn system_prompt_supports_blended_answering() {
        let notes = build_system_prompt("[1] 2026-03-14 — from \"x\":\nhi", AnswerMode::Notes);
        // Must not force "only answer from context".
        assert!(!notes.to_lowercase().contains("only answer from"));
        assert!(notes.contains("Retrieved notes"));
        assert!(notes.contains("[1] 2026-03-14"));

        let blend = build_system_prompt("[1] x", AnswerMode::Blend);
        assert!(blend.contains("Retrieved notes"));
        assert!(blend.contains("general knowledge"));

        let general = build_system_prompt("", AnswerMode::General);
        assert!(general.contains("No relevant notes"));
        assert!(general.contains("general knowledge"));
    }

    #[test]
    fn relevance_gate_requires_salient_overlap_for_lexical_and_a_floor_for_dense() {
        // A lexical hit whose content shares a *salient* term with the query
        // is real evidence and passes — regardless of the dense arm.
        let mut salient_match = mk_hit("a");
        salient_match.lexical = true;
        salient_match.cosine = None;
        salient_match.content = "painted the bedroom today".to_string();
        assert!(passes_relevance_gate(
            &salient_match,
            &["paint".to_string(), "room".to_string()],
            false,
        ));

        // A lexical hit that only overlaps on filler must be REJECTED — this is
        // the HIGH 5 fix. "explain how mutexes work" reduces to the salient
        // term "mutexes"; a note about unrelated work must not count just
        // because BM25 matched a common word.
        let mut filler_only = mk_hit("b");
        filler_only.lexical = true;
        filler_only.cosine = None;
        filler_only.content = "digest what you read and store the concept".to_string();
        assert!(!passes_relevance_gate(
            &filler_only,
            &["mutexes".to_string()],
            true,
        ));

        // A lexical hit with no salient terms at all (pure-filler question) is
        // not evidence either.
        let mut no_salient = mk_hit("c");
        no_salient.lexical = true;
        assert!(!passes_relevance_gate(&no_salient, &[], true));

        // A dense hit passes only when it clears the floor AND the dense arm
        // carries signal.
        let mut strong = mk_hit("d");
        strong.lexical = false;
        strong.cosine = Some(0.7);
        assert!(passes_relevance_gate(&strong, &[], true));
        // Same strong cosine, but the arm as a whole has no signal (everything
        // bunched at the model's baseline) → dropped.
        assert!(!passes_relevance_gate(&strong, &[], false));

        let mut weak = mk_hit("e");
        weak.lexical = false;
        weak.cosine = Some(0.1);
        assert!(!passes_relevance_gate(&weak, &[], true));
    }

    #[test]
    fn dense_arm_signal_uses_relative_margin_not_absolute_floor() {
        // Real signal (measured shape): top 0.78 well clear of a ~0.51 median.
        let signal = vec![0.78, 0.61, 0.55, 0.53, 0.51, 0.49, 0.47, 0.45];
        assert!(dense_arm_has_signal(&signal));

        // Pure noise (measured shape): a tight cluster at the model's baseline,
        // top 0.42 barely above a ~0.40 median → gated out even though 0.42 is
        // far above the absolute 0.25 floor. This is the mutexes case.
        let noise = vec![0.42, 0.42, 0.41, 0.41, 0.40, 0.40, 0.40, 0.39];
        assert!(!dense_arm_has_signal(&noise));

        // Degenerate cases must not panic or divide by zero.
        assert!(!dense_arm_has_signal(&[]));
        // Single candidate: no pack to stand clear of → no signal.
        assert!(!dense_arm_has_signal(&[0.9]));
        // All-identical scores: zero margin → no signal, even if high.
        assert!(!dense_arm_has_signal(&[0.8, 0.8, 0.8, 0.8]));
        // A genuinely high top over a low pack passes even with few points.
        assert!(dense_arm_has_signal(&[0.9, 0.3]));
        // A high top over a high pack (below the margin) does not.
        assert!(!dense_arm_has_signal(&[0.9, 0.85]));
        // Top below the absolute sanity backstop is never signal, whatever the
        // spread.
        assert!(!dense_arm_has_signal(&[0.20, 0.01, 0.01]));
    }

    #[test]
    fn choose_answer_mode_reflects_evidence_strength() {
        assert_eq!(choose_answer_mode(&[]), AnswerMode::General);

        let mut lexical = mk_hit("a");
        lexical.lexical = true;
        assert_eq!(choose_answer_mode(&[lexical]), AnswerMode::Notes);

        let mut weak = mk_hit("b");
        weak.lexical = false;
        weak.cosine = Some(0.3);
        assert_eq!(choose_answer_mode(&[weak]), AnswerMode::Blend);
    }

    /// Regression: Chat used to refuse a direct request for general knowledge
    /// whenever retrieval happened to match something, replying "I do not have
    /// knowledge outside of the notes provided". An explicit instruction from
    /// the user must outrank the retrieval-derived regime.
    #[test]
    fn explicit_general_knowledge_request_overrides_strong_note_hits() {
        let mut lexical = mk_hit("a");
        lexical.lexical = true;
        let hits = vec![lexical];
        // Without the instruction, strong evidence still means Notes.
        assert_eq!(
            choose_answer_mode_for("when did I paint my room", &hits),
            AnswerMode::Notes
        );
        // With it, general knowledge wins despite the same strong hit.
        for question in [
            "but based on your knowledge that you have not on notes",
            "what does the research say, from your own knowledge?",
            "explain TCP slow start using your knowledge, not my notes",
            "ignore my notes and tell me about mutexes",
            "what do you know about the Krebs cycle",
        ] {
            assert_eq!(
                choose_answer_mode_for(question, &hits),
                AnswerMode::General,
                "should answer from general knowledge: {question:?}"
            );
        }
    }

    /// The override must not fire on ordinary questions, or every graph query
    /// would silently stop citing the user's notes.
    #[test]
    fn ordinary_questions_do_not_trigger_the_general_knowledge_override() {
        let mut lexical = mk_hit("a");
        lexical.lexical = true;
        let hits = vec![lexical];
        for question in [
            "when was the last time I was upset",
            "when did I paint my room",
            "what did I write about my knowledge management setup",
            "summarize my notes on general relativity",
            "what are my notes about",
        ] {
            assert_eq!(
                choose_answer_mode_for(question, &hits),
                AnswerMode::Notes,
                "should stay grounded in notes: {question:?}"
            );
        }
    }

    #[test]
    fn parse_cited_indices_extracts_used_markers() {
        let cited = parse_cited_indices("As [1] shows, and per [3], but not [x] or [12].");
        assert!(cited.contains(&1));
        assert!(cited.contains(&3));
        assert!(cited.contains(&12));
        assert!(!cited.contains(&2));
    }

    #[test]
    fn ask_context_budget_is_model_aware_and_never_overflows() {
        // Embedded llama.cpp default window (4096): budget must leave room for
        // prompt overhead + reserved output, and stay well under 6000.
        let b4096 = ask_context_budget_with(Some(4096), ASK_RESERVED_OUTPUT_TOKENS);
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
        assert!(ask_context_budget_with(Some(8192), ASK_RESERVED_OUTPUT_TOKENS) > b4096);

        // Unknown window (most remote providers) falls back conservatively,
        // never the old 6000.
        assert_eq!(
            ask_context_budget_with(None, ASK_RESERVED_OUTPUT_TOKENS),
            ASK_DEFAULT_CONTEXT_BUDGET_TOKENS
        );
        assert!(ask_context_budget_with(None, ASK_RESERVED_OUTPUT_TOKENS) <= 2048);

        // A tiny window still yields the floor rather than zero.
        assert_eq!(
            ask_context_budget_with(Some(256), ASK_RESERVED_OUTPUT_TOKENS),
            ASK_MIN_CONTEXT_BUDGET_TOKENS
        );
    }

    #[test]
    fn thinking_models_reserve_more_output_and_still_fit_the_window() {
        // A reasoning model needs a larger output allowance (room to think
        // *and* answer), which shrinks the context budget rather than
        // overflowing the window.
        let normal = ask_context_budget_with(Some(4096), ASK_RESERVED_OUTPUT_TOKENS);
        let thinking = ask_context_budget_with(Some(4096), ASK_THINKING_OUTPUT_TOKENS);

        assert!(
            ASK_THINKING_OUTPUT_TOKENS > ASK_RESERVED_OUTPUT_TOKENS,
            "thinking models must get a bigger output budget"
        );
        assert!(
            thinking < normal,
            "the larger output reservation must come out of the context budget"
        );
        assert!(
            thinking + ASK_PROMPT_OVERHEAD_TOKENS + ASK_THINKING_OUTPUT_TOKENS <= 4096,
            "prompt + (thinking) output must still fit the window"
        );
    }

    // ── Two-part "research on the web" flow ─────────────────────────────────

    /// A queue-backed `LlmProvider`: returns the next canned response for each
    /// `complete` call regardless of the prompt, so a single queue can serve
    /// the notes arm (1 call) and then the web arm's plan/pick/synthesize (3
    /// calls) across one `ask_stream_with_web` run.
    struct QueueLlm {
        responses: Mutex<std::collections::VecDeque<String>>,
    }

    impl QueueLlm {
        fn new(responses: impl IntoIterator<Item = &'static str>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().map(String::from).collect()),
            }
        }
    }

    impl LlmProvider for QueueLlm {
        fn complete<'a>(
            &'a self,
            _messages: &'a [crate::ai::traits::ChatMessage],
            _options: &'a crate::ai::traits::CompletionOptions,
        ) -> BoxFuture<'a, Result<String>> {
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_default();
            Box::pin(async move { Ok(response) })
        }

        fn name(&self) -> &str {
            "queue"
        }

        fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
            Box::pin(async move { Ok(true) })
        }
    }

    /// A browser that serves canned Brave results HTML for any `search.brave.com`
    /// URL and a canned page for each mapped URL — enough to drive the web arm
    /// entirely offline.
    struct CannedBrowser {
        search_html: String,
        pages: HashMap<String, String>,
    }

    impl crate::scraping::browser::BrowserDriver for CannedBrowser {
        fn fetch<'a>(
            &'a self,
            url: &'a str,
        ) -> BoxFuture<'a, Result<crate::scraping::browser::FetchedResource>> {
            Box::pin(async move {
                let bytes = if url.starts_with("https://search.brave.com/") {
                    self.search_html.clone().into_bytes()
                } else {
                    self.pages
                        .get(url)
                        .cloned()
                        .ok_or_else(|| CoreError::Other(format!("no canned page for {url}")))?
                        .into_bytes()
                };
                Ok(crate::scraping::browser::FetchedResource {
                    url: url.to_string(),
                    content_type: Some("text/html".to_string()),
                    bytes,
                })
            })
        }
    }

    fn test_engine_with_llm(llm: Box<dyn LlmProvider>) -> Result<KnowledgeEngine> {
        let config = AiConfig::default();
        let registry_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("graph_registry.engine-test.json");
        Ok(KnowledgeEngine {
            config: config.clone(),
            llm: Some(llm),
            embedder: None,
            vector_store: None,
            pipeline: RwLock::new(EmbeddingPipeline::new(config.embedding.clone())),
            reference_engine: ReferenceEngine::new(config.references.clone()),
            registry: RwLock::new(GraphRegistry::load(&registry_path)?),
            data_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            models_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        })
    }

    fn two_result_search_html() -> String {
        r#"<html><body>
        <div class="snippet" data-type="web"><a href="https://a.example/x"><div class="title">Study A</div></a><div class="generic-snippet"><div class="content">snip a</div></div></div>
        <div class="snippet" data-type="web"><a href="https://b.example/y"><div class="title">Study B</div></a><div class="generic-snippet"><div class="content">snip b</div></div></div>
        </body></html>"#
            .to_string()
    }

    fn stub_page_html(title: &str, body: &str) -> String {
        format!("<html><head><title>{title}</title></head><body><p>{body}</p></body></html>")
    }

    #[tokio::test]
    async fn ask_stream_with_web_assembles_both_parts() -> Result<()> {
        use crate::db::Database;
        use crate::models::BlockType;

        let db = Database::in_memory()?;
        let page = db.create_page("Creatine notes", false)?;
        db.create_block(
            &page.id,
            None,
            0,
            "Creatine is one of the most studied supplements for strength.",
            BlockType::Text,
            json!({}),
        )?;

        // notes answer, then web plan / pick / synthesize.
        let llm = QueueLlm::new([
            "Your notes say creatine is among the most studied supplements for strength [1].",
            r#"{"queries": ["does creatine cause cancer"]}"#,
            r#"{"picks": [0, 1]}"#,
            r#"{"title_answer": "There is no strong evidence that creatine causes cancer.", "topics": [{"topic": "Creatine and cancer risk", "summary": "Reviews report no causal link[1]; long-term data remain limited[2].", "tags": []}]}"#,
        ]);
        let engine = test_engine_with_llm(Box::new(llm))?;

        let browser = CannedBrowser {
            search_html: two_result_search_html(),
            pages: HashMap::from([
                (
                    "https://a.example/x".to_string(),
                    stub_page_html("Study A", "No causal link found."),
                ),
                (
                    "https://b.example/y".to_string(),
                    stub_page_html("Study B", "More research is ongoing."),
                ),
            ]),
        };

        let mut answer = String::new();
        let mut phases = Vec::new();
        let mut notes = Vec::new();
        let outcome = engine
            .ask_stream_with_web_using(
                &db,
                "does creatine cause cancer",
                None,
                &browser,
                None,
                &mut |ev| match ev {
                    AskStreamEvent::Delta(d) => answer.push_str(d),
                    AskStreamEvent::Phase(p) => phases.push(p.as_str()),
                    AskStreamEvent::Note(n) => notes.push(n.to_string()),
                },
            )
            .await?;

        // Both section headers, in order, each with its content.
        let notes_idx = answer.find("## From your notes").expect("notes header");
        let web_idx = answer.find("## From the web").expect("web header");
        assert!(notes_idx < web_idx, "notes section must come first");
        assert!(answer.contains("most studied supplements")); // notes arm answer
        assert!(answer.contains("no strong evidence that creatine causes cancer")); // web answer
        assert!(answer.contains("Creatine and cancer risk")); // web topic heading

        // New phases surfaced for the UI, and per-source progress notes.
        assert!(phases.contains(&"searching_web"));
        assert!(phases.contains(&"reading_sources"));
        assert!(notes.iter().any(|n| n.starts_with("Reading source")));

        // Graph + web sources both plumbed through the outcome.
        assert_eq!(outcome.sources.len(), 1, "notes citation [1]");
        assert_eq!(outcome.sources[0].page_title, "Creatine notes");
        assert_eq!(outcome.web_citations.len(), 2, "two web sources read");
        assert_eq!(outcome.web_citations[0].url, "https://a.example/x");
        Ok(())
    }

    #[tokio::test]
    async fn ask_stream_with_web_says_when_notes_have_nothing_but_still_researches() -> Result<()> {
        use crate::db::Database;

        // Empty graph → the notes arm must find nothing.
        let db = Database::in_memory()?;
        let llm = QueueLlm::new([
            r#"{"queries": ["capital of atlantis"]}"#,
            r#"{"picks": [0]}"#,
            r#"{"title_answer": "Atlantis is a mythical island with no real capital.", "topics": []}"#,
        ]);
        let engine = test_engine_with_llm(Box::new(llm))?;
        let browser = CannedBrowser {
            search_html: r#"<html><body><div class="snippet" data-type="web"><a href="https://m.example/a"><div class="title">Myth</div></a><div class="generic-snippet"><div class="content">s</div></div></div></body></html>"#.to_string(),
            pages: HashMap::from([(
                "https://m.example/a".to_string(),
                stub_page_html("Myth", "Atlantis is a legend from Plato."),
            )]),
        };

        let mut answer = String::new();
        let outcome = engine
            .ask_stream_with_web_using(
                &db,
                "capital of atlantis",
                None,
                &browser,
                None,
                &mut |ev| {
                    if let AskStreamEvent::Delta(d) = ev {
                        answer.push_str(d);
                    }
                },
            )
            .await?;

        assert!(answer.contains("couldn't find anything about this in your notes"));
        assert!(answer.contains("## From the web"));
        assert!(answer.contains("mythical island")); // web arm still ran
        assert!(outcome.sources.is_empty());
        assert_eq!(outcome.web_citations.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn ask_stream_with_web_survives_a_failed_web_arm() -> Result<()> {
        use crate::db::Database;
        use crate::models::BlockType;

        let db = Database::in_memory()?;
        let page = db.create_page("Creatine notes", false)?;
        db.create_block(
            &page.id,
            None,
            0,
            "Creatine supports strength training.",
            BlockType::Text,
            json!({}),
        )?;
        // Notes answer present; web search returns nothing parseable → web arm errors.
        let llm = QueueLlm::new([
            "Creatine supports strength training [1].",
            r#"{"queries": ["creatine cancer"]}"#,
        ]);
        let engine = test_engine_with_llm(Box::new(llm))?;
        let browser = CannedBrowser {
            search_html: "<html><body>no results at all</body></html>".to_string(),
            pages: HashMap::new(),
        };

        let mut answer = String::new();
        let outcome = engine
            .ask_stream_with_web_using(&db, "creatine cancer", None, &browser, None, &mut |ev| {
                if let AskStreamEvent::Delta(d) = ev {
                    answer.push_str(d);
                }
            })
            .await?;

        // Notes answer intact; web failure shown as a calm note, not a crash.
        assert!(answer.contains("supports strength training"));
        assert!(answer.contains("## From the web"));
        assert!(answer.contains("couldn't complete the web research"));
        // A throttle is the most common and most recoverable failure, so it
        // must read as "wait a moment", not as a raw HTTP error.
        let throttled = describe_web_failure("returned HTTP 429 Too Many Requests");
        assert!(throttled.contains("rate-limiting"));
        assert!(throttled.contains("clears within a minute"));
        assert!(!throttled.contains("429"));
        assert_eq!(outcome.sources.len(), 1);
        assert!(outcome.web_citations.is_empty());
        Ok(())
    }

    #[test]
    fn render_web_section_includes_answer_and_topics_and_is_never_blank() {
        use crate::ai::web_research::{Citation, ResearchTopic, WebResearchResult};

        let full = WebResearchResult {
            title_answer: Some("Short direct answer.".to_string()),
            topics: vec![ResearchTopic {
                topic: "A topic".to_string(),
                summary: "Body with a marker[1].".to_string(),
                tags: vec![],
            }],
            citations: vec![Citation {
                number: 1,
                title: "S".to_string(),
                url: "https://s.example".to_string(),
            }],
        };
        let md = render_web_section(&full);
        assert!(md.contains("Short direct answer."));
        assert!(md.contains("**A topic**"));
        assert!(md.contains("Body with a marker[1]."));

        // Even a totally empty result yields a non-blank line under the header.
        let empty = WebResearchResult {
            title_answer: None,
            topics: vec![],
            citations: vec![],
        };
        assert!(!render_web_section(&empty).trim().is_empty());
    }
}
