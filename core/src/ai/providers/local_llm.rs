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
use crate::ai::gpu_fit::{
    self, detect_free_vram_bytes, detect_free_vram_bytes_best, vram_safety_margin_bytes,
    VRAM_SAFETY_MARGIN_MAX_BYTES, VRAM_SAFETY_MARGIN_MIN_BYTES,
};
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

/// Outcome of the GPU-offload decision, carried alongside the chosen
/// `gpu_layers` so the load path can report *why* it landed on CPU vs GPU
/// (surfaced to the UI via [`crate::ai::traits::AcceleratorStatus`]).
struct GpuDecision {
    gpu_layers: u32,
    free_vram_bytes: Option<u64>,
    model_size_bytes: Option<u64>,
}

/// Adapts the shared [`gpu_fit`] verdict onto llama.cpp's `n_gpu_layers`
/// knob. The fit arithmetic itself deliberately lives in `gpu_fit` so that
/// Settings' model picker warns using the *same* rule this loader applies —
/// see that module's docs for why they must not diverge.
fn gpu_layers_for_vram(model_size_bytes: u64, free_vram_bytes: u64) -> u32 {
    if gpu_fit::fits_in_vram(model_size_bytes, free_vram_bytes) {
        OFFLOAD_ALL_LAYERS
    } else {
        0
    }
}

