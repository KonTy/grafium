//! Shared model infrastructure. Applications own data access, persistence,
//! credentials, action permissions, and the native inference implementations.

pub mod error;
pub mod gguf;
pub mod gpu;
pub mod providers;
pub mod protocol;
pub mod resources;
pub mod recovery;
pub mod supervisor;
pub mod types;
