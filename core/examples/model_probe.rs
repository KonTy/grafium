//! Prints what the loader infers about a model: whether it looks like a
//! reasoning model, and the chat template that decision is read from.
//!
//! Reasoning detection drives the `/no_think` directive, and when it guesses
//! wrong the symptom is subtle — the model emits raw chain-of-thought as its
//! answer instead of answering. This makes that decision inspectable.
//!
//! Usage:
//!   cargo run -p grafium-core --release --features llm-local-vulkan \
//!       --example model_probe -- <model.gguf>
use std::path::PathBuf;

use grafium_core::ai::providers::local_llm::LocalLlm;
use grafium_core::ai::traits::LlmProvider;

fn main() {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: model_probe <model.gguf>"),
    );
    let llm = LocalLlm::load(&path, Some(2048), None).expect("load failed");
    println!("name:              {}", llm.name());
    println!("supports_thinking: {}", llm.supports_thinking());
    println!("context_window:    {:?}", llm.context_window());
    println!("--- chat template ---");
    println!(
        "{}",
        llm.chat_template_for_debug()
            .unwrap_or_else(|| "(none)".into())
    );
}
