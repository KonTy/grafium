//! End-to-end check of the full web-research pipeline against the real
//! internet and a real local model: plan queries, search, fetch, synthesize.
//!
//! Everything below `web_search` is exercised only against fakes in the unit
//! tests, so this is what proves the feature actually works rather than merely
//! compiling. Prints each progress step so a stall is attributable to a
//! specific stage.
//!
//! Usage:
//!   cargo run -p grafium-core --release --features llm-local-vulkan \
//!       --example research_e2e -- <data-dir> "question"
use std::path::PathBuf;
use std::time::Instant;

use grafium_core::ai::config::AiConfig;
use grafium_core::ai::providers::local_llm::LocalLlm;
use grafium_core::ai::web_research::WebResearchEngine;
use grafium_core::scraping::browser::HttpBrowserDriver;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let data_dir = PathBuf::from(args.next().expect("usage: <data-dir> <question>"));
    let question = args
        .next()
        .unwrap_or_else(|| "does creatine cause cancer".to_string());

    let config_path = data_dir.join("ai_config.json");
    let config: AiConfig = serde_json::from_str(
        &std::fs::read_to_string(&config_path).expect("cannot read ai_config.json"),
    )
    .expect("cannot parse ai_config.json");

    let llm = LocalLlm::from_config(&config, &data_dir).expect("model load failed");
    let browser = HttpBrowserDriver::new();
    let engine = WebResearchEngine::new(&llm, &browser);

    let started = Instant::now();
    let mut progress = |msg: &str| {
        println!("[{:>6.1}s] {msg}", started.elapsed().as_secs_f64());
    };

    println!("question: {question:?}\n");
    match engine.research(&question, "", &mut progress).await {
        Ok(result) => {
            println!("\n=== title answer ===");
            println!("{}", result.title_answer.as_deref().unwrap_or("(none)"));
            println!("\n=== topics ({}) ===", result.topics.len());
            for t in &result.topics {
                println!("\n## {}\n{}", t.topic, t.summary);
            }
            println!("\n=== citations ({}) ===", result.citations.len());
            for c in &result.citations {
                println!("  [{}] {} — {}", c.number, c.title, c.url);
            }
            println!("\ntotal: {:.1}s", started.elapsed().as_secs_f64());
        }
        Err(err) => {
            eprintln!("FAIL after {:.1}s: {err}", started.elapsed().as_secs_f64());
            std::process::exit(1);
        }
    }
}
