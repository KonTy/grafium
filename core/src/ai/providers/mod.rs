//! AI provider implementations.

use crate::ai::traits::{
    AcceleratorStatus, BoxFuture, ChatMessage, CompletionOptions, Concurrency, Embedder,
    LlmProvider,
};
use crate::error::{CoreError, Result};

pub mod anthropic;
#[cfg(feature = "llm-local")]
mod llama_shared;
#[cfg(feature = "llm-local")]
pub(crate) mod native_gpu;
#[cfg(feature = "llm-local")]
#[cfg(feature = "llm-local")]
pub mod local_embedder;
#[cfg(feature = "llm-local")]
pub mod local_llm;
#[cfg(feature = "llm-local")]
pub mod local_llm_process;
pub mod ollama;
pub mod openai;
pub mod openai_compatible;

// Keep Grafium's CoreError-based trait contracts for native providers and test
// doubles, while all HTTP work belongs to the independent runtime.
macro_rules! bridge_llm {
    ($($provider:ty),+ $(,)?) => {$(
        impl LlmProvider for $provider {
            fn complete<'a>(
                &'a self,
                messages: &'a [ChatMessage],
                options: &'a CompletionOptions,
            ) -> BoxFuture<'a, Result<String>> {
                Box::pin(async move {
                    model_runtime::types::LlmProvider::complete(self, messages, options)
                        .await.map_err(CoreError::from)
                })
            }
            fn complete_stream<'a>(
                &'a self,
                messages: &'a [ChatMessage],
                options: &'a CompletionOptions,
                on_token: &'a mut (dyn FnMut(&str) + Send),
            ) -> BoxFuture<'a, Result<String>> {
                Box::pin(async move {
                    model_runtime::types::LlmProvider::complete_stream(self, messages, options, on_token)
                        .await.map_err(CoreError::from)
                })
            }
            fn name(&self) -> &str {
                model_runtime::types::LlmProvider::name(self)
            }
            fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
                Box::pin(async move {
                    model_runtime::types::LlmProvider::health_check(self)
                        .await.map_err(CoreError::from)
                })
            }
            fn count_prompt_tokens<'a>(
                &'a self,
                messages: &'a [ChatMessage],
                options: &'a CompletionOptions,
            ) -> BoxFuture<'a, Result<Option<usize>>> {
                Box::pin(async move {
                    model_runtime::types::LlmProvider::count_prompt_tokens(self, messages, options)
                        .await.map_err(CoreError::from)
                })
            }
            fn context_window(&self) -> Option<usize> {
                model_runtime::types::LlmProvider::context_window(self)
            }
            fn supports_thinking(&self) -> bool {
                model_runtime::types::LlmProvider::supports_thinking(self)
            }
            fn concurrency(&self) -> Concurrency {
                model_runtime::types::LlmProvider::concurrency(self)
            }
            fn accelerator_status(&self) -> Option<AcceleratorStatus> {
                model_runtime::types::LlmProvider::accelerator_status(self)
            }
            fn backend_summary(&self) -> Option<String> {
                model_runtime::types::LlmProvider::backend_summary(self)
            }
            fn abort_in_flight(&self) {
                model_runtime::types::LlmProvider::abort_in_flight(self)
            }
        }
    )+};
}

macro_rules! bridge_embedder {
    ($($provider:ty),+ $(,)?) => {$(
        impl Embedder for $provider {
            fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
                Box::pin(async move {
                    model_runtime::types::Embedder::embed(self, texts).await.map_err(CoreError::from)
                })
            }
            fn embed_documents<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
                Box::pin(async move {
                    model_runtime::types::Embedder::embed_documents(self, texts)
                        .await.map_err(CoreError::from)
                })
            }
            fn embed_queries<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
                Box::pin(async move {
                    model_runtime::types::Embedder::embed_queries(self, texts)
                        .await.map_err(CoreError::from)
                })
            }
            fn embed_query<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>>> {
                Box::pin(async move {
                    model_runtime::types::Embedder::embed_query(self, text)
                        .await.map_err(CoreError::from)
                })
            }
            fn dimension(&self) -> usize {
                model_runtime::types::Embedder::dimension(self)
            }
            fn model_name(&self) -> &str {
                model_runtime::types::Embedder::model_name(self)
            }
            fn embedding_scheme_id(&self) -> String {
                model_runtime::types::Embedder::embedding_scheme_id(self)
            }
        }
    )+};
}

bridge_llm!(
    openai::OpenAiLlm,
    openai_compatible::OpenAiCompatibleLlm,
    anthropic::AnthropicLlm,
    ollama::OllamaLlm,
);
bridge_embedder!(
    openai::OpenAiEmbedder,
    openai_compatible::OpenAiCompatibleEmbedder,
    ollama::OllamaEmbedder,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn network_adapter_preserves_grafium_trait_and_converts_cancellation() {
        let provider: Box<dyn LlmProvider> = Box::new(
            openai_compatible::OpenAiCompatibleLlm::new("http://127.0.0.1:1/v1", "synthetic", None)
                .unwrap(),
        );
        assert_eq!(provider.name(), "openai-compatible");
        assert_eq!(provider.concurrency(), Concurrency::Parallel);
        assert_eq!(
            provider
                .count_prompt_tokens(&[], &CompletionOptions::default())
                .await
                .unwrap(),
            None
        );
        let options = CompletionOptions {
            cancel: Some(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                true,
            ))),
            ..Default::default()
        };
        assert!(matches!(
            provider.complete(&[], &options).await,
            Err(CoreError::ModelRuntime(
                model_runtime::error::RuntimeError::Cancelled
            ))
        ));
    }
}
