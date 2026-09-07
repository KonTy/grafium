//! Process-wide safety limits for embedded AI runtimes.
//!
//! llama.cpp and whisper.cpp allocate through native code. Rust cannot recover
//! after the OS OOM killer or a GPU driver reset, so expensive work must be
//! rejected before either runtime sees it.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

use sysinfo::System;

use crate::error::{CoreError, Result};

const GIB: u64 = 1024 * 1024 * 1024;
const MIN_OS_RESERVE_BYTES: u64 = 2 * GIB;
const MAX_CONTEXT_TOKENS: u32 = 16_384;
const DEFAULT_CONTEXT_TOKENS: u32 = 4_096;
const MAX_GENERATED_TOKENS: u32 = 4_096;
const MAX_PROMPT_BYTES: usize = 2 * 1024 * 1024;
const MIN_WORKER_LIMIT_BYTES: u64 = 2 * GIB;

static INFERENCE_SLOT: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
pub enum ModelWorkload {
    Llm { context_tokens: u32 },
    Whisper,
}

pub fn inference_slot() -> Result<MutexGuard<'static, ()>> {
    Ok(INFERENCE_SLOT
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()))
}

pub fn safe_context_size(requested: Option<u32>) -> Result<u32> {
    let context = requested.unwrap_or(DEFAULT_CONTEXT_TOKENS);
    if context == 0 || context > MAX_CONTEXT_TOKENS {
        return Err(CoreError::Other(format!(
            "Local AI context size must be between 1 and {MAX_CONTEXT_TOKENS} tokens; \
             requested {context}. Large contexts can exhaust RAM or VRAM."
        )));
    }
    Ok(context)
}

pub fn safe_generated_tokens(requested: Option<u32>) -> Result<u32> {
    let tokens = requested.unwrap_or(1_024);
    if tokens == 0 || tokens > MAX_GENERATED_TOKENS {
        return Err(CoreError::Other(format!(
            "Local AI output must be between 1 and {MAX_GENERATED_TOKENS} tokens; requested {tokens}."
        )));
    }
    Ok(tokens)
}

/// GPU offload is allowed by default.
///
/// This used to refuse any offload unless an environment variable opted in,
/// on the grounds that portable VRAM admission checks could not reliably
/// prevent a driver reset from taking the process down. That reasoning was
/// sound when the model ran inside the application: a reset was fatal and
/// unrecoverable, so refusing to risk it was the safer trade.
///
/// It no longer holds. The model runs in a supervised child process, so a
/// driver reset ends the worker and the next request starts a fresh one — the
/// exact failure the refusal existed to avoid is now contained. Keeping it
/// would mean every model silently running on CPU, roughly forty times slower,
/// to avoid something that can no longer happen.
///
/// `GRAFIUM_DISABLE_GPU_OFFLOAD=1` still forces CPU, for a machine whose
/// driver is genuinely unstable.
pub fn safe_gpu_layers(requested: Option<u32>) -> Result<u32> {
    let layers = requested.unwrap_or(0);
    if layers > 0 && std::env::var_os("GRAFIUM_DISABLE_GPU_OFFLOAD").is_some() {
        tracing::info!("GPU offload disabled by GRAFIUM_DISABLE_GPU_OFFLOAD; running on CPU");
        return Ok(0);
    }
    Ok(layers)
}

pub fn validate_prompt_size(prompt: &str) -> Result<()> {
    validate_prompt_bytes(prompt.len())
}

pub fn validate_prompt_bytes(bytes: usize) -> Result<()> {
    if bytes > MAX_PROMPT_BYTES {
        return Err(CoreError::Other(format!(
            "AI prompt is {:.1} MiB; the local safety limit is {:.1} MiB. \
             Shorten the input or index it and ask a narrower question.",
            bytes as f64 / (1024.0 * 1024.0),
            MAX_PROMPT_BYTES as f64 / (1024.0 * 1024.0)
        )));
    }
    Ok(())
}

pub fn validate_model_load(path: &Path, workload: ModelWorkload) -> Result<()> {
    let file_size = std::fs::metadata(path)
        .map_err(|e| CoreError::Other(format!("Cannot inspect model {}: {e}", path.display())))?
        .len();
    validate_model_size(&path.display().to_string(), file_size, workload)
}

pub fn validate_model_size(label: &str, file_size: u64, workload: ModelWorkload) -> Result<()> {
    let (total, available) = memory_snapshot();
    let os_reserve = MIN_OS_RESERVE_BYTES.max(total / 4);
    let working_set = estimated_model_working_set(file_size, workload);
    let required_available = working_set.saturating_add(os_reserve);

    if available < required_available {
        return Err(CoreError::Other(format!(
            "Refusing to load {} ({:.1} GiB): estimated AI working set is {:.1} GiB, \
             but only {:.1} GiB RAM is available and Grafium reserves {:.1} GiB for the OS. \
             Choose a smaller or more heavily quantized model.",
            label,
            file_size as f64 / GIB as f64,
            working_set as f64 / GIB as f64,
            available as f64 / GIB as f64,
            os_reserve as f64 / GIB as f64
        )));
    }

    Ok(())
}

