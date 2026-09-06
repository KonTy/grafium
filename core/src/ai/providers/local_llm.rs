//! Embedded local LLM inference via llama.cpp (through the `llama-cpp-2`
//! bindings) — the LLM-side counterpart to `media::transcribe`'s
//! `WhisperTranscriber`. Both:
//!   * resolve a model file through the shared `model_library` instead of
//!     requiring an exact path (`from_settings`/`from_config` mean the same
//!     thing in both modules — see `media::transcribe` for the sibling this
//!     one is deliberately shaped to match),
//!   * silence their native library's verbose stderr logging once per
//!     process (whisper.cpp there, llama.cpp here) so a raw-mode terminal
//!     UI is never corrupted,
//!   * and expose themselves through this crate's existing trait
//!     abstractions (`Transcriber` there, `LlmProvider` here) rather than a
//!     bespoke call site — so summarization code depends on "an
//!     `LlmProvider`", never on "llama.cpp specifically".
//!
//! Gated behind the `llm-local` Cargo feature (mirrors `media`/
//! `media-vulkan`); enable `llm-local-vulkan` to offload inference to a GPU
//! via Vulkan (no CUDA toolkit required — same rationale as `media-vulkan`).

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

use super::llama_shared::{shared_backend, OFFLOAD_ALL_LAYERS};
use crate::ai::config::LocalLlmSettings;
use crate::ai::traits::{BoxFuture, ChatMessage, CompletionOptions, LlmProvider, MessageRole};
use crate::error::{CoreError, Result};
use crate::model_library::{self, ModelKind};

/// Context window used when neither the model's own trained context length
/// nor an explicit `context_size` setting is usable.
const DEFAULT_CTX_SIZE: u32 = 4096;

/// Upper bound applied to a model's *own* trained context length when
/// auto-deriving a default (i.e. when the user hasn't set an explicit
/// `context_size`). Modern models increasingly advertise very large trained
/// context windows (some Qwen3 checkpoints report 262144) — allocating a
/// llama.cpp KV cache that size by default pins tens of gigabytes of RAM
/// and can peg every CPU core for a single request, even for a short
/// prompt, which reads as "it's just stuck". Users who actually need more
/// than this can still opt in explicitly via `context_size` in Settings.
const DEFAULT_AUTO_CTX_CAP: u32 = 8192;

/// Safety margin subtracted from detected free VRAM before deciding whether
/// a model fits — leaves headroom for the KV cache/context buffers (which
/// scale with context size and aren't accounted for by the model file size
/// alone) and for other GPU consumers (compositor, other apps).
const VRAM_SAFETY_MARGIN_BYTES: u64 = 1536 * 1024 * 1024; // 1.5 GiB

/// Picks a default `n_gpu_layers` for a model the caller hasn't pinned an
/// explicit `gpu_layers` setting for.
///
/// Best-effort only: queries VRAM via `nvidia-smi` (present whenever
/// there's an NVIDIA GPU) and compares the GGUF file's on-disk size
/// against it. When detection isn't possible (no `nvidia-smi`,
/// non-NVIDIA GPU, parse failure, etc.) this falls back to
/// "offload everything" rather than guessing further — a Vulkan
/// backend on a card we can't query is treated the same as before.
///
/// # Design: total VRAM, not free
///
/// The original version of this function compared model size against
/// **free** VRAM (via `memory.free`). That produced a nasty class of
/// bug where a user with a 16 GB card and a 5 GB model would still
/// get dropped to CPU-only because Chrome / the compositor / a
/// previously-loaded model's not-yet-fully-released allocation had
/// eaten a couple of GB of VRAM at the exact moment we checked. The
/// user then saw 4 tok/s and (correctly) asked "why isn't it using
/// the GPU?" — reported directly by a real user with zen-pro-qwen3-8b
/// after they swapped away from Fable-Fusion. Using **total** VRAM
/// instead means we make the decision on the card's fixed capacity,
/// not on the instantaneous busyness of other GPU consumers. If the
/// model doesn't actually fit at load time, llama.cpp itself will
/// raise an OOM (which we surface with the "not enough free VRAM"
/// error path elsewhere in this module), which is a better failure
/// than silently defaulting to slow CPU inference.
fn default_gpu_layers_for(model_path: &Path) -> u32 {
    let Ok(model_size_bytes) = std::fs::metadata(model_path).map(|m| m.len()) else {
        return OFFLOAD_ALL_LAYERS;
    };

    // Prefer the cross-vendor GPU probe: gives us total VRAM on
    // NVIDIA (nvidia-smi), AMD (rocm-smi), any GPU with vulkaninfo,
    // or Linux sysfs — matching whatever the model picker used to
    // show its ⚠️ fit warning.
    let gpu = crate::gpu_info::detect_primary_gpu();

    let Some(total_vram_bytes) = gpu.total_vram_bytes else {
        // No GPU detected at all → nothing to offload to. Signal that
        // downstream rather than lying with OFFLOAD_ALL_LAYERS (which
        // would then hit a Vulkan/CUDA runtime error at load).
        //
        // …except we return OFFLOAD_ALL_LAYERS anyway because on
        // machines without GPU probes installed but *with* a working
        // Vulkan runtime, the previous behaviour was "just try it and
        // see" — matches user expectations from ~all pre-existing
        // installs. Detection failing is not the same as no-GPU.
        tracing::info!(
            "No GPU total-VRAM info available (source={:?}); defaulting to \
             offload-all layers and letting llama.cpp decide.",
            gpu.source
        );
        return OFFLOAD_ALL_LAYERS;
    };

    if model_size_bytes + VRAM_SAFETY_MARGIN_BYTES <= total_vram_bytes {
        // The card is big enough for this model plus a KV cache.
        // Offload everything. If some other app is temporarily
        // squatting on VRAM, llama.cpp will still succeed (falling
        // back to shared memory / swapping GPU allocations) or fail
        // loudly with an OOM, which is a signal to close that other
        // app — either outcome is better than silently doing CPU
        // inference.
        tracing::info!(
            "GPU has {} MiB total VRAM, model is {} MiB — offloading all layers ({} \
             MiB free at check time).",
            total_vram_bytes / (1024 * 1024),
            model_size_bytes / (1024 * 1024),
            gpu.available_vram_bytes.unwrap_or(0) / (1024 * 1024)
        );
        return OFFLOAD_ALL_LAYERS;
    }

    // Model genuinely doesn't fit on this card even with the whole
    // thing to itself. Fall back to CPU-only with a warning that
    // names the numbers so the user knows what to change (smaller
    // model, or explicit partial gpu_layers in Settings).
    tracing::warn!(
        "Model {} is ~{} MiB but the GPU has only ~{} MiB VRAM — defaulting to \
         CPU-only (gpu_layers=0) instead of offloading everything, since it would \
         not fit. Set an explicit \"GPU layers\" value in Settings to force partial \
         GPU offload.",
        model_path.display(),
        model_size_bytes / (1024 * 1024),
        total_vram_bytes / (1024 * 1024)
    );
    0
}

