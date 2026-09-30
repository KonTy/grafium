//! OpenAI chat/embedding wire format, including self-hosted LAN endpoints.

use std::time::Duration;

use reqwest::{Method, RequestBuilder};
use serde::{Deserialize, Serialize};

use super::{
    canonicalize_messages,
    http::{self, Endpoint},
    validate_embeddings, NetworkConfig,
};
use crate::{
    error::{Result, RuntimeError},
    types::{BoxFuture, ChatMessage, CompletionOptions, Embedder, LlmProvider, MessageRole},
};

pub struct OpenAiCompatibleLlm {
    endpoint: Endpoint,
    api_key: Option<String>,
    model: String,
}

impl OpenAiCompatibleLlm {
    pub fn new(base_url: &str, model: &str, api_key: Option<String>) -> Result<Self> {
        Self::with_network(
            base_url,
            model,
            api_key,
            NetworkConfig::new(Duration::from_secs(120))?,
        )
    }

    pub fn with_network(
        base_url: &str,
        model: &str,
        api_key: Option<String>,
        network: NetworkConfig,
    ) -> Result<Self> {
        Ok(Self {
            endpoint: Endpoint::new(base_url, network)?,
            api_key,
            model: model.into(),
        })
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        auth(self.endpoint.request(method, path), &self.api_key)
    }

    async fn chat(
        &self,
        messages: &[ChatMessage],
        options: &CompletionOptions,
        stream: bool,
    ) -> Result<reqwest::Response> {
        let request = self
            .request(Method::POST, "chat/completions")
            .json(&build_chat_request(&self.model, messages, options, stream));
        http::success(self.endpoint.send(request, &options.cancel).await?)
    }
}

fn auth(request: RequestBuilder, key: &Option<String>) -> RequestBuilder {
    match key {
        Some(key) => request.bearer_auth(key),
        None => request,
    }
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

fn build_chat_request(
    model: &str,
    messages: &[ChatMessage],
    options: &CompletionOptions,
    stream: bool,
) -> ChatRequest {
    ChatRequest {
        model: model.into(),
        messages: canonicalize_messages(messages, options),
        max_tokens: options.max_tokens,
        temperature: options.temperature,
        stop: options.stop.clone(),
        stream: stream.then_some(true),
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}
#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

fn response_text(response: ChatResponse) -> Result<String> {
    response
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .ok_or_else(|| RuntimeError::Other("OpenAI-compatible returned no completion".into()))
}

#[derive(Deserialize)]
struct StreamResponse {
    choices: Vec<StreamChoice>,
    error: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct StreamChoice {
    index: usize,
    delta: Delta,
}
#[derive(Deserialize)]
struct Delta {
    content: Option<String>,
}

impl LlmProvider for OpenAiCompatibleLlm {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let response = self.chat(messages, options, false).await?;
            let parsed = http::json(response, self.endpoint.limits(), &options.cancel).await?;
            http::completion(response_text(parsed)?, self.endpoint.limits())
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
            // Some compatible servers ignore stream=true and return ordinary JSON.
            if http::is_json(&response) {
                let parsed = http::json(response, limits, &options.cancel).await?;
                http::append(
                    &mut text,
                    &response_text(parsed)?,
                    limits,
                    &options.cancel,
                    on_token,
                )?;
            } else {
                http::frames(response, true, limits, &options.cancel, |frame| {
                    if frame == "[DONE]" {
                        return Ok(true);
                    }
                    let parsed: StreamResponse = serde_json::from_str(frame)?;
                    if parsed.error.is_some() {
                        return Err(RuntimeError::Other(
                            "OpenAI-compatible endpoint reported a stream error".into(),
                        ));
                    }
                    for choice in parsed.choices {
                        if choice.index != 0 {
                            return Err(RuntimeError::Other(
                                "Unexpected completion choice index".into(),
                            ));
                        }
                        if let Some(delta) = choice.delta.content {
                            http::append(&mut text, &delta, limits, &options.cancel, on_token)?;
                        }
                    }
                    Ok(false)
                })
                .await?;
            }
            http::completion(text, limits)
        })
    }

    fn name(&self) -> &str {
        "openai-compatible"
    }

    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let response = self
                .endpoint
                .send(self.request(Method::GET, "models"), &None)
                .await?;
            if response.status().is_success() {
                return Ok(true);
            }
            // Preserve the existing explicit health probe for GitHub Models,
            // which does not always expose /inference/models.
            if self.endpoint.is_github_models() {
                let request = build_chat_request(
                    &self.model,
                    &[ChatMessage {
                        role: MessageRole::User,
                        content: "ping".into(),
                    }],
                    &CompletionOptions {
                        max_tokens: Some(1),
                        temperature: Some(0.0),
                        ..Default::default()
                    },
                    false,
                );
                return Ok(self
                    .endpoint
                    .send(
                        self.request(Method::POST, "chat/completions")
                            .json(&request),
                        &None,
                    )
                    .await?
                    .status()
                    .is_success());
            }
            Ok(false)
        })
    }
}

pub struct OpenAiCompatibleEmbedder {
    endpoint: Endpoint,
    api_key: Option<String>,
    model: String,
    dimension: usize,
}

impl OpenAiCompatibleEmbedder {
    pub fn new(
        base_url: &str,
        model: &str,
        dimension: usize,
        api_key: Option<String>,
    ) -> Result<Self> {
        Self::with_network(
            base_url,
            model,
            dimension,
            api_key,
            NetworkConfig::new(Duration::from_secs(60))?,
        )
    }

    pub fn with_network(
        base_url: &str,
        model: &str,
        dimension: usize,
        api_key: Option<String>,
        network: NetworkConfig,
    ) -> Result<Self> {
        if dimension == 0 {
            return Err(RuntimeError::Other(
                "Embedding dimension must be positive".into(),
            ));
        }
        Ok(Self {
            endpoint: Endpoint::new(base_url, network)?,
            api_key,
            model: model.into(),
            dimension,
        })
    }
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}
#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedData>,
}
#[derive(Deserialize)]
struct EmbedData {
    index: usize,
    embedding: Vec<f32>,
}

impl Embedder for OpenAiCompatibleEmbedder {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let request = auth(
                self.endpoint.request(Method::POST, "embeddings"),
                &self.api_key,
            )
            .json(&EmbedRequest {
                model: &self.model,
                input: texts,
            });
            let response = http::success(self.endpoint.send(request, &None).await?)?;
            let parsed: EmbedResponse = http::json(response, self.endpoint.limits(), &None).await?;
            if parsed.data.len() != texts.len() {
                return Err(RuntimeError::Other(
                    "Provider returned invalid embedding cardinality".into(),
                ));
            }
            let mut ordered = vec![None; texts.len()];
            for item in parsed.data {
                let slot = ordered.get_mut(item.index).ok_or_else(|| {
                    RuntimeError::Other("Provider returned out-of-range embedding index".into())
                })?;
                if slot.is_some() {
                    return Err(RuntimeError::Other(
                        "Provider returned duplicate embedding index".into(),
                    ));
                }
                *slot = Some(item.embedding);
            }
            let vectors = ordered
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| RuntimeError::Other("Provider omitted an embedding index".into()))?;
            validate_embeddings(&vectors, texts.len(), self.dimension)?;
            Ok(vectors)
        })
    }

    fn dimension(&self) -> usize {
        self.dimension
    }
    fn model_name(&self) -> &str {
        &self.model
    }
}
