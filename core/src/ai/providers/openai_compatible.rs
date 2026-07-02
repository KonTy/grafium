//! OpenAI-compatible provider — works with OpenAI-style APIs (e.g., vLLM).

use serde::{Deserialize, Serialize};

use crate::ai::traits::{
    BoxFuture, ChatMessage, CompletionOptions, Embedder, LlmProvider, MessageRole,
};
use crate::error::{CoreError, Result};

/// OpenAI-compatible LLM provider (custom base URL, optional API key).
pub struct OpenAiCompatibleLlm {
    base_url: String,
    api_key: Option<String>,
    model: String,
    client: reqwest::Client,
}

impl OpenAiCompatibleLlm {
    pub fn new(base_url: &str, model: &str, api_key: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default();

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model: model.to_string(),
            client,
        }
    }

    fn with_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(key) = &self.api_key {
            req.header("Authorization", format!("Bearer {}", key))
        } else {
            req
        }
    }
}

#[derive(Serialize)]
struct OpenAiChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
}

#[derive(Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
}

impl LlmProvider for OpenAiCompatibleLlm {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let oai_messages: Vec<OpenAiMessage> = messages
                .iter()
                .map(|m| OpenAiMessage {
                    role: match m.role {
                        MessageRole::System => "system".to_string(),
                        MessageRole::User => "user".to_string(),
                        MessageRole::Assistant => "assistant".to_string(),
                    },
                    content: m.content.clone(),
                })
                .collect();

            let request = OpenAiChatRequest {
                model: self.model.clone(),
                messages: oai_messages,
                max_tokens: options.max_tokens,
                temperature: options.temperature,
                stop: options.stop.clone(),
            };

            let req = self
                .client
                .post(format!("{}/chat/completions", self.base_url))
                .json(&request);
            let resp = self
                .with_auth(req)
                .send()
                .await
                .map_err(|e| CoreError::Other(format!("OpenAI-compatible request failed: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(CoreError::Other(format!(
                    "OpenAI-compatible endpoint returned {}: {}",
                    status, body
                )));
            }

            let response: OpenAiChatResponse = resp
                .json()
                .await
                .map_err(|e| CoreError::Other(format!("OpenAI-compatible parse error: {}", e)))?;

            response
                .choices
                .first()
                .and_then(|c| c.message.content.clone())
                .ok_or_else(|| CoreError::Other("OpenAI-compatible returned empty response".to_string()))
        })
    }

    fn name(&self) -> &str {
        "openai-compatible"
    }

    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let req = self.client.get(format!("{}/models", self.base_url));
            let resp = self.with_auth(req).send().await;
            if let Ok(r) = resp {
                if r.status().is_success() {
                    return Ok(true);
                }
            }

            // GitHub Models uses /inference/chat/completions and may not expose /models.
            if self.base_url.contains("models.github.ai/inference") {
                let request = OpenAiChatRequest {
                    model: self.model.clone(),
                    messages: vec![OpenAiMessage {
                        role: "user".to_string(),
                        content: "ping".to_string(),
                    }],
                    max_tokens: Some(1),
                    temperature: Some(0.0),
                    stop: None,
                };
                let req = self
                    .client
                    .post(format!("{}/chat/completions", self.base_url))
                    .json(&request);
                let resp = self.with_auth(req).send().await;
                return Ok(resp.map(|r| r.status().is_success()).unwrap_or(false));
            }

            Ok(false)
        })
    }
}

/// OpenAI-compatible embedding provider.
pub struct OpenAiCompatibleEmbedder {
    base_url: String,
    api_key: Option<String>,
    model: String,
    dimension: usize,
    client: reqwest::Client,
}

impl OpenAiCompatibleEmbedder {
    pub fn new(base_url: &str, model: &str, dimension: usize, api_key: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model: model.to_string(),
            dimension,
            client,
        }
    }

    fn with_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(key) = &self.api_key {
            req.header("Authorization", format!("Bearer {}", key))
        } else {
            req
        }
    }
}

#[derive(Serialize)]
struct OpenAiEmbedRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct OpenAiEmbedResponse {
    data: Vec<OpenAiEmbedData>,
}

#[derive(Deserialize)]
struct OpenAiEmbedData {
    embedding: Vec<f32>,
}

impl Embedder for OpenAiCompatibleEmbedder {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            if texts.is_empty() {
                return Ok(vec![]);
            }

            let request = OpenAiEmbedRequest {
                model: self.model.clone(),
                input: texts.to_vec(),
            };

            let req = self
                .client
                .post(format!("{}/embeddings", self.base_url))
                .json(&request);
            let resp = self
                .with_auth(req)
                .send()
                .await
                .map_err(|e| CoreError::Other(format!("OpenAI-compatible embed request failed: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(CoreError::Other(format!(
                    "OpenAI-compatible embed returned {}: {}",
                    status, body
                )));
            }

            let response: OpenAiEmbedResponse = resp
                .json()
                .await
                .map_err(|e| CoreError::Other(format!("OpenAI-compatible embed parse error: {}", e)))?;

            Ok(response.data.into_iter().map(|d| d.embedding).collect())
        })
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}
