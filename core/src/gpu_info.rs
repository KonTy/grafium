//! Best-effort detection of the machine's discrete-GPU total/free VRAM, so
//! the UI can annotate which chat models will comfortably fit on the GPU
//! versus which will spill onto host RAM / system swap and be
//! painfully slow.
//!
//! There is no cross-vendor, cross-OS "how much VRAM does this box have?"
//! API we can call from userspace without pulling in either a full CUDA
//! toolkit dependency (nvml, which requires the NVIDIA developer runtime)
//! or a Vulkan crate that would inflate binary size just for a couple of
//! integers. Instead we shell out to whichever of `nvidia-smi`,
//! `vulkaninfo`, `rocm-smi`, or Linux sysfs is present — the same
//! best-effort pattern the existing free-VRAM check in `local_llm.rs`
//! uses — and return `None` if nothing worked. `None` intentionally reads
//! at the UI layer as "I don't know, so don't warn the user about
//! anything" rather than "your GPU has zero VRAM" — silent fallback is
//! the safe behaviour when detection is uncertain.
//!
//! # Why not just look at file size + margin without knowing VRAM?
//!
//! Because "will this model be slow" isn't a global fact about the model;
//! it's a fact about *this model on this box*. A 14 GB Q4_K_M weight
//! genuinely runs fine on a 24 GB card and thrashes horribly on a 12 GB
//! card. Without the VRAM number we'd have to hardcode a threshold (which
//! would be wrong for half our users) or annotate every model as
//! potentially-slow (which is UX noise). The trade-off of "sometimes we
//! can't detect and don't warn" is much better than either alternative.

use std::process::Command;

/// What we learned about the primary discrete GPU. All fields are
/// optional because different detection paths surface different subsets
/// (e.g. sysfs gives us name + total but never free; nvidia-smi gives
/// all three; vulkaninfo gives name + heap size but not live free).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GpuInfo {
    /// Human-readable device name — e.g. `"NVIDIA GeForce RTX 4060 Ti"`
    /// or `"AMD Radeon RX 7900 XTX"`. Shown in the model picker so the
    /// user can sanity-check that we detected the *right* card (e.g.
    /// they might have both an iGPU and a dGPU, and we want them to know
    /// we're comparing against the dGPU's 16 GB not the iGPU's 512 MiB).
    pub name: Option<String>,
    /// Total device memory in bytes. This is what we compare model
    /// sizes against for the "fits on GPU" annotation, since "free" at
    /// any given instant is dominated by whatever else is running and
    /// isn't a stable property of the machine.
    pub total_vram_bytes: Option<u64>,
    /// Instantaneously free device memory in bytes, when the detection
    /// path can give it (only nvidia-smi does today). Purely
    /// informational — the "will it fit" decision uses `total_vram_bytes`.
    pub available_vram_bytes: Option<u64>,
    /// Which command actually produced these numbers, for the settings
    /// UI to explain the source (or to help debug when the answer looks
    /// wrong on an exotic setup).
    pub source: DetectionSource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DetectionSource {
    /// Nothing worked. `GpuInfo::default()` uses this.
    #[default]
    None,
    /// `nvidia-smi` on the PATH — the gold-standard when present (gives
    /// name, total, and free in one call).
    NvidiaSmi,
    /// `rocm-smi` on the PATH — AMD's equivalent, less structured
    /// output but total is reliably in the "GPU memory" field.
    RocmSmi,
    /// `vulkaninfo --summary` — cross-vendor, present anywhere Vulkan
    /// works. Gives name and heap size but not live free.
    Vulkaninfo,
    /// `/sys/class/drm/card*/device/mem_info_vram_total` — Linux AMDGPU
    /// / i915 sysfs entry. Present on AMD + Intel dGPUs without needing
    /// any userspace tool. Last-resort because it doesn't give a name;
    /// we combine it with a name from `/sys/class/drm/card*/device/vendor`
    /// when we can.
    Sysfs,
}

/// Try each detection path in decreasing order of quality until one
/// succeeds. Returns a `GpuInfo` with source `None` (and no other fields
/// set) if all paths fail — that's the "we don't know" state the UI
/// treats as "don't annotate anything".
pub fn detect_primary_gpu() -> GpuInfo {
    if let Some(info) = detect_via_nvidia_smi() {
        return info;
    }
    if let Some(info) = detect_via_rocm_smi() {
        return info;
    }
    if let Some(info) = detect_via_vulkaninfo() {
        return info;
    }
    if let Some(info) = detect_via_sysfs() {
        return info;
    }
    GpuInfo::default()
}

fn detect_via_nvidia_smi() -> Option<GpuInfo> {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,memory.free",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let first = text.lines().next()?.trim();
    // Fields are comma-separated with spaces after commas, all in MiB.
    let parts: Vec<&str> = first.split(',').map(str::trim).collect();
    if parts.len() < 3 {
        return None;
    }
    let name = parts[0].to_string();
    let total_mib: u64 = parts[1].parse().ok()?;
    let free_mib: u64 = parts[2].parse().ok()?;
    Some(GpuInfo {
        name: Some(name),
        total_vram_bytes: Some(total_mib * 1024 * 1024),
        available_vram_bytes: Some(free_mib * 1024 * 1024),
        source: DetectionSource::NvidiaSmi,
    })
}

