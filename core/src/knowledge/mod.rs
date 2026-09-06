//! Knowledge module — multi-graph registry, vector store, schemas.
//!
//! This is the "knowledge OS" layer that sits above individual graphs.

pub mod collections;
pub mod conversation;
pub mod engine;
pub mod registry;
pub mod research_intent;
pub mod retrieval;
pub mod schemas;
pub mod tree;
pub mod vector_store;

pub use collections::{clear_collection, collection_of, mark_collection, CollectionInfo};
pub use engine::{HealthStatus, KnowledgeEngine};
pub use registry::{GraphRegistry, GraphType, RegisteredGraph};
pub use research_intent::{detect_research_intent, ResearchIntent};
pub use schemas::{FieldType, Schema, SchemaField, SchemaManager};
pub use tree::{build_namespace_tree, build_tag_tree, TreeKind, TreeNode};
pub use vector_store::SqliteVectorStore;
