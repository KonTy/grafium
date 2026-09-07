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
//!   GEN_TEST_SYSTEM      system prompt (default: none)
//!   GEN_TEST_GPU_LAYERS  layers to offload (default: CPU)
//!   GEN_TEST_MAX_TOKENS  output token cap (default: 400)

use grafium_core::ai::providers::local_llm::LocalLlm;
use grafium_core::ai::reasoning::{strip_think_blocks, ThinkStripResult};
use grafium_core::ai::traits::{ChatMessage, CompletionOptions, LlmProvider, MessageRole};
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    // Post-isolation, completions run in a re-exec of this binary, so the
    // harness has to answer worker invocations and register itself as the host.
    if grafium_core::ai::worker::is_worker_invocation() {
        std::process::exit(grafium_core::ai::worker::run_from_stdio());
    }
    grafium_core::ai::worker::configure_current_executable()
        .expect("failed to configure native AI worker");

    let model = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: gen_test <model.gguf>"),
    );
    let prompt = std::env::var("GEN_TEST_PROMPT")
        .unwrap_or_else(|_| "Say hello in exactly five words.".to_string());
    let system_prompt = std::env::var("GEN_TEST_SYSTEM").ok();
    let gpu_layers: Option<u32> = std::env::var("GEN_TEST_GPU_LAYERS")
        .ok()
        .and_then(|v| v.parse().ok());
    let max_tokens: u32 = std::env::var("GEN_TEST_MAX_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400);

    let t0 = std::time::Instant::now();
    let llm = LocalLlm::load(&model, Some(2048), gpu_layers).expect("load failed");
    println!("LOAD TOOK {:?}", t0.elapsed());
    println!("SUPPORTS_THINKING: {}", llm.supports_thinking());

    let msgs = vec![ChatMessage {
        role: MessageRole::User,
        content: prompt,
    }];
    let opts = CompletionOptions {
        max_tokens: Some(max_tokens),
        temperature: Some(0.0),
        system_prompt,
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
