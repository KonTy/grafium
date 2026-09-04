//! Anthropic provider — Claude via Anthropic Messages API.

use serde::{Deserialize, Serialize};

use super::read_limited_response;
use crate::ai::traits::{BoxFuture, ChatMessage, CompletionOptions, LlmProvider, MessageRole};
use crate::error::{CoreError, Result};

/// Anthropic LLM provider (Claude).
pub struct AnthropicLlm {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl AnthropicLlm {
    pub fn new(api_key: &str, model: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default();

        Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            client,
        }
    }

    /// Default: Claude 3.5 Sonnet.
    pub fn sonnet(api_key: &str) -> Self {
        Self::new(api_key, "claude-sonnet-4-20250514")
    }
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Deserialize)]
struct AnthropicContent {
    text: Option<String>,
}

impl LlmProvider for AnthropicLlm {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            // Extract system message separately (Anthropic API requirement).
            let system = messages
                .iter()
                .find(|m| m.role == MessageRole::System)
                .map(|m| m.content.clone())
                .or_else(|| options.system_prompt.clone());

            let anthropic_messages: Vec<AnthropicMessage> = messages
                .iter()
                .filter(|m| m.role != MessageRole::System)
                .map(|m| AnthropicMessage {
                    role: match m.role {
                        MessageRole::User => "user".to_string(),
                        MessageRole::Assistant => "assistant".to_string(),
                        _ => "user".to_string(),
                    },
                    content: m.content.clone(),
                })
                .collect();

            let request = AnthropicRequest {
                model: self.model.clone(),
                max_tokens: options.max_tokens.unwrap_or(2048),
                system,
                messages: anthropic_messages,
                temperature: options.temperature,
                stop_sequences: options.stop.clone(),
            };

            let resp = self
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&request)
                .send()
                .await
                .map_err(|e| CoreError::Other(format!("Anthropic request failed: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body =
                    String::from_utf8_lossy(&read_limited_response(resp, "Anthropic").await?)
                        .into_owned();
                return Err(CoreError::Other(format!(
                    "Anthropic returned {}: {}",
                    status, body
                )));
            }

            let body = read_limited_response(resp, "Anthropic").await?;
            let response: AnthropicResponse = serde_json::from_slice(&body)
                .map_err(|e| CoreError::Other(format!("Anthropic response parse error: {e}")))?;

            response
                .content
                .first()
                .and_then(|c| c.text.clone())
                .ok_or_else(|| CoreError::Other("Anthropic returned empty response".to_string()))
        })
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            // Anthropic doesn't have a simple health endpoint, just verify key format.
            Ok(!self.api_key.is_empty())
        })
    }
}
