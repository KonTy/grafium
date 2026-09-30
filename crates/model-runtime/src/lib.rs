//! Shared settings, model management, transports, and optional native inference.
//! Applications own data access, persistence, credentials, and action permissions.

pub mod error;
pub mod gguf;
pub mod gpu;
pub mod gpu_info;
pub mod log_tap;
pub mod manager;
pub mod models;
#[cfg(any(feature = "llm-local", feature = "media"))]
pub mod native;
pub mod protocol;
pub mod providers;
pub mod recovery;
pub mod resources;
pub mod settings;
pub mod supervisor;
pub mod transcription;
pub mod types;

pub use error::{Result, RuntimeError};
