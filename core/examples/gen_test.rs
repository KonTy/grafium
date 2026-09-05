use std::path::PathBuf;
use grafium_core::ai::providers::local_llm::LocalLlm;
use grafium_core::ai::traits::{ChatMessage, CompletionOptions, LlmProvider, MessageRole};

#[tokio::main]
async fn main() {
    let model = PathBuf::from(std::env::args().nth(1).expect("model path"));
    let t0 = std::time::Instant::now();
    let llm = LocalLlm::load(&model, Some(2048), None).expect("load failed");
    println!("LOAD TOOK {:?}", t0.elapsed());
    let msgs = vec![ChatMessage { role: MessageRole::User, content: "Say hello in exactly five words.".into() }];
    let opts = CompletionOptions { max_tokens: Some(400), temperature: Some(0.0), ..Default::default() };
    let t1 = std::time::Instant::now();
    let out = llm.complete(&msgs, &opts).await.expect("gen failed");
    println!("GEN TOOK {:?} -> {out}", t1.elapsed());
}
