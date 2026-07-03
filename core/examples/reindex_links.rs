//! One-shot maintenance tool: rebuild the `links` index from wiki-link/tag
//! references already present in block content.
//!
//! Usage:
//!   cargo run -p grafium-core --release --example reindex_links -- <path/to/index.db>

use std::time::Instant;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: reindex_links <path/to/index.db>");
            std::process::exit(2);
        }
    };

    println!("Opening database: {path}");
    let db = grafium_core::Database::new(&path).expect("failed to open database");

    let start = Instant::now();
    let mut last_report = Instant::now();
    let (blocks, links) = db
        .reindex_links(|blocks_scanned, links_inserted| {
            if last_report.elapsed().as_secs_f64() >= 1.0 {
                println!(
                    "  scanned {blocks_scanned} blocks, inserted {links_inserted} links ({:.0}s)",
                    start.elapsed().as_secs_f64()
                );
                last_report = Instant::now();
            }
        })
        .expect("reindex_links failed");

    println!(
        "Done in {:.1}s: scanned {blocks} blocks, inserted {links} new links.",
        start.elapsed().as_secs_f64()
    );
}
