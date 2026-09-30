//! Anthropic Messages API, preserving its distinct system-message convention.

use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};

use super::{
    http::{self, Endpoint},
    NetworkConfig,
};
use crate::{
    error::{Result, RuntimeError},
    types::{BoxFuture, ChatMessage, CompletionOptions, LlmProvider, MessageRole},
};

pub struct AnthropicLlm {
    endpoint: Endpoint,
    api_key: String,
    model: String,
}

impl AnthropicLlm {
    pub fn new(api_key: &str, model: &str) -> Result<Self> {
        Self::with_network(
            "https://api.anthropic.com/v1",
            api_key,
            model,
            NetworkConfig::new(Duration::from_secs(120))?,
        )
    }

    /// Explicit endpoint override supports host-managed gateways and test servers.
    pub fn with_network(
        base_url: &str,
        api_key: &str,
        model: &str,
        network: NetworkConfig,
    ) -> Result<Self> {
        Ok(Self {
            endpoint: Endpoint::new(base_url, network)?,
            api_key: api_key.into(),
            model: model.into(),
        })
    }

    pub fn sonnet(api_key: &str) -> Result<Self> {
        Self::new(api_key, "claude-sonnet-4-20250514")
    }

    async fn chat(
        &self,
        messages: &[ChatMessage],
        options: &CompletionOptions,
        stream: bool,
    ) -> Result<reqwest::Response> {
        let request = self
            .endpoint
            .request(Method::POST, "messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&build_request(&self.model, messages, options, stream));
        http::success(self.endpoint.send(request, &options.cancel).await?)
    }
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

fn build_request(
    model: &str,
    messages: &[ChatMessage],
    options: &CompletionOptions,
    stream: bool,
) -> ChatRequest {
    // Preserve Grafium's existing precedence and original non-system role order.
    ChatRequest {
        model: model.into(),
        max_tokens: options.max_tokens.unwrap_or(2048),
        system: messages
            .iter()
            .find(|m| m.role == MessageRole::System)
            .map(|m| m.content.clone())
            .or_else(|| options.system_prompt.clone()),
        messages: messages
            .iter()
            .filter(|m| m.role != MessageRole::System)
            .cloned()
            .collect(),
        temperature: options.temperature,
        stop_sequences: options.stop.clone(),
        stream: stream.then_some(true),
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    content: Vec<Content>,
}
#[derive(Deserialize)]
struct Content {
    text: Option<String>,
}

fn response_text(response: ChatResponse) -> Result<String> {
    response
        .content
        .into_iter()
        .next()
        .and_then(|c| c.text)
        .ok_or_else(|| RuntimeError::Other("Anthropic returned no completion".into()))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamEvent {
    MessageStart {},
    ContentBlockStart { content_block: serde_json::Value },
    ContentBlockDelta { delta: serde_json::Value },
    ContentBlockStop {},
    MessageDelta {},
    MessageStop {},
    Ping {},
    Error {},
}

impl LlmProvider for AnthropicLlm {
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
                    match serde_json::from_str::<StreamEvent>(frame)? {
                        StreamEvent::MessageStop {} => return Ok(true),
                        StreamEvent::Error {} => {
                            return Err(RuntimeError::Other(
                                "Anthropic reported a stream error".into(),
                            ))
                        }
                        StreamEvent::ContentBlockStart { content_block }
                            if content_block["type"] == "text" =>
                        {
                            let delta = content_block["text"].as_str().ok_or_else(|| {
                                RuntimeError::Other("Malformed Anthropic text block".into())
                            })?;
                            http::append(&mut text, delta, limits, &options.cancel, on_token)?;
                        }
                        StreamEvent::ContentBlockDelta { delta }
                            if delta["type"] == "text_delta" =>
                        {
                            let delta = delta["text"].as_str().ok_or_else(|| {
                                RuntimeError::Other("Malformed Anthropic text delta".into())
                            })?;
                            http::append(&mut text, delta, limits, &options.cancel, on_token)?;
                        }
                        _ => {}
                    }
                    Ok(false)
                })
                .await?;
            }
            http::completion(text, limits)
        })
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        // Preserve the existing key-presence check; this is not a network probe.
        Box::pin(async move { Ok(!self.api_key.is_empty()) })
    }
}
