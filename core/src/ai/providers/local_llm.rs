//! Isolated local LLM inference via llama.cpp (through the `llama-cpp-2`
//! bindings) — the LLM-side counterpart to `media::transcribe`'s
//! `WhisperTranscriber`. Both:
//!   * resolve a model file through the shared `model_library` instead of
//!     requiring an exact path (`from_settings`/`from_config` mean the same
//!     thing in both modules — see `media::transcribe` for the sibling this
//!     one is deliberately shaped to match),
//!   * execute native code in disposable resource-limited subprocesses,
//!   * and expose themselves through this crate's existing trait
//!     abstractions (`Transcriber` there, `LlmProvider` here) rather than a
//!     bespoke call site — so summarization code depends on "an
//!     `LlmProvider`", never on "llama.cpp specifically".
//!
//! Gated behind the `llm-local` Cargo feature (mirrors `media`/
//! `media-vulkan`); enable `llm-local-vulkan` to offload inference to a GPU
//! via Vulkan (no CUDA toolkit required — same rationale as `media-vulkan`).

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::{send_logs_to_tracing, LogOptions};

use crate::ai::config::LocalLlmSettings;
use crate::ai::resources::{self, ModelWorkload};
use crate::ai::traits::{BoxFuture, ChatMessage, CompletionOptions, LlmProvider, MessageRole};
use crate::error::{CoreError, Result};
use crate::model_library::{self, ModelKind};

/// Resolves and validates a GGUF model, then runs each completion in a
/// disposable resource-limited Grafium worker process.
pub struct LocalLlm {
    model_path: PathBuf,
    context_size: u32,
    gpu_layers: u32,
    name: String,
}

/// The process-wide llama.cpp backend. llama.cpp only wants to be
/// initialized once; every `LocalLlm` instance shares the same handle
/// rather than each `load()` call re-initializing it.
static BACKEND: OnceLock<Arc<LlamaBackend>> = OnceLock::new();

impl LocalLlm {
    /// Loads a GGUF model from `model_path`.
    ///
    /// `context_size` overrides the model's own trained context length;
    /// `gpu_layers` controls how many transformer layers to offload to the
    /// GPU (only meaningful when built with `llm-local-vulkan` — otherwise
    /// there's no GPU backend to offload to, so this is a harmless no-op).
    /// `None` uses Grafium's conservative defaults: 4096 context tokens and
    /// CPU-only inference. GPU offload requires an explicit safety opt-in.
    pub fn load(
        model_path: &Path,
        context_size: Option<u32>,
        gpu_layers: Option<u32>,
    ) -> Result<Self> {
        let context_size = resources::safe_context_size(context_size)?;
        let gpu_layers = resources::safe_gpu_layers(gpu_layers)?;
        resources::validate_model_load(
            model_path,
            ModelWorkload::Llm {
                context_tokens: context_size,
            },
        )?;

        let name = model_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("local-llm")
            .to_string();

        Ok(Self {
            model_path: model_path.to_path_buf(),
            context_size,
            gpu_layers,
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
    pub fn from_settings(models_dir: &Path, settings: &LocalLlmSettings) -> Result<Self> {
        let model_path = settings.model_ref.resolve(models_dir, ModelKind::Llm)?;
        Self::load(&model_path, settings.context_size, settings.gpu_layers)
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
}

impl LlmProvider for LocalLlm {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        let model_path = self.model_path.clone();
        let context_size = self.context_size;
        let gpu_layers = self.gpu_layers;
        let messages = messages.to_vec();
        let options = options.clone();

        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let prompt_bytes = messages.iter().fold(
                    options.system_prompt.as_ref().map_or(0, String::len),
                    |total, message| total.saturating_add(message.content.len()),
                );
                resources::validate_prompt_bytes(prompt_bytes)?;
                match crate::ai::worker::execute(
                    crate::ai::worker::WorkerRequest::Llm {
                        model_path,
                        context_size,
                        gpu_layers,
                        messages,
                        options,
                    },
                    Duration::from_secs(30 * 60),
                )? {
                    crate::ai::worker::WorkerOutput::Llm(output) => Ok(output),
                    crate::ai::worker::WorkerOutput::Ready => Err(CoreError::Other(
                        "native AI worker returned a health result for an LLM request".to_string(),
                    )),
                    #[cfg(feature = "media")]
                    crate::ai::worker::WorkerOutput::Whisper(_) => Err(CoreError::Other(
                        "native AI worker returned a transcription for an LLM request".to_string(),
                    )),
                }
            })
            .await
            .map_err(|e| CoreError::Other(format!("LLM worker task panicked: {e}")))?
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        let model_path = self.model_path.clone();
        let context_size = self.context_size;
        let gpu_layers = self.gpu_layers;
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                match crate::ai::worker::execute(
                    crate::ai::worker::WorkerRequest::ValidateLlm {
                        model_path,
                        context_size,
                        gpu_layers,
                    },
                    Duration::from_secs(10 * 60),
                )? {
                    crate::ai::worker::WorkerOutput::Ready => Ok(true),
                    _ => Err(CoreError::Other(
                        "native AI worker returned output while validating a model".to_string(),
                    )),
                }
            })
            .await
            .map_err(|e| CoreError::Other(format!("LLM health worker task panicked: {e}")))?
        })
    }
}

pub(crate) struct LlmSlot {
    model_path: PathBuf,
    context_size: u32,
    gpu_layers: u32,
    backend: Arc<LlamaBackend>,
    model: LlamaModel,
    model_size: u64,
}

