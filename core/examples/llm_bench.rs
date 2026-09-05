//! Throughput benchmark for the embedded local LLM path.
//!
//! Loads one or more GGUF models and measures real decode throughput
//! (tokens/second) via the streaming callback, so the number reflects what
//! Chat actually feels like rather than a synthetic kernel benchmark.
//!
//! Usage:
//!   cargo run -p grafium-core --release --features llm-local-vulkan \
//!       --example llm_bench -- <models-dir> <model.gguf> [more.gguf ...]
use std::path::PathBuf;
use std::time::Instant;

use grafium_core::ai::providers::local_llm::LocalLlm;
use grafium_core::ai::traits::{ChatMessage, CompletionOptions, LlmProvider, MessageRole};

const MAX_TOKENS: u32 = 160;
const PROMPT: &str = "Write a short paragraph explaining what a knowledge graph is.";

/// Overridable so the same harness can probe refusal behaviour, not just speed.
fn prompt_text_base() -> String {
    std::env::var("BENCH_PROMPT").unwrap_or_else(|_| PROMPT.to_string())
}

/// Builds a prompt padded with `filler_tokens`-ish worth of synthetic
/// retrieved context. Chat sends thousands of tokens of graph context, so a
/// throughput number measured on a 12-token prompt says nothing useful about
/// how the app actually feels — prefill dominates there, not decode.
fn padded_prompt(filler_tokens: usize) -> String {
    let mut s = String::new();
    if filler_tokens > 0 {
        s.push_str("Context from the user's notes:\n");
        for i in 0..filler_tokens / 12 {
            s.push_str(&format!(
                "- On day {i} I recorded a note about project planning, meetings, \
                 and follow-up tasks that needed attention later that week.\n"
            ));
        }
        s.push('\n');
    }
    s.push_str(&prompt_text_base());
    s
}

#[tokio::main]
async fn main() {
    // llama.cpp logs its own "offloaded N/M layers to GPU" line to stderr,
    // which is the signal we care about here, so no subscriber is needed.
    let mut args = std::env::args().skip(1);
    let models_dir = PathBuf::from(args.next().expect("usage: <models-dir> <model.gguf>..."));
    let models: Vec<String> = args.collect();
    assert!(!models.is_empty(), "need at least one model file name");

    // Sized to bracket what Chat really sends: a bare question, a modest
    // retrieval, and a near-full 8k context window.
    let filler: usize = std::env::var("BENCH_CONTEXT_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let ctx_size: u32 = std::env::var("BENCH_CTX_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8192);
    let prompt_text = padded_prompt(filler);
    println!(
        "context filler: ~{filler} tokens ({} chars)\n",
        prompt_text.len()
    );

    println!(
        "{:<52} {:>8} {:>9} {:>10} {:>9}",
        "model", "load_s", "ttft_s", "tok", "tok/s"
    );
    println!("{}", "-".repeat(92));

    for name in &models {
        let path = models_dir.join(name);
        if !path.exists() {
            println!("{name:<52} MISSING");
            continue;
        }

        let load_start = Instant::now();
        // `None` gpu_layers exercises the same auto-offload heuristic the app
        // uses, so a bad decision shows up here instead of only in the UI.
        let llm = match LocalLlm::load(&path, Some(ctx_size), None) {
            Ok(llm) => llm,
            Err(err) => {
                println!("{name:<52} LOAD FAILED: {err}");
                continue;
            }
        };
        let load_s = load_start.elapsed().as_secs_f64();

        let messages = vec![ChatMessage {
            role: MessageRole::User,
            content: prompt_text.clone(),
        }];
        let options = CompletionOptions {
            max_tokens: Some(MAX_TOKENS),
            temperature: Some(0.0),
            ..Default::default()
        };

        let mut tokens = 0usize;
        let mut first_token_at: Option<Instant> = None;
        let gen_start = Instant::now();
        let mut on_token = |_: &str| {
            if first_token_at.is_none() {
                first_token_at = Some(Instant::now());
            }
            tokens += 1;
        };

        let result = llm
            .complete_stream(&messages, &options, &mut on_token)
            .await;
        let total = gen_start.elapsed().as_secs_f64();

        match result {
            Ok(text) => {
                let ttft = first_token_at
                    .map(|t| t.duration_since(gen_start).as_secs_f64())
                    .unwrap_or(f64::NAN);
                // Decode rate excludes prefill so it isn't diluted by prompt
                // processing, which scales with context rather than model speed.
                let decode_s = (total - ttft).max(f64::EPSILON);
                let rate = (tokens.saturating_sub(1)) as f64 / decode_s;
                println!("{name:<52} {load_s:>8.1} {ttft:>9.2} {tokens:>10} {rate:>9.1}");
                let preview: String = text.chars().take(110).collect();
                println!("    -> {}", preview.replace('\n', " "));
            }
            Err(err) => println!("{name:<52} GEN FAILED: {err}"),
        }
    }
}
