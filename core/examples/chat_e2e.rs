//! End-to-end smoke test for graph-aware Chat against a *real* graph.
//!
//! Indexes the live graph with the configured local embedder, then runs the
//! literal questions the feature was built for and prints what was retrieved.
//! Unlike the unit tests (which use a deterministic stand-in embedder), this
//! exercises the real GGUF embedding model, real SQLite FTS, and the real
//! fusion/gating/ordering path — so it is the check that matters before
//! shipping.
//!
//! Usage:
//!   cargo run -p grafium-core --release --features llm-local \
//!       --example chat_e2e -- <graph-dir> <knowledge-data-dir> [--index]

use std::path::PathBuf;

use grafium_core::ai::config::AiConfig;
use grafium_core::knowledge::KnowledgeEngine;
use grafium_core::Graph;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let graph_dir = PathBuf::from(args.next().expect("usage: <graph-dir> <data-dir> [--index]"));
    let data_dir = PathBuf::from(args.next().expect("usage: <graph-dir> <data-dir> [--index]"));
    let do_index = args.any(|a| a == "--index");

    let config_path = data_dir.join("ai_config.json");
    let config: AiConfig = serde_json::from_str(
        &std::fs::read_to_string(&config_path).expect("cannot read ai_config.json"),
    )
    .expect("cannot parse ai_config.json");
    println!(
        "config: mode={:?} llm={:?} embedder={:?}",
        config.mode,
        config.local.as_ref().map(|l| &l.local_llm.model_ref.model),
        config.local.as_ref().map(|l| &l.local_embedding.model_ref.model)
    );

    let graph = Graph::open(&graph_dir).expect("cannot open graph");
    let engine = KnowledgeEngine::new(&data_dir, config).expect("cannot build engine");
    println!(
        "engine: llm_ready={} ready={}",
        engine.is_llm_ready(),
        engine.is_ready()
    );

    let graph_id = graph_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("graph")
        .to_string();

    if do_index {
        let pages = graph.db.list_pages(100000, 0).expect("cannot list pages");
        println!("\nindexing {} pages...", pages.len());
        let started = std::time::Instant::now();
        let mut chunks = 0usize;
        let mut failed = 0usize;
        for (i, page) in pages.iter().enumerate() {
            let blocks = match graph.db.list_blocks_for_page(&page.id) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("  ! blocks for {}: {e}", page.title);
                    failed += 1;
                    continue;
                }
            };
            match engine.index_page(page, &blocks, &graph_id).await {
                Ok(n) => chunks += n,
                Err(e) => {
                    eprintln!("  ! index {}: {e}", page.title);
                    failed += 1;
                }
            }
            if (i + 1) % 20 == 0 {
                println!(
                    "  {}/{} pages, {chunks} chunks, {:.0}s",
                    i + 1,
                    pages.len(),
                    started.elapsed().as_secs_f64()
                );
            }
        }
        println!(
            "indexed {chunks} chunks from {} pages ({failed} failed) in {:.1}s",
            pages.len(),
            started.elapsed().as_secs_f64()
        );
    }

    let questions = [
        "when was the last time I was upset",
        "when did I paint my room",
        "explain how mutexes work",
    ];

    for q in questions {
        println!("\n=== {q} ===");
        let started = std::time::Instant::now();
        match engine.hybrid_search(&graph.db, q, 8, Some(&graph_id)).await {
            Ok(hits) => {
                println!("retrieved {} hits in {:?}", hits.len(), started.elapsed());
                for (i, h) in hits.iter().take(5).enumerate() {
                    let snippet: String = h.content.chars().take(90).collect();
                    println!("  [{}] {} :: {}", i + 1, h.page_title, snippet.replace('\n', " "));
                }
            }
            Err(e) => println!("  retrieval error: {e}"),
        }
    }
}