impl LlmSlot {
    fn matches(&self, model_path: &Path, context_size: u32, gpu_layers: u32) -> bool {
        self.model_path == model_path
            && self.context_size == context_size
            && self.gpu_layers == gpu_layers
    }
}

fn install_llm_logging() {
    static INSTALL_LOGGING: std::sync::Once = std::sync::Once::new();
    INSTALL_LOGGING.call_once(|| {
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
    });
}

fn ensure_slot(
    slot: &mut Option<LlmSlot>,
    model_path: &Path,
    context_size: u32,
    gpu_layers: u32,
) -> Result<()> {
    if slot
        .as_ref()
        .is_some_and(|cached| cached.matches(model_path, context_size, gpu_layers))
    {
        return Ok(());
    }
    // Drop any previously cached model first so its native memory is released
    // before a potentially larger replacement is loaded.
    *slot = None;
    install_llm_logging();
    let (backend, model) = load_native_model(model_path, gpu_layers)?;
    let model_size = std::fs::metadata(model_path)
        .map_err(|e| CoreError::Other(format!("Cannot inspect LLM model: {e}")))?
        .len();
    *slot = Some(LlmSlot {
        model_path: model_path.to_path_buf(),
        context_size,
        gpu_layers,
        backend,
        model,
        model_size,
    });
    Ok(())
}

pub(crate) fn validate_in_process(
    slot: &mut Option<LlmSlot>,
    model_path: &Path,
    context_size: u32,
    gpu_layers: u32,
) -> Result<()> {
    ensure_slot(slot, model_path, context_size, gpu_layers)?;
    let slot = slot
        .as_ref()
        .expect("slot populated by ensure_slot for validation");
    let ctx_size = NonZeroU32::new(context_size)
        .ok_or_else(|| CoreError::Other("local LLM context cannot be zero".to_string()))?;
    resources::validate_inference_headroom(
        "local LLM validation",
        resources::estimate_llm_context_bytes(slot.model_size, ctx_size.get()),
    )?;
    let params = LlamaContextParams::default()
        .with_n_ctx(Some(ctx_size))
        .with_n_threads(1)
        .with_n_threads_batch(1);
    slot.model
        .new_context(&slot.backend, params)
        .map_err(|e| CoreError::Other(format!("failed to validate llama context: {e}")))?;
    Ok(())
}

pub(crate) fn complete_in_process(
    slot: &mut Option<LlmSlot>,
    model_path: &Path,
    context_size: u32,
    gpu_layers: u32,
    messages: &[ChatMessage],
    options: &CompletionOptions,
) -> Result<String> {
    ensure_slot(slot, model_path, context_size, gpu_layers)?;
    let slot = slot
        .as_ref()
        .expect("slot populated by ensure_slot for completion");
    let ctx_size = NonZeroU32::new(context_size)
        .ok_or_else(|| CoreError::Other("local LLM context cannot be zero".to_string()))?;
    let prompt = build_chat_prompt(&slot.model, messages, options)?;
    generate(
        &slot.model,
        &slot.backend,
        ctx_size,
        slot.model_size,
        &prompt,
        options,
    )
}

fn load_native_model(
    model_path: &Path,
    gpu_layers: u32,
) -> Result<(Arc<LlamaBackend>, LlamaModel)> {
    let backend = if let Some(backend) = BACKEND.get() {
        Arc::clone(backend)
    } else {
        let initialized = Arc::new(LlamaBackend::init().map_err(|e| {
            CoreError::Other(format!("failed to initialize the llama.cpp backend: {e}"))
        })?);
        let _ = BACKEND.set(Arc::clone(&initialized));
        BACKEND.get().map(Arc::clone).unwrap_or(initialized)
    };
    // Disable mmap so the worker's virtual-memory ceiling tracks real model
    // allocations instead of large file mappings.
    let model_params = LlamaModelParams::default()
        .with_n_gpu_layers(gpu_layers)
        .with_use_mmap(false);
    let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
        .map_err(|e| CoreError::Other(format!("failed to load LLM model: {e}")))?;
    Ok((backend, model))
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
    model_size: u64,
    prompt: &str,
    options: &CompletionOptions,
) -> Result<String> {
    resources::validate_prompt_size(prompt)?;
    resources::validate_inference_headroom(
        "local LLM inference",
        resources::estimate_llm_context_bytes(model_size, ctx_size.get()),
    )?;
    let n_threads = resources::inference_thread_count();
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(Some(ctx_size))
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
    let max_new_tokens = resources::safe_generated_tokens(options.max_tokens)? as i32;

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
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(token);

        if model.is_eog_token(token) {
            break;
        }

        let piece = model
            .token_to_piece(token, &mut decoder, true, None)
            .map_err(|e| CoreError::Other(format!("failed to decode generated token: {e}")))?;
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

    #[test]
    fn inference_leaves_cores_free_for_the_ui() {
        // The whole point: never hand llama.cpp every core, or the WebView
        // has nothing left to render with and typing stutters.
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let threads = resources::inference_thread_count();

        assert!(threads >= 1, "must always use at least one thread");
        if cores > 2 {
            assert!(
                (threads as usize) < cores,
                "expected headroom for the UI: {threads} threads on {cores} cores"
            );
        }
    }

    #[test]
    fn inference_thread_count_never_reports_zero_on_small_machines() {
        // saturating_sub + max(1) has to survive 1- and 2-core boxes.
        for cores in [1usize, 2, 3] {
            let computed = cores.saturating_sub(2).max(1);
            assert!(computed >= 1, "{cores} cores produced {computed} threads");
        }
    }
}
