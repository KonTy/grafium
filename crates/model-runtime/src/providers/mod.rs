//! HTTP transports without application configuration, storage, or tools.
//!
//! Default clients allow configured HTTP(S) endpoints (including LAN servers),
//! disable redirects and protocol retries, and retain reqwest's system proxy
//! behavior. Provider names never confer a "local-only" security guarantee.
//! Hosts may install a URL policy and/or a custom client. A URL policy alone
//! cannot constrain resolved IPs, DNS rebinding, proxies, or custom-client
//! redirects: strict local-only hosts must also control DNS, proxy and redirect
//! behavior in their client/network environment.

pub mod anthropic;
mod http;
pub mod ollama;
pub mod openai;
pub mod openai_compatible;

pub use http::{NetworkConfig, NetworkPolicy, ResponseLimits};

use crate::{
    error::{Result, RuntimeError},
    types::{ChatMessage, CompletionOptions, MessageRole},
};

fn canonicalize_messages(
    messages: &[ChatMessage],
    options: &CompletionOptions,
) -> Vec<ChatMessage> {
    let mut result =
        Vec::with_capacity(messages.len() + usize::from(options.system_prompt.is_some()));
    if let Some(system) = &options.system_prompt {
        result.push(ChatMessage {
            role: MessageRole::System,
            content: system.clone(),
        });
    }
    result.extend_from_slice(messages);
    result
}

fn validate_embeddings(vectors: &[Vec<f32>], count: usize, dimension: usize) -> Result<()> {
    if vectors.len() != count
        || vectors
            .iter()
            .any(|v| v.is_empty() || v.len() != dimension || v.iter().any(|x| !x.is_finite()))
    {
        return Err(RuntimeError::Other(
            "Provider returned invalid embedding cardinality or dimensions".into(),
        ));
    }
    Ok(())
}