fn detect_via_rocm_smi() -> Option<GpuInfo> {
    // `rocm-smi --showmeminfo vram --showproductname` — output is a bit
    // messy so we do best-effort line-by-line parsing. `--json` would be
    // cleaner but isn't available on older ROCm installs.
    let out = Command::new("rocm-smi")
        .args(["--showmeminfo", "vram", "--showproductname"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let mut name = None;
    let mut total_bytes = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("Card series:").map(str::trim) {
            name = Some(format!("AMD {v}"));
        }
        if let Some(v) = line
            .split_once("VRAM Total Memory (B):")
            .map(|(_, v)| v.trim())
        {
            total_bytes = v.parse::<u64>().ok();
        }
    }
    let total_bytes = total_bytes?;
    Some(GpuInfo {
        name,
        total_vram_bytes: Some(total_bytes),
        available_vram_bytes: None,
        source: DetectionSource::RocmSmi,
    })
}

fn detect_via_vulkaninfo() -> Option<GpuInfo> {
    // `vulkaninfo --summary` prints one section per device including
    // `deviceName` and `deviceType`, but not heap size — so we fall back
    // to the *full* `vulkaninfo` output and grep the first
    // `DEVICE_LOCAL` heap's size. Slower (couple hundred ms) but that's
    // fine: we only call this once at Settings-tab open.
    let out = Command::new("vulkaninfo").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;

    // Pick the first non-CPU device: `deviceType = PHYSICAL_DEVICE_TYPE_DISCRETE_GPU`
    // if present, else INTEGRATED, else nothing.
    let name = extract_first_gpu_name(&text)?;
    let total_bytes = extract_first_device_local_heap_size(&text)?;
    Some(GpuInfo {
        name: Some(name),
        total_vram_bytes: Some(total_bytes),
        available_vram_bytes: None,
        source: DetectionSource::Vulkaninfo,
    })
}

fn extract_first_gpu_name(text: &str) -> Option<String> {
    // vulkaninfo prints per-device blocks. Grab the first
    // `deviceName = <name>` line whose companion `deviceType` says
    // DISCRETE_GPU or INTEGRATED_GPU (not CPU).
    let mut current_name: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(v) = trimmed.strip_prefix("deviceName") {
            current_name = v
                .split_once('=')
                .map(|(_, v)| v.trim().to_string())
                .filter(|s| !s.is_empty());
        }
        if let Some(v) = trimmed.strip_prefix("deviceType") {
            let dtype = v.split_once('=').map(|(_, v)| v.trim()).unwrap_or("");
            if dtype.contains("DISCRETE_GPU") || dtype.contains("INTEGRATED_GPU") {
                if let Some(n) = current_name.take() {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn extract_first_device_local_heap_size(text: &str) -> Option<u64> {
    // Look for `heapIndex ... : ... size = 15757MiB (16522608640) ...
    // flags: MEMORY_HEAP_DEVICE_LOCAL_BIT` — sizes appear both in decimal
    // bytes (parenthesised) and MiB. The `(NUMBER)` form is the least
    // ambiguous to parse, so that's what we look for on any line that
    // eventually gets tagged as DEVICE_LOCAL.
    //
    // vulkaninfo prints the heap header first (with size), then the
    // flags line separately. We track the most recent size we saw and
    // commit it when we hit a DEVICE_LOCAL flags line.
    let mut candidate_size: Option<u64> = None;
    for line in text.lines() {
        if let Some(size) = parse_bytes_from_paren(line) {
            candidate_size = Some(size);
        }
        if line.contains("MEMORY_HEAP_DEVICE_LOCAL_BIT") {
            if let Some(s) = candidate_size {
                return Some(s);
            }
        }
    }
    None
}

fn parse_bytes_from_paren(line: &str) -> Option<u64> {
    // First `(...)` group whose contents parse as a plain u64. Anchored
    // to `size =` so we don't grab random other parenthesised numbers.
    let idx = line.find("size")?;
    let after_size = &line[idx..];
    let open = after_size.find('(')?;
    let close = after_size[open + 1..].find(')')?;
    let inner = &after_size[open + 1..open + 1 + close];
    inner.trim().parse::<u64>().ok()
}

fn detect_via_sysfs() -> Option<GpuInfo> {
    // Only meaningful on Linux; the paths simply don't exist elsewhere
    // so `read_to_string` returns Err and we fall through.
    let cards = std::fs::read_dir("/sys/class/drm").ok()?;
    for entry in cards.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            // We want `card0`, not `card0-DP-1`.
            continue;
        }
        let path = entry.path().join("device").join("mem_info_vram_total");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(total_bytes) = text.trim().parse::<u64>() else {
            continue;
        };
        if total_bytes < 128 * 1024 * 1024 {
            // iGPU with a tiny UMA carve-out — not a real "GPU with
            // VRAM" for our purposes, keep looking.
            continue;
        }
        // Read the vendor ID for a best-effort name — we don't have the
        // marketing name but "AMD GPU (sysfs)" is enough for the user
        // to tell what we detected.
        let vendor = std::fs::read_to_string(entry.path().join("device").join("vendor"))
            .ok()
            .map(|s| s.trim().to_lowercase());
        let vendor_name = match vendor.as_deref() {
            Some("0x1002") => "AMD GPU",
            Some("0x10de") => "NVIDIA GPU",
            Some("0x8086") => "Intel GPU",
            _ => "GPU",
        };
        return Some(GpuInfo {
            name: Some(format!("{vendor_name} (via sysfs)")),
            total_vram_bytes: Some(total_bytes),
            available_vram_bytes: None,
            source: DetectionSource::Sysfs,
        });
    }
    None
}

/// Rough estimate of the total device memory a GGUF chat model will
/// need at inference time: the weights themselves plus KV cache and
/// compute buffers. Deliberately conservative on the KV side because
/// context-size is a Settings option we can't see from here — assuming
/// a "typical" 4-8k prompt window is a reasonable middle ground.
///
/// This is what the UI compares against `GpuInfo::total_vram_bytes` to
/// decide whether to show a "will be slow" warning on a model. If we
/// overestimate the requirement we err on the side of a false-positive
/// warning; if we underestimate, users see no warning and hit the
/// existing "not enough free VRAM, falling back to CPU" runtime
/// message from `local_llm.rs`. Both outcomes are safe; only the
/// former nudges users to the right choice up front.
pub fn estimated_vram_needed_bytes(model_size_bytes: u64) -> u64 {
    // 1.25x weights + 1.5 GiB fixed overhead (context + compute
    // buffers). Numbers picked to align with what we observed loading
    // real GGUFs of various sizes on the user's 16 GB card — a 14 GB
    // Mistral-Small-3.2-24B thrashes at these thresholds, matching
    // reality; a 4 GB Qwen3-4B comfortably fits, also matching reality.
    let weights = model_size_bytes.saturating_mul(5) / 4;
    let overhead: u64 = 1_536 * 1024 * 1024;
    weights.saturating_add(overhead)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bytes_from_paren_finds_the_size_group() {
        let line = "\t\tsize = 15757MiB (16522608640) (100.00%)";
        assert_eq!(parse_bytes_from_paren(line), Some(16_522_608_640));
    }

    #[test]
    fn parse_bytes_from_paren_ignores_non_size_lines() {
        assert_eq!(parse_bytes_from_paren("someOther = 42 (foo)"), None);
    }

    #[test]
    fn extract_first_gpu_name_returns_discrete_over_cpu() {
        let text = "\
            deviceName          = llvmpipe (LLVM 18)\n\
            deviceType          = PHYSICAL_DEVICE_TYPE_CPU\n\
            deviceName          = NVIDIA GeForce RTX 4060 Ti\n\
            deviceType          = PHYSICAL_DEVICE_TYPE_DISCRETE_GPU\n";
        assert_eq!(
            extract_first_gpu_name(text),
            Some("NVIDIA GeForce RTX 4060 Ti".to_string())
        );
    }

    #[test]
    fn estimated_vram_needed_bytes_scales_with_size_and_adds_overhead() {
        // 8 GiB weights → 10 GiB + 1.5 GiB = 11.5 GiB estimate
        let eight_gib = 8u64 * 1024 * 1024 * 1024;
        let expected = 10u64 * 1024 * 1024 * 1024 + 1_536 * 1024 * 1024;
        assert_eq!(estimated_vram_needed_bytes(eight_gib), expected);
    }

    #[test]
    fn detect_primary_gpu_never_panics_and_returns_something() {
        // We can't assert what the CI/local box returns, but the
        // function itself must not panic and must return a valid struct
        // (either populated or default). Guards against a future
        // refactor that adds a `.unwrap()` on a missing tool.
        let info = detect_primary_gpu();
        // At minimum, the source field is set.
        let _ = info.source;
    }

    #[test]
    fn extract_first_device_local_heap_size_finds_the_first_device_local_heap() {
        let text = "\
        VkPhysicalDeviceMemoryProperties:\n\
        =================================\n\
        memoryHeaps: count = 3\n\
            memoryHeaps[0]:\n\
                size   = 15757MiB (16522608640) (94.16%)\n\
                budget = 15757MiB (16522608640) (94.16%)\n\
                usage  = 0MiB (0) (0.00%)\n\
                flags: count = 1\n\
                    MEMORY_HEAP_DEVICE_LOCAL_BIT\n\
            memoryHeaps[1]:\n\
                size   = 977MiB (1024000000) (5.84%)\n";
        assert_eq!(
            extract_first_device_local_heap_size(text),
            Some(16_522_608_640)
        );
    }
}
