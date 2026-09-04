//! AI provider implementations.

use crate::ai::traits::{ChatMessage, CompletionOptions, MessageRole};
use crate::error::{CoreError, Result};

const MAX_PROVIDER_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub mod anthropic;
#[cfg(feature = "llm-local")]
pub mod local_llm;
pub mod ollama;
pub mod openai;

pub(super) fn canonicalize_messages(
    messages: &[ChatMessage],
    options: &CompletionOptions,
) -> Vec<ChatMessage> {
    let mut canonical =
        Vec::with_capacity(messages.len() + usize::from(options.system_prompt.is_some()));
    if let Some(system_prompt) = &options.system_prompt {
        canonical.push(ChatMessage {
            role: MessageRole::System,
            content: system_prompt.clone(),
        });
    }

    canonical.extend(messages.iter().cloned());
    canonical
}

pub(super) async fn read_limited_response(
    mut response: reqwest::Response,
    provider: &str,
) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PROVIDER_RESPONSE_BYTES as u64)
    {
        return Err(CoreError::Other(format!(
            "{provider} response exceeds the 16 MiB safety limit"
        )));
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| CoreError::Other(format!("{provider} response read failed: {e}")))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_PROVIDER_RESPONSE_BYTES {
            return Err(CoreError::Other(format!(
                "{provider} response exceeds the 16 MiB safety limit"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
