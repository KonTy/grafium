//! Quick manual verification of the Anki importer.
//!
//! Usage:
//!   cargo run -p grafium-core --example import_anki -- <path.apkg> <graph_dir>
//!
//! Creates/opens a graph at <graph_dir> and imports the .apkg into it.

use grafium_core::graph::Graph;
use grafium_core::import::anki;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let apkg = args.next().expect("usage: import_anki <path.apkg> <graph_dir>");
    let graph_dir = args.next().expect("usage: import_anki <path.apkg> <graph_dir>");

    let graph = Graph::open(&PathBuf::from(&graph_dir)).expect("open graph");
    let start = std::time::Instant::now();
    let summary = anki::import_apkg(&graph, &PathBuf::from(&apkg)).expect("import failed");
    let elapsed = start.elapsed();

    println!("Imported in {:.2}s", elapsed.as_secs_f64());
    println!("{:#?}", summary);
}
