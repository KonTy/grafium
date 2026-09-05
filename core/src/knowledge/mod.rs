//! Knowledge module — multi-graph registry, vector store, schemas.
//!
//! This is the "knowledge OS" layer that sits above individual graphs.

pub mod engine;
pub mod registry;
pub mod research_intent;
pub mod retrieval;
pub mod schemas;
pub mod vector_store;

pub use engine::{HealthStatus, KnowledgeEngine};
pub use registry::{GraphRegistry, GraphType, RegisteredGraph};
pub use research_intent::{detect_research_intent, ResearchIntent};
pub use schemas::{FieldType, Schema, SchemaField, SchemaManager};
pub use vector_store::SqliteVectorStore;
