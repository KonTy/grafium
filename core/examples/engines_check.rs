//! Queries every built-in search engine and reports what each returned.
//!
//! The engine registry is only as good as the engines actually working, and
//! the two failure modes — a wrong selector and an engine blocking us — look
//! identical from inside the app. Running them all against one query makes the
//! difference visible, and is the fastest way to tell a code regression from
//! somebody else's rate limit.
//!
//! Usage:
//!   cargo run -p grafium-core --release --example engines_check -- "query"
use grafium_core::research::ResearchConfig;
use grafium_core::scraping::browser::HttpBrowserDriver;
use grafium_core::scraping::engines::search_one;

#[tokio::main]
async fn main() {
    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bioelectricity morphogenesis".to_string());
    let config = ResearchConfig::default();
    let browser = HttpBrowserDriver::new();

    println!("query: {query:?}\n");
    println!("{:<18} {:<10} {:>7}  {}", "engine", "category", "hits", "first result");
    println!("{}", "-".repeat(100));

    let mut working = 0usize;
    for engine in &config.engines {
        let label = format!("{:?}", engine.category);
        match search_one(&browser, engine, &query, 5).await {
            Ok(results) if results.is_empty() => {
                println!("{:<18} {:<10} {:>7}  (no results)", engine.id, label, 0);
            }
            Ok(results) => {
                working += 1;
                let first = results[0].title.chars().take(56).collect::<String>();
                println!("{:<18} {:<10} {:>7}  {}", engine.id, label, results.len(), first);
            }
            Err(err) => {
                let msg = err.to_string().chars().take(56).collect::<String>();
                println!("{:<18} {:<10} {:>7}  ERROR: {}", engine.id, label, 0, msg);
            }
        }
    }
    println!("\n{working}/{} engines returned results", config.engines.len());
}
