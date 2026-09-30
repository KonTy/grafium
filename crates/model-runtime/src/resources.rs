//! Conservative admission checks for embedded AI runtimes.
//!
//! llama.cpp and whisper.cpp allocate through native code. Rust cannot recover
//! after the OS OOM killer or a GPU driver reset. These estimates reduce risk;
//! neither estimates nor subprocess isolation guarantee kernel/driver safety.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

use sysinfo::System;

use crate::error::{Result, RuntimeError};

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
        return Err(RuntimeError::Other(format!(
            "Local AI context size must be between 1 and {MAX_CONTEXT_TOKENS} tokens; \
             requested {context}. Large contexts can exhaust RAM or VRAM."
        )));
    }
    Ok(context)
}

pub fn safe_generated_tokens(requested: Option<u32>) -> Result<u32> {
    let tokens = requested.unwrap_or(1_024);
    if tokens == 0 || tokens > MAX_GENERATED_TOKENS {
        return Err(RuntimeError::Other(format!(
            "Local AI output must be between 1 and {MAX_GENERATED_TOKENS} tokens; requested {tokens}."
        )));
    }
    Ok(tokens)
}

/// Validate a request, not permission to allocate GPU memory. Hosts must also
/// apply [`gpu_admission`] immediately before loading a native GPU model.
pub fn safe_gpu_layers(requested: Option<u32>) -> Result<u32> {
    let layers = requested.unwrap_or(0);
    if layers > i32::MAX as u32 {
        return Err(RuntimeError::Other(
            "GPU layer count exceeds the native int32 limit".into(),
        ));
    }
    Ok(layers)
}

pub fn validate_prompt_size(prompt: &str) -> Result<()> {
    validate_prompt_bytes(prompt.len())
}

