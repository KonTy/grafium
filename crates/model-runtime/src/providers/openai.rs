//! OpenAI's hosted endpoints share the compatible wire format.

use super::{
    openai_compatible::{OpenAiCompatibleEmbedder, OpenAiCompatibleLlm},
    NetworkConfig,
};
use crate::{
    error::Result,
    types::{BoxFuture, ChatMessage, CompletionOptions, Embedder, LlmProvider},
};

pub struct OpenAiLlm(OpenAiCompatibleLlm);

impl OpenAiLlm {
    pub fn new(api_key: &str, model: &str) -> Result<Self> {
        Ok(Self(OpenAiCompatibleLlm::new(
            "https://api.openai.com/v1",
            model,
            Some(api_key.into()),
        )?))
    }

    pub fn with_network(api_key: &str, model: &str, network: NetworkConfig) -> Result<Self> {
        Ok(Self(OpenAiCompatibleLlm::with_network(
            "https://api.openai.com/v1",
            model,
            Some(api_key.into()),
            network,
        )?))
    }
}

impl LlmProvider for OpenAiLlm {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        self.0.complete(messages, options)
    }

    fn complete_stream<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
        on_token: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<String>> {
        self.0.complete_stream(messages, options, on_token)
    }

    fn name(&self) -> &str {
        "openai"
    }
    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        self.0.health_check()
    }
}

pub struct OpenAiEmbedder(OpenAiCompatibleEmbedder);

impl OpenAiEmbedder {
    pub fn new(api_key: &str, model: &str, dimension: usize) -> Result<Self> {
        Ok(Self(OpenAiCompatibleEmbedder::new(
            "https://api.openai.com/v1",
            model,
            dimension,
            Some(api_key.into()),
        )?))
    }

    pub fn with_network(
        api_key: &str,
        model: &str,
        dimension: usize,
        network: NetworkConfig,
    ) -> Result<Self> {
        Ok(Self(OpenAiCompatibleEmbedder::with_network(
            "https://api.openai.com/v1",
            model,
            dimension,
            Some(api_key.into()),
            network,
        )?))
    }

    pub fn default_small(api_key: &str) -> Result<Self> {
        Self::new(api_key, "text-embedding-3-small", 1536)
    }
}

impl Embedder for OpenAiEmbedder {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        self.0.embed(texts)
    }
    fn dimension(&self) -> usize {
        self.0.dimension()
    }
    fn model_name(&self) -> &str {
        self.0.model_name()
    }
}
