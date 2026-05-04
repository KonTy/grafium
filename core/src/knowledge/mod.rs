//! Knowledge module — multi-graph registry, vector store, schemas.
//!
//! This is the "knowledge OS" layer that sits above individual graphs.

pub mod engine;
pub mod registry;
pub mod schemas;
pub mod vector_store;

pub use engine::{HealthStatus, KnowledgeEngine};
pub use registry::{GraphRegistry, GraphType, RegisteredGraph};
pub use schemas::{FieldType, Schema, SchemaField, SchemaManager};
pub use vector_store::SqliteVectorStore;
