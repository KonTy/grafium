//! Ollama provider — local LLM and embedding via Ollama's HTTP API.
//!
//! Ollama runs at http://localhost:11434 by default.
//! Supports both chat completions and embeddings.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::{canonicalize_messages, read_limited_response};
use crate::ai::resources::{self, ModelWorkload};
use crate::ai::traits::{
    BoxFuture, ChatMessage, CompletionOptions, Embedder, LlmProvider, MessageRole,
};
use crate::error::{CoreError, Result};

const OLLAMA_KEEP_ALIVE: &str = "30s";
const OLLAMA_PREFLIGHT_TTL: Duration = Duration::from_secs(30);

/// Ollama LLM provider.
pub struct OllamaLlm {
    base_url: String,
    model: String,
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelTag>,
}

#[derive(Deserialize)]
struct OllamaModelTag {
    name: String,
    size: u64,
}

async fn preflight_local_ollama_model(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
) -> Result<()> {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return Err(CoreError::Other("Invalid Ollama base URL".to_string()));
    };
    if !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1")) {
        return Ok(());
    }

    static PREFLIGHTS: OnceLock<
        tokio::sync::Mutex<HashMap<(String, String), Instant>>,
    > = OnceLock::new();
    let preflights = PREFLIGHTS.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()));
    let cache_key = (base_url.to_string(), model.to_string());
    let mut approvals = preflights.lock().await;
    if approvals
        .get(&cache_key)
        .is_some_and(|approved| approved.elapsed() < OLLAMA_PREFLIGHT_TTL)
    {
        return Ok(());
    }

    let response = client
        .get(format!("{}/api/tags", base_url.trim_end_matches('/')))
        .send()
        .await
        .map_err(|e| CoreError::Other(format!("Cannot inspect local Ollama models: {e}")))?;
    if !response.status().is_success() {
        return Err(CoreError::Other(
            "Cannot verify local Ollama model size; refusing unsafe model load".to_string(),
        ));
    }
    let body = read_limited_response(response, "Ollama model list").await?;
    let tags: OllamaTagsResponse = serde_json::from_slice(&body)
        .map_err(|e| CoreError::Other(format!("Cannot parse Ollama model list: {e}")))?;
    let tag = tags
        .models
        .iter()
        .find(|tag| tag.name == model || tag.name.strip_suffix(":latest") == Some(model))
        .ok_or_else(|| {
            CoreError::Other(format!(
                "Ollama model '{model}' is not installed, so Grafium cannot verify its size"
            ))
        })?;
    resources::validate_model_size(
        &format!("Ollama model {}", tag.name),
        tag.size,
        ModelWorkload::Llm {
            context_tokens: 4_096,
        },
    )?;
    approvals.insert(cache_key, Instant::now());
    Ok(())
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
    keep_alive: String,
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
    num_ctx: Option<u32>,
    num_thread: Option<i32>,
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
        keep_alive: OLLAMA_KEEP_ALIVE.to_string(),
        options: Some(OllamaOptions {
            temperature: options.temperature,
            num_predict: options.max_tokens,
            stop: options.stop.clone(),
            num_ctx: Some(4_096),
            num_thread: Some(resources::inference_thread_count()),
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
            preflight_local_ollama_model(&self.client, &self.base_url, &self.model).await?;
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
                let body = String::from_utf8_lossy(&read_limited_response(resp, "Ollama").await?)
                    .into_owned();
                return Err(CoreError::Other(format!(
                    "Ollama returned {}: {}",
                    status, body
                )));
            }

            let body = read_limited_response(resp, "Ollama").await?;
            let response: OllamaChatResponse = serde_json::from_slice(&body)
                .map_err(|e| CoreError::Other(format!("Ollama response parse error: {e}")))?;

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
    keep_alive: String,
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
            preflight_local_ollama_model(&self.client, &self.base_url, &self.model).await?;

            let request = OllamaEmbedRequest {
                model: self.model.clone(),
                input: texts.to_vec(),
                keep_alive: OLLAMA_KEEP_ALIVE.to_string(),
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
                let body =
                    String::from_utf8_lossy(&read_limited_response(resp, "Ollama embed").await?)
                        .into_owned();
                return Err(CoreError::Other(format!(
                    "Ollama embed returned {}: {}",
                    status, body
                )));
            }

            let body = read_limited_response(resp, "Ollama embed").await?;
            let response: OllamaEmbedResponse = serde_json::from_slice(&body)
                .map_err(|e| CoreError::Other(format!("Ollama embed parse error: {e}")))?;

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
