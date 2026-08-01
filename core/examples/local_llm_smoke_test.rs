//! Manual smoke test for the embedded local LLM path (not run in CI — needs
//! a real GGUF file on disk). Mirrors `model_settings_smoke_test.rs`:
//! import a downloaded model into the managed models directory, resolve it
//! via settings alone (zero explicit path), and run a real completion
//! through `LlmProvider::complete`.
//!
//! Usage:
//!   cargo run -p grafium-core --features llm-local --example local_llm_smoke_test \
//!       -- <downloaded-gguf-file> <data-dir> "<prompt>"
use std::path::PathBuf;

use grafium_core::ai::config::AiConfig;
use grafium_core::ai::providers::local_llm::LocalLlm;
use grafium_core::ai::traits::{ChatMessage, CompletionOptions, LlmProvider, MessageRole};
use grafium_core::model_library;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let downloaded_model = PathBuf::from(
        args.next()
            .expect("usage: <downloaded-gguf> <data-dir> <prompt>"),
    );
    let data_dir = PathBuf::from(
        args.next()
            .expect("usage: <downloaded-gguf> <data-dir> <prompt>"),
    );
    let prompt = args
        .next()
        .unwrap_or_else(|| "What is 2 + 2? Answer in one short sentence.".to_string());

    // Step 1: "import" — copy the model the user downloaded into Grafium's
    // managed models directory, exactly like the Whisper flow.
    let models_dir = model_library::default_models_dir(&data_dir);
    let info = model_library::import_model(&downloaded_model, &models_dir).expect("import failed");
    println!(
        "Imported: {} ({} bytes, kind={:?})",
        info.file_name, info.size_bytes, info.kind
    );

    // Step 2: settings has *nothing* configured for `local.local_llm.model`
    // — the zero-config path: with exactly one LLM model imported, it's
    // just picked automatically.
    let config = AiConfig::default();
    let llm = LocalLlm::from_config(&config, &data_dir).expect("model resolution/load failed");
    println!(
        "Resolved + loaded model successfully from settings alone: {}",
        llm.name()
    );

    // Step 3: run a real chat completion through the `LlmProvider` trait —
    // the same interface `OllamaLlm`/`OpenAiLlm` implement, proving the
    // embedded runtime is a drop-in provider, not a bespoke call site.
    let messages = vec![ChatMessage {
        role: MessageRole::User,
        content: prompt.clone(),
    }];
    let options = CompletionOptions {
        max_tokens: Some(128),
        temperature: Some(0.0),
        ..Default::default()
    };

    println!("\nPrompt: {prompt}\n--- response ---");
    let response = llm
        .complete(&messages, &options)
        .await
        .expect("completion failed");
    println!("{response}");
}