/// Picks a default `n_gpu_layers` for a model the caller hasn't pinned an
/// explicit `gpu_layers` setting for.
///
/// Best-effort only: queries free VRAM via `nvidia-smi` (present whenever
/// there's an NVIDIA GPU, which is what free-VRAM auto-detection can
/// realistically support without vendor-specific APIs) and compares it
/// against the GGUF file's on-disk size as a rough proxy for how much VRAM
/// full offload would need. This is deliberately coarse — no attempt to
/// count layers or split partially — because the goal here is narrow: stop
/// defaulting to "offload everything" for a model that obviously can't
/// fit at all (e.g. an ~18GB file on a 16GB card), which previously caused
/// a hard load failure. When detection isn't possible (no `nvidia-smi`,
/// non-NVIDIA GPU, parse failure, etc.) this falls back to the previous
/// "offload everything" default rather than guessing further — a Vulkan
/// backend on a card we can't query is treated the same as before.
fn decide_gpu_layers(model_path: &Path) -> GpuDecision {
    let free_vram_bytes = detect_free_vram_bytes_best();
    let model_size_bytes = std::fs::metadata(model_path).map(|m| m.len()).ok();

    let gpu_layers = match (model_size_bytes, free_vram_bytes) {
        (Some(model), Some(free)) => {
            let layers = gpu_layers_for_vram(model, free);
            if layers == 0 {
                // Visible on stderr (not only `tracing::warn!`, which had no
                // subscriber capturing it on the affected machine — the log
                // there showed no trace of this decision at all). This is a
                // 5–10× slowdown; the user needs to be able to see why.
                let msg = format!(
                    "grafium: local chat model is ~{} MiB but only ~{} MiB VRAM was free at \
                     load — running on CPU (much slower). Free VRAM and use \"Retry on GPU\" in \
                     Chat, or set an explicit \"GPU layers\" value in Settings.",
                    model / (1024 * 1024),
                    free / (1024 * 1024)
                );
                eprintln!("{msg}");
                tracing::warn!("{msg}");
            }
            layers
        }
        // Can't measure one side — keep the historical "offload everything"
        // default rather than guessing CPU.
        _ => OFFLOAD_ALL_LAYERS,
    };

    GpuDecision {
        gpu_layers,
        free_vram_bytes,
        model_size_bytes,
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

/// When the configured (or auto-picked) chat model doesn't fit in
/// available RAM, looks for another already-downloaded LLM-kind model in
/// the same directory that does — preferring the largest one that still
/// fits (best quality available), so chat "just works" with whatever's
/// actually usable right now rather than being completely unavailable
/// until the user manually reconfigures Settings. Mirrors the "it just
/// works" expectation the embedded Whisper transcription pipeline already
/// delivers for the exact same models directory.
fn find_fallback_llm_model(models_dir: &Path, exclude: &Path) -> Option<std::path::PathBuf> {
    let mut candidates: Vec<_> = model_library::scan_models_dir(models_dir)
        .ok()?
        .into_iter()
        .filter(|m| m.kind == ModelKind::Llm && m.path != exclude)
        .collect();
    candidates.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    candidates
        .into_iter()
        .find(|m| check_cpu_ram_budget(&m.path).is_ok())
        .map(|m| m.path)
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
    /// Whether the model's baked-in chat template marks it as a reasoning
    /// ("thinking") model — detected once at load from the template text.
    supports_thinking: bool,
    /// Whether inference actually landed on the GPU, and the VRAM figures that
    /// drove that decision — surfaced to the UI so a silent CPU fallback is
    /// visible instead of presenting as a hang.
    accel: crate::ai::traits::AcceleratorStatus,
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
        // VRAM at all and decide up front, so we only ever allocate once. An
        // *explicit* `gpu_layers` always wins over the heuristic.
        let explicit = gpu_layers.is_some();
        let decision = match gpu_layers {
            Some(layers) => GpuDecision {
                gpu_layers: layers,
                // Report the current free VRAM for context, but a single
                // (non-retried) read is fine here since it doesn't drive any
                // decision — the user pinned the value.
                free_vram_bytes: detect_free_vram_bytes(),
                model_size_bytes: std::fs::metadata(model_path).map(|m| m.len()).ok(),
            },
            None => decide_gpu_layers(model_path),
        };
        let requested_gpu_layers = decision.gpu_layers;

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
        let model_params = if requested_gpu_layers == 0 {
            model_params.with_use_mmap(false)
        } else {
            model_params
        };

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
        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|e| CoreError::Other(format!("failed to load LLM model: {e}")))?;

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

        // Reasoning models (Qwen3, DeepSeek-R1, ...) advertise themselves in
        // their chat template — Qwen3's references `enable_thinking` and both
        // families emit `<think>` control markers. Detecting it here (once,
        // from the template text) lets the engine request non-thinking mode
        // and budget output tokens for a model that reasons before answering.
        let supports_thinking = model
            .chat_template(None)
            .ok()
            .and_then(|t| t.to_string().ok())
            .map(|tmpl| {
                let lower = tmpl.to_lowercase();
                lower.contains("enable_thinking") || lower.contains("<think>")
            })
            .unwrap_or(false);

        // GPU offload is only physically possible when a GPU backend was
        // compiled in; otherwise CPU is expected and the UI must not warn.
        let gpu_supported = cfg!(feature = "llm-local-vulkan");
        let accel = crate::ai::traits::AcceleratorStatus {
            gpu_supported,
            on_gpu: gpu_supported && requested_gpu_layers > 0,
            gpu_layers: requested_gpu_layers,
            free_vram_mib_at_load: decision.free_vram_bytes.map(|b| b / (1024 * 1024)),
            model_mib: decision.model_size_bytes.map(|b| b / (1024 * 1024)),
            explicit,
        };

        Ok(Self {
            backend,
            model: Arc::new(model),
            ctx_size,
            name,
            supports_thinking,
            accel,
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
    /// If the resolved model doesn't fit in available RAM, automatically
    /// retries with the largest other already-downloaded LLM-kind model in
    /// the same directory that does fit, instead of leaving chat entirely
    /// unavailable — see `find_fallback_llm_model`. This is safe to do
    /// (i.e. doesn't risk the double-allocation hazard `load()` warns
    /// about) because `check_cpu_ram_budget` always runs *before* any
    /// weights are actually read for a CPU-only load, so the first
    /// (failing) attempt never allocated anything to begin with.
    pub fn from_settings(models_dir: &Path, settings: &LocalLlmSettings) -> Result<Self> {
        let model_path = settings.model_ref.resolve(models_dir, ModelKind::Llm)?;
        match Self::load(&model_path, settings.context_size, settings.gpu_layers) {
            Ok(llm) => Ok(llm),
            Err(e) if is_ram_budget_error(&e) => {
                match find_fallback_llm_model(models_dir, &model_path) {
                    Some(fallback_path) => {
                        tracing::warn!(
                            configured = %model_path.display(),
                            fallback = %fallback_path.display(),
                            "Configured chat model doesn't fit in available RAM; falling back \
                             to a smaller already-downloaded model so chat stays usable. Pick a \
                             different model explicitly in Settings to change this."
                        );
                        Self::load(&fallback_path, settings.context_size, settings.gpu_layers)
                    }
                    None => Err(e),
                }
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

    /// User-initiated "try the GPU now" reload: same resolution as
    /// [`Self::from_config`] but forces full GPU offload, bypassing both the
    /// free-VRAM heuristic and any configured `gpu_layers`. This is the
    /// action behind Chat's "Retry on GPU" button — the free-VRAM heuristic
    /// may have landed on CPU because VRAM was *transiently* busy at startup
    /// (the embedder mid-index, a previous instance shutting down); once that
    /// has cleared, this lets the user move inference onto the GPU without
    /// restarting or editing Settings. A fresh single load attempt, so it
    /// doesn't risk the double-allocation hazard `load()` warns about.
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
        let disable_thinking = self.supports_thinking;

        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let prompt = build_chat_prompt(&model, &messages, &options, disable_thinking)?;
                generate(&model, &backend, ctx_size, &prompt, &options, None)
            })
            .await
            .map_err(|e| CoreError::Other(format!("LLM inference task panicked: {e}")))?
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn context_window(&self) -> Option<usize> {
        Some(self.ctx_size.get() as usize)
    }

    fn supports_thinking(&self) -> bool {
        self.supports_thinking
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
        let disable_thinking = self.supports_thinking;

        Box::pin(async move {
            // `generate()` runs on a blocking thread (llama.cpp is
            // synchronous), so pieces are handed back here through a
            // channel rather than calling `on_token` directly from that
            // thread — `on_token` is an arbitrary `&mut` closure the caller
            // owns, and this keeps it running only on this async task.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

            let generation = tokio::task::spawn_blocking(move || {
                let prompt = build_chat_prompt(&model, &messages, &options, disable_thinking)?;
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

    fn accelerator_status(&self) -> Option<crate::ai::traits::AcceleratorStatus> {
        Some(self.accel.clone())
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
    disable_thinking: bool,
) -> Result<String> {
    let mut chat = Vec::with_capacity(messages.len() + 1);
    if let Some(system) = &options.system_prompt {
        chat.push(new_chat_message("system", system)?);
    }
    // For reasoning models, append the `/no_think` soft switch to the final
    // user turn. Qwen3 (and compatible templates) honour this directive to
    // skip the <think> reasoning pass — cheaper and more reliable than hoping
    // the model answers before exhausting its budget. `<think>` stripping in
    // the engine remains as a backstop for models that ignore the directive.
    let last_user = messages.iter().rposition(|m| m.role == MessageRole::User);
    for (i, message) in messages.iter().enumerate() {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };
        let content = if disable_thinking && Some(i) == last_user {
            format!("{}\n\n/no_think", message.content)
        } else {
            message.content.clone()
        };
        chat.push(new_chat_message(role, &content)?);
    }

    let template = match model.chat_template(None) {
        Ok(template) => template,
        Err(_) => LlamaChatTemplate::new("chatml").map_err(|e| {
            CoreError::Other(format!("failed to build fallback chat template: {e}"))
        })?,
    };

    model
        .apply_chat_template(&template, &chat, true)
        .map_err(|e| CoreError::Other(format!("failed to apply chat template: {e}")))
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
    let n_threads = std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4);
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(Some(ctx_size))
        // llama.cpp's decode() asserts the whole batch fits within
        // `n_batch` (default 2048), independent of `n_ctx` — without this,
        // a single-shot prompt longer than 2048 tokens (easy to hit once
        // page content is included) crashes the whole process instead of
        // returning an error. Matching n_batch/n_ubatch to the context
        // size means "fits in the context window" is the only limit a
        // caller needs to reason about.
        .with_n_batch(ctx_size.get())
        .with_n_ubatch(ctx_size.get())
        .with_n_threads(n_threads)
        .with_n_threads_batch(n_threads);

    let mut ctx = model
        .new_context(backend, ctx_params)
        .map_err(|e| CoreError::Other(format!("failed to create llama context: {e}")))?;

    let tokens = model
        .str_to_token(prompt, AddBos::Always)
        .map_err(|e| CoreError::Other(format!("failed to tokenize prompt: {e}")))?;

    let n_ctx = ctx.n_ctx() as i32;
    if tokens.len() as i32 >= n_ctx {
        return Err(CoreError::Other(format!(
            "prompt ({} tokens) exceeds the context window ({n_ctx} tokens) — shorten the input \
             or increase `context_size` in local LLM settings",
            tokens.len()
        )));
    }
    let max_new_tokens = options.max_tokens.unwrap_or(1024) as i32;

    let mut batch = LlamaBatch::new(tokens.len().max(512) + 1, 1);
    let last_index = tokens.len() as i32 - 1;
    for (i, token) in tokens.iter().enumerate() {
        let is_last = i as i32 == last_index;
        batch
            .add(*token, i as i32, &[0], is_last)
            .map_err(|e| CoreError::Other(format!("failed to queue prompt token: {e}")))?;
    }
    ctx.decode(&mut batch)
        .map_err(|e| CoreError::Other(format!("llama.cpp decode of the prompt failed: {e}")))?;

    let mut sampler = build_sampler(options);
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut output = String::new();
    let mut n_cur = batch.n_tokens();
    let stop_at_token = n_cur + max_new_tokens;

    while n_cur < stop_at_token && n_cur < n_ctx {
        // Cooperative cancellation: the UI can flip this flag (via
        // `CompletionOptions.cancel`) to abort a slow local generation
        // instead of leaving the user staring at a frozen pane. Return what
        // we have so far rather than erroring.
        if options
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
        {
            break;
        }

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
            .map(|_| ())
            .unwrap_err();

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
            .map(|_| ())
            .unwrap_err();

        let default_dir = data_dir.path().join("models");
        assert!(
            err.to_string().contains(&default_dir.display().to_string()),
            "expected default models_dir ({}) in error, got: {err}",
            default_dir.display()
        );
    }

    /// `find_fallback_llm_model` is what makes chat "just work" when the
    /// configured/auto-picked model is too large for available RAM (e.g. a
    /// user picked a 30B model but only has enough free RAM for a 4B one):
    /// it should skip the excluded (too-large) model and pick the largest
    /// *other* LLM-kind model that still passes the RAM budget check,
    /// ignoring non-LLM files (embeddings) entirely.
    #[test]
    fn find_fallback_llm_model_prefers_largest_model_that_still_fits_ram_budget() {
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
        // Not an LLM at all — must never be picked as a chat-model fallback.
        let embedding = dir.path().join("nomic-embed-text.gguf");
        std::fs::write(&embedding, vec![0u8; 8192]).unwrap();

        let fallback = find_fallback_llm_model(dir.path(), &too_big);
        assert_eq!(fallback, Some(fits_larger));
    }

    #[test]
    fn find_fallback_llm_model_returns_none_when_nothing_else_fits() {
        let dir = tempfile::tempdir().unwrap();
        let only_model = dir.path().join("qwen-2b-instruct.gguf");
        std::fs::write(&only_model, vec![0u8; 1024]).unwrap();

        assert_eq!(find_fallback_llm_model(dir.path(), &only_model), None);
    }
}

#[cfg(test)]
mod gpu_decision_tests {
    use super::*;

    #[test]
    fn safety_margin_scales_with_model_and_clamps() {
        // Tiny model: 20% would be well under the floor → clamped to 512 MiB.
        let tiny = 256 * 1024 * 1024; // 256 MiB
        assert_eq!(vram_safety_margin_bytes(tiny), VRAM_SAFETY_MARGIN_MIN_BYTES);

        // Mid model: 5 GiB → 20% = 1 GiB, inside the band → used as-is.
        let mid = 5 * 1024 * 1024 * 1024; // 5 GiB
        assert_eq!(vram_safety_margin_bytes(mid), 1024 * 1024 * 1024);

        // Huge model: 30 GiB → 20% = 6 GiB, above the ceiling → clamped.
        let huge = 30u64 * 1024 * 1024 * 1024;
        assert_eq!(vram_safety_margin_bytes(huge), VRAM_SAFETY_MARGIN_MAX_BYTES);
    }

    #[test]
    fn gpu_layers_decision_fits_and_does_not_fit() {
        let model = 5 * 1024 * 1024 * 1024; // 5 GiB → margin 1 GiB → needs 6 GiB free
        let needed = model + vram_safety_margin_bytes(model);

        // Comfortably enough free VRAM → offload everything.
        assert_eq!(gpu_layers_for_vram(model, needed), OFFLOAD_ALL_LAYERS);
        assert_eq!(
            gpu_layers_for_vram(model, needed + 1024 * 1024 * 1024),
            OFFLOAD_ALL_LAYERS
        );

        // One byte short of the requirement → CPU-only.
        assert_eq!(gpu_layers_for_vram(model, needed - 1), 0);

        // The exact real-machine numbers from the bug report: a 4,983 MiB
        // model with only ~5,000 MiB free (a transient dip) does not fit;
        // with 16 GiB free it does.
        let real_model = 4_983u64 * 1024 * 1024;
        assert_eq!(gpu_layers_for_vram(real_model, 5_000 * 1024 * 1024), 0);
        assert_eq!(
            gpu_layers_for_vram(real_model, 16 * 1024 * 1024 * 1024),
            OFFLOAD_ALL_LAYERS
        );
    }

    #[test]
    fn best_of_free_readings_ignores_a_transient_dip() {
        // The whole point of sampling the *max*: a momentary dip (5 GiB)
        // between two healthy readings (16 GiB) must not be what we decide
        // on. Emulate the reduction the sampler performs.
        let readings = [16u64, 5, 16].map(|g| g * 1024 * 1024 * 1024);
        let best = readings.iter().copied().reduce(|a, b| a.max(b)).unwrap();
        assert_eq!(best, 16 * 1024 * 1024 * 1024);

        let real_model = 4_983u64 * 1024 * 1024;
        // With the dip we'd have (wrongly) picked CPU; with the best reading
        // we correctly offload.
        assert_eq!(gpu_layers_for_vram(real_model, best), OFFLOAD_ALL_LAYERS);
    }
}
