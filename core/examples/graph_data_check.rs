//! Reports what the graph view would render for a real graph.
//!
//! The view showing "one lonely node" was invisible from inside the app: the
//! query succeeded, returned a valid single-node result, and rendered it
//! faithfully. Printing node/edge counts against a real database is the
//! cheapest way to tell "the graph is genuinely sparse" from "the traversal
//! gave up early".
//!
//! Usage:
//!   cargo run -p grafium-core --release --example graph_data_check -- <graph-dir>
use std::path::PathBuf;

use grafium_core::Graph;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: <graph-dir>"));
    let graph = Graph::open(&dir).expect("cannot open graph");

    for limit in [50, 200] {
        let (nodes, edges) = graph.db.graph_data(None, limit).expect("graph_data failed");
        println!(
            "limit {limit:>4}: {} nodes, {} edges",
            nodes.len(),
            edges.len()
        );
    }
}
