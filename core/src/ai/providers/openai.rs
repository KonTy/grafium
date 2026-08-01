//! OpenAI provider — GPT-4o / text-embedding-3-small via OpenAI API.

use serde::{Deserialize, Serialize};

use super::canonicalize_messages;
use crate::ai::traits::{
    BoxFuture, ChatMessage, CompletionOptions, Embedder, LlmProvider, MessageRole,
};
use crate::error::{CoreError, Result};

/// OpenAI LLM provider.
pub struct OpenAiLlm {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiLlm {
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

fn build_chat_request(
    model: &str,
    messages: &[ChatMessage],
    options: &CompletionOptions,
) -> OpenAiChatRequest {
    let messages = canonicalize_messages(messages, options)
        .into_iter()
        .map(|message| OpenAiMessage {
            role: match message.role {
                MessageRole::System => "system".to_string(),
                MessageRole::User => "user".to_string(),
                MessageRole::Assistant => "assistant".to_string(),
            },
            content: message.content,
        })
        .collect();

    OpenAiChatRequest {
        model: model.to_string(),
        messages,
        max_tokens: options.max_tokens,
        temperature: options.temperature,
        stop: options.stop.clone(),
    }
}

impl LlmProvider for OpenAiLlm {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let request = build_chat_request(&self.model, messages, options);

            let resp = self
                .client
                .post("https://api.openai.com/v1/chat/completions")
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&request)
                .send()
                .await
                .map_err(|e| CoreError::Other(format!("OpenAI request failed: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(CoreError::Other(format!(
                    "OpenAI returned {}: {}",
                    status, body
                )));
            }

            let response: OpenAiChatResponse = resp
                .json()
                .await
                .map_err(|e| CoreError::Other(format!("OpenAI response parse error: {}", e)))?;

            response
                .choices
                .first()
                .and_then(|c| c.message.content.clone())
                .ok_or_else(|| CoreError::Other("OpenAI returned empty response".to_string()))
        })
    }

    fn name(&self) -> &str {
        "openai"
    }

    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let resp = self
                .client
                .get("https://api.openai.com/v1/models")
                .header("Authorization", format!("Bearer {}", self.api_key))
                .send()
                .await;
            Ok(resp.map(|r| r.status().is_success()).unwrap_or(false))
        })
    }
}

/// OpenAI embedding provider.
pub struct OpenAiEmbedder {
    api_key: String,
    model: String,
    dimension: usize,
    client: reqwest::Client,
}

impl OpenAiEmbedder {
    pub fn new(api_key: &str, model: &str, dimension: usize) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();

        Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            dimension,
            client,
        }
    }

    /// Default: text-embedding-3-small (1536 dimensions).
    pub fn default_small(api_key: &str) -> Self {
        Self::new(api_key, "text-embedding-3-small", 1536)
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

impl Embedder for OpenAiEmbedder {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            if texts.is_empty() {
                return Ok(vec![]);
            }

            let request = OpenAiEmbedRequest {
                model: self.model.clone(),
                input: texts.to_vec(),
            };

            let resp = self
                .client
                .post("https://api.openai.com/v1/embeddings")
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&request)
                .send()
                .await
                .map_err(|e| CoreError::Other(format!("OpenAI embed request failed: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(CoreError::Other(format!(
                    "OpenAI embed returned {}: {}",
                    status, body
                )));
            }

            let response: OpenAiEmbedResponse = resp
                .json()
                .await
                .map_err(|e| CoreError::Other(format!("OpenAI embed parse error: {}", e)))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_chat_request_includes_system_prompt_message() {
        let request = build_chat_request(
            "gpt-test",
            &[ChatMessage {
                role: MessageRole::User,
                content: "hello".to_string(),
            }],
            &CompletionOptions {
                system_prompt: Some("be concise".to_string()),
                ..Default::default()
            },
        );

        let body = serde_json::to_value(request).unwrap();
        assert_eq!(
            body["messages"][0],
            json!({"role": "system", "content": "be concise"})
        );
        assert_eq!(
            body["messages"][1],
            json!({"role": "user", "content": "hello"})
        );
    }
}
