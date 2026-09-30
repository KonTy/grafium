//! Whether a given GGUF model can realistically run on the GPU, and what to
//! tell the user when it can't.
//!
//! Picker guidance lives outside `llm-local` so Settings can compare models
//! without loading native code. It uses the runtime's device-memory probe, but
//! is not permission to load: runtime admission also includes the requested
//! context and desktop headroom, and can select CPU at execution time.

/// Lower/upper bounds for the VRAM safety margin (see
/// [`vram_safety_margin_bytes`]).
pub const VRAM_SAFETY_MARGIN_MIN_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB
pub const VRAM_SAFETY_MARGIN_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB

/// Headroom to subtract from detected free VRAM before deciding whether a
/// model fits, scaled with model size instead of a fixed constant.
///
/// The margin exists for the KV cache/context buffers (which aren't part of
/// the model file's on-disk size) and for other GPU consumers (compositor,
/// browser, a second app). A *fixed* 1.5 GiB — the previous value — is a bad
/// fit at both ends: it's wastefully large for a tiny 1 GB model (demanding
/// 2.5 GB free for something that needs ~1 GB) yet arguably too small for a
/// 30 GB model whose KV cache alone can exceed 1.5 GiB. Scaling at ~20% of
/// the model size, clamped to a sane [512 MiB, 2 GiB] band, tracks the KV
/// cache's rough growth without ballooning.
///
/// Tradeoff to keep in mind if tuning: too *small* a margin risks a GPU OOM
/// at generation time (KV cache grows with context length, which this can't
/// see up front); too *large* needlessly pins capable models to CPU — the
/// exact silent-slowdown bug this whole path exists to avoid. The band above
/// was chosen conservatively; prefer raising the floor over the ceiling if a
/// real GPU-OOM is observed.
pub fn vram_safety_margin_bytes(model_size_bytes: u64) -> u64 {
    (model_size_bytes / 5).clamp(VRAM_SAFETY_MARGIN_MIN_BYTES, VRAM_SAFETY_MARGIN_MAX_BYTES)
}

/// Total VRAM a model needs to run fully on the GPU: weights plus the margin.
///
/// Derived from the same margin the verdict uses, so the "needs about N GB"
/// hint and the fits/tight/cpu-only verdict can never contradict each other —
/// which they did while two independent estimators were shipped side by side.
///
/// The thresholds are deliberately the ones calibrated against measured
/// throughput on a real card (see `large_model_on_16gb_card_is_reported_tight_not_fast`)
/// rather than a more pessimistic estimate: a 13.6 GB model on a 16 GB card
/// genuinely does load and run, just badly, and calling that "won't fit"
/// contradicts the measurement.
pub fn estimated_vram_needed_bytes(model_size_bytes: u64) -> u64 {
    model_size_bytes.saturating_add(vram_safety_margin_bytes(model_size_bytes))
}

/// Picker estimate: given the model's on-disk size and currently observed free
/// VRAM, does full offload fit? Returns `true` when every layer can be
/// offloaded. Split out from I/O so it can be unit-tested.
pub fn fits_in_vram(model_size_bytes: u64, free_vram_bytes: u64) -> bool {
    model_size_bytes.saturating_add(vram_safety_margin_bytes(model_size_bytes)) <= free_vram_bytes
}

/// How a model is expected to perform on this machine, for display in the
/// model picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuFit {
    /// Fits in free VRAM with comfortable headroom — expect full GPU speed.
    Fits,
    /// Fits, but only just. This is a genuinely distinct case, not a rounding
    /// detail: a 13.6 GB model on a 16 GB card clears the margin by ~180 MiB,
    /// so it loads onto the GPU when the desktop is idle and silently demotes
    /// to CPU the moment anything else claims VRAM (a browser, the embedding
    /// model mid-index, a second Grafium window). The user then sees the same
    /// model be fast once and take minutes the next time, with nothing in the
    /// UI explaining why. Saying "tight" up front is more truthful than a
    /// binary yes.
    Tight,
    /// Too large for free VRAM: llama.cpp will run it on the CPU, which in
    /// practice means single-digit tokens/second for a multi-billion
    /// parameter model.
    CpuOnly,
    /// No unambiguous dedicated GPU budget is available. Native admission
    /// selects CPU rather than assuming that unknown VRAM is free.
    Unknown,
}

impl GpuFit {
    /// Stable identifier for the frontend, so the UI doesn't duplicate this
    /// enum or pattern-match on prose.
    pub fn as_str(self) -> &'static str {
        match self {
            GpuFit::Fits => "fits",
            GpuFit::Tight => "tight",
            GpuFit::CpuOnly => "cpu_only",
            GpuFit::Unknown => "unknown",
        }
    }
}

/// Headroom beyond the safety margin below which a model is reported as
/// [`GpuFit::Tight`] rather than [`GpuFit::Fits`].
///
/// The safety margin already covers the KV cache for a *typical* context, but
/// it's clamped to 2 GiB and takes no account of context length, so a large
/// model at a large context can clear the margin on paper and still be the
/// first thing evicted in practice. This is the "and don't cut it fine"
/// allowance on top.
const TIGHT_FIT_HEADROOM_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB

/// Classifies `model_size_bytes` against `free_vram_bytes`.
///
/// `free_vram_bytes` is `None` when VRAM couldn't be measured, which maps to
/// [`GpuFit::Unknown`] rather than an optimistic guess.
pub fn assess_gpu_fit(model_size_bytes: u64, free_vram_bytes: Option<u64>) -> GpuFit {
    let Some(free) = free_vram_bytes else {
        return GpuFit::Unknown;
    };
    if !fits_in_vram(model_size_bytes, free) {
        return GpuFit::CpuOnly;
    }
    let required = model_size_bytes.saturating_add(vram_safety_margin_bytes(model_size_bytes));
    if free.saturating_sub(required) < TIGHT_FIT_HEADROOM_BYTES {
        GpuFit::Tight
    } else {
        GpuFit::Fits
    }
}