/// Free VRAM in bytes on the first NVIDIA GPU reported by `nvidia-smi`, or
/// `None` if the tool isn't installed / no GPU is reported / its output
/// can't be parsed. No longer used by `default_gpu_layers_for` (which
/// switched to total VRAM via [`crate::gpu_info::detect_primary_gpu`]),
/// but retained for future callers that want the instantaneous free
/// number specifically — e.g. a future retry-on-OOM path that wants to
/// re-measure after a failed load.
#[allow(dead_code)]
fn detect_free_vram_bytes() -> Option<u64> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.free", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let first_line = text.lines().next()?.trim();
    let free_mib: u64 = first_line.parse().ok()?;
    Some(free_mib * 1024 * 1024)
}

/// One-line "used/free VRAM (MiB)" snapshot for debug logging, best-effort
/// via `nvidia-smi`. Unlike [`detect_free_vram_bytes`] this also reports
/// *used* memory, since the VA-space/driver-level crashes this local-LLM
/// path is prone to (see the comment on `.with_op_offload` in `generate()`)
/// aren't always simple "not enough free VRAM" — logging both numbers
/// around every risky step (load, context creation, each decode call) is
/// what lets a crash be correlated against the exact GPU memory state that
/// preceded it, without needing to separately shell out to `nvidia-smi` or
/// dig through `journalctl`/`dmesg` after the fact.
fn vram_snapshot() -> String {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used,memory.free", "--format=csv,noheader,nounits"])
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let first_line = text.lines().next().unwrap_or("").trim();
            format!("vram_used_free_mib=[{first_line}]")
        }
        _ => "vram_used_free_mib=[unavailable]".to_string(),
    }
}

/// Multiplier applied to a GGUF file's on-disk size to estimate the *host
/// RAM* required to load and run it fully on CPU. Loading the raw weights
/// alone would only need ~1x the file size, but llama.cpp additionally
/// needs KV cache buffers (scaling with context size) and per-op compute
/// buffers, and MoE architectures in particular (this exists because of a
/// real incident loading an ~17.3GB Q4_K_M MoE GGUF, which peaked at
/// ~32-34GB resident) can need substantially more scratch space than a
/// dense model of the same file size. This factor is deliberately generous
/// (over-cautious) estimate — better to sometimes refuse a model that
/// would actually have fit than to let the kernel OOM-kill the whole
/// process, which previously happened twice in a row and is not
/// recoverable (unlike a clean `Err` from this function, which the caller
/// already treats as best-effort/non-fatal).
const CPU_RAM_SIZE_FACTOR: f64 = 2.5;

/// Returns `Err` if loading `model_path` fully on CPU is estimated to need
/// more RAM than is currently available, rather than letting the OS decide
/// (via the OOM killer) partway through a multi-gigabyte allocation. Only
/// meaningful when the model will actually run on CPU (`gpu_layers == 0`);
/// GPU-resident layers are already accounted for by `default_gpu_layers_for`
/// / an explicit `gpu_layers` setting, not this check.
fn check_cpu_ram_budget(model_path: &Path) -> Result<()> {
    let Ok(model_size_bytes) = std::fs::metadata(model_path).map(|m| m.len()) else {
        return Ok(()); // Can't stat it; let the real load attempt surface the error.
    };
    let Some(available_bytes) = available_system_ram_bytes() else {
        return Ok(()); // Can't detect (non-Linux, parse failure); proceed as before.
    };

    let required_bytes = (model_size_bytes as f64 * CPU_RAM_SIZE_FACTOR) as u64;
    if required_bytes > available_bytes {
        return Err(CoreError::Other(format!(
            "refusing to load {} fully on CPU: estimated RAM need (~{} MiB, {}x its ~{} MiB \
             file size) exceeds currently available RAM (~{} MiB). Loading anyway risks the \
             OS killing the whole app outright instead of a clean error. Free up RAM, close \
             other applications, or pick a smaller/more quantized model.",
            model_path.display(),
            required_bytes / (1024 * 1024),
            CPU_RAM_SIZE_FACTOR,
            model_size_bytes / (1024 * 1024),
            available_bytes / (1024 * 1024)
        )));
    }
    Ok(())
}

/// Distinguishes `check_cpu_ram_budget`'s "won't fit in RAM" error from any
/// other load failure (missing file, corrupt GGUF, etc.), purely by
/// checking for its known message prefix — there's no dedicated
/// `CoreError` variant for this yet, and adding one is more churn than
/// warranted for a single internal call site. Used to decide whether a
/// fallback-to-smaller-model retry is safe/appropriate: it only ever makes
/// sense when the *reason* was "too big for available RAM", not e.g. "file
/// doesn't exist" (falling back on that would silently mask what's likely
/// a misconfiguration).
const RAM_BUDGET_ERROR_PREFIX: &str = "refusing to load";

fn is_ram_budget_error(e: &CoreError) -> bool {
    matches!(e, CoreError::Other(msg) if msg.starts_with(RAM_BUDGET_ERROR_PREFIX))
}

/// `general.architecture` values whose Gated-Delta-Net (hybrid recurrent
/// memory) layers were known, in an older bundled llama.cpp version, to
/// segfault (SIGSEGV in `ggml_compute_forward_set`, observed twice in
/// practice with a `Qwen3.6` GGUF) rather than fail cleanly. That crash was
/// a genuine upstream llama.cpp graph-splitting bug
/// (`ggml-org/llama.cpp` issue #19864), fixed by PR #19866 (merged
/// 2026-02-24) — confirmed fixed in *our* bundled build too: re-tested
/// 2026-08-05 against the exact `qwen35`-architecture GGUF that crashed
/// before, after bumping `llama-cpp-2`/`llama-cpp-sys-2` 0.1.153 -> 0.1.154
/// (which already carried that fix), with fused Gated Delta Net now
/// reported "enabled" and a full decode+generation round-trip completing
/// cleanly with correct output. [`KNOWN_UNSTABLE_ARCHITECTURES`] is
/// therefore empty for now — see that constant's doc comment for the full
/// re-test writeup — but this check is kept in place (rather than removed)
/// so a future architecture with the same failure mode can be blocked the
/// same way again without re-plumbing anything.
///
/// Defined in `model_library` (not here) and re-exported so the model
/// picker UI's advisory warning (`ModelInfo::unstable_architecture`) and
/// this load-time enforcement always agree on exactly the same list.
use crate::model_library::KNOWN_UNSTABLE_ARCHITECTURES;

