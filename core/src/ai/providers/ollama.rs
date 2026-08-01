//! Ollama provider — local LLM and embedding via Ollama's HTTP API.
//!
//! Ollama runs at http://localhost:11434 by default.
//! Supports both chat completions and embeddings.

use serde::{Deserialize, Serialize};

use super::canonicalize_messages;
use crate::ai::traits::{
    BoxFuture, ChatMessage, CompletionOptions, Embedder, LlmProvider, MessageRole,
};
use crate::error::{CoreError, Result};

/// Ollama LLM provider.
pub struct OllamaLlm {
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl OllamaLlm {
    pub fn new(base_url: &str, model: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default();

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            client,
        }
    }
}

#[derive(Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    options: Option<OllamaOptions>,
}

#[derive(Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
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
struct OllamaChatResponse {
    message: OllamaResponseMessage,
}

#[derive(Deserialize)]
struct OllamaResponseMessage {
    content: String,
}

fn build_chat_request(
    model: &str,
    messages: &[ChatMessage],
    options: &CompletionOptions,
) -> OllamaChatRequest {
    let messages = canonicalize_messages(messages, options)
        .into_iter()
        .map(|message| OllamaMessage {
            role: match message.role {
                MessageRole::System => "system".to_string(),
                MessageRole::User => "user".to_string(),
                MessageRole::Assistant => "assistant".to_string(),
            },
            content: message.content,
        })
        .collect();

    OllamaChatRequest {
        model: model.to_string(),
        messages,
        stream: false,
        options: Some(OllamaOptions {
            temperature: options.temperature,
            num_predict: options.max_tokens,
            stop: options.stop.clone(),
        }),
    }
}

impl LlmProvider for OllamaLlm {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let request = build_chat_request(&self.model, messages, options);

            let resp = self
                .client
                .post(format!("{}/api/chat", self.base_url))
                .json(&request)
                .send()
                .await
                .map_err(|e| CoreError::Other(format!("Ollama request failed: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(CoreError::Other(format!(
                    "Ollama returned {}: {}",
                    status, body
                )));
            }

            let response: OllamaChatResponse = resp
                .json()
                .await
                .map_err(|e| CoreError::Other(format!("Ollama response parse error: {}", e)))?;

            Ok(response.message.content)
        })
    }

    fn name(&self) -> &str {
        "ollama"
    }

    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let resp = self
                .client
                .get(format!("{}/api/tags", self.base_url))
                .send()
                .await;
            Ok(resp.is_ok())
        })
    }
}

/// Ollama embedding provider.
pub struct OllamaEmbedder {
    base_url: String,
    model: String,
    dimension: usize,
    client: reqwest::Client,
}

impl OllamaEmbedder {
    pub fn new(base_url: &str, model: &str, dimension: usize) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            dimension,
            client,
        }
    }

    /// Default for nomic-embed-text (768 dimensions).
    pub fn nomic(base_url: &str) -> Self {
        Self::new(base_url, "nomic-embed-text", 768)
    }
}

#[derive(Serialize)]
struct OllamaEmbedRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl Embedder for OllamaEmbedder {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            if texts.is_empty() {
                return Ok(vec![]);
            }

            let request = OllamaEmbedRequest {
                model: self.model.clone(),
                input: texts.to_vec(),
            };

            let resp = self
                .client
                .post(format!("{}/api/embed", self.base_url))
                .json(&request)
                .send()
                .await
                .map_err(|e| CoreError::Other(format!("Ollama embed request failed: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(CoreError::Other(format!(
                    "Ollama embed returned {}: {}",
                    status, body
                )));
            }

            let response: OllamaEmbedResponse = resp
                .json()
                .await
                .map_err(|e| CoreError::Other(format!("Ollama embed parse error: {}", e)))?;

            Ok(response.embeddings)
        })
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_chat_request_includes_system_prompt_message() {
        let request = build_chat_request(
            "ollama-test",
            &[ChatMessage {
                role: MessageRole::User,
                content: "hello".to_string(),
            }],
            &CompletionOptions {
                system_prompt: Some("stay grounded".to_string()),
                ..Default::default()
            },
        );

        let body = serde_json::to_value(request).unwrap();
        assert_eq!(
            body["messages"][0],
            json!({"role": "system", "content": "stay grounded"})
        );
        assert_eq!(
            body["messages"][1],
            json!({"role": "user", "content": "hello"})
        );
    }
}
