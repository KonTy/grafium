//! Standalone crash-repro / debug harness for the embedded local LLM path,
//! driven directly (no GUI needed) so real GGUF models on disk can be
//! exercised the same way "Analyze this Page" exercises them: a real
//! model load through the exact same auto gpu_layers/ctx_size logic the
//! app uses, followed by a long-ish prompt completion.
//!
//! Usage:
//!   cargo run -p grafium-core --features llm-local-vulkan --example debug_llm_repro \
//!       -- <path-to-gguf> [prompt-char-count] [max_tokens]
use std::path::PathBuf;
use std::time::Instant;

use grafium_core::ai::providers::local_llm::LocalLlm;
use grafium_core::ai::traits::{ChatMessage, CompletionOptions, LlmProvider, MessageRole};

fn vram_snapshot(label: &str) {
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used,memory.free", "--format=csv,noheader"])
        .output();
    match out {
        Ok(o) => eprintln!(
            "[vram:{label}] {}",
            String::from_utf8_lossy(&o.stdout).trim()
        ),
        Err(e) => eprintln!("[vram:{label}] nvidia-smi failed: {e}"),
    }
}

#[tokio::main]
async fn main() {
    // This harness predates AI process isolation. Completions now run in a
    // re-exec of this same binary, so it has to both answer worker
    // invocations and register itself as the worker host — otherwise every
    // completion fails with "native AI worker is not configured".
    if grafium_core::ai::worker::is_worker_invocation() {
        std::process::exit(grafium_core::ai::worker::run_from_stdio());
    }
    grafium_core::ai::worker::configure_current_executable()
        .expect("failed to configure native AI worker");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut args = std::env::args().skip(1);
    let model_path = PathBuf::from(args.next().expect("usage: <gguf-path> [chars] [max_tokens] [rounds] [gpu_layers]"));
    let prompt_chars: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6000);
    let max_tokens: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(256);
    let rounds: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let gpu_layers: Option<u32> = args.next().and_then(|s| s.parse().ok());

    eprintln!("[repro] model = {}", model_path.display());
    vram_snapshot("before-load");
    eprintln!("[repro] loading (context_size=None, gpu_layers={gpu_layers:?})...");
    let t0 = Instant::now();
    let llm = LocalLlm::load(&model_path, None, gpu_layers).expect("load failed");
    eprintln!("[repro] loaded '{}' in {:?}", llm.name(), t0.elapsed());
    vram_snapshot("after-load");

    // Mimic a realistic "Analyze this Page" prompt: a decent chunk of
    // repeated filler text standing in for real page content, long enough
    // to push the context window and one-shot decode batch size.
    let filler = "Grafium is a local-first knowledge graph and note-taking tool. ";
    let mut content = String::with_capacity(prompt_chars + filler.len());
    while content.len() < prompt_chars {
        content.push_str(filler);
    }
    content.truncate(prompt_chars);
    let prompt = format!(
        "Summarize the following page content and list 3 key topics:\n\n{content}"
    );
    eprintln!(
        "[repro] prompt length: {} chars, requesting max_tokens={max_tokens}, rounds={rounds}",
        prompt.len()
    );

    let messages = vec![ChatMessage {
        role: MessageRole::User,
        content: prompt,
    }];
    let options = CompletionOptions {
        max_tokens: Some(max_tokens),
        temperature: Some(0.0),
        ..Default::default()
    };

    for round in 1..=rounds {
        let t1 = Instant::now();
        eprintln!("[repro] round {round}/{rounds}: starting completion...");
        match llm.complete(&messages, &options).await {
            Ok(resp) => {
                eprintln!("[repro] round {round}: completion OK in {:?}", t1.elapsed());
                println!("--- round {round} response ({} chars) ---\n{resp}", resp.len());
            }
            Err(e) => {
                eprintln!("[repro] round {round}: completion FAILED after {:?}: {e}", t1.elapsed());
                vram_snapshot(&format!("round-{round}-failed"));
                std::process::exit(1);
            }
        }
        vram_snapshot(&format!("round-{round}-done"));
    }
}
