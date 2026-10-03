//! Bounded, opt-in native GPU smoke test with synthetic text and private,
//! temporary recovery state. Reads the supplied model without importing it.
//!
//! cargo run -p model-runtime --release --features llm-local-vulkan,media-vulkan \
//!   --example gpu_smoke -- /path/to/model.gguf

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use model_runtime::native::{llm::LocalLlm, worker};
use model_runtime::types::{ChatMessage, CompletionOptions, LlmProvider, MessageRole};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if worker::is_worker_invocation() {
        worker::run_from_stdio();
    }
    let path = PathBuf::from(std::env::args().nth(1).ok_or("supply a local GGUF model")?);
    let state = tempfile::tempdir()?;
    let mut host = worker::NativeHostConfig::new(std::env::current_exe()?, worker::WORKER_ARGUMENT);
    host.recovery_state_dir = Some(state.path().join("recovery"));
    host.inference_threads = Some(2);
    worker::configure_with_options(host)?;
    let result = run(&path).await;
    let shutdown = worker::shutdown();
    result?;
    shutdown?;
    Ok(())
}

async fn run(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let model = LocalLlm::load(path, Some(6144), Some(999))?;
    let messages = [ChatMessage {
        role: MessageRole::User,
        content: "What is love? Answer in two short sentences.".into(),
    }];
    let options = CompletionOptions {
        max_tokens: Some(64),
        temperature: Some(0.0),
        cancel: Some(Arc::new(AtomicBool::new(false))),
        ..Default::default()
    };
    for round in ["cold", "warm"] {
        let cancel = options.cancel.clone().ok_or("missing cancellation flag")?;
        let deadline = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            cancel.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        let mut first_text = None;
        let mut pieces = Vec::new();
        let result = model
            .complete_stream(&messages, &options, &mut |text| {
                first_text.get_or_insert_with(|| start.elapsed());
                pieces.push(text.to_owned());
            })
            .await;
        deadline.abort();
        let answer = result?;
        let status = model
            .accelerator_status()
            .ok_or("no resident model status")?;
        println!(
            "{}",
            serde_json::json!({
                "round": round,
                "first_text_seconds": first_text.map(|time| time.as_secs_f64()),
                "seconds": start.elapsed().as_secs_f64(),
                "streamed_pieces": pieces.len(),
                "on_gpu": status.on_gpu, "gpu_layers": status.gpu_layers,
                "answer": answer,
            })
        );
        if !status.on_gpu {
            return Err(format!("GPU was not used: {:?}", worker::runtime_warnings()).into());
        }
        if answer.trim().is_empty() {
            return Err("model returned no text".into());
        }
        if pieces.len() < 2 || pieces.concat().trim() != answer {
            return Err("generated text was not streamed incrementally and completely".into());
        }
    }
    Ok(())
}
