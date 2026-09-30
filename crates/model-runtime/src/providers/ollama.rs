//! Ollama HTTP chat and embeddings; no assumption that its host is local.

use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};

use super::{
    canonicalize_messages,
    http::{self, Endpoint},
    validate_embeddings, NetworkConfig,
};
use crate::{
    error::{Result, RuntimeError},
    types::{BoxFuture, ChatMessage, CompletionOptions, Embedder, LlmProvider},
};

pub struct OllamaLlm {
    endpoint: Endpoint,
    model: String,
}

impl OllamaLlm {
    pub fn new(base_url: &str, model: &str) -> Result<Self> {
        Self::with_network(
            base_url,
            model,
            NetworkConfig::new(Duration::from_secs(120))?,
        )
    }

    pub fn with_network(base_url: &str, model: &str, network: NetworkConfig) -> Result<Self> {
        Ok(Self {
            endpoint: Endpoint::new(base_url, network)?,
            model: model.into(),
        })
    }

    async fn chat(
        &self,
        messages: &[ChatMessage],
        options: &CompletionOptions,
        stream: bool,
    ) -> Result<reqwest::Response> {
        let request = self
            .endpoint
            .request(Method::POST, "api/chat")
            .json(&ChatRequest {
                model: &self.model,
                messages: canonicalize_messages(messages, options),
                stream,
                options: OllamaOptions {
                    temperature: options.temperature,
                    num_predict: options.max_tokens,
                    stop: options.stop.clone(),
                },
            });
        http::success(self.endpoint.send(request, &options.cancel).await?)
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage>,
    stream: bool,
    options: OllamaOptions,
}
#[derive(Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
}
#[derive(Deserialize)]
struct ChatResponse {
    message: ResponseMessage,
}
#[derive(Deserialize)]
struct ResponseMessage {
    content: String,
}
#[derive(Deserialize)]
struct StreamResponse {
    message: Option<ResponseMessage>,
    done: bool,
    error: Option<String>,
}

impl LlmProvider for OllamaLlm {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let response = self.chat(messages, options, false).await?;
            let parsed: ChatResponse =
                http::json(response, self.endpoint.limits(), &options.cancel).await?;
            http::completion(parsed.message.content, self.endpoint.limits())
        })
    }

    fn complete_stream<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
        on_token: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let response = self.chat(messages, options, true).await?;
            let limits = self.endpoint.limits();
            let mut text = String::new();
            http::frames(response, false, limits, &options.cancel, |frame| {
                let parsed: StreamResponse = serde_json::from_str(frame)?;
                if parsed.error.is_some() {
                    return Err(RuntimeError::Other("Ollama reported a stream error".into()));
                }
                if let Some(message) = parsed.message {
                    http::append(
                        &mut text,
                        &message.content,
                        limits,
                        &options.cancel,
                        on_token,
                    )?;
                } else if !parsed.done {
                    return Err(RuntimeError::Other(
                        "Ollama stream omitted its message".into(),
                    ));
                }
                Ok(parsed.done)
            })
            .await?;
            http::completion(text, limits)
        })
    }

    fn name(&self) -> &str {
        "ollama"
    }
    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            Ok(self
                .endpoint
                .send(self.endpoint.request(Method::GET, "api/tags"), &None)
                .await?
                .status()
                .is_success())
        })
    }
}

pub struct OllamaEmbedder {
    endpoint: Endpoint,
    model: String,
    dimension: usize,
}

impl OllamaEmbedder {
    pub fn new(base_url: &str, model: &str, dimension: usize) -> Result<Self> {
        Self::with_network(
            base_url,
            model,
            dimension,
            NetworkConfig::new(Duration::from_secs(60))?,
        )
    }
    pub fn with_network(
        base_url: &str,
        model: &str,
        dimension: usize,
        network: NetworkConfig,
    ) -> Result<Self> {
        if dimension == 0 {
            return Err(RuntimeError::Other(
                "Embedding dimension must be positive".into(),
            ));
        }
        Ok(Self {
            endpoint: Endpoint::new(base_url, network)?,
            model: model.into(),
            dimension,
        })
    }
    pub fn nomic(base_url: &str) -> Result<Self> {
        Self::new(base_url, "nomic-embed-text", 768)
    }
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}
#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl Embedder for OllamaEmbedder {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let request = self
                .endpoint
                .request(Method::POST, "api/embed")
                .json(&EmbedRequest {
                    model: &self.model,
                    input: texts,
                });
            let response = http::success(self.endpoint.send(request, &None).await?)?;
            let parsed: EmbedResponse = http::json(response, self.endpoint.limits(), &None).await?;
            validate_embeddings(&parsed.embeddings, texts.len(), self.dimension)?;
            Ok(parsed.embeddings)
        })
    }
    fn dimension(&self) -> usize {
        self.dimension
    }
    fn model_name(&self) -> &str {
        &self.model
    }
}
