//! Device-bound budget validation. Native hosts supply backend device identities.

use crate::resources::GpuMemory;

/// Some Vulkan backends report the whole heap as free without memory-budget
/// support. Do not treat that fallback as measured headroom.
pub fn measured_budget(
    total: u64,
    free: u64,
    corroborated: Option<GpuMemory>,
) -> Option<GpuMemory> {
    let native = (total > 0 && free < total).then_some(GpuMemory {
        total,
        available: free,
    });
    let corroborated = corroborated.filter(|m| m.total > 0 && m.available <= m.total);
    match (native, corroborated) {
        (Some(a), Some(b)) => {
            let total = a.total.min(b.total);
            Some(GpuMemory {
                total,
                available: a.available.min(b.available).min(total),
            })
        }
        (Some(memory), None) | (None, Some(memory)) => Some(memory),
        (None, None) => None,
    }
}

/// Linux vendor counters are matched to the backend's PCI identity, never
/// "the first GPU" or an unrelated display adapter.
pub fn pci_memory(pci_id: &str) -> Option<GpuMemory> {
    #[cfg(target_os = "linux")]
    {
        read_pci_memory(std::path::Path::new("/sys/bus/pci/devices"), pci_id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pci_id;
        None
    }
}

#[cfg(any(target_os = "linux", test))]
fn read_pci_memory(root: &std::path::Path, pci_id: &str) -> Option<GpuMemory> {
    let id = pci_id.to_ascii_lowercase();
    let bytes = id.as_bytes();
    if bytes.len() != 12
        || bytes[4] != b':'
        || bytes[7] != b':'
        || bytes[10] != b'.'
        || bytes
            .iter()
            .enumerate()
            .any(|(i, c)| !matches!(i, 4 | 7 | 10) && !c.is_ascii_hexdigit())
    {
        return None;
    }
    let device = root.join(id);
    let read = |name| {
        std::fs::read_to_string(device.join(name))
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()
    };
    let total = read("mem_info_vram_total")?;
    let used = read("mem_info_vram_used")?;
    (total > 0).then_some(GpuMemory {
        total,
        available: total.checked_sub(used)?,
    })
}

pub fn reserve_bytes(memory: GpuMemory) -> u64 {
    (512 * 1024 * 1024).max(memory.total / 8)
}

pub fn critical_pressure(memory: GpuMemory) -> bool {
    memory.available < (256 * 1024 * 1024).max(memory.total / 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_heap_is_not_a_measurement_of_free_memory() {
        assert_eq!(measured_budget(100, 100, None), None);
        assert_eq!(measured_budget(100, 101, None), None);
        assert_eq!(
            measured_budget(100, 40, None),
            Some(GpuMemory {
                total: 100,
                available: 40
            })
        );
        assert_eq!(
            measured_budget(
                100,
                100,
                Some(GpuMemory {
                    total: 100,
                    available: 20
                })
            ),
            Some(GpuMemory {
                total: 100,
                available: 20
            })
        );
    }

    #[test]
    fn vendor_counter_must_match_the_specific_device() {
        let dir = tempfile::tempdir().unwrap();
        let device = dir.path().join("0000:03:00.0");
        std::fs::create_dir(&device).unwrap();
        std::fs::write(device.join("mem_info_vram_total"), "16000").unwrap();
        std::fs::write(device.join("mem_info_vram_used"), "6000").unwrap();
        assert_eq!(
            read_pci_memory(dir.path(), "0000:03:00.0"),
            Some(GpuMemory {
                total: 16000,
                available: 10000
            })
        );
        assert_eq!(read_pci_memory(dir.path(), "0000:04:00.0"), None);
        assert_eq!(read_pci_memory(dir.path(), "../0000:03:00.0"), None);
    }
}
