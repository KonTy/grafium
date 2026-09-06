//! Minimal local-LLM generation harness for diagnosing reasoning ("thinking")
//! models. Loads a GGUF, runs one prompt, and prints load/generation timing,
//! whether the model was detected as a reasoning model, the raw output, and
//! the `<think>`-stripped result — so a model that spends its whole budget
//! reasoning without answering is obvious at a glance.
//!
//! Usage:
//!   cargo run -p grafium-core --example gen_test --features llm-local -- /path/to/model.gguf
//!
//! Optional env vars:
//!   GEN_TEST_PROMPT      prompt text (default: "Say hello in exactly five words.")
//!   GEN_TEST_MAX_TOKENS  output token cap (default: 400)

use grafium_core::ai::providers::local_llm::LocalLlm;
use grafium_core::ai::reasoning::{strip_think_blocks, ThinkStripResult};
use grafium_core::ai::traits::{ChatMessage, CompletionOptions, LlmProvider, MessageRole};
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    let model = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: gen_test <model.gguf>"),
    );
    let prompt = std::env::var("GEN_TEST_PROMPT")
        .unwrap_or_else(|_| "Say hello in exactly five words.".to_string());
    let max_tokens: u32 = std::env::var("GEN_TEST_MAX_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400);

    let t0 = std::time::Instant::now();
    let llm = LocalLlm::load(&model, Some(2048), None).expect("load failed");
    println!("LOAD TOOK {:?}", t0.elapsed());
    println!("SUPPORTS_THINKING: {}", llm.supports_thinking());

    let msgs = vec![ChatMessage {
        role: MessageRole::User,
        content: prompt,
    }];
    let opts = CompletionOptions {
        max_tokens: Some(max_tokens),
        temperature: Some(0.0),
        ..Default::default()
    };

    let t1 = std::time::Instant::now();
    let out = llm.complete(&msgs, &opts).await.expect("gen failed");
    println!("GEN TOOK {:?}", t1.elapsed());
    println!("--- RAW ---\n{out}");
    match strip_think_blocks(&out) {
        ThinkStripResult::Answer(a) => println!("--- STRIPPED ANSWER ---\n{a}"),
        ThinkStripResult::ReasoningOnly => {
            println!("--- REASONING ONLY (budget exhausted, no answer) ---")
        }
    }
}