fn is_incompatible_architecture_error(e: &CoreError) -> bool {
    matches!(e, CoreError::Other(msg) if msg.contains("known-unstable Gated Delta Net architecture"))
}

/// Checks `model`'s `general.architecture` metadata against
/// [`KNOWN_UNSTABLE_ARCHITECTURES`] and returns a clear, actionable error
/// instead of letting the caller proceed toward a near-certain crash.
/// Currently a no-op in practice since that list is empty (see its doc
/// comment) — kept so the enforcement point already exists if the list
/// needs entries again.
fn check_architecture_compatibility(model: &LlamaModel, model_path: &Path) -> Result<()> {
    let Ok(architecture) = model.meta_val_str("general.architecture") else {
        return Ok(()); // Can't read it; let generation proceed as before.
    };
    if KNOWN_UNSTABLE_ARCHITECTURES.contains(&architecture.as_str()) {
        return Err(CoreError::Other(format!(
            "refusing to load {}: this GGUF uses the '{architecture}' architecture, which has a \
             known-unstable Gated Delta Net architecture in the bundled llama.cpp version — this \
             specific model has previously crashed the whole app (SIGSEGV) during generation, not \
             just failed gracefully. This is an upstream llama.cpp bug (tracked publicly for this \
             architecture family across Vulkan/CUDA/ROCm/SYCL), not something fixable via \
             Grafium's settings. Please use a different model until llama.cpp resolves this, or \
             try a different GGUF conversion of the same model (the crash is triggered by \
             specific tensor naming in how this file was converted).",
            model_path.display()
        )));
    }
    Ok(())
}

/// When the configured chat model can't be used (doesn't fit in available
/// RAM, or has a known-unstable architecture), looks for other
/// already-downloaded LLM-kind models in the same directory that *would*
/// work — largest-fitting first (best quality available) — purely to list
/// as suggestions in the error message [`LocalLlm::from_settings`] returns.
/// Deliberately advisory only: nothing here gets loaded automatically, see
/// [`LocalLlm::from_settings`]'s docs for why silently substituting a
/// different model than the one the user picked is a footgun rather than
/// a convenience.
///
/// Excludes [`model_library::KNOWN_UNSTABLE_ARCHITECTURES`] — suggesting a
/// model that's just as certain to be refused (or worse, crash) wouldn't
/// help the user at all. This is why `unstable_architecture` is computed
/// once in `scan_models_dir` and shared by both this selection logic and
/// the Settings model-picker UI's warning icon, rather than being
/// duplicated.
fn find_suggested_llm_models(models_dir: &Path, exclude: &Path) -> Vec<std::path::PathBuf> {
    let Ok(mut candidates) = model_library::scan_models_dir(models_dir) else {
        return Vec::new();
    };
    candidates.retain(|m| m.kind == ModelKind::Llm && m.path != exclude && !m.unstable_architecture);
    candidates.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    candidates
        .into_iter()
        .filter(|m| check_cpu_ram_budget(&m.path).is_ok())
        .map(|m| m.path)
        .collect()
}

