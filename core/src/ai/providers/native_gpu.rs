//! GPU identity, fitting and live budgets, queried only in the native worker.

use std::ffi::{CStr, CString};
use std::num::NonZeroU32;
use std::path::Path;
use std::pin::Pin;
use std::time::{Duration, Instant};

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::model::params::{FitError, LlamaModelParams, LlamaSplitMode};
use llama_cpp_2::{list_llama_ggml_backend_devices, LlamaBackendDeviceType};
use model_runtime::resources::GpuMemory;

use crate::ai::resources;
use crate::error::{CoreError, Result};

pub(crate) const FORCE_CPU_ENV: &str = "GRAFIUM_NATIVE_FORCE_CPU";

#[derive(Debug, Clone)]
pub(crate) struct Device {
    pub index: usize,
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
        || std::env::var_os("GRAFIUM_DISABLE_GPU_OFFLOAD").is_some()
        || std::env::var("GRAFIUM_GPU_LEASE_ACTIVE").as_deref() != Ok("1")
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
    let mut ordinal = 0;
    let mut result = Vec::new();
    for native in list_llama_ggml_backend_devices() {
        if !matches!(
            native.device_type,
            LlamaBackendDeviceType::Gpu | LlamaBackendDeviceType::IntegratedGpu
        ) {
            continue;
        }
        let whisper_index = ordinal;
        ordinal += 1;
        // Unified-memory GPU admission needs a shared RAM budget model rather
        // than treating system RAM as independently available VRAM.
        if !matches!(native.device_type, LlamaBackendDeviceType::Gpu) {
            continue;
        }
        let pci = pci_id(native.index);
        let memory = model_runtime::gpu::measured_budget(
            native.memory_total as u64,
            native.memory_free as u64,
            pci.as_deref().and_then(model_runtime::gpu::pci_memory),
        );
        if let Some(memory) = memory {
            result.push(Device {
                index: native.index,
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
            .ok_or_else(|| CoreError::Other("The selected GPU is no longer available".into()))?;
        model_runtime::gpu::measured_budget(
            native.memory_total as u64,
            native.memory_free as u64,
            self.pci_id
                .as_deref()
                .and_then(model_runtime::gpu::pci_memory),
        )
        .ok_or_else(|| {
            CoreError::Other("The selected GPU no longer reports a usable memory budget".into())
        })
    }

    pub fn check_pressure(&self) -> Result<()> {
        if model_runtime::gpu::critical_pressure(self.current_memory()?) {
            return Err(CoreError::Other(format!(
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
                .saturating_sub(model_runtime::gpu::reserve_bytes(now) / 2),
        );
        crate::ai::worker::emit_gpu_info(crate::gpu_info::GpuInfo {
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
            return Err(CoreError::Other(
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
        return Ok((cpu().map_err(|e| CoreError::Other(e.to_string()))?, None));
    }
    let Some(device) = devices().into_iter().next() else {
        crate::ai::worker::emit_runtime_warning(
            "No dedicated GPU has a measured backend memory budget; using CPU.",
        );
        return Ok((cpu().map_err(|e| CoreError::Other(e.to_string()))?, None));
    };
    let mut parameters = Box::pin(
        LlamaModelParams::default()
            .with_split_mode(LlamaSplitMode::None)
            .with_main_gpu(0)
            .with_use_mmap(false)
            .with_devices(&[device.index])
            .map_err(|e| CoreError::Other(e.to_string()))?,
    );
    let mut context_params = context_params(context, embeddings);
    let model_path = CString::new(
        path.to_str()
            .ok_or_else(|| CoreError::Other("Model path is not UTF-8".into()))?,
    )
    .map_err(|_| CoreError::Other("Model path contains a NUL byte".into()))?;
    let mut margins =
        vec![model_runtime::gpu::reserve_bytes(device.memory) as usize; llama_cpp_2::max_devices()];
    match parameters.as_mut().fit_params(
        &model_path,
        &mut context_params,
        &mut margins,
        context.get(),
        llama_cpp_sys_2::GGML_LOG_LEVEL_WARN,
    ) {
        Ok(fit) if fit.n_ctx == context.get() && parameters.n_gpu_layers() > 0 => {}
        Ok(_) | Err(FitError::Failure) => {
            crate::ai::worker::emit_runtime_warning("The model's tensors, KV cache and compute buffers do not fit GPU headroom at the requested context; using CPU without shortening context.");
            return Ok((cpu().map_err(|e| CoreError::Other(e.to_string()))?, None));
        }
        Err(FitError::Error) => {
            return Err(CoreError::Other(
                "Native model memory fitting failed; inspect the model file and runtime log".into(),
            ))
        }
    }
    device.check_pressure()?;
    let parameters = *Pin::into_inner(parameters);
    let layers = (parameters.n_gpu_layers() as u32).min(requested_layers);
    crate::ai::worker::emit_runtime_warning(&format!(
        "Using {} ({}) with {} GPU layers; context remains {} tokens. GPU budgets are estimates, not a driver-failure guarantee.",
        device.description, device.name, layers, context.get(),
    ));
    let mut pressure = PressureWatch::new(Some(device.clone()));
    Ok((
        parameters
            .with_n_gpu_layers(layers)
            .with_progress_callback(move |_| match pressure.check() {
                Ok(()) => true,
                Err(error) => {
                    crate::ai::worker::emit_runtime_warning(&error.to_string());
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
        if model_runtime::resources::gpu_admission(
            model_size,
            model_runtime::resources::ModelWorkload::Whisper,
            Some(device.memory),
        ) == model_runtime::resources::GpuAdmission::Gpu
        {
            crate::ai::worker::emit_runtime_warning(&format!(
                "Whisper GPU admission selected {} ({}).",
                device.description, device.name,
            ));
            return Ok(Some(device));
        }
    }
    crate::ai::worker::emit_runtime_warning(
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
        if let Some(reason) = model_runtime::resources::critical_memory_pressure() {
            return Err(CoreError::Other(reason));
        }
        if let Some(device) = &self.device {
            device.check_pressure()?;
        }
        Ok(())
    }
}
