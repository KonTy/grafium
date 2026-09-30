//! Application-independent model inputs and provider contracts.

use std::{
    future::Future,
    pin::Pin,
    sync::{atomic::AtomicBool, Arc},
};

use serde::{Deserialize, Serialize};

use crate::error::{Result, RuntimeError};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionOptions {
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub system_prompt: Option<String>,
    pub stop: Option<Vec<String>>,
    /// Live cooperative cancellation handle, never serialized. HTTP providers
    /// return `RuntimeError::Cancelled`, including after streaming partial text.
    #[serde(skip)]
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Default for CompletionOptions {
    fn default() -> Self {
        Self {
            max_tokens: Some(2048),
            temperature: Some(0.3),
            system_prompt: None,
            stop: None,
            cancel: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceleratorStatus {
    pub gpu_supported: bool,
    pub on_gpu: bool,
    pub gpu_layers: u32,
    pub free_vram_mib_at_load: Option<u64>,
    pub model_mib: Option<u64>,
    pub explicit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Concurrency {
    Parallel,
    #[serde(rename_all = "camelCase")]
    Serialized {
        slots: usize,
    },
}

impl Concurrency {
    pub fn serialized(slots: usize) -> Self {
        Self::Serialized {
            slots: slots.max(1),
        }
    }

    pub fn queues(&self) -> bool {
        matches!(self, Self::Serialized { .. })
    }

    pub fn slots(&self) -> Option<usize> {
        match self {
            Self::Parallel => None,
            Self::Serialized { slots } => Some(*slots),
        }
    }
}

pub trait LlmProvider: Send + Sync {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>>;

    fn name(&self) -> &str;

    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>>;

    fn context_window(&self) -> Option<usize> {
        None
    }

    /// `None` means no tokenizer; errors must not be disguised as unavailable.
    fn count_prompt_tokens<'a>(
        &'a self,
        _messages: &'a [ChatMessage],
        _options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<Option<usize>>> {
        Box::pin(async { Ok(None) })
    }

    fn supports_thinking(&self) -> bool {
        false
    }

    fn concurrency(&self) -> Concurrency {
        Concurrency::Parallel
    }

    /// Deltas are provisional until this future succeeds. On failure or
    /// cancellation, a host must not treat previously emitted text as success.
    fn complete_stream<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
        on_token: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let text = self.complete(messages, options).await?;
            on_token(&text);
            Ok(text)
        })
    }

    fn accelerator_status(&self) -> Option<AcceleratorStatus> {
        None
    }
    fn backend_summary(&self) -> Option<String> {
        None
    }
    fn abort_in_flight(&self) {}
}

pub trait Embedder: Send + Sync {
    /// One nonempty vector per input, in input order.
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>>;

    fn embed_documents<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        self.embed(texts)
    }

    fn embed_query<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>>> {
        Box::pin(async move {
            let texts = [text.to_string()];
            let mut out = self.embed_queries(&texts).await?;
            if out.len() != 1 || out[0].is_empty() {
                return Err(RuntimeError::Other(
                    "embedder returned invalid query vectors".into(),
                ));
            }
            Ok(out.remove(0))
        })
    }

    fn embed_queries<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        self.embed(texts)
    }

    fn dimension(&self) -> usize;
    fn model_name(&self) -> &str;
    fn embedding_scheme_id(&self) -> String {
        String::new()
    }
}