/// Renders [`find_suggested_llm_models`]'s result into the human-readable
/// tail appended to [`LocalLlm::from_settings`]'s error, so the message
/// doesn't just say "this model doesn't work" but also "...and here's what
/// does, pick one in Settings".
fn format_model_suggestions(suggestions: &[std::path::PathBuf]) -> String {
    if suggestions.is_empty() {
        return "No other already-downloaded model in this folder currently fits either — \
                download a smaller/more quantized GGUF, or free up RAM/VRAM, then try again."
            .to_string();
    }
    let list = suggestions
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .map(|name| format!("  • {name}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Other already-downloaded models that should work on this machine instead — pick one \
         in Settings \u{2192} AI / Knowledge Engine:\n{list}"
    )
}

/// Currently available system RAM in bytes (`MemAvailable` from
/// `/proc/meminfo` on Linux — already accounts for reclaimable page cache,
/// unlike `MemFree`). `None` on other platforms or if parsing fails.
fn available_system_ram_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
        for line in contents.lines() {
            if let Some(rest) = line.strip_prefix("MemAvailable:") {
                let kib: u64 = rest.trim().trim_end_matches(" kB").trim().parse().ok()?;
                return Some(kib * 1024);
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Runs a GGUF model fully in-process via llama.cpp. Stateless per call
/// beyond the loaded model, so one instance can be reused across many
/// `complete()` calls (avoids re-loading the model each time — same
/// reasoning as `WhisperTranscriber`).
///
/// `backend`/`model` are wrapped in `Arc` (not owned directly) purely so
/// `complete()` can move cheap handles into a `tokio::task::spawn_blocking`
/// closure — llama.cpp inference is synchronous CPU/GPU-bound work and must
/// never run directly on the async executor.
pub struct LocalLlm {
    backend: Arc<LlamaBackend>,
    model: Arc<LlamaModel>,
    ctx_size: NonZeroU32,
    name: String,
}


/// The process-wide llama.cpp backend. llama.cpp only wants to be
/// initialized once; every `LocalLlm` instance shares the same handle
/// rather than each `load()` call re-initializing it — see
/// `llama_shared::shared_backend`, also used by `LocalEmbedder` so both can
/// be in use in the same process at once.

impl LocalLlm {
    /// Loads a GGUF model from `model_path`.
    ///
    /// `context_size` overrides the model's own trained context length;
    /// `gpu_layers` controls how many transformer layers to offload to the
    /// GPU (only meaningful when built with `llm-local-vulkan` — otherwise
    /// there's no GPU backend to offload to, so this is a harmless no-op).
    /// `None` for either means "use a sensible default" (the model's
    /// trained context length capped at `DEFAULT_AUTO_CTX_CAP`, and
    /// "offload everything", respectively).
    pub fn load(
        model_path: &Path,
        context_size: Option<u32>,
        gpu_layers: Option<u32>,
        use_mmap: Option<bool>,
    ) -> Result<Self> {
        let backend = shared_backend();

        // Only one load attempt is ever made — deliberately NOT "try GPU,
        // retry fully-on-CPU on failure": llama.cpp/ggml gives no guarantee
        // that a failed load releases whatever buffers it *did* manage to
        // allocate before hitting the fatal one, so a retry-after-failure
        // pattern here can leak the first attempt's memory and then
        // allocate the *entire* model again for the second attempt — for
        // an ~18GB model that's enough to exhaust RAM+swap and get the
        // whole process OOM-killed by the kernel (observed in practice).
        // Instead, when the caller hasn't pinned an explicit `gpu_layers`,
        // proactively estimate whether the model can plausibly fit in free
        // VRAM at all and decide up front, so we only ever allocate once.
        let requested_gpu_layers =
            gpu_layers.unwrap_or_else(|| default_gpu_layers_for(model_path));

        if requested_gpu_layers == 0 {
            check_cpu_ram_budget(model_path)?;
        }

        let model_params = LlamaModelParams::default().with_n_gpu_layers(requested_gpu_layers);

        // Critical: llama.cpp defaults to `use_mmap(true)`, which makes
        // `load_from_file` return almost immediately without actually
        // reading the weights into RAM — pages are faulted in lazily,
        // *later*, as generation touches each tensor (worse still for a
        // MoE model like this, where different experts get paged in over
        // the course of a run). That laziness is exactly what defeated
        // `check_cpu_ram_budget` above in practice: it observed a stale,
        // too-early snapshot of "available RAM" that had drifted by the
        // time the real memory pressure hit, minutes later, during the
        // token-generation loop. Forcing an eager (non-mmap) read for
        // CPU-only loads makes the full cost of the model paid for, and
        // checked, right here in one shot, immediately after the check
        // above — closing that gap between "we checked" and "we actually
        // used the memory". GPU-resident loads keep mmap enabled (its
        // laziness/backing-store behavior only concerns host RAM, not
        // VRAM, so it's not part of this specific hazard).
        //
        // An explicit `use_mmap` override (`Some(true)` / `Some(false)`)
        // always wins over that heuristic — used by the process wrapper
        // to force mmap off after a SIGBUS-flavored worker crash so the
        // follow-up request stops crashing, and by the Settings UI so
        // users on unreliable storage (removable drives, network mounts)
        // can pin the safer behavior. `None` falls back to the
        // GPU-vs-CPU heuristic above.
        let use_mmap_effective = use_mmap.unwrap_or(requested_gpu_layers != 0);
        let model_params = model_params.with_use_mmap(use_mmap_effective);

        // NOTE: an OS-level `RLIMIT_AS` hard ceiling was also tried here as
        // a last-resort safety net (in case the estimate above is still
        // wrong), but was reverted: when llama.cpp/ggml's own allocator
        // hits ENOMEM under a tightened `RLIMIT_AS`, it does not surface a
        // clean `Result::Err` the way the Vulkan/GPU OOM path does — it
        // segfaults (confirmed empirically: the process exited with
        // SIGSEGV, code 139, the moment the self-imposed ceiling was hit).
        // That's no safer than the kernel's own OOM killer, so the size
        // estimate above (`check_cpu_ram_budget`) — now much more reliable
        // since `use_mmap(false)` removes the drift window — is the only
        // gate; if it's ever wrong, prefer lowering `CPU_RAM_SIZE_FACTOR`'s
        // safety margin further rather than reintroducing a hard rlimit.
        tracing::info!(
            "loading LLM {} (requested_gpu_layers={requested_gpu_layers}, use_mmap={use_mmap_effective}) — {}",
            model_path.display(),
            vram_snapshot()
        );
        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|e| CoreError::Other(format!("failed to load LLM model: {e}")))?;
        tracing::info!(
            "loaded LLM {} — {}",
            model_path.display(),
            vram_snapshot()
        );

        // Check *before* any generation is attempted: some architectures'
        // GGUF conversions crash the whole app (SIGSEGV) partway through
        // the very first prompt decode — see `check_architecture_compatibility`.
        // Catching this here, right after the weights are already loaded,
        // is the earliest point the architecture is knowable, and still
        // well before the crash-prone code path (context creation/decode)
        // ever runs.
        check_architecture_compatibility(&model, model_path)?;

        let ctx_size = context_size
            .filter(|&n| n > 0)
            .or_else(|| {
                Some(model.n_ctx_train())
                    .filter(|&n| n > 0)
                    .map(|n| n.min(DEFAULT_AUTO_CTX_CAP))
            })
            .and_then(NonZeroU32::new)
            .unwrap_or_else(|| NonZeroU32::new(DEFAULT_CTX_SIZE).expect("nonzero constant"));

        let name = model_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("local-llm")
            .to_string();

        Ok(Self {
            backend,
            model: Arc::new(model),
            ctx_size,
            name,
        })
    }

    /// Settings-driven alternative to [`Self::load`]: resolves which GGUF
    /// file to use via the shared [`model_library`] (through
    /// `settings.model_ref`) instead of requiring an exact path up front —
    /// mirrors `WhisperTranscriber::from_settings` exactly. This is what
    /// makes "download a model from Hugging Face, put it in the models
    /// folder, it just works" apply identically to LLMs as it already does
    /// to Whisper.
    ///
    /// Deliberately does **not** silently substitute a different model
    /// when the configured one doesn't fit in available RAM or has a
    /// known-unstable architecture — an earlier version of this function
    /// did exactly that, and it's a footgun: the user picks (and believes
    /// they're using) one model, but every response actually comes from a
    /// different one they never chose, with no visible indication
    /// anywhere that a swap happened. Instead this returns a clear error
    /// naming the actual problem and, via [`find_suggested_llm_models`],
    /// listing other already-downloaded models that would work — so the
    /// user makes an informed choice in Settings rather than unknowingly
    /// talking to the wrong model.
    pub fn from_settings(models_dir: &Path, settings: &LocalLlmSettings) -> Result<Self> {
        let model_path = settings.model_ref.resolve(models_dir, ModelKind::Llm)?;
        match Self::load(
            &model_path,
            settings.context_size,
            settings.gpu_layers,
            settings.use_mmap,
        ) {
            Ok(llm) => Ok(llm),
            Err(e) if is_ram_budget_error(&e) || is_incompatible_architecture_error(&e) => {
                let suggestions = find_suggested_llm_models(models_dir, &model_path);
                Err(CoreError::Other(format!(
                    "{e}\n\n{}",
                    format_model_suggestions(&suggestions)
                )))
            }
            Err(e) => Err(e),
        }
    }

    /// Same as [`Self::from_settings`], but takes the whole
    /// [`crate::ai::config::AiConfig`] + app data dir instead of
    /// pre-extracted fields — the shape a caller loading settings straight
    /// from disk (e.g. the Tauri command layer, mirroring
    /// `ai_get_config`/`ai_set_config`) will actually have on hand.
    pub fn from_config(config: &crate::ai::config::AiConfig, data_dir: &Path) -> Result<Self> {
        let local = config
            .local
            .as_ref()
            .ok_or_else(|| CoreError::Other("No local AI provider configured".to_string()))?;
        let models_dir = local
            .models_dir
            .clone()
            .unwrap_or_else(|| model_library::default_models_dir(data_dir));
        Self::from_settings(&models_dir, &local.local_llm)
    }

    /// Load the configured chat model with every layer forced onto the GPU.
    ///
    /// Used where a slow CPU fallback is worse than a clear failure — the
    /// analysis passes would otherwise appear to hang rather than report that
    /// the model does not fit.
    pub fn from_config_forcing_gpu(
        config: &crate::ai::config::AiConfig,
        data_dir: &Path,
    ) -> Result<Self> {
        let local = config
            .local
            .as_ref()
            .ok_or_else(|| CoreError::Other("No local AI provider configured".to_string()))?;
        let models_dir = local
            .models_dir
            .clone()
            .unwrap_or_else(|| model_library::default_models_dir(data_dir));
        let model_path = local
            .local_llm
            .model_ref
            .resolve(&models_dir, ModelKind::Llm)?;
        Self::load(
            &model_path,
            local.local_llm.context_size,
            Some(OFFLOAD_ALL_LAYERS),
            local.local_llm.use_mmap,
        )
    }
}

impl LlmProvider for LocalLlm {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        let model = self.model.clone();
        let backend = self.backend.clone();
        let ctx_size = self.ctx_size;
        let messages = messages.to_vec();
        let options = options.clone();

        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let prompt = build_chat_prompt(&model, &messages, &options)?;
                generate(&model, &backend, ctx_size, &prompt, &options, None)
            })
            .await
            .map_err(|e| CoreError::Other(format!("LLM inference task panicked: {e}")))?
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn complete_stream<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
        on_token: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<String>> {
        let model = self.model.clone();
        let backend = self.backend.clone();
        let ctx_size = self.ctx_size;
        let messages = messages.to_vec();
        let options = options.clone();

        Box::pin(async move {
            // `generate()` runs on a blocking thread (llama.cpp is
            // synchronous), so pieces are handed back here through a
            // channel rather than calling `on_token` directly from that
            // thread — `on_token` is an arbitrary `&mut` closure the caller
            // owns, and this keeps it running only on this async task.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

            let generation = tokio::task::spawn_blocking(move || {
                let prompt = build_chat_prompt(&model, &messages, &options)?;
                generate(&model, &backend, ctx_size, &prompt, &options, Some(&tx))
            });

            let forward_tokens = async {
                while let Some(piece) = rx.recv().await {
                    on_token(&piece);
                }
            };

            let (result, ()) = tokio::join!(generation, forward_tokens);
            result.map_err(|e| CoreError::Other(format!("LLM inference task panicked: {e}")))?
        })
    }

    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        // The model is loaded fully in-process at construction time — if
        // `LocalLlm::load`/`from_config` succeeded, it's ready by definition
        // (no network endpoint to probe, unlike Ollama/OpenAI-compatible).
        Box::pin(async move { Ok(true) })
    }
}

