//! Optional native inference. Hosts supply bootstrap and private recovery paths;
//! this module owns kernels and worker lifecycle for the shared model manager.

#[cfg(feature = "llm-local")]
pub mod embedder;
#[cfg(feature = "llm-local")]
mod llama_shared;
#[cfg(feature = "llm-local")]
pub mod llm;
mod model_file;
#[cfg(feature = "llm-local")]
mod native_gpu;
mod policy;
#[cfg(feature = "media")]
pub mod transcribe;
pub mod worker;

pub use worker::{
    configure_with_options, NativeHostConfig, NativeModelKind, NativeStatus, ResidentModel,
    ResidentStatus,
};

#[cfg(test)]
mod tests;
