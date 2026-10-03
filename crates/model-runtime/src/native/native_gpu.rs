//! GPU identity, fitting and live budgets, queried only in the native worker.

use std::ffi::{CStr, CString};
use std::num::NonZeroU32;
use std::path::Path;
use std::pin::Pin;
use std::time::{Duration, Instant};

use crate::resources::GpuMemory;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::model::params::{FitError, LlamaModelParams, LlamaSplitMode};
use llama_cpp_2::{list_llama_ggml_backend_devices, LlamaBackendDeviceType};

use crate::error::{Result, RuntimeError};
use crate::native::policy as resources;

pub(crate) const FORCE_CPU_ENV: &str = "MODEL_RUNTIME_NATIVE_FORCE_CPU";

#[derive(Debug, Clone)]
pub(crate) struct Device {
    pub index: usize,
    #[cfg(feature = "media")]
    pub whisper_index: i32,
    pub name: String,
    pub description: String,
    pub backend: String,
    pci_id: Option<String>,
    pub memory: GpuMemory,
    context_floor: Option<u64>,
}

pub(crate) fn cpu_forced() -> bool {
    std::env::var_os(FORCE_CPU_ENV).is_some()
        || std::env::var_os("MODEL_RUNTIME_DISABLE_GPU_OFFLOAD").is_some()
        || std::env::var("MODEL_RUNTIME_GPU_LEASE_ACTIVE").as_deref() != Ok("1")
}