pub fn validate_prompt_bytes(bytes: usize) -> Result<()> {
    if bytes > MAX_PROMPT_BYTES {
        return Err(RuntimeError::Other(format!(
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
        .map_err(|e| RuntimeError::Other(format!("Cannot inspect model {}: {e}", path.display())))?
        .len();
    let (total, available) = memory_snapshot();
    let reserve = MIN_OS_RESERVE_BYTES.max(total / 4);
    let estimated = model_working_set(path, file_size, workload);
    if estimated.saturating_add(reserve) > available {
        return Err(RuntimeError::Other(format!(
            "Refusing to load {}: model and context need about {:.1} GiB RAM, \
             with {:.1} GiB available after OS headroom.",
            path.display(),
            estimated as f64 / GIB as f64,
            available.saturating_sub(reserve) as f64 / GIB as f64,
        )));
    }
    Ok(())
}

pub fn validate_model_size(label: &str, file_size: u64, workload: ModelWorkload) -> Result<()> {
    let (total, available) = memory_snapshot();
    let os_reserve = MIN_OS_RESERVE_BYTES.max(total / 4);
    let working_set = estimated_model_working_set(file_size, workload);
    let required_available = working_set.saturating_add(os_reserve);

    if available < required_available {
        return Err(RuntimeError::Other(format!(
            "Refusing to load {} ({:.1} GiB): estimated AI working set is {:.1} GiB, \
             but only {:.1} GiB RAM is available and the runtime reserves {:.1} GiB for the OS. \
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
        return Err(RuntimeError::Other(format!(
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
            RuntimeError::Other(format!(
                "Cannot inspect model {}: {e}",
                model_path.display()
            ))
        })?
        .len();
    Ok(model_working_set(model_path, model_size, workload)
        .saturating_add(input_bytes.saturating_mul(4))
        .saturating_add(512 * 1024 * 1024))
}

pub fn validate_audio_buffer(path: &Path) -> Result<()> {
    let file_size = std::fs::metadata(path)
        .map_err(|e| RuntimeError::Other(format!("Cannot inspect audio {}: {e}", path.display())))?
        .len();
    let (total, available) = memory_snapshot();
    let os_reserve = MIN_OS_RESERVE_BYTES.max(total / 4);
    // PCM i16 expands to f32, and whisper.cpp needs additional scratch space.
    let required = file_size.saturating_mul(4).saturating_add(os_reserve);
    if available < required {
        return Err(RuntimeError::Other(format!(
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
        return Err(RuntimeError::Other(format!(
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

pub fn estimate_context_for_model(path: &Path, context_tokens: u32) -> Result<u64> {
    match crate::gguf::read_transformer_shape(path) {
        Ok(Some(shape)) if shape.context_bytes(context_tokens).is_some() => Ok(shape
            .context_bytes(context_tokens)
            .expect("checked context estimate")),
        Ok(_) => {
            tracing::info!(
                "Model architecture has no scalar KV estimate; using conservative admission"
            );
            Ok(estimate_llm_context_bytes(
                std::fs::metadata(path)?.len(),
                context_tokens,
            ))
        }
        Err(error) => {
            tracing::warn!(%error, "Cannot inspect architecture; using conservative admission before native validation");
            Ok(estimate_llm_context_bytes(
                std::fs::metadata(path)?.len(),
                context_tokens,
            ))
        }
    }
}

fn model_working_set(path: &Path, file_size: u64, workload: ModelWorkload) -> u64 {
    match workload {
        ModelWorkload::Llm { context_tokens } => {
            match estimate_context_for_model(path, context_tokens) {
                Ok(context) => file_size
                    .saturating_mul(135)
                    .saturating_div(100)
                    .saturating_add(context),
                Err(error) => {
                    tracing::warn!(%error, "Using conservative working-set estimate");
                    estimated_model_working_set(file_size, workload)
                }
            }
        }
        ModelWorkload::Whisper => estimated_model_working_set(file_size, workload),
    }
}

/// Hard containment gets some estimate headroom but never the OS reserve.
pub fn containment_limit(
    model_path: &Path,
    workload: ModelWorkload,
    input_bytes: u64,
) -> Result<u64> {
    let estimate = estimate_worker_working_set(model_path, workload, input_bytes)?;
    let (total, available) = memory_snapshot();
    let safe = available.saturating_sub(MIN_OS_RESERVE_BYTES.max(total / 4));
    if estimate > safe {
        return Err(RuntimeError::Other(
            "Insufficient RAM for the native worker and OS reserve".into(),
        ));
    }
    Ok(estimate
        .saturating_add(estimate / 4)
        .max(MIN_WORKER_LIMIT_BYTES)
        .min(safe))
}

pub fn critical_memory_pressure() -> Option<String> {
    let (total, available) = memory_snapshot();
    let minimum = GIB.max(total / 16);
    (available < minimum).then(|| {
        format!(
            "System RAM fell below {:.1} GiB of emergency headroom; native inference stopped.",
            minimum as f64 / GIB as f64,
        )
    })
}

/// True when free RAM is at or below the OS reserve, meaning it's unsafe
/// to keep an idle native model resident.
pub fn is_memory_pressure_high() -> bool {
    let (total, available) = memory_snapshot();
    let os_reserve = MIN_OS_RESERVE_BYTES.max(total / 4);
    available <= os_reserve
}

pub fn inference_thread_count() -> i32 {
    bounded_thread_count(None)
}

pub fn bounded_thread_count(requested: Option<usize>) -> i32 {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let safe_max = (cores / 2).clamp(1, 8);
    requested
        .filter(|threads| *threads > 0)
        .map(|threads| threads.min(safe_max))
        .unwrap_or(safe_max) as i32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuMemory {
    pub total: u64,
    pub available: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuAdmission {
    Gpu,
    Cpu { reason: String },
}

/// Includes context/scratch estimates as well as weights and desktop headroom.
/// An unknown device or budget is not permission to attempt GPU allocation.
pub fn gpu_admission(
    model_bytes: u64,
    workload: ModelWorkload,
    memory: Option<GpuMemory>,
) -> GpuAdmission {
    let Some(memory) = memory.filter(|m| m.total > 0 && m.available <= m.total) else {
        return GpuAdmission::Cpu {
            reason: "GPU memory could not be measured for an unambiguous device; using CPU.".into(),
        };
    };
    let reserve = (512 * 1024 * 1024).max(memory.total / 8);
    let required = estimated_model_working_set(model_bytes, workload).saturating_add(reserve);
    if required > memory.available {
        return GpuAdmission::Cpu {
            reason: format!(
                "GPU admission needs about {:.1} GiB including context and desktop headroom, \
                 but only {:.1} GiB is free; using CPU.",
                required as f64 / GIB as f64,
                memory.available as f64 / GIB as f64,
            ),
        };
    }
    GpuAdmission::Gpu
}

/// Recheck before allocating a context on a resident GPU model. If the budget
/// changed, abort; the host can evict and choose CPU on a subsequent request.
pub fn validate_gpu_context_headroom(
    additional_bytes: u64,
    memory: Option<GpuMemory>,
) -> Result<()> {
    let Some(memory) = memory.filter(|m| m.total > 0 && m.available <= m.total) else {
        return Err(RuntimeError::Other(
            "GPU headroom is no longer measurable; inference stopped. Retry on CPU or a network server.".into(),
        ));
    };
    let reserve = (512 * 1024 * 1024).max(memory.total / 8);
    if additional_bytes.saturating_add(reserve) > memory.available {
        return Err(RuntimeError::Other(
            "GPU headroom changed after model loading; inference stopped before allocating a context. Retry to reassess CPU fallback.".into(),
        ));
    }
    Ok(())
}

/// Read Linux DRM counters without initializing a GPU driver in this process.
/// Only a single dedicated GPU with complete counters is accepted. Unknown,
/// multi-GPU and unsupported-platform configurations conservatively use CPU.
pub fn gpu_memory_snapshot() -> Option<GpuMemory> {
    #[cfg(target_os = "linux")]
    {
        gpu_memory_from_drm(Path::new("/sys/class/drm"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(any(target_os = "linux", test))]
fn gpu_memory_from_drm(root: &Path) -> Option<GpuMemory> {
    let mut devices = Vec::new();
    for entry in std::fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let name = name.to_str()?;
        let Some(number) = name.strip_prefix("card") else {
            continue;
        };
        if number.is_empty() || !number.bytes().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let device = entry.path().join("device");
        if device.exists() {
            devices.push(device);
        }
    }
    if devices.len() != 1 {
        return None;
    }
    let device = &devices[0];
    let read = |name: &str| {
        std::fs::read_to_string(device.join(name))
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()
    };
    let total = read("mem_info_vram_total")?;
    let used = read("mem_info_vram_used")?;
    // Tiny UMA apertures are not a dedicated VRAM budget.
    if total < 512 * 1024 * 1024 {
        return None;
    }
    Some(GpuMemory {
        total,
        available: total.checked_sub(used)?,
    })
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

    #[test]
    fn unknown_or_invalid_gpu_budget_never_admits_gpu() {
        let workload = ModelWorkload::Llm {
            context_tokens: 4096,
        };
        for memory in [
            None,
            Some(GpuMemory {
                total: GIB,
                available: 2 * GIB,
            }),
        ] {
            assert!(matches!(
                gpu_admission(GIB, workload, memory),
                GpuAdmission::Cpu { .. }
            ));
            assert!(validate_gpu_context_headroom(GIB, memory).is_err());
        }
    }

    #[test]
    fn gpu_admission_includes_context_and_desktop_reserve() {
        let memory = Some(GpuMemory {
            total: 8 * GIB,
            available: 4 * GIB,
        });
        assert_eq!(
            gpu_admission(
                GIB,
                ModelWorkload::Llm {
                    context_tokens: 4096
                },
                memory
            ),
            GpuAdmission::Gpu,
        );
        assert!(matches!(
            gpu_admission(
                GIB,
                ModelWorkload::Llm {
                    context_tokens: 16_384
                },
                memory
            ),
            GpuAdmission::Cpu { .. },
        ));
        assert!(matches!(
            gpu_admission(3 * GIB, ModelWorkload::Whisper, memory),
            GpuAdmission::Cpu { .. },
        ));
        assert!(matches!(
            gpu_admission(u64::MAX, ModelWorkload::Whisper, memory),
            GpuAdmission::Cpu { .. },
        ));
    }

    #[test]
    fn resident_context_rechecks_remaining_budget() {
        assert!(validate_gpu_context_headroom(
            GIB,
            Some(GpuMemory {
                total: 8 * GIB,
                available: 2 * GIB
            }),
        )
        .is_ok());
        assert!(validate_gpu_context_headroom(
            GIB,
            Some(GpuMemory {
                total: 8 * GIB,
                available: 2 * GIB - 1
            }),
        )
        .is_err());
    }

    #[test]
    fn drm_probe_rejects_missing_ambiguous_and_inconsistent_counters() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(gpu_memory_from_drm(dir.path()), None);
        let device = dir.path().join("card0/device");
        std::fs::create_dir_all(&device).unwrap();
        std::fs::write(device.join("mem_info_vram_total"), (8 * GIB).to_string()).unwrap();
        assert_eq!(gpu_memory_from_drm(dir.path()), None);
        std::fs::write(device.join("mem_info_vram_used"), (2 * GIB).to_string()).unwrap();
        assert_eq!(
            gpu_memory_from_drm(dir.path()),
            Some(GpuMemory {
                total: 8 * GIB,
                available: 6 * GIB
            })
        );
        std::fs::write(device.join("mem_info_vram_used"), (9 * GIB).to_string()).unwrap();
        assert_eq!(gpu_memory_from_drm(dir.path()), None);
        std::fs::write(device.join("mem_info_vram_used"), "0").unwrap();
        std::fs::create_dir_all(dir.path().join("card1/device")).unwrap();
        assert_eq!(gpu_memory_from_drm(dir.path()), None);
    }

    #[test]
    fn native_gpu_layer_count_cannot_overflow() {
        assert_eq!(safe_gpu_layers(None).unwrap(), 0);
        assert_eq!(safe_gpu_layers(Some(1_000_000)).unwrap(), 1_000_000);
        assert!(safe_gpu_layers(Some(u32::MAX)).is_err());
    }
}
