//! Environment/host policy at the native boundary, without application paths.

use crate::error::Result;
pub use crate::resources::*;

pub fn safe_gpu_layers(requested: Option<u32>) -> Result<u32> {
    if super::worker::cpu_disabled() || std::env::var_os("MODEL_RUNTIME_NATIVE_FORCE_CPU").is_some()
    {
        return Ok(0);
    }
    crate::resources::safe_gpu_layers(requested)
}

pub fn inference_thread_count() -> i32 {
    let requested = super::worker::configured_threads().or_else(|| {
        std::env::var("MODEL_RUNTIME_LLM_THREADS")
            .ok()
            .and_then(|value| {
                let parsed = value.trim().parse().ok();
                if parsed.is_none() {
                    tracing::warn!(
                        "Invalid MODEL_RUNTIME_LLM_THREADS; using bounded automatic threads"
                    );
                }
                parsed
            })
    });
    crate::resources::bounded_thread_count(requested)
}

#[cfg(all(feature = "media", not(feature = "llm-local")))]
pub fn admitted_gpu_layers(
    path: &std::path::Path,
    workload: ModelWorkload,
    requested: u32,
) -> Result<u32> {
    validate_model_load(path, workload)?;
    if requested == 0 {
        return Ok(0);
    }
    let reason = if std::env::var("MODEL_RUNTIME_GPU_LEASE_ACTIVE").as_deref() != Ok("1") {
        Some("GPU execution requires a supervised recovery lease; using CPU.".to_owned())
    } else if safe_gpu_layers(Some(requested))? == 0 {
        Some("GPU offload is disabled by native host policy; using CPU.".to_owned())
    } else {
        match gpu_admission(
            std::fs::metadata(path)?.len(),
            workload,
            gpu_memory_snapshot(),
        ) {
            GpuAdmission::Gpu => None,
            GpuAdmission::Cpu { reason } => Some(reason),
        }
    };
    if let Some(reason) = reason {
        super::worker::emit_runtime_warning(&reason);
        Ok(0)
    } else {
        Ok(requested)
    }
}

#[cfg(all(feature = "media", not(feature = "llm-local")))]
pub fn validate_gpu_context_headroom(additional_bytes: u64) -> Result<()> {
    crate::resources::validate_gpu_context_headroom(additional_bytes, gpu_memory_snapshot())
}
