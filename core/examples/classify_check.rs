//! Measures the cost and accuracy of the LLM web-research classifier.
//!
//! The rule-based trigger can't see through typos or novel phrasing, so the
//! classifier is the fallback. But it runs on the critical path of every
//! question, so its latency is a product decision, not an implementation
//! detail — this makes both the latency and the verdicts measurable.
//!
//! Usage:
//!   cargo run -p grafium-core --release --features llm-local-vulkan \
//!       --example classify_check -- <data-dir>
use std::path::PathBuf;
use std::time::Instant;

use grafium_core::ai::config::AiConfig;
use grafium_core::ai::providers::local_llm::LocalLlm;
use grafium_core::knowledge::research_intent::{classify_needs_web, rules_reject_research};

/// `(question, expected)` — expected is whether a web search is wanted.
const CASES: &[(&str, bool)] = &[
    // Typos and phrasings the rules miss.
    (
        "eatch on the internet what is the last publication by Michael Levin",
        true,
    ),
    (
        "can you check what papers did Michael Levin published recently",
        true,
    ),
    ("what is the latest news about nvidia earnings", true),
    ("who won the world cup last year", true),
    ("find the newest research on creatine and cancer", true),
    ("what is the current price of bitcoin", true),
    // Things that must stay local or general.
    ("when was the last time I was upset", false),
    ("when did I paint my room", false),
    ("summarize my notes about scientology", false),
    ("what did I write yesterday", false),
    ("explain how mutexes work", false),
    ("what is 17 times 23", false),
    ("write me a haiku about rain", false),
    ("how do I search the web for academic papers", false),
];

#[tokio::main]
async fn main() {
    let data_dir = PathBuf::from(std::env::args().nth(1).expect("usage: <data-dir>"));
    let config: AiConfig = serde_json::from_str(
        &std::fs::read_to_string(data_dir.join("ai_config.json")).expect("cannot read config"),
    )
    .expect("cannot parse config");
    let llm = LocalLlm::from_config(&config, &data_dir).expect("model load failed");

    let mut correct = 0usize;
    let mut total_ms = 0f64;
    println!("{:<66} {:>8} {:>7} {:>7}", "question", "want", "got", "ms");
    println!("{}", "-".repeat(92));
    for (question, want) in CASES {
        let started = Instant::now();
        // Mirrors the routing in `ai_ask_stream`: a confident rule veto short
        // circuits before the model is consulted.
        let got = if rules_reject_research(question) {
            false
        } else {
            classify_needs_web(&llm, question).await
        };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        total_ms += ms;
        let ok = got == *want;
        if ok {
            correct += 1;
        }
        println!(
            "{:<66} {:>8} {:>7} {:>7.0} {}",
            truncate(question, 64),
            want,
            got,
            ms,
            if ok { "" } else { "  <-- WRONG" }
        );
    }
    println!(
        "\n{correct}/{} correct, mean {:.0} ms",
        CASES.len(),
        total_ms / CASES.len() as f64
    );
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}