/// Formats a conversation (`options.system_prompt` + `messages`) using the
/// model's own baked-in chat template, falling back to the widely-supported
/// "chatml" template if the model doesn't ship one. Keeping this as one
/// shared function (rather than inlining it in `complete()`) is what lets
/// any future entry point — e.g. a streaming variant — reuse the exact same
/// prompt formatting instead of re-deriving it.
fn build_chat_prompt(
    model: &LlamaModel,
    messages: &[ChatMessage],
    options: &CompletionOptions,
) -> Result<String> {
    let mut chat = Vec::with_capacity(messages.len() + 1);
    if let Some(system) = &options.system_prompt {
        chat.push(new_chat_message("system", system)?);
    }
    for message in messages {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };
        chat.push(new_chat_message(role, &message.content)?);
    }

    let template = match model.chat_template(None) {
        Ok(template) => template,
        Err(_) => LlamaChatTemplate::new("chatml").map_err(|e| {
            CoreError::Other(format!("failed to build fallback chat template: {e}"))
        })?,
    };

    let raw_prompt = model
        .apply_chat_template(&template, &chat, true)
        .map_err(|e| CoreError::Other(format!("failed to apply chat template: {e}")))?;

    // Strip an auto-injected `<think>...` opener at the *tail* of the
    // applied template. Qwen3.6's default chat template (and other newer
    // "hybrid reasoning" templates) unconditionally prepend `<think>\n`
    // to the assistant turn — the Jinja is roughly:
    //     ...<|im_start|>assistant\n{% if enable_thinking %}<think>\n{% endif %}
    // and `enable_thinking` defaults to `true`. llama.cpp's older
    // template-string-only `llama_chat_apply_template` API (the one
    // llama-cpp-rs binds) has no way to pass `enable_thinking: false`
    // as a Jinja variable, so the `<think>` block gets baked into every
    // prompt whether we want it or not.
    //
    // For aggressively-quantized creative-writing fine-tunes (e.g. the
    // Fable-Fusion Qwen3.6 IQ2_M user-report that motivated this),
    // reasoning-mode training is often damaged — the model emits
    // `<think>` (already in the prompt scaffolding) and then either
    // hits EOS immediately or generates whitespace that never resolves
    // the tag. The whole response comes back as literally the string
    // `<think>` and every downstream parser (JSON, plain-text fallback,
    // ...) collapses to "no summary".
    //
    // Fix: rewrite the prompt so it ends *without* the auto-injected
    // `<think>`. Then the model just generates a normal assistant reply
    // from a clean `<|im_start|>assistant\n` opening, and even a
    // reasoning-broken fine-tune produces usable output.
    let prompt = strip_trailing_think_prompt(&raw_prompt);

    if tracing::enabled!(tracing::Level::INFO) {
        let tail_len = 240usize.min(prompt.len());
        let tail_start = prompt.len().saturating_sub(tail_len);
        tracing::info!(
            target: "grafium_core::ai::providers::local_llm",
            "build_chat_prompt: len={} stripped_think={} tail={:?}",
            prompt.len(),
            prompt.len() != raw_prompt.len(),
            &prompt[tail_start..],
        );
    }

    Ok(prompt)
}

/// Returns `input` with any trailing `<think>...` opener the chat
/// template baked in stripped off. Preserves everything before the
/// `<think>` tag verbatim. If no trailing `<think>` (with only optional
/// whitespace after it) is found, returns the input unchanged.
///
/// Handles both variants observed in the wild:
/// - `...<|im_start|>assistant\n<think>\n\n`
/// - `...<|im_start|>assistant\n<think>`
/// - `...<|im_start|>assistant\n<thinking>\n`
///
/// Anything else after `<think>` (e.g. a closed reasoning block ending
/// in `</think>`) is treated as legitimate content and left alone —
/// only *unclosed* trailing think-openers get removed.
fn strip_trailing_think_prompt(input: &str) -> String {
    let trimmed_end = input.trim_end_matches(|c: char| c.is_whitespace());
    for open in ["<think>", "<thinking>"] {
        if let Some(before) = trimmed_end.strip_suffix(open) {
            // Only strip if this is really the assistant-turn opener,
            // i.e. it isn't preceded by a *closing* `</think>` (which
            // would mean the template already emitted a closed
            // reasoning block and we shouldn't touch anything).
            if !before.ends_with("</think>") && !before.ends_with("</thinking>") {
                return before.to_string();
            }
        }
    }
    input.to_string()
}

