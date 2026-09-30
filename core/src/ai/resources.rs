//! Grafium's environment and user-visible warning adapters for model admission.

#[cfg(any(feature = "llm-local", feature = "media"))]
use std::path::Path;

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

#[cfg(any(feature = "llm-local", feature = "media"))]
pub fn admitted_gpu_layers(path: &Path, workload: ModelWorkload, requested: u32) -> Result<u32> {
    validate_model_load(path, workload)?;
    if requested == 0 {
        return Ok(0);
    }
    if std::env::var("GRAFIUM_GPU_LEASE_ACTIVE").as_deref() != Ok("1") {
        crate::ai::worker::emit_runtime_warning(
            "GPU execution requires a supervised recovery lease; using CPU.",
        );
        return Ok(0);
    }

    if safe_gpu_layers(Some(requested))? == 0 {
        crate::ai::worker::emit_runtime_warning(
            "GPU offload is disabled by GRAFIUM_DISABLE_GPU_OFFLOAD; using CPU.",
        );
        return Ok(0);
    }
    let decision = gpu_admission(
        std::fs::metadata(path)?.len(),
        workload,
        gpu_memory_snapshot(),
    );
    match decision {
        GpuAdmission::Gpu => Ok(requested),
        GpuAdmission::Cpu { reason } => {
            crate::ai::worker::emit_runtime_warning(&reason);
            Ok(0)
        }
    }
}

pub fn validate_gpu_context_headroom(additional_bytes: u64) -> Result<()> {
    Ok(model_runtime::resources::validate_gpu_context_headroom(
        additional_bytes,
        gpu_memory_snapshot(),
    )?)
}

pub fn runtime_warnings() -> Vec<String> {
    #[cfg(any(feature = "llm-local", feature = "media"))]
    {
        crate::ai::worker::runtime_warnings()
    }

    #[cfg(not(any(feature = "llm-local", feature = "media")))]
    {
        Vec::new()
    }
}

pub fn runtime_recovery() -> Vec<model_runtime::recovery::BlockedModel> {
    #[cfg(any(feature = "llm-local", feature = "media"))]
    {
        crate::ai::worker::recovery_status()
    }
    #[cfg(not(any(feature = "llm-local", feature = "media")))]
    {
        Vec::new()
    }
}

#[cfg(any(feature = "llm-local", feature = "media", test))]
pub(crate) fn accept_deferred_eviction(result: Result<()>) -> Result<()> {
    match result {
        Err(crate::CoreError::ModelRuntime(model_runtime::error::RuntimeError::WorkerBusy)) => {
            tracing::info!("Native worker eviction deferred: another native job is still running");
            Ok(())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model_runtime::error::RuntimeError;

    #[test]
    fn provider_changes_preserve_active_jobs_but_surface_cleanup_failures() {
        assert!(accept_deferred_eviction(Ok(())).is_ok());
        assert!(accept_deferred_eviction(Err(RuntimeError::WorkerBusy.into())).is_ok());
        assert!(accept_deferred_eviction(Err(RuntimeError::Cancelled.into())).is_err());
        assert!(
            accept_deferred_eviction(Err(RuntimeError::Other("reap failed".into()).into()))
                .is_err()
        );
    }
}
