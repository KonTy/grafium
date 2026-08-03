//! Process-wide llama.cpp backend, shared between every in-process llama.cpp
//! consumer (`local_llm::LocalLlm` for chat, `local_embedder::LocalEmbedder`
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

static BACKEND: OnceLock<Arc<LlamaBackend>> = OnceLock::new();
static INSTALL_LOGGING: std::sync::Once = std::sync::Once::new();

/// Returns the shared process-wide llama.cpp backend handle, initializing
/// it (and silencing llama.cpp/GGML's verbose stderr logging, which would
/// otherwise corrupt a raw-mode terminal UI) on first use.
pub(crate) fn shared_backend() -> Arc<LlamaBackend> {
    INSTALL_LOGGING.call_once(|| {
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
    });
    BACKEND
        .get_or_init(|| Arc::new(LlamaBackend::init().expect("failed to initialize the llama.cpp backend")))
        .clone()
}
