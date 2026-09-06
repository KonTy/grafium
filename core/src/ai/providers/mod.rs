//! AI provider implementations.

use crate::ai::traits::{ChatMessage, CompletionOptions, MessageRole};

pub mod anthropic;
#[cfg(feature = "llm-local")]
pub mod local_embedder;
#[cfg(feature = "llm-local")]
pub mod local_llm;
#[cfg(feature = "llm-local")]
pub mod local_llm_process;
#[cfg(feature = "llm-local")]
mod llama_shared;
pub mod ollama;
pub mod openai;
pub mod openai_compatible;

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