fn pci_id(index: usize) -> Option<String> {
    // SAFETY: indices come from GGML's own registry. Properties point to
    // backend-owned strings that remain alive for this worker's lifetime.
    unsafe {
        if index >= llama_cpp_sys_2::ggml_backend_dev_count() {
            return None;
        }
        let device = llama_cpp_sys_2::ggml_backend_dev_get(index);
        let mut properties = std::mem::zeroed();
        llama_cpp_sys_2::ggml_backend_dev_get_props(device, &mut properties);
        if properties.device_id.is_null() {
            return None;
        }
        Some(
            CStr::from_ptr(properties.device_id)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

pub(crate) fn devices() -> Vec<Device> {
    #[cfg(feature = "media")]
    let mut ordinal = 0;
    let mut result = Vec::new();
    for native in list_llama_ggml_backend_devices() {
        if !matches!(
            native.device_type,
            LlamaBackendDeviceType::Gpu | LlamaBackendDeviceType::IntegratedGpu
        ) {
            continue;
        }
        #[cfg(feature = "media")]
        let whisper_index = {
            let index = ordinal;
            ordinal += 1;
            index
        };
        // Unified-memory GPU admission needs a shared RAM budget model rather
        // than treating system RAM as independently available VRAM.
        if !matches!(native.device_type, LlamaBackendDeviceType::Gpu) {
            continue;
        }
        let pci = pci_id(native.index);
        let memory = crate::gpu::measured_budget(
            native.memory_total as u64,
            native.memory_free as u64,
            pci.as_deref().and_then(crate::gpu::pci_memory),
        );
        if let Some(memory) = memory {
            result.push(Device {
                index: native.index,
                #[cfg(feature = "media")]
                whisper_index,
                name: native.name,
                description: native.description,
                backend: native.backend,
                pci_id: pci,
                memory,
                context_floor: None,
            });
        }
    }
    result.sort_by(|a, b| {
        b.memory
            .available
            .cmp(&a.memory.available)
            .then(a.index.cmp(&b.index))
    });
    result
}

impl Device {
    pub fn current_memory(&self) -> Result<GpuMemory> {
        let native = list_llama_ggml_backend_devices()
            .into_iter()
            .find(|d| d.index == self.index && d.name == self.name && d.backend == self.backend)
            .ok_or_else(|| RuntimeError::Other("The selected GPU is no longer available".into()))?;
        crate::gpu::measured_budget(
            native.memory_total as u64,
            native.memory_free as u64,
            self.pci_id.as_deref().and_then(crate::gpu::pci_memory),
        )
        .ok_or_else(|| {
            RuntimeError::Other("The selected GPU no longer reports a usable memory budget".into())
        })
    }

    pub fn check_pressure(&self) -> Result<()> {
        if crate::gpu::critical_pressure(self.current_memory()?) {
            return Err(RuntimeError::Other(format!(
                "{} memory fell below emergency desktop headroom; inference stopped.",
                self.description,
            )));
        }
        Ok(())
    }

    pub fn reserve_context(&mut self) -> Result<()> {
        let now = self.current_memory()?;
        self.context_floor = Some(
            now.available
                .saturating_sub(crate::gpu::reserve_bytes(now) / 2),
        );
        crate::native::worker::emit_gpu_info(crate::gpu_info::GpuInfo {
            name: Some(format!("{} ({})", self.description, self.name)),
            total_vram_bytes: Some(now.total),
            available_vram_bytes: Some(now.available),
            source: crate::gpu_info::DetectionSource::NativeBackend,
        });
        self.check_pressure()
    }

    pub fn check_context(&self) -> Result<()> {
        let now = self.current_memory()?;
        if self
            .context_floor
            .is_some_and(|floor| now.available < floor)
        {
            return Err(RuntimeError::Other(
                "GPU headroom changed since model fitting; stopped before context allocation. The next request will use recovery policy.".into(),
            ));
        }
        self.check_pressure()
    }
}

pub(crate) fn context_params(context: NonZeroU32, embeddings: bool) -> LlamaContextParams {
    let batch = if embeddings {
        context.get()
    } else {
        context.get().min(512)
    };
    LlamaContextParams::default()
        .with_n_ctx(Some(context))
        .with_n_batch(batch)
        .with_n_ubatch(if embeddings { batch } else { batch.min(128) })
        .with_n_threads(resources::inference_thread_count())
        .with_n_threads_batch(resources::inference_thread_count())
        .with_embeddings(embeddings)
}

fn fitted_gpu_layers(fitted: i32, requested: u32) -> Option<u32> {
    // A successful fit keeps llama.cpp's -1 (all layers) when no adjustment
    // was needed. It is not a zero-offload result.
    let layers = match fitted {
        -1 => requested,
        1.. => (fitted as u32).min(requested),
        _ => 0,
    };
    (layers > 0).then_some(layers)
}

pub(crate) fn fitted_params(
    path: &Path,
    context: NonZeroU32,
    requested_layers: u32,
    embeddings: bool,
) -> Result<(LlamaModelParams, Option<Device>)> {
    let cpu = || {
        LlamaModelParams::default()
            .with_n_gpu_layers(0)
            .with_use_mmap(false)
            .with_devices(&[])
    };
    if requested_layers == 0 || cpu_forced() {
        return Ok((cpu().map_err(|e| RuntimeError::Other(e.to_string()))?, None));
    }
    let Some(device) = devices().into_iter().next() else {
        crate::native::worker::emit_runtime_warning(
            "No dedicated GPU has a measured backend memory budget; using CPU.",
        );
        return Ok((cpu().map_err(|e| RuntimeError::Other(e.to_string()))?, None));
    };
    let mut parameters = Box::pin(
        LlamaModelParams::default()
            .with_split_mode(LlamaSplitMode::None)
            .with_main_gpu(0)
            .with_use_mmap(false)
            .with_devices(&[device.index])
            .map_err(|e| RuntimeError::Other(e.to_string()))?,
    );
    let mut context_params = context_params(context, embeddings);
    let model_path = CString::new(
        path.to_str()
            .ok_or_else(|| RuntimeError::Other("Model path is not UTF-8".into()))?,
    )
    .map_err(|_| RuntimeError::Other("Model path contains a NUL byte".into()))?;
    let mut margins =
        vec![crate::gpu::reserve_bytes(device.memory) as usize; llama_cpp_2::max_devices()];
    let layers = match parameters.as_mut().fit_params(
        &model_path,
        &mut context_params,
        &mut margins,
        context.get(),
        llama_cpp_sys_2::GGML_LOG_LEVEL_WARN,
    ) {
        Ok(fit) if fit.n_ctx == context.get() => {
            fitted_gpu_layers(parameters.n_gpu_layers(), requested_layers)
        }
        Ok(_) | Err(FitError::Failure) => None,
        Err(FitError::Error) => {
            return Err(RuntimeError::Other(
                "Native model memory fitting failed; inspect the model file and runtime log".into(),
            ))
        }
    };
    let Some(layers) = layers else {
        crate::native::worker::emit_runtime_warning("The model's tensors, KV cache and compute buffers do not fit GPU headroom at the requested context; using CPU without shortening context.");
        return Ok((cpu().map_err(|e| RuntimeError::Other(e.to_string()))?, None));
    };
    device.check_pressure()?;
    let parameters = *Pin::into_inner(parameters);
    tracing::info!(
        device = %device.description,
        layers,
        context_tokens = context.get(),
        "Native model GPU admission succeeded"
    );
    let mut pressure = PressureWatch::new(Some(device.clone()));
    Ok((
        parameters
            .with_n_gpu_layers(layers)
            .with_progress_callback(move |_| match pressure.check() {
                Ok(()) => true,
                Err(error) => {
                    crate::native::worker::emit_runtime_warning(&error.to_string());
                    false
                }
            }),
        Some(device),
    ))
}

#[cfg(feature = "media")]
pub(crate) fn speech_device(path: &Path) -> Result<Option<Device>> {
    if !cfg!(feature = "media-vulkan") || cpu_forced() {
        return Ok(None);
    }
    let _backend = super::llama_shared::shared_backend()?;
    let model_size = std::fs::metadata(path)?.len();
    for device in devices() {
        if crate::resources::gpu_admission(
            model_size,
            crate::resources::ModelWorkload::Whisper,
            Some(device.memory),
        ) == crate::resources::GpuAdmission::Gpu
        {
            crate::native::worker::emit_runtime_warning(&format!(
                "Whisper GPU admission selected {} ({}).",
                device.description, device.name,
            ));
            return Ok(Some(device));
        }
    }
    crate::native::worker::emit_runtime_warning(
        "No measured GPU budget can hold this Whisper model with desktop headroom; using CPU.",
    );
    Ok(None)
}

pub(crate) struct PressureWatch {
    device: Option<Device>,
    checked_at: Instant,
}

impl PressureWatch {
    pub fn new(device: Option<Device>) -> Self {
        Self {
            device,
            checked_at: Instant::now() - Duration::from_secs(1),
        }

    }
    pub fn check(&mut self) -> Result<()> {
        if self.checked_at.elapsed() < Duration::from_millis(500) {
            return Ok(());
        }
        self.checked_at = Instant::now();
        if let Some(reason) = crate::resources::critical_memory_pressure() {
            return Err(RuntimeError::Other(reason));
        }
        if let Some(device) = &self.device {
            device.check_pressure()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_fit_preserves_default_all_layers_offload() {
        let default_layers = LlamaModelParams::default().n_gpu_layers();
        assert_eq!(default_layers, -1);
        assert_eq!(fitted_gpu_layers(default_layers, 999), Some(999));
        assert_eq!(fitted_gpu_layers(default_layers, 1), Some(1));
    }

    #[test]
    fn fitted_partial_offload_respects_the_requested_limit() {
        assert_eq!(fitted_gpu_layers(24, 999), Some(24));
        assert_eq!(fitted_gpu_layers(24, 8), Some(8));
    }

    #[test]
    fn zero_offload_and_unknown_sentinels_never_enable_gpu() {
        for fitted in [-2, -1, 0, 24] {
            assert_eq!(fitted_gpu_layers(fitted, 0), None);
        }
        assert_eq!(fitted_gpu_layers(0, 999), None);
        assert_eq!(fitted_gpu_layers(-2, 999), None);
    }
}
