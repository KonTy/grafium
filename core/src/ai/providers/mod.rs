//! AI provider implementations.

pub mod anthropic;
#[cfg(feature = "llm-local")]
pub mod local_llm;
pub mod ollama;
pub mod openai;
pub mod openai_compatible;