fn new_chat_message(role: &str, content: &str) -> Result<LlamaChatMessage> {
    LlamaChatMessage::new(role.to_string(), content.to_string())
        .map_err(|e| CoreError::Other(format!("invalid chat message: {e}")))
}

/// Builds the sampler chain from `CompletionOptions.temperature`: greedy
/// (fully deterministic) at `0.0`/unset, otherwise the standard
/// top-k/top-p/temperature/distribution chain llama.cpp examples use.
/// Extracted as its own function so it's obvious this is the *only* place
/// sampling policy is decided — nothing else should construct a
/// `LlamaSampler` by hand.
fn build_sampler(options: &CompletionOptions) -> LlamaSampler {
    const SEED: u32 = 1234;
    match options.temperature {
        Some(t) if t > 0.0 => LlamaSampler::chain_simple([
            LlamaSampler::top_k(40),
            LlamaSampler::top_p(0.95, 1),
            LlamaSampler::temp(t),
            LlamaSampler::dist(SEED),
        ]),
        _ => LlamaSampler::chain_simple([LlamaSampler::greedy()]),
    }
}

/// Rewrites the low-level `llama_new_context_with_model` failure — most
/// often reported by the underlying binding as the bare string "null
/// reference from llama.cpp" — into a user-actionable explanation.
/// llama.cpp returns NULL from this call whenever the KV cache + compute
/// buffer at the requested `n_ctx`/`n_batch` won't fit in whatever
/// backend (Vulkan/CUDA VRAM, or host RAM on CPU-only) the model is
/// running on, without distinguishing "backend allocator refused" from
/// any other init failure — the bare message alone is next-to-useless.
/// Sharing this one shaper between `local_llm::generate` and
/// `local_embedder::embed_all` keeps the two paths' error phrasing
/// identical so the surrounding chunking/error-handling logic doesn't
/// have to string-match two subtly different messages.
pub(crate) fn context_creation_error_message(
    raw: &str,
    ctx_size: u32,
    n_batch: u32,
    prompt_tokens: usize,
) -> String {
    // Include the most recent llama.cpp / GGML log lines verbatim — the
    // binding's "null reference from llama.cpp" alone hides what
    // actually went wrong (e.g. "ggml_vulkan: allocation failed", "no
    // Vulkan device available"). Cap at the last ~30 seconds so this
    // catches the current context-creation attempt without swallowing
    // logs from a totally different operation. The cutoff intentionally
    // predates the caller — we can't take an `Instant::now()` here
    // without changing the call sites' signatures, so we look back a
    // reasonable window.
    let cutoff = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(30))
        .unwrap_or_else(std::time::Instant::now);
    let backend_log =
        crate::log_tap::snapshot_since_targets(cutoff, &["llama", "ggml"]);
    let details = if backend_log.is_empty() {
        String::new()
    } else {
        let lines = backend_log
            .iter()
            .map(|ev| format!("  [{:?} {}] {}", ev.level, ev.target, ev.message))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n\nllama.cpp / GGML log:\n{lines}")
    };
    format!(
        "failed to create llama.cpp inference context ({raw}). This almost always \
         means the requested context/batch size didn't fit in available GPU (or CPU) \
         memory. Context size {ctx_size}, batch size {n_batch}, prompt tokens \
         {prompt_tokens}. Try (a) closing other apps that are using VRAM, (b) lowering \
         the local LLM \"context_size\" or \"GPU layers\" in Settings, or (c) picking \
         a smaller/more quantized model.{details}"
    )
}