/// A short, human-readable explanation to show next to a model in the picker.
///
/// Phrased in terms of the consequence the user actually cares about (speed),
/// not the mechanism (layer offload), because the whole point is to make an
/// otherwise invisible 40× slowdown legible at the moment of choosing.
pub fn fit_detail(fit: GpuFit, model_size_bytes: u64, free_vram_bytes: Option<u64>) -> String {
    let mib = |b: u64| b / (1024 * 1024);
    match fit {
        GpuFit::Fits => format!(
            "Estimated GPU fit (~{} MiB model, ~{} MiB VRAM free). The runtime also checks context and desktop headroom before loading.",
            mib(model_size_bytes),
            free_vram_bytes.map(mib).unwrap_or(0)
        ),
        GpuFit::Tight => format!(
            "Tight estimated fit (~{} MiB model vs ~{} MiB free). Runtime admission may \
             select CPU to preserve context and desktop headroom.",
            mib(model_size_bytes),
            free_vram_bytes.map(mib).unwrap_or(0)
        ),
        GpuFit::CpuOnly => format!(
            "Too big for free VRAM (~{} MiB model vs ~{} MiB free) — will run on the CPU, \
             typically a few tokens per second. Pick a smaller or more heavily quantized model \
             for interactive chat.",
            mib(model_size_bytes),
            free_vram_bytes.map(mib).unwrap_or(0)
        ),
        GpuFit::Unknown => {
            "Couldn't detect GPU memory, so speed can't be predicted for this model.".to_string()
        }
    }
}

/// Free VRAM from the same unambiguous-device probe used at model load.
pub fn detect_free_vram_bytes() -> Option<u64> {
    crate::gpu_info::detect_primary_gpu().available_vram_bytes
}

/// Compatibility entry point. Never use an earlier optimistic reading for
/// admission: another application may have allocated GPU memory since then.
pub fn detect_free_vram_bytes_best() -> Option<u64> {
    detect_free_vram_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn margin_scales_with_model_size_within_band() {
        // Small model: clamped up to the floor.
        assert_eq!(vram_safety_margin_bytes(GIB), VRAM_SAFETY_MARGIN_MIN_BYTES);
        // Mid model: 20% of size, inside the band.
        assert_eq!(vram_safety_margin_bytes(5 * GIB), GIB);
        // Huge model: clamped down to the ceiling.
        assert_eq!(
            vram_safety_margin_bytes(30 * GIB),
            VRAM_SAFETY_MARGIN_MAX_BYTES
        );
    }

    #[test]
    fn fits_exactly_at_the_margin_boundary() {
        let model = 5 * GIB;
        let needed = model + vram_safety_margin_bytes(model);
        assert!(fits_in_vram(model, needed));
        assert!(!fits_in_vram(model, needed - 1));
    }

    /// The regression this module was written for. A 13.6 GB Q4 model on a
    /// 16 GB card clears the safety margin by only ~180 MiB, and that sliver
    /// is exactly why it measured **1.5 tok/s**: it loads on the GPU when
    /// nothing else is running and falls back to CPU under any other VRAM
    /// pressure. A 2.4 GB model on the same card hit 60-74 tok/s. The picker
    /// must distinguish these three states, not two.
    #[test]
    fn large_model_on_16gb_card_is_reported_tight_not_fast() {
        let free = 15_900 * 1024 * 1024; // ~all of an idle 16 GB card
        let mistral_24b = 13_670 * 1024 * 1024;
        let qwen_4b = 2_382 * 1024 * 1024;

        assert_eq!(assess_gpu_fit(mistral_24b, Some(free)), GpuFit::Tight);
        assert_eq!(assess_gpu_fit(qwen_4b, Some(free)), GpuFit::Fits);
    }

    /// Same big model, but with a browser/compositor already holding a couple
    /// of gigabytes — the everyday case, where it genuinely can't fit.
    #[test]
    fn large_model_is_cpu_only_once_something_else_uses_vram() {
        let free = 12_000 * 1024 * 1024;
        let mistral_24b = 13_670 * 1024 * 1024;
        assert_eq!(assess_gpu_fit(mistral_24b, Some(free)), GpuFit::CpuOnly);
    }

    #[test]
    fn tight_detail_warns_about_falling_back() {
        let detail = fit_detail(
            GpuFit::Tight,
            13_670 * 1024 * 1024,
            Some(15_900 * 1024 * 1024),
        );
        assert!(detail.contains("Tight estimated fit"));
        assert!(detail.contains("CPU"));
    }

    #[test]
    fn unmeasurable_vram_is_unknown_not_optimistic() {
        assert_eq!(assess_gpu_fit(2 * GIB, None), GpuFit::Unknown);
        assert!(fit_detail(GpuFit::Unknown, 2 * GIB, None).contains("Couldn't detect"));
    }

    #[test]
    fn cpu_only_detail_names_the_speed_consequence() {
        let detail = fit_detail(
            GpuFit::CpuOnly,
            13_670 * 1024 * 1024,
            Some(15_900 * 1024 * 1024),
        );
        assert!(detail.contains("CPU"));
        assert!(detail.contains("tokens per second"));
    }

    #[test]
    fn fit_strings_are_stable_for_the_frontend() {
        assert_eq!(GpuFit::Fits.as_str(), "fits");
        assert_eq!(GpuFit::Tight.as_str(), "tight");
        assert_eq!(GpuFit::CpuOnly.as_str(), "cpu_only");
        assert_eq!(GpuFit::Unknown.as_str(), "unknown");
    }
}
