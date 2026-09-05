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
            // Relevance gate (HIGH 5): drop dense nearest-neighbours that are
            // only weakly similar. A lexical (BM25) hit is inherently on-topic
            // and always passes; a dense-only hit must clear the similarity
            // floor. This is what keeps a general question ("explain mutexes")
            // from dragging irrelevant notes into the prompt once the index is
            // populated — while an empty index (BM25-only) is unaffected since
            // every hit is lexical.
            .filter(|h| passes_relevance_gate(h))
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

        // Hybrid retrieval degrades gracefully: works with an empty vector
        // index (pure BM25) and even with no embedder at all. The context
        // budget is sized to the model's own window so the assembled prompt
        // can't overflow it (embedded llama.cpp hard-errors on overflow).
        let budget = ask_context_budget(llm.context_window());
        let mut hits = self
            .hybrid_search(db, question, ASK_TOP_K, graph_id)
            .await
            .unwrap_or_default();

        // Choose the prompt regime outside the model (HIGH 5) from the gated
        // hits, so it answers a single clear instruction instead of branching.
        let mode = choose_answer_mode(&hits);

        // In General mode we deliberately include no context at all — nothing
        // relevant was retrieved, so any notes would only contaminate a
        // general answer.
        let entries = if mode == AnswerMode::General {
            Vec::new()
        } else {
            self.expand_hits(db, &mut hits);
            retrieval::assemble_within_budget(&hits, budget)
        };

        let context_block = build_context_block(&entries);
        let system_prompt = build_system_prompt(&context_block, mode);

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

        // Show only sources the model actually cited (HIGH 5), not everything
        // retrieved. General answers carry no sources.
        let cited = parse_cited_indices(&answer);
        let sources: Vec<Source> = entries
            .iter()
            .filter(|e| cited.contains(&e.index))
            .map(|e| Source {
                index: e.index,
                page_id: e.page_id.clone(),
                page_title: e.page_title.clone(),
                block_id: e.block_id.clone(),
                // Only a defensible *event* date is surfaced to the UI chip; a
                // note's saved/imported timestamp is never presented as when
                // the event happened (HIGH 4).
                date: e.date_ms.map(retrieval::format_date_ms),
            })
            .collect();

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

/// Minimum cosine similarity for a *dense-only* hit to be considered relevant
/// enough to enter the prompt. Below this, a nearest-neighbour is just the
/// closest of an irrelevant bunch and would only contaminate a general answer
/// (HIGH 5). Lexical (BM25) hits bypass this — a term match is inherently
/// on-topic — which also preserves the empty-index BM25-only path.
const ASK_SIMILARITY_FLOOR: f32 = 0.25;
/// Cosine at/above which a dense hit is treated as strong evidence, enough to
/// answer purely from notes rather than a cautious blend.
const ASK_STRONG_SIMILARITY: f32 = 0.6;

/// Relevance gate for a single hit: keep lexical (term-match) hits always;
/// keep dense hits only when they clear the similarity floor. Hits with no
/// cosine at all (pure BM25 / empty index) are lexical and pass.
fn passes_relevance_gate(hit: &RetrievedHit) -> bool {
    if hit.lexical {
        return true;
    }
    matches!(hit.cosine, Some(c) if c >= ASK_SIMILARITY_FLOOR)
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
- Never invent citations or dates. If the notes don't contain the answer, say so plainly.\n\n\
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
    fn relevance_gate_keeps_lexical_and_drops_weak_dense() {
        let mut lexical = mk_hit("a");
        lexical.lexical = true;
        lexical.cosine = None;
        assert!(passes_relevance_gate(&lexical));

        let mut strong = mk_hit("b");
        strong.lexical = false;
        strong.cosine = Some(0.7);
        assert!(passes_relevance_gate(&strong));

        let mut weak = mk_hit("c");
        weak.lexical = false;
        weak.cosine = Some(0.1);
        assert!(!passes_relevance_gate(&weak));
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
