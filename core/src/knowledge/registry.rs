//! Multi-graph registry — track and manage multiple knowledge graphs.
//!
//! Each graph is an independent set of markdown files with its own SQLite index.
//! The registry provides unified cross-graph search via the shared vector store.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};

/// Type of graph in the registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GraphType {
    /// User-created primary graph (notes, journal, etc.).
    Primary,
    /// Reference material (books, papers, documentation).
    Reference,
    /// Ingested from external sources (web scrapes, OCR'd PDFs).
    Ingested,
    /// Archive (read-only, not actively indexed).
    Archive,
}

/// A registered graph in the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredGraph {
    /// Unique identifier for this graph.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Filesystem path to the graph root.
    pub path: PathBuf,
    /// What kind of graph this is.
    pub graph_type: GraphType,
    /// When the graph was last indexed.
    pub last_indexed: Option<i64>,
    /// Number of pages in this graph (cached).
    pub page_count: Option<usize>,
    /// Number of vectors in the store for this graph (cached).
    pub vector_count: Option<usize>,
    /// Whether to include in cross-graph searches.
    pub cross_searchable: bool,
    /// Optional description.
    pub description: Option<String>,
}

/// The multi-graph registry.
/// Persisted as a JSON file in the app's data directory.
pub struct GraphRegistry {
    graphs: HashMap<String, RegisteredGraph>,
    config_path: PathBuf,
}

impl GraphRegistry {
    /// Load registry from disk, or create empty.
    pub fn load(config_path: &Path) -> Result<Self> {
        let graphs = if config_path.exists() {
            let content = std::fs::read_to_string(config_path)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };

        Ok(Self {
            graphs,
            config_path: config_path.to_path_buf(),
        })
    }

    /// Persist registry to disk.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&self.graphs)?;
        std::fs::write(&self.config_path, content)?;
        Ok(())
    }

    /// Register a new graph.
    pub fn register(&mut self, graph: RegisteredGraph) -> Result<()> {
        if !graph.path.exists() {
            return Err(CoreError::NotFound(format!(
                "Graph path does not exist: {:?}",
                graph.path
            )));
        }
        self.graphs.insert(graph.id.clone(), graph);
        self.save()
    }

    /// Unregister a graph (does not delete files).
    pub fn unregister(&mut self, graph_id: &str) -> Result<()> {
        self.graphs.remove(graph_id);
        self.save()
    }

    /// Get a graph by ID.
    pub fn get(&self, graph_id: &str) -> Option<&RegisteredGraph> {
        self.graphs.get(graph_id)
    }

    /// Get mutable reference to a graph.
    pub fn get_mut(&mut self, graph_id: &str) -> Option<&mut RegisteredGraph> {
        self.graphs.get_mut(graph_id)
    }

    /// List all registered graphs.
    pub fn list(&self) -> Vec<&RegisteredGraph> {
        self.graphs.values().collect()
    }

    /// List graphs that participate in cross-graph search.
    pub fn cross_searchable_graphs(&self) -> Vec<&RegisteredGraph> {
        self.graphs
            .values()
            .filter(|g| g.cross_searchable)
            .collect()
    }

    /// Update cached stats for a graph.
    pub fn update_stats(
        &mut self,
        graph_id: &str,
        page_count: usize,
        vector_count: usize,
    ) -> Result<()> {
        if let Some(graph) = self.graphs.get_mut(graph_id) {
            graph.page_count = Some(page_count);
            graph.vector_count = Some(vector_count);
            graph.last_indexed = Some(chrono::Utc::now().timestamp_millis());
            self.save()?;
        }
        Ok(())
    }

    /// Find which graph a file path belongs to.
    pub fn find_graph_for_path(&self, file_path: &Path) -> Option<&RegisteredGraph> {
        self.graphs.values().find(|g| file_path.starts_with(&g.path))
    }

    /// Generate a unique graph ID.
    pub fn generate_id() -> String {
        uuid::Uuid::new_v4().to_string()[..8].to_string()
    }
}
