//! Live check that the web-search scrape still works against the real
//! internet.
//!
//! The research feature's unit tests use a fake `BrowserDriver`, which proves
//! the parsing and orchestration but says nothing about whether the search
//! engine still serves the markup the parser expects — the one part of this
//! feature that can break without a code change. This is the canary for that.
//!
//! Usage:
//!   cargo run -p grafium-core --release --example web_search_check -- "query"
use grafium_core::scraping::browser::HttpBrowserDriver;
use grafium_core::scraping::search::web_search;

#[tokio::main]
async fn main() {
    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "does creatine cause cancer".to_string());

    let browser = HttpBrowserDriver::new();
    println!("query: {query:?}");
    match web_search(&browser, &query, 5).await {
        Ok(results) if results.is_empty() => {
            eprintln!("FAIL: search returned 0 results (blocked, or markup changed)");
            std::process::exit(1);
        }
        Ok(results) => {
            println!("OK: {} results", results.len());
            for r in &results {
                println!("  - {}\n    {}", r.title, r.url);
            }
        }
        Err(err) => {
            eprintln!("FAIL: {err}");
            std::process::exit(1);
        }
    }
}
