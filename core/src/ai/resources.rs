//! Grafium's legacy environment/configuration adapters over shared admission.

use crate::error::Result;

pub use model_runtime::resources::{
    estimate_llm_context_bytes, estimate_worker_working_set, gpu_admission, gpu_memory_snapshot,
    inference_slot, is_memory_pressure_high, safe_context_size, safe_generated_tokens,
    validate_audio_buffer, validate_inference_headroom, validate_model_load, validate_model_size,
    validate_prompt_bytes, validate_prompt_size, worker_memory_limit, GpuAdmission, ModelWorkload,
};

pub fn safe_gpu_layers(requested: Option<u32>) -> Result<u32> {
    if std::env::var_os("GRAFIUM_DISABLE_GPU_OFFLOAD").is_some()
        || std::env::var_os("GRAFIUM_NATIVE_FORCE_CPU").is_some()
    {
        return Ok(0);
    }
    Ok(model_runtime::resources::safe_gpu_layers(requested)?)
}

pub fn inference_thread_count() -> i32 {
    let requested = std::env::var("GRAFIUM_LLM_THREADS")
        .ok()
        .and_then(|value| value.trim().parse().ok());
    model_runtime::resources::bounded_thread_count(requested)
}

pub fn runtime_warnings() -> Vec<String> {
    #[cfg(any(feature = "llm-local", feature = "media"))]
    {
        model_runtime::native::worker::runtime_warnings()
    }
    #[cfg(not(any(feature = "llm-local", feature = "media")))]
    {
        Vec::new()
    }
}

pub fn runtime_recovery() -> Vec<model_runtime::recovery::BlockedModel> {
    #[cfg(any(feature = "llm-local", feature = "media"))]
    {
        model_runtime::native::worker::recovery_status()
    }
    #[cfg(not(any(feature = "llm-local", feature = "media")))]
    {
        Vec::new()
    }
}