/// The single tokenize → batch → decode → sample loop every completion
/// (chat-formatted or, in the future, raw) goes through — this is the
/// manual generation loop `llama-cpp-2` requires (it has no high-level
/// "generate" helper), written once here rather than duplicated per caller.
fn generate(
    model: &LlamaModel,
    backend: &LlamaBackend,
    ctx_size: NonZeroU32,
    prompt: &str,
    options: &CompletionOptions,
    on_token: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
) -> Result<String> {
    // Tokenize *before* building the context: `str_to_token` only needs the
    // model, not a context, and knowing the real prompt length up front is
    // what lets `n_batch`/`n_ubatch` below be sized off actual usage
    // instead of the worst case.
    let tokens = model
        .str_to_token(prompt, AddBos::Always)
        .map_err(|e| CoreError::Other(format!("failed to tokenize prompt: {e}")))?;

    let n_ctx = ctx_size.get() as i32;
    if tokens.len() as i32 >= n_ctx {
        return Err(CoreError::Other(format!(
            "prompt ({} tokens) exceeds the context window ({n_ctx} tokens) — shorten the input \
             or increase `context_size` in local LLM settings",
            tokens.len()
        )));
    }

    let n_threads = std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4);
    // `n_batch`/`n_ubatch` control the size of the one-shot compute buffer
    // llama.cpp reserves at context-creation time — *not* how many tokens
    // actually get processed. Previously this was pinned to the *full*
    // context size (`ctx_size`) unconditionally, on the theory that
    // llama.cpp's decode() asserts the whole batch fits within `n_batch`
    // (true — a single-shot prompt longer than 2048 tokens, easy to hit
    // once page content is included, would otherwise crash the whole
    // process instead of returning an error). But that means even a
    // trivial few-hundred-token prompt reserved a compute buffer sized for
    // the worst case (e.g. an 8192-token context), which measured multiple
    // GiB of VRAM/RAM regardless of the actual prompt — plenty to push a
    // "fits at load time" model past 100% VRAM the moment a real
    // completion runs, failing (or on some allocators, crashing) context
    // creation. Sizing the batch off the *real* tokenized prompt length
    // (with a small floor for very short prompts, still capped at
    // `ctx_size`) keeps the "whole prompt always fits in one batch"
    // guarantee while no longer over-provisioning by 10x or more for
    // ordinary-sized inputs.
    let n_batch_size = (tokens.len() as u32 + 1)
        .max(512.min(ctx_size.get()))
        .min(ctx_size.get());
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(Some(ctx_size))
        .with_n_batch(n_batch_size)
        .with_n_ubatch(n_batch_size)
        .with_n_threads(n_threads)
        .with_n_threads_batch(n_threads)
        // `op_offload`/`offload_kqv` default to `true` in llama.cpp: even
        // with zero *model* layers assigned to the GPU, it will still
        // opportunistically schedule individual compute ops on whatever
        // GPU backend is available, since that's normally a speed win.
        // Disabled unconditionally as a first attempt at avoiding a Vulkan/
        // NVIDIA-driver-level crash seen on this machine (`NVRM:
        // dmaAllocMapping_GM107: can't alloc VA space for mapping` /
        // `NV_ERR_NO_MEMORY`, surfacing as a SIGSEGV in
        // `ggml_compute_forward_set` during `llama_decode` instead of a
        // graceful error) — but the same crash was later reproduced *again*
        // with both already disabled, so this was NOT the (or not the
        // only) root cause. Left disabled anyway since it's a genuine
        // stability/correctness hazard in its own right (llama.cpp does
        // not check the host-visible-mapping failure before writing
        // through the resulting invalid pointer), but do not assume this
        // alone explains any future crash with the same signature — see
        // the `vram_snapshot()` logging around context creation/decode
        // below, added specifically to narrow down what's actually
        // triggering the VA-space exhaustion.
        .with_op_offload(false)
        .with_offload_kqv(false);

    tracing::info!(
        "creating llama context: ctx_size={} n_batch={n_batch_size} prompt_tokens={} — {}",
        ctx_size.get(),
        tokens.len(),
        vram_snapshot()
    );
    let mut ctx = model.new_context(backend, ctx_params).map_err(|e| {
        CoreError::Other(context_creation_error_message(
            &e.to_string(),
            ctx_size.get(),
            n_batch_size,
            tokens.len(),
        ))
    })?;
    tracing::info!("llama context created — {}", vram_snapshot());

    let max_new_tokens = options.max_tokens.unwrap_or(1024) as i32;

    let mut batch = LlamaBatch::new(tokens.len().max(512) + 1, 1);
    let last_index = tokens.len() as i32 - 1;
    for (i, token) in tokens.iter().enumerate() {
        let is_last = i as i32 == last_index;
        batch
            .add(*token, i as i32, &[0], is_last)
            .map_err(|e| CoreError::Other(format!("failed to queue prompt token: {e}")))?;
    }
    tracing::info!(
        "decoding prompt: {} tokens — {}",
        tokens.len(),
        vram_snapshot()
    );
    ctx.decode(&mut batch)
        .map_err(|e| CoreError::Other(format!("llama.cpp decode of the prompt failed: {e}")))?;
    tracing::info!("prompt decode done — {}", vram_snapshot());

    let mut sampler = build_sampler(options);
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut output = String::new();
    let mut n_cur = batch.n_tokens();
    let stop_at_token = n_cur + max_new_tokens;

    while n_cur < stop_at_token && n_cur < n_ctx {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(token);

        if model.is_eog_token(token) {
            break;
        }

        let piece = model
            .token_to_piece(token, &mut decoder, true, None)
            .map_err(|e| CoreError::Other(format!("failed to decode generated token: {e}")))?;
        // Best-effort: if the receiving end was dropped (caller stopped
        // listening), keep generating rather than aborting on a send error.
        if let Some(tx) = on_token {
            let _ = tx.send(piece.clone());
        }
        output.push_str(&piece);

        if let Some(hit_len) = options
            .stop
            .as_ref()
            .and_then(|stops| stops.iter().find(|s| output.ends_with(s.as_str())))
            .map(|s| s.len())
        {
            output.truncate(output.len() - hit_len);
            break;
        }

        batch.clear();
        batch
            .add(token, n_cur, &[0], true)
            .map_err(|e| CoreError::Other(format!("failed to queue generated token: {e}")))?;
        n_cur += 1;
        // Logged every step (not throttled): a SIGSEGV in the native
        // llama.cpp/ggml code `ctx.decode()` calls into kills the process
        // instantly with no chance for Rust to unwind or log anything
        // *after* the fact, so the only way to know which decode call
        // actually died is to have logged its position immediately before
        // calling it. `tracing`'s default writer flushes per line, so this
        // reliably lands in the log file even when the very next line is
        // the crash itself.
        tracing::debug!("decode step: n_cur={n_cur} — {}", vram_snapshot());
        ctx.decode(&mut batch)
            .map_err(|e| CoreError::Other(format!("llama.cpp decode failed: {e}")))?;
    }

    Ok(output.trim().to_string())
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use crate::ai::config::{AiConfig, LocalConfig, ProviderType};

    /// `from_config` must resolve the model against `LocalConfig::models_dir`
    /// when it's set, rather than always falling back to
    /// `<data_dir>/models` — this is what lets a user point Grafium at a
    /// models folder shared with other apps (e.g. `~/Documents/models`)
    /// instead of duplicating multi-gigabyte GGUF files into Grafium's own
    /// data directory. We don't need a real model file to prove this: the
    /// "no model found" error message names the directory that was
    /// actually searched, so asserting on that message is enough.
    #[test]
    fn from_config_honors_models_dir_override_over_default_data_dir() {
        let data_dir = tempfile::tempdir().unwrap();
        let custom_models_dir = tempfile::tempdir().unwrap();

        let mut ai_config = AiConfig::default();
        ai_config.local = Some(LocalConfig {
            provider: ProviderType::HuggingFace,
            models_dir: Some(custom_models_dir.path().to_path_buf()),
            ..LocalConfig::default()
        });

        let err = LocalLlm::from_config(&ai_config, data_dir.path())
            .map(|_| ()).unwrap_err();

        let message = err.to_string();
        assert!(
            message.contains(&custom_models_dir.path().display().to_string()),
            "expected error to name the configured models_dir ({}), got: {message}",
            custom_models_dir.path().display()
        );
        assert!(
            !message.contains(&data_dir.path().join("models").display().to_string()),
            "must not fall back to the default data_dir/models path when an override is set, got: {message}"
        );
    }

    #[test]
    fn from_config_falls_back_to_default_models_dir_when_unset() {
        let data_dir = tempfile::tempdir().unwrap();

        let mut ai_config = AiConfig::default();
        ai_config.local = Some(LocalConfig {
            provider: ProviderType::HuggingFace,
            models_dir: None,
            ..LocalConfig::default()
        });

        let err = LocalLlm::from_config(&ai_config, data_dir.path())
            .map(|_| ()).unwrap_err();

        let default_dir = data_dir.path().join("models");
        assert!(
            err.to_string().contains(&default_dir.display().to_string()),
            "expected default models_dir ({}) in error, got: {err}",
            default_dir.display()
        );
    }

    /// `find_suggested_llm_models` is what powers the "here's what would \
    /// work instead" part of the error `LocalLlm::from_settings` returns
    /// when the configured/auto-picked model is too large for available
    /// RAM (e.g. a user picked a 30B model but only has enough free RAM
    /// for a 4B one): it should skip the excluded (too-large) model and
    /// list LLM-kind models largest-first, only including ones that pass
    /// the RAM budget check, ignoring non-LLM files (embeddings) entirely.
    #[test]
    fn find_suggested_llm_models_prefers_largest_model_that_still_fits_ram_budget() {
        let dir = tempfile::tempdir().unwrap();

        let too_big = dir.path().join("huge-model.gguf");
        // Sparse file: reports a real (enormous) size via metadata without
        // actually using that much disk space, guaranteeing it exceeds
        // whatever RAM `check_cpu_ram_budget` sees on the test machine.
        std::fs::File::create(&too_big)
            .unwrap()
            .set_len(10 * 1024 * 1024 * 1024 * 1024) // 10 TiB
            .unwrap();

        let fits_larger = dir.path().join("qwen-7b-instruct.gguf");
        std::fs::write(&fits_larger, vec![0u8; 4096]).unwrap();
        let fits_smaller = dir.path().join("qwen-4b-instruct.gguf");
        std::fs::write(&fits_smaller, vec![0u8; 1024]).unwrap();
        // Not an LLM at all — must never be suggested as a chat-model
        // replacement.
        let embedding = dir.path().join("nomic-embed-text.gguf");
        std::fs::write(&embedding, vec![0u8; 8192]).unwrap();

        let suggestions = find_suggested_llm_models(dir.path(), &too_big);
        assert_eq!(suggestions, vec![fits_larger, fits_smaller]);
    }

    #[test]
    fn find_suggested_llm_models_returns_empty_when_nothing_else_fits() {
        let dir = tempfile::tempdir().unwrap();
        let only_model = dir.path().join("qwen-2b-instruct.gguf");
        std::fs::write(&only_model, vec![0u8; 1024]).unwrap();

        assert_eq!(
            find_suggested_llm_models(dir.path(), &only_model),
            Vec::<std::path::PathBuf>::new()
        );
    }

    /// `LocalLlm::from_settings` must never silently substitute a
    /// different model than the one configured — it should return an
    /// error naming the actual problem, with the *file names* of
    /// alternatives in the message text (not load one automatically).
    #[test]
    fn from_settings_never_silently_substitutes_a_different_model() {
        let dir = tempfile::tempdir().unwrap();

        let too_big = dir.path().join("huge-model.gguf");
        std::fs::File::create(&too_big)
            .unwrap()
            .set_len(10 * 1024 * 1024 * 1024 * 1024) // 10 TiB
            .unwrap();
        let smaller = dir.path().join("qwen-4b-instruct.gguf");
        std::fs::write(&smaller, vec![0u8; 1024]).unwrap();

        let settings = LocalLlmSettings {
            model_ref: model_library::LocalModelRef::named("huge-model.gguf"),
            ..LocalLlmSettings::default()
        };
        let err = LocalLlm::from_settings(dir.path(), &settings)
            .map(|_| ())
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("huge-model.gguf"), "got: {msg}");
        assert!(msg.contains("qwen-4b-instruct.gguf"), "got: {msg}");
    }

    /// The user-facing context-creation error must surface *any* recent
    /// llama.cpp/GGML log lines verbatim — that's the entire point of
    /// the log_tap wiring. Without this, the message reduces to a bare
    /// "null reference from llama.cpp" with the actual VRAM/allocator
    /// diagnostic swallowed. Exercises the tap → message pipeline
    /// end-to-end without needing an actual failed llama_new_context call.
    #[test]
    fn context_creation_error_message_includes_recent_llama_ggml_log_lines() {
        crate::log_tap::record(
            crate::log_tap::TapLevel::Error,
            "ggml_vulkan",
            "ggml_vulkan: allocation failed (out of device memory)",
        );
        let msg = super::context_creation_error_message(
            "null reference from llama.cpp",
            8192,
            1024,
            5000,
        );
        assert!(
            msg.contains("null reference from llama.cpp"),
            "expected the raw binding error to be preserved, got: {msg}"
        );
        assert!(
            msg.contains("ggml_vulkan: allocation failed"),
            "expected the recent GGML log line to be embedded, got: {msg}"
        );
    }

    /// Qwen3.6's chat template auto-appends `<think>\n\n` to the
    /// assistant turn opening — see the big comment on
    /// `strip_trailing_think_prompt` for why we strip it. Confirms both
    /// the "\n\n" and bare variants get cleaned so the model isn't
    /// forced to start every response inside a reasoning tag.
    #[test]
    fn strip_trailing_think_prompt_removes_qwen3_style_auto_opener() {
        let raw =
            "<|im_start|>system\nYou are helpful<|im_end|>\n<|im_start|>user\nHi<|im_end|>\n<|im_start|>assistant\n<think>\n\n";
        let out = super::strip_trailing_think_prompt(raw);
        assert!(
            out.ends_with("<|im_start|>assistant\n"),
            "expected clean assistant turn ending, got tail: {:?}",
            &out[out.len().saturating_sub(80)..]
        );
        assert!(!out.contains("<think>"), "should not leak <think>");
    }

    #[test]
    fn strip_trailing_think_prompt_removes_bare_think() {
        let raw =
            "<|im_start|>assistant\n<think>";
        let out = super::strip_trailing_think_prompt(raw);
        assert_eq!(out, "<|im_start|>assistant\n");
    }

    #[test]
    fn strip_trailing_think_prompt_removes_thinking_alias() {
        let raw =
            "<|im_start|>assistant\n<thinking>\n";
        let out = super::strip_trailing_think_prompt(raw);
        assert_eq!(out, "<|im_start|>assistant\n");
    }

    #[test]
    fn strip_trailing_think_prompt_leaves_closed_reasoning_alone() {
        // The template has already emitted a closed `<think>...</think>`
        // block from a prior assistant turn — that's real content, not
        // an auto-injected opener. Must not strip it (would corrupt the
        // conversation history).
        let raw = "<|im_start|>assistant\n<think>reasoned</think>\nActual answer<|im_end|>\n<|im_start|>assistant\n";
        let out = super::strip_trailing_think_prompt(raw);
        assert_eq!(out, raw);
    }

    #[test]
    fn strip_trailing_think_prompt_is_a_no_op_for_normal_prompts() {
        let raw = "<|im_start|>user\nHi<|im_end|>\n<|im_start|>assistant\n";
        let out = super::strip_trailing_think_prompt(raw);
        assert_eq!(out, raw);
    }
}

