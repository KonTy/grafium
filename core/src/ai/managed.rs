//! Adapt shared manager handles to Grafium's existing error/trait boundary.

use std::sync::Arc;

use super::traits::{
    AcceleratorStatus, BoxFuture, ChatMessage, CompletionOptions, Concurrency, Embedder,
    LlmProvider,
};
use crate::error::{CoreError, Result};

impl<T: model_runtime::types::LlmProvider + ?Sized> LlmProvider for Arc<T> {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            model_runtime::types::LlmProvider::complete(self.as_ref(), messages, options)
                .await
                .map_err(CoreError::from)
        })
    }

    fn complete_stream<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
        on_token: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            model_runtime::types::LlmProvider::complete_stream(
                self.as_ref(),
                messages,
                options,
                on_token,
            )
            .await
            .map_err(CoreError::from)
        })
    }

    fn name(&self) -> &str {
        model_runtime::types::LlmProvider::name(self.as_ref())
    }
    fn native_model_path(&self) -> Option<&std::path::Path> {
        model_runtime::types::LlmProvider::native_model_path(self.as_ref())
    }
    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            model_runtime::types::LlmProvider::health_check(self.as_ref())
                .await
                .map_err(CoreError::from)
        })
    }
    fn context_window(&self) -> Option<usize> {
        model_runtime::types::LlmProvider::context_window(self.as_ref())
    }
    fn count_prompt_tokens<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<Option<usize>>> {
        Box::pin(async move {
            model_runtime::types::LlmProvider::count_prompt_tokens(self.as_ref(), messages, options)
                .await
                .map_err(CoreError::from)
        })
    }
    fn supports_thinking(&self) -> bool {
        model_runtime::types::LlmProvider::supports_thinking(self.as_ref())
    }
    fn concurrency(&self) -> Concurrency {
        model_runtime::types::LlmProvider::concurrency(self.as_ref())
    }
    fn accelerator_status(&self) -> Option<AcceleratorStatus> {
        model_runtime::types::LlmProvider::accelerator_status(self.as_ref())
    }
    fn backend_summary(&self) -> Option<String> {
        model_runtime::types::LlmProvider::backend_summary(self.as_ref())
    }
    fn abort_in_flight(&self) {
        model_runtime::types::LlmProvider::abort_in_flight(self.as_ref())
    }
}

impl<T: model_runtime::types::Embedder + ?Sized> Embedder for Arc<T> {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            model_runtime::types::Embedder::embed(self.as_ref(), texts)
                .await
                .map_err(CoreError::from)
        })
    }

    fn embed_documents<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            model_runtime::types::Embedder::embed_documents(self.as_ref(), texts)
                .await
                .map_err(CoreError::from)
        })
    }
    fn embed_query<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>>> {
        Box::pin(async move {
            model_runtime::types::Embedder::embed_query(self.as_ref(), text)
                .await
                .map_err(CoreError::from)
        })
    }
    fn embed_queries<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            model_runtime::types::Embedder::embed_queries(self.as_ref(), texts)
                .await
                .map_err(CoreError::from)
        })
    }
    fn dimension(&self) -> usize {
        model_runtime::types::Embedder::dimension(self.as_ref())
    }
    fn model_name(&self) -> &str {
        model_runtime::types::Embedder::model_name(self.as_ref())
    }
    fn embedding_scheme_id(&self) -> String {
        model_runtime::types::Embedder::embedding_scheme_id(self.as_ref())
    }
}

#[cfg(feature = "media")]
impl<T: model_runtime::transcription::Transcriber + ?Sized> crate::media::Transcriber for Arc<T> {
    fn transcribe_with_progress(
        &self,
        path: &std::path::Path,
        progress: &mut dyn FnMut(crate::media::TranscribeProgress),
    ) -> Result<crate::media::Transcript> {
        model_runtime::transcription::Transcriber::transcribe_with_progress(
            self.as_ref(),
            path,
            progress,
        )
        .map_err(CoreError::from)
    }
}
