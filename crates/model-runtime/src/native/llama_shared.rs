//! Process-wide llama.cpp backend, shared between every in-process llama.cpp
//! consumer (`llm::LocalLlm` for chat, `embedder::LocalEmbedder`
//! for embeddings). llama.cpp's backend is meant to be initialized exactly
//! once per process — sharing one `OnceLock` here (rather than each module
//! keeping its own) is what makes it safe for both to be in use at the same
//! time, which is exactly the "Embedded chat + embedded search" combination
//! this module exists to unlock.

use std::sync::{Arc, OnceLock};

use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::{send_logs_to_tracing, LogOptions};

/// Offload every transformer layer to the GPU — the llama.cpp convention
/// for "as many as exist" (the upstream `simple` example uses the same
/// sentinel). A no-op unless built with a GPU feature (`llm-local-vulkan`).
pub(crate) const OFFLOAD_ALL_LAYERS: u32 = 1_000_000;

static BACKEND: OnceLock<std::result::Result<Arc<LlamaBackend>, String>> = OnceLock::new();
static INSTALL_LOGGING: std::sync::Once = std::sync::Once::new();

/// Returns the shared process-wide llama.cpp backend handle, initializing
/// it on first use. llama.cpp/GGML's own logs (buffer allocations, memory
/// type selection, Vulkan driver errors, etc.) are routed through `tracing`
/// rather than suppressed. The supervised child installs its subscriber and
/// diagnostic tap before loading any model. Direct offline callers supply
/// their own subscriber.
pub(crate) fn shared_backend() -> crate::error::Result<Arc<LlamaBackend>> {
    INSTALL_LOGGING.call_once(|| {
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(true));
    });
    BACKEND
        .get_or_init(|| {
            LlamaBackend::init()
                .map(Arc::new)
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map(Arc::clone)
        .map_err(|e| {
            crate::error::RuntimeError::Other(format!("Failed to initialize native backend: {e}"))
        })
}
