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
use crate::ai::references::{PageReferencesMeta, ReferenceEngine};
use crate::ai::traits::{Embedder, LlmProvider, SearchResult, VectorStore};
use crate::error::{CoreError, Result};
use crate::knowledge::registry::GraphRegistry;
use crate::knowledge::vector_store::SqliteVectorStore;
use crate::models::{Block, Page};

/// The Knowledge Engine — main orchestrator for all AI/knowledge operations.
pub struct KnowledgeEngine {
    config: AiConfig,
    llm: Option<Box<dyn LlmProvider>>,
    embedder: Option<Box<dyn Embedder>>,
    vector_store: Option<Arc<SqliteVectorStore>>,
    pipeline: RwLock<EmbeddingPipeline>,
    reference_engine: ReferenceEngine,
    registry: RwLock<GraphRegistry>,
    data_dir: PathBuf,
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
        };

        if config.enabled {
            engine.initialize_providers()?;
        }

        Ok(engine)
    }

    /// Initialize AI providers based on config.
    fn initialize_providers(&mut self) -> Result<()> {
        match &self.config.mode {
            AiMode::Local => {
                if let Some(local) = &self.config.local {
                    self.llm = Some(Box::new(OllamaLlm::new(
                        &local.ollama_url,
                        &local.llm_model,
                    )));
                    self.embedder = Some(Box::new(OllamaEmbedder::new(
                        &local.ollama_url,
                        &local.embedding_model,
                        768, // nomic-embed-text default
                    )));
                }
            }
            AiMode::Cloud => {
                if let Some(cloud) = &self.config.cloud {
                    match cloud.llm_provider {
                        ProviderType::OpenAi => {
                            self.llm =
                                Some(Box::new(OpenAiLlm::new(&cloud.llm_api_key, &cloud.llm_model)));
                        }
                        ProviderType::Anthropic => {
                            self.llm = Some(Box::new(AnthropicLlm::new(
                                &cloud.llm_api_key,
                                &cloud.llm_model,
                            )));
                        }
                        _ => {}
                    }

                    let embed_key = cloud
                        .embedding_api_key
                        .as_deref()
                        .unwrap_or(&cloud.llm_api_key);

                    match cloud.embedding_provider {
                        ProviderType::OpenAi => {
                            self.embedder = Some(Box::new(OpenAiEmbedder::new(
                                embed_key,
                                &cloud.embedding_model,
                                1536,
                            )));
                        }
                        _ => {}
                    }
                }
            }
            AiMode::Hybrid => {
                // Embeddings from local, LLM from cloud.
                if let Some(local) = &self.config.local {
                    self.embedder = Some(Box::new(OllamaEmbedder::new(
                        &local.ollama_url,
                        &local.embedding_model,
                        768,
                    )));
                }
                if let Some(cloud) = &self.config.cloud {
                    match cloud.llm_provider {
                        ProviderType::OpenAi => {
                            self.llm =
                                Some(Box::new(OpenAiLlm::new(&cloud.llm_api_key, &cloud.llm_model)));
                        }
                        ProviderType::Anthropic => {
                            self.llm = Some(Box::new(AnthropicLlm::new(
                                &cloud.llm_api_key,
                                &cloud.llm_model,
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

        if config.enabled {
            self.initialize_providers()?;
        }

        Ok(())
    }

    /// Check if the engine is ready (providers initialized).
    pub fn is_ready(&self) -> bool {
        self.config.enabled && self.llm.is_some() && self.embedder.is_some() && self.vector_store.is_some()
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
    pub async fn index_page(
        &self,
        page: &Page,
        blocks: &[Block],
        graph_id: &str,
    ) -> Result<usize> {
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| CoreError::Other("Embedder not initialized".to_string()))?;
        let store = self
            .vector_store
            .as_ref()
            .ok_or_else(|| CoreError::Other("Vector store not initialized".to_string()))?;

        // Delete old vectors for this page first.
        store.delete_by_page(graph_id, &page.id).await?;

        // Chunk the page.
        let mut pipeline = self.pipeline.write().await;
        let chunks = pipeline.chunk_page(page, blocks);
        let dirty_chunks = pipeline.filter_unchanged(chunks);

        // Embed and store.
        let count = pipeline
            .embed_and_store(&dirty_chunks, graph_id, embedder.as_ref(), store.as_ref())
            .await?;

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

    /// Generate references for a page using AI.
    pub async fn generate_references(
        &self,
        page_id: &str,
        page_title: &str,
        blocks: &[(String, String)], // (block_id, content)
        graph_id: &str,
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
            )
            .await
    }

    /// Ask a question against the knowledge base (RAG).
    pub async fn ask(&self, question: &str, graph_id: Option<&str>) -> Result<String> {
        let llm = self
            .llm
            .as_ref()
            .ok_or_else(|| CoreError::Other("LLM not initialized".to_string()))?;

        // Search for relevant context.
        let results = self.search(question, 10, graph_id).await?;

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
            .map(|(i, r)| {
                format!(
                    "[{}] From \"{}\":\n{}\n",
                    i + 1,
                    r.page_title,
                    r.content
                )
            })
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