pub fn worker_memory_limit(
    model_path: &Path,
    workload: ModelWorkload,
    input_bytes: u64,
) -> Result<u64> {
    let estimated = estimate_worker_working_set(model_path, workload, input_bytes)?;
    let (total, available) = memory_snapshot();
    let os_reserve = MIN_OS_RESERVE_BYTES.max(total / 4);
    let maximum_safe = available.saturating_sub(os_reserve);
    let limit = estimated.max(MIN_WORKER_LIMIT_BYTES).min(maximum_safe);
    if limit < estimated {
        return Err(CoreError::Other(format!(
            "Refusing to start the native AI worker: it needs about {:.1} GiB, but only {:.1} GiB \
             can be safely assigned while preserving OS headroom.",
            estimated as f64 / GIB as f64,
            maximum_safe as f64 / GIB as f64
        )));
    }
    Ok(limit)
}

/// Raw (uncapped, non-admission) working-set estimate for a worker.
///
/// Callers use this to decide whether an already-resident worker's stored
/// memory cap is large enough for a new request, without failing due to the
/// resident model's own RSS lowering `available`.
pub fn estimate_worker_working_set(
    model_path: &Path,
    workload: ModelWorkload,
    input_bytes: u64,
) -> Result<u64> {
    let model_size = std::fs::metadata(model_path)
        .map_err(|e| {
            CoreError::Other(format!(
                "Cannot inspect model {}: {e}",
                model_path.display()
            ))
        })?
        .len();
    Ok(estimated_model_working_set(model_size, workload)
        .saturating_add(input_bytes.saturating_mul(4))
        .saturating_add(512 * 1024 * 1024))
}

pub fn validate_audio_buffer(path: &Path) -> Result<()> {
    let file_size = std::fs::metadata(path)
        .map_err(|e| CoreError::Other(format!("Cannot inspect audio {}: {e}", path.display())))?
        .len();
    let (total, available) = memory_snapshot();
    let os_reserve = MIN_OS_RESERVE_BYTES.max(total / 4);
    // PCM i16 expands to f32, and whisper.cpp needs additional scratch space.
    let required = file_size.saturating_mul(4).saturating_add(os_reserve);
    if available < required {
        return Err(CoreError::Other(format!(
            "Refusing to transcribe this audio: it needs about {:.1} GiB of free RAM \
             while preserving OS headroom, but only {:.1} GiB is available.",
            required.saturating_sub(os_reserve) as f64 / GIB as f64,
            available as f64 / GIB as f64
        )));
    }
    Ok(())
}

pub fn validate_inference_headroom(label: &str, additional_bytes: u64) -> Result<()> {
    let (total, available) = memory_snapshot();
    let os_reserve = MIN_OS_RESERVE_BYTES.max(total / 4);
    let required = additional_bytes.saturating_add(os_reserve);
    if available < required {
        return Err(CoreError::Other(format!(
            "Refusing to start {label}: it may allocate another {:.1} GiB, but only {:.1} GiB \
             is available and {:.1} GiB is reserved for the OS.",
            additional_bytes as f64 / GIB as f64,
            available as f64 / GIB as f64,
            os_reserve as f64 / GIB as f64
        )));
    }
    Ok(())
}

pub fn estimate_llm_context_bytes(model_size: u64, context_tokens: u32) -> u64 {
    let kv_per_token = (model_size / 16_384).max(256 * 1024);
    kv_per_token
        .saturating_mul(context_tokens as u64)
        .saturating_add(512 * 1024 * 1024)
}

/// True when free RAM is at or below Grafium's OS reserve, meaning it's unsafe
/// to keep an idle native model resident.
pub fn is_memory_pressure_high() -> bool {
    let (total, available) = memory_snapshot();
    let os_reserve = MIN_OS_RESERVE_BYTES.max(total / 4);
    available <= os_reserve
}

pub fn inference_thread_count() -> i32 {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let safe_max = (cores / 2).clamp(1, 8);
    std::env::var("GRAFIUM_LLM_THREADS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|threads| *threads > 0)
        .map(|threads| threads.min(safe_max))
        .unwrap_or(safe_max) as i32
}

fn memory_snapshot() -> (u64, u64) {
    let mut system = System::new();
    system.refresh_memory();
    (system.total_memory(), system.available_memory())
}

fn estimated_model_working_set(file_size: u64, workload: ModelWorkload) -> u64 {
    match workload {
        ModelWorkload::Llm { context_tokens } => {
            // Quantized weights are not the whole allocation. Add 35% for
            // runtime tensors and a model-size-scaled KV-cache estimate.
            let runtime = file_size.saturating_mul(135) / 100;
            let kv_per_token = (file_size / 16_384).max(256 * 1024);
            runtime.saturating_add(kv_per_token.saturating_mul(context_tokens as u64))
        }
        ModelWorkload::Whisper => file_size.saturating_mul(2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_rejects_oversized_requests() {
        assert!(safe_context_size(Some(MAX_CONTEXT_TOKENS + 1)).is_err());
    }

    #[test]
    fn generated_tokens_reject_oversized_requests() {
        assert!(safe_generated_tokens(Some(MAX_GENERATED_TOKENS + 1)).is_err());
    }

    #[test]
    fn thread_count_preserves_at_least_half_the_machine() {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        assert!((inference_thread_count() as usize) <= (cores / 2).clamp(1, 8));
    }

    #[test]
    fn context_estimate_grows_with_model_and_context() {
        let small = estimate_llm_context_bytes(GIB, 4_096);
        let larger_model = estimate_llm_context_bytes(8 * GIB, 4_096);
        let larger_context = estimate_llm_context_bytes(GIB, 8_192);

        assert!(larger_model > small);
        assert!(larger_context > small);
    }
}
