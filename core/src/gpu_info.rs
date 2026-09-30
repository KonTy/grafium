//! Picker telemetry without loading a graphics driver in the application process.
//! Native device queries run in the supervised worker; host-side fallback reads
//! Linux counters only. These snapshots are guidance, not load authorization.

use std::sync::{Mutex, PoisonError};

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GpuInfo {
    pub name: Option<String>,
    pub total_vram_bytes: Option<u64>,
    pub available_vram_bytes: Option<u64>,
    pub source: DetectionSource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DetectionSource {
    #[default]
    None,
    NativeBackend,
    Sysfs,
    NvidiaSmi,
    RocmSmi,
    Vulkaninfo,
}

static NATIVE_GPU: Mutex<Option<GpuInfo>> = Mutex::new(None);

#[cfg(any(feature = "llm-local", feature = "media"))]
pub(crate) fn remember_native_gpu(info: GpuInfo) {
    *NATIVE_GPU.lock().unwrap_or_else(PoisonError::into_inner) = Some(info);
}

pub fn detect_primary_gpu() -> GpuInfo {
    if let Some(info) = NATIVE_GPU
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    {
        return info;
    }
    let Some(memory) = model_runtime::resources::gpu_memory_snapshot() else {
        return GpuInfo::default();
    };
    GpuInfo {
        name: None,
        total_vram_bytes: Some(memory.total),
        available_vram_bytes: Some(memory.available),
        source: DetectionSource::Sysfs,
    }
}

pub fn estimated_vram_needed_bytes(model_size_bytes: u64) -> u64 {
    model_size_bytes
        .saturating_mul(5)
        .saturating_div(4)
        .saturating_add(1536 * 1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_is_saturating_and_includes_context_headroom() {
        let gib = 1024 * 1024 * 1024;
        assert_eq!(
            estimated_vram_needed_bytes(8 * gib),
            10 * gib + 1536 * 1024 * 1024
        );
        assert!(estimated_vram_needed_bytes(u64::MAX) > gib);
    }

    #[test]
    fn picker_telemetry_does_not_execute_vendor_tools() {
        let info = detect_primary_gpu();
        assert!(matches!(
            info.source,
            DetectionSource::None | DetectionSource::NativeBackend | DetectionSource::Sysfs
        ));
    }
}
