//! Isolated llama.cpp inference. Hosts resolve settings/catalog references to
//! paths; this provider prepares metadata and dispatches to the shared worker.
//!
//! Gated behind the `llm-local` Cargo feature (mirrors `media`/
//! `media-vulkan`); enable `llm-local-vulkan` to offload inference to a GPU
//! via Vulkan (no CUDA toolkit required — same rationale as `media-vulkan`).

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::{send_logs_to_tracing, LogOptions};

use super::{llama_shared::shared_backend, native_gpu};
use crate::error::{Result, RuntimeError};
use crate::native::policy::{self as resources, ModelWorkload};
use crate::types::{
    BoxFuture, ChatMessage, CompletionOptions, Concurrency, LlmProvider, MessageRole,
};

pub const ALL_GPU_LAYERS: u32 = super::llama_shared::OFFLOAD_ALL_LAYERS;
const GENERATION_TIMEOUT: Duration = Duration::from_secs(30 * 60);

fn validate_prompt(messages: &[ChatMessage], options: &CompletionOptions) -> Result<()> {
    resources::validate_prompt_bytes(messages.iter().fold(
        options.system_prompt.as_ref().map_or(0, String::len),
        |total, message| total.saturating_add(message.content.len()),
    ))
}

fn generated_text(output: crate::native::worker::WorkerOutput) -> Result<String> {
    match output {
        crate::native::worker::WorkerOutput::Llm(output) => Ok(output),
        _ => Err(RuntimeError::Other(
            "native AI worker returned an unexpected result for an LLM request".to_string(),
        )),
    }
}

/// Resolves and validates a GGUF model, then runs each completion in a
/// disposable host worker process.
pub struct LocalLlm {
    model_path: PathBuf,
    context_size: u32,
    gpu_layers: u32,
    name: String,
}

impl LocalLlm {
    /// Loads a GGUF model from `model_path`.
    ///
    /// `context_size` overrides the model's own trained context length;
    /// `gpu_layers` controls how many transformer layers to offload to the
    /// GPU (only meaningful when built with `llm-local-vulkan` — otherwise
    /// there's no GPU backend to offload to, so this is a harmless no-op).
    /// `None` uses host's conservative defaults: 4096 context tokens and
    /// CPU-only inference. GPU offload requires an explicit safety opt-in.
    pub fn load(
        model_path: &Path,
        context_size: Option<u32>,
        gpu_layers: Option<u32>,
    ) -> Result<Self> {
        let model_path = super::model_file::canonical_model_path(model_path)?;
        let context_size = resources::safe_context_size(context_size)?;
        let gpu_layers = resources::safe_gpu_layers(gpu_layers)?;
        resources::validate_model_load(
            &model_path,
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

    /// The model's raw chat template, for diagnosing reasoning detection.
    ///
    /// Unlike completions — which run in an isolated worker child so a native
    /// llama.cpp fault cannot take the app down — this loads the model in the
    /// calling process. It is for the `model_probe` example and other offline
    /// diagnostics, never for the request path. `None` means the GGUF carries
    /// no template at all, which is itself the interesting answer: a stripped
    /// template is what made an abliterated Qwen3 build get classified as a
    /// non-reasoning model and emit raw chain-of-thought as its answer.
    pub fn chat_template_for_debug(&self) -> Option<String> {
        let locked = super::model_file::LockedModelFile::acquire(&self.model_path).ok()?;
        let (_backend, model) = load_native_model(locked.path(), 0).ok()?;
        model
            .chat_template(None)
            .ok()
            .and_then(|template| template.to_string().ok())
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
                validate_prompt(&messages, &options)?;
                generated_text(crate::native::worker::execute(
                    crate::native::worker::WorkerRequest::Llm {
                        model_path,
                        context_size,
                        gpu_layers,
                        messages,
                        options,
                        stream: false,
                    },
                    GENERATION_TIMEOUT,
                )?)
            })
            .await
            .map_err(|e| RuntimeError::Other(format!("LLM worker task panicked: {e}")))?
        })
    }

    fn complete_stream<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
        on_token: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<String>> {
        let model_path = self.model_path.clone();
        let context_size = self.context_size;
        let gpu_layers = self.gpu_layers;
        let messages = messages.to_vec();
        let options = options.clone();

        Box::pin(async move {
            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
            let generation = tokio::task::spawn_blocking(move || {
                validate_prompt(&messages, &options)?;
                generated_text(crate::native::worker::execute_streaming(
                    crate::native::worker::WorkerRequest::Llm {
                        model_path,
                        context_size,
                        gpu_layers,
                        messages,
                        options,
                        stream: true,
                    },
                    GENERATION_TIMEOUT,
                    // A closed receiver means the caller abandoned this request.
                    &mut |text| drop(sender.send(text.to_owned())),
                )?)
            });
            while let Some(text) = receiver.recv().await {
                on_token(&text);
            }
            generation
                .await
                .map_err(|e| RuntimeError::Other(format!("LLM worker task panicked: {e}")))?
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn native_model_path(&self) -> Option<&Path> {
        Some(&self.model_path)
    }

    fn context_window(&self) -> Option<usize> {
        Some(self.context_size as usize)
    }

    fn accelerator_status(&self) -> Option<crate::types::AcceleratorStatus> {
        let resident = super::worker::resident_status()?;
        if resident.model.kind != super::worker::NativeModelKind::Chat
            || resident.model.model_path != self.model_path
            || resident.model.context_size != Some(self.context_size)
        {
            return None;
        }
        Some(crate::types::AcceleratorStatus {
            gpu_supported: cfg!(feature = "llm-local-vulkan"),
            on_gpu: resident.model.on_gpu,
            gpu_layers: resident.model.gpu_layers.unwrap_or(0),
            free_vram_mib_at_load: None,
            model_mib: None,
            explicit: self.gpu_layers > 0,
        })
    }

    fn backend_summary(&self) -> Option<String> {
        self.accelerator_status().map(|status| {
            if status.on_gpu {
                format!("Native worker: {} GPU layers", status.gpu_layers)
            } else {
                "Native worker: CPU".into()
            }
        })
    }

    /// One worker child, one model resident in VRAM, one request at a time.
    ///
    /// `worker::execute` takes a process-wide lock for every request, and a
    /// request whose `(model, context, gpu_layers)` key differs evicts the
    /// running child. So overlapping chats here don't run concurrently — the
    /// second one blocks, invisibly, until the first finishes. Reporting that
    /// honestly lets the UI say "waiting for the model" instead of showing a
    /// spinner that looks broken.
    ///
    /// `slots` is 1 rather than something derived from free VRAM because the
    /// constraint is structural, not capacity: even on a card with room for
    /// three copies, there is still exactly one worker. Raising it needs a
    /// multi-slot execution path first, and the number would be a lie until
    /// then.
    fn concurrency(&self) -> Concurrency {
        Concurrency::serialized(1)
    }

    fn count_prompt_tokens<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<Option<usize>>> {
        let model_path = self.model_path.clone();
        let context_size = self.context_size;
        let gpu_layers = self.gpu_layers;
        let messages = messages.to_vec();
        let options = options.clone();

        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                crate::native::worker::check_cancelled(options.cancel.as_deref())?;
                validate_prompt(&messages, &options)?;
                match crate::native::worker::execute(
                    crate::native::worker::WorkerRequest::CountPrompt {
                        model_path,
                        context_size,
                        gpu_layers,
                        messages,
                        options,
                    },
                    Duration::from_secs(10 * 60),
                )? {
                    crate::native::worker::WorkerOutput::PromptTokenCount(count) => Ok(Some(count)),
                    _ => Err(RuntimeError::Other(
                        "native AI worker returned an unexpected result for a prompt count request"
                            .to_string(),
                    )),
                }
            })
            .await
            .map_err(|e| RuntimeError::Other(format!("LLM prompt count task panicked: {e}")))?
        })
    }

    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        let model_path = self.model_path.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                // Health polling checks availability, not native tensor validity.
                // Loading here would spend recovery credit without a user request.
                let locked = super::model_file::LockedModelFile::acquire(&model_path)?;
                crate::gguf::inspect_metadata(locked.path())?;
                Ok(true)
            })
            .await
            .map_err(|e| RuntimeError::Other(format!("LLM health inspection task panicked: {e}")))?
        })
    }
}

pub(crate) struct LlmSlot {
    model_path: PathBuf,
    context_size: u32,
    gpu_layers: u32,
    device: Option<native_gpu::Device>,
    backend: Arc<LlamaBackend>,
    model: LlamaModel,
    _model_file: super::model_file::LockedModelFile,
}

impl LlmSlot {
    fn matches(&self, model_path: &Path, context_size: u32, gpu_layers: u32) -> bool {
        self.model_path == model_path
            && self.context_size == context_size
            && self.gpu_layers == gpu_layers
    }
}

pub(crate) fn install_llm_logging() {
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
    let canonical = super::model_file::canonical_model_path(model_path)?;
    let model_path = canonical.as_path();
    if slot
        .as_ref()
        .is_some_and(|cached| cached.matches(model_path, context_size, gpu_layers))
    {
        return Ok(());
    }
    // Drop any previously cached model first so its native memory is released
    // before a potentially larger replacement is loaded.
    *slot = None;
    let model_file = super::model_file::LockedModelFile::acquire(model_path)?;
    let canonical = model_file.path().to_path_buf();
    let model_path = canonical.as_path();
    install_llm_logging();
    resources::validate_model_load(
        model_path,
        ModelWorkload::Llm {
            context_tokens: context_size,
        },
    )?;
    let backend = shared_backend()?;
    let context = NonZeroU32::new(context_size)
        .ok_or_else(|| RuntimeError::Other("Local context cannot be zero".into()))?;
    let (parameters, mut device) = native_gpu::fitted_params(
        model_path,
        context,
        if cfg!(feature = "llm-local-vulkan") {
            gpu_layers
        } else {
            0
        },
        false,
    )?;
    let model = LlamaModel::load_from_file(&backend, model_path, &parameters)
        .map_err(|e| RuntimeError::Other(format!("Failed to load fitted model: {e}")))?;
    if let Some(device) = &mut device {
        device.reserve_context()?;
    }
    super::worker::emit_resident(super::worker::ResidentModel {
        kind: super::worker::NativeModelKind::Chat,
        model_path: model_path.to_path_buf(),
        context_size: Some(context_size),
        on_gpu: device.is_some(),
        gpu_layers: Some(if device.is_some() {
            (parameters.n_gpu_layers().max(0) as u32).min(model.n_layer().saturating_add(1))
        } else {
            0
        }),
    });
    *slot = Some(LlmSlot {
        model_path: model_path.to_path_buf(),
        context_size,
        gpu_layers,
        device,
        backend,
        model,
        _model_file: model_file,
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
        .ok_or_else(|| RuntimeError::Other("local LLM context cannot be zero".to_string()))?;
    resources::validate_inference_headroom(
        "local LLM validation",
        crate::resources::estimate_context_for_model(&slot.model_path, ctx_size.get())?,
    )?;
    if let Some(device) = &slot.device {
        device.check_context()?;
    }
    let params = native_gpu::context_params(ctx_size, false);
    slot.model
        .new_context(&slot.backend, params)
        .map_err(|e| RuntimeError::Other(format!("failed to validate llama context: {e}")))?;
    Ok(())
}

pub(crate) fn complete_in_process(
    slot: &mut Option<LlmSlot>,
    model_path: &Path,
    context_size: u32,
    gpu_layers: u32,
    messages: &[ChatMessage],
    options: &CompletionOptions,
    on_text: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    ensure_slot(slot, model_path, context_size, gpu_layers)?;
    let slot = slot
        .as_ref()
        .expect("slot populated by ensure_slot for completion");
    let ctx_size = NonZeroU32::new(context_size)
        .ok_or_else(|| RuntimeError::Other("local LLM context cannot be zero".to_string()))?;
    let tokens = prepare_prompt_tokens(&slot.model, messages, options)?;
    if let Some(device) = &slot.device {
        device.check_context()?;
    }
    generate(
        &slot.model,
        &slot.backend,
        ctx_size,
        crate::resources::estimate_context_for_model(&slot.model_path, ctx_size.get())?,
        &tokens,
        options,
        slot.device.clone(),
        on_text,
    )
}

pub(crate) fn count_prompt_tokens_in_process(
    slot: &mut Option<LlmSlot>,
    model_path: &Path,
    context_size: u32,
    gpu_layers: u32,
    messages: &[ChatMessage],
    options: &CompletionOptions,
) -> Result<usize> {
    ensure_slot(slot, model_path, context_size, gpu_layers)?;
    let slot = slot
        .as_ref()
        .expect("slot populated by ensure_slot for prompt counting");
    Ok(prepare_prompt_tokens(&slot.model, messages, options)?.len())
}

fn prepare_prompt_tokens(
    model: &LlamaModel,
    messages: &[ChatMessage],
    options: &CompletionOptions,
) -> Result<Vec<LlamaToken>> {
    tokenize_rendered_prompt(
        build_chat_prompt(model, messages, options),
        |prompt, bos| {
            model
                .str_to_token(prompt, bos)
                .map_err(|e| RuntimeError::Other(format!("failed to tokenize prompt: {e}")))
        },
    )
}

fn tokenize_rendered_prompt(
    prompt: Result<String>,
    tokenize: impl FnOnce(&str, AddBos) -> Result<Vec<LlamaToken>>,
) -> Result<Vec<LlamaToken>> {
    let prompt = prompt?;
    resources::validate_prompt_size(&prompt)?;
    tokenize(&prompt, AddBos::Always)
}

fn load_native_model(
    model_path: &Path,
    gpu_layers: u32,
) -> Result<(Arc<LlamaBackend>, LlamaModel)> {
    let backend = shared_backend()?;
    // Fully resident weights avoid SIGBUS on later faults from removable files.
    let model_params = LlamaModelParams::default()
        .with_n_gpu_layers(gpu_layers)
        .with_use_mmap(false)
        .with_devices(&[])
        .map_err(|e| RuntimeError::Other(e.to_string()))?;
    let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
        .map_err(|e| RuntimeError::Other(format!("failed to load LLM model: {e}")))?;
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
    let chat = build_chat_messages(messages, options)?;
    let template = match model.chat_template(None) {
        Ok(template) => template,
        Err(_) => LlamaChatTemplate::new("chatml").map_err(|e| {
            RuntimeError::Other(format!("failed to build fallback chat template: {e}"))
        })?,
    };

    model
        .apply_chat_template(&template, &chat, true)
        .map_err(|e| RuntimeError::Other(format!("failed to apply chat template: {e}")))
}

fn build_chat_messages(
    messages: &[ChatMessage],
    options: &CompletionOptions,
) -> Result<Vec<LlamaChatMessage>> {
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

    Ok(chat)
}

fn new_chat_message(role: &str, content: &str) -> Result<LlamaChatMessage> {
    LlamaChatMessage::new(role.to_string(), content.to_string())
        .map_err(|e| RuntimeError::Other(format!("invalid chat message: {e}")))
}

/// Builds the sampler chain from `CompletionOptions.temperature`: greedy
/// (fully deterministic) at `0.0`/unset, otherwise the standard
/// top-k/top-p/temperature/distribution chain llama.cpp examples use.
/// Extracted as its own function so it's obvious this is the *only* place
/// sampling policy is decided — nothing else should construct a
/// `LlamaSampler` by hand.
fn build_sampler(model: &LlamaModel, options: &CompletionOptions) -> LlamaSampler {
    const SEED: u32 = 1234;
    // Repetition control is applied to *both* chains, greedy included. Without
    // it, greedy decoding has nothing to break a degenerate attractor: once the
    // argmax token reproduces its own context, it stays the argmax forever and
    // generation becomes the same token repeated until max_tokens. That is not
    // hypothetical — it produced a full screen of "起来" from an English prompt,
    // because these are Qwen-family models and their degenerate attractors land
    // on common Chinese tokens.
    //
    // llama.cpp's own long-standing defaults: penalize within the last 64
    // tokens, 1.1x repeat penalty, frequency/presence penalties off.
    let penalties = || LlamaSampler::penalties(model.n_vocab(), 64, 1.1, 0.0, 0.0);
    match options.temperature {
        Some(t) if t > 0.0 => LlamaSampler::chain_simple([
            penalties(),
            LlamaSampler::top_k(40),
            LlamaSampler::top_p(0.95, 1),
            LlamaSampler::temp(t),
            LlamaSampler::dist(SEED),
        ]),
        _ => LlamaSampler::chain_simple([penalties(), LlamaSampler::greedy()]),
    }
}

/// The single batch → decode → sample loop every completion
/// (chat-formatted or, in the future, raw) goes through — this is the
/// manual generation loop `llama-cpp-2` requires (it has no high-level
/// "generate" helper), written once here rather than duplicated per caller.
fn generate(
    model: &LlamaModel,
    backend: &LlamaBackend,
    ctx_size: NonZeroU32,
    context_bytes: u64,
    tokens: &[LlamaToken],
    options: &CompletionOptions,
    device: Option<native_gpu::Device>,
    on_text: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    resources::validate_inference_headroom("local LLM inference", context_bytes)?;
    let ctx_params = native_gpu::context_params(ctx_size, false);
    let mut pressure = native_gpu::PressureWatch::new(device);
    pressure.check()?;

    let mut ctx = model
        .new_context(backend, ctx_params)
        .map_err(|e| RuntimeError::Other(format!("failed to create llama context: {e}")))?;

    let n_ctx = ctx.n_ctx() as i32;
    if tokens.len() as i32 >= n_ctx {
        return Err(RuntimeError::Other(format!(
            "prompt ({} tokens) exceeds the context window ({n_ctx} tokens) — shorten the input \
             or increase `context_size` in local LLM settings",
            tokens.len()
        )));
    }
    let max_new_tokens = resources::safe_generated_tokens(options.max_tokens)? as i32;

    // Prefill in chunks of at most n_batch tokens. Decoding the whole prompt in
    // one batch trips `GGML_ASSERT(n_tokens_all <= cparams.n_batch)` as soon as
    // the prompt is longer than n_batch (2048 by default, regardless of n_ctx),
    // and a failed GGML_ASSERT is an abort() — it kills the process rather than
    // returning an error we can surface. Only the final token needs logits,
    // since that is the one generation samples from.
    let n_batch = (ctx.n_batch() as usize).max(1);
    let last_index = tokens.len() as i32 - 1;
    let mut batch = LlamaBatch::new(n_batch.min(tokens.len()).max(1), 1);
    for chunk_start in (0..tokens.len()).step_by(n_batch) {
        pressure.check()?;
        let chunk_end = (chunk_start + n_batch).min(tokens.len());
        batch.clear();
        for (offset, token) in tokens[chunk_start..chunk_end].iter().enumerate() {
            let i = (chunk_start + offset) as i32;
            batch
                .add(*token, i, &[0], i == last_index)
                .map_err(|e| RuntimeError::Other(format!("failed to queue prompt token: {e}")))?;
        }
        ctx.decode(&mut batch).map_err(|e| {
            RuntimeError::Other(format!("llama.cpp decode of the prompt failed: {e}"))
        })?;
    }

    let mut sampler = build_sampler(model, options);
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut output = String::new();
    let stops = options.stop.as_deref().unwrap_or_default();
    let mut released = TextRelease::default();
    // Absolute position in the sequence, which after a chunked prefill is the
    // full prompt length rather than the size of the last batch decoded.
    let mut n_cur = tokens.len() as i32;
    let stop_at_token = n_cur + max_new_tokens;

    while n_cur < stop_at_token && n_cur < n_ctx {
        pressure.check()?;
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(token);

        if model.is_eog_token(token) {
            break;
        }

        let piece = model
            .token_to_piece(token, &mut decoder, true, None)
            .map_err(|e| RuntimeError::Other(format!("failed to decode generated token: {e}")))?;
        output.push_str(&piece);

        if let Some(hit_len) = stops
            .iter()
            .find(|s| output.ends_with(s.as_str()))
            .map(|s| s.len())
        {
            output.truncate(output.len() - hit_len);
            break;
        }
        let ready = released.ready(&output, stops);
        if !ready.is_empty() {
            on_text(ready)?;
        }

        batch.clear();
        batch
            .add(token, n_cur, &[0], true)
            .map_err(|e| RuntimeError::Other(format!("failed to queue generated token: {e}")))?;
        n_cur += 1;
        ctx.decode(&mut batch)
            .map_err(|e| RuntimeError::Other(format!("llama.cpp decode failed: {e}")))?;
    }

    let rest = released.finish(&output)?;
    if !rest.is_empty() {
        on_text(rest)?;
    }
    Ok(output.trim().to_string())
}

/// Releases generated text in order without exposing a stop sequence: a
/// trailing fragment that could still grow into one is withheld until the
/// next token resolves it. Everything released is a prefix of the final,
/// stop-truncated output.
#[derive(Debug, Default)]
struct TextRelease {
    released: usize,
}

impl TextRelease {
    /// `output` only grows between calls, except by removing a completed stop.
    fn ready<'o>(&mut self, output: &'o str, stops: &[String]) -> &'o str {
        let end = output.len() - pending_stop_prefix(output, stops);
        if end <= self.released {
            return "";
        }
        let start = std::mem::replace(&mut self.released, end);
        &output[start..end]
    }

    fn finish<'o>(&mut self, output: &'o str) -> Result<&'o str> {
        let rest = output.get(self.released..).ok_or_else(|| {
            RuntimeError::Other("Streamed text no longer matches the generated answer".into())
        })?;
        self.released = output.len();
        Ok(rest)
    }
}

/// Length of the longest suffix of `output` that is an incomplete stop sequence.
fn pending_stop_prefix(output: &str, stops: &[String]) -> usize {
    stops
        .iter()
        .filter_map(|stop| {
            (1..stop.len())
                .rev()
                .filter(|&length| stop.is_char_boundary(length))
                .find(|&length| output.ends_with(&stop[..length]))
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod release_tests {
    use super::*;

    /// Feeds pieces through the same order of operations as `generate`.
    fn stream(pieces: &[&str], stops: &[&str]) -> (Vec<String>, String) {
        let stops: Vec<String> = stops.iter().map(|stop| stop.to_string()).collect();
        let mut release = TextRelease::default();
        let mut output = String::new();
        let mut shown = Vec::new();
        for piece in pieces {
            output.push_str(piece);
            if let Some(stop) = stops.iter().find(|stop| output.ends_with(stop.as_str())) {
                output.truncate(output.len() - stop.len());
                break;
            }
            let ready = release.ready(&output, &stops);
            if !ready.is_empty() {
                shown.push(ready.to_string());
            }
        }
        let rest = release.finish(&output).unwrap();
        if !rest.is_empty() {
            shown.push(rest.to_string());
        }
        (shown, output)
    }

    #[test]
    fn text_is_released_per_token_and_reassembles_the_untrimmed_output() {
        let (shown, output) = stream(&["\n", "Love", " is", " patient", ".\n"], &[]);
        assert_eq!(shown, ["\n", "Love", " is", " patient", ".\n"]);
        assert_eq!(shown.concat(), output);
    }

    #[test]
    fn a_stop_sequence_split_across_tokens_is_never_released() {
        let (shown, output) = stream(&["Answer", "\n\nUs", "er:", " ignored"], &["\n\nUser:"]);
        assert_eq!(output, "Answer");
        assert_eq!(shown.concat(), "Answer");
        assert!(shown.iter().all(|text| !text.contains("Us")));
    }

    #[test]
    fn a_withheld_partial_stop_is_released_when_it_does_not_complete() {
        let (shown, output) = stream(&["A <", "|e", "nd", "ing"], &["<|end|>"]);
        assert_eq!(output, "A <|ending");
        assert_eq!(shown, ["A ", "<|ending"]);
        let (shown, output) = stream(&["Done <|e"], &["<|end|>"]);
        assert_eq!(
            shown,
            ["Done ", "<|e"],
            "generation ended before the stop completed"
        );
        assert_eq!(shown.concat(), output);
    }

    #[test]
    fn multibyte_stop_prefixes_stay_on_character_boundaries() {
        let (shown, output) = stream(&["Café ", "🙂", "🙃 tail"], &["🙂🙂"]);
        assert_eq!(shown.concat(), output);
        assert_eq!(output, "Café 🙂🙃 tail");
        let (shown, output) = stream(&["Café ", "🙂", "🙂"], &["🙂🙂"]);
        assert_eq!(
            (shown.concat(), output.as_str()),
            ("Café ".to_string(), "Café ")
        );
    }

    #[test]
    fn released_text_never_overlaps_a_completed_stop() {
        for split in 1.."xab".len() {
            let (shown, output) = stream(&[&"xab"[..split], &"xab"[split..]], &["ab", "b"]);
            assert_eq!(shown.concat(), output, "split at {split}");
            assert_eq!(output, "x");
        }
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

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

#[cfg(test)]
mod prompt_count_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn local_provider() -> LocalLlm {
        LocalLlm {
            model_path: PathBuf::from("synthetic-model.gguf"),
            context_size: 6144,
            gpu_layers: 0,
            name: "synthetic".into(),
        }
    }

    #[test]
    fn configured_window_is_reported_without_loading_model() {
        assert_eq!(local_provider().context_window(), Some(6144));
    }

    #[tokio::test]
    async fn health_inspects_metadata_without_starting_a_gpu_worker() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("metadata-only.gguf");
        let mut metadata = b"GGUF".to_vec();
        metadata.extend(3_u32.to_le_bytes());
        metadata.extend(0_u64.to_le_bytes());
        metadata.extend(1_u64.to_le_bytes());
        let key = "general.architecture";
        metadata.extend((key.len() as u64).to_le_bytes());
        metadata.extend(key.as_bytes());
        metadata.extend(8_u32.to_le_bytes());
        metadata.extend(5_u64.to_le_bytes());
        metadata.extend(b"llama");
        std::fs::write(&path, metadata).unwrap();
        let mut provider = local_provider();
        provider.model_path = path.clone();
        provider.gpu_layers = ALL_GPU_LAYERS;
        for _ in 0..3 {
            assert!(provider.health_check().await.unwrap());
        }
        std::fs::write(path, b"damaged synthetic metadata").unwrap();
        assert!(provider.health_check().await.is_err());
    }

    #[tokio::test]
    async fn cancelled_count_does_not_start_a_worker() {
        let options = CompletionOptions {
            cancel: Some(Arc::new(AtomicBool::new(true))),
            ..Default::default()
        };
        let error = local_provider()
            .count_prompt_tokens(&[], &options)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"), "{error}");
    }

    #[tokio::test]
    async fn oversized_count_keeps_the_existing_prompt_safety_limit() {
        let options = CompletionOptions {
            system_prompt: Some("x".repeat(3 * 1024 * 1024)),
            ..Default::default()
        };
        let error = local_provider()
            .count_prompt_tokens(&[], &options)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("safety limit"), "{error}");
        let error = tokenize_rendered_prompt(Ok(options.system_prompt.unwrap()), |_, _| {
            panic!("oversized rendered prompts must not reach the tokenizer")
        })
        .unwrap_err();
        assert!(error.to_string().contains("safety limit"), "{error}");
    }

    #[test]
    fn chat_messages_keep_system_options_history_and_unicode() {
        let messages = [
            ChatMessage {
                role: MessageRole::System,
                content: "Message system".into(),
            },
            ChatMessage {
                role: MessageRole::User,
                content: "中文 🦀".into(),
            },
            ChatMessage {
                role: MessageRole::Assistant,
                content: "Previous answer".into(),
            },
            ChatMessage {
                role: MessageRole::User,
                content: "Follow-up?".into(),
            },
        ];
        let options = CompletionOptions {
            system_prompt: Some("Options system".into()),
            ..Default::default()
        };
        let expected = vec![
            new_chat_message("system", "Options system").unwrap(),
            new_chat_message("system", "Message system").unwrap(),
            new_chat_message("user", "中文 🦀").unwrap(),
            new_chat_message("assistant", "Previous answer").unwrap(),
            new_chat_message("user", "Follow-up?").unwrap(),
        ];
        assert_eq!(build_chat_messages(&messages, &options).unwrap(), expected);
    }

    #[test]
    fn invalid_system_or_history_is_an_error_not_a_fallback() {
        let options = CompletionOptions {
            system_prompt: Some("Invalid\0system".into()),
            ..Default::default()
        };
        assert!(build_chat_messages(&[], &options).is_err());
        let messages = [ChatMessage {
            role: MessageRole::Assistant,
            content: "Invalid\0history".into(),
        }];
        assert!(build_chat_messages(&messages, &CompletionOptions::default()).is_err());
    }

    #[test]
    fn shared_tokenization_preserves_rendered_template_and_bos() {
        let rendered = "<s>[SYSTEM]指示[/SYSTEM][USER]中文 🦀[/USER][ASSISTANT]";
        let tokens = tokenize_rendered_prompt(Ok(rendered.into()), |prompt, bos| {
            assert_eq!(prompt, rendered);
            assert_eq!(bos, AddBos::Always);
            Ok(vec![LlamaToken::new(1), LlamaToken::new(2)])
        })
        .unwrap();
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn rendering_and_tokenization_errors_propagate() {
        let error = tokenize_rendered_prompt(
            Err(RuntimeError::Other("synthetic template error".into())),
            |_, _| panic!("must not tokenize a failed template"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("synthetic template error"));
        let error = tokenize_rendered_prompt(Ok("synthetic prompt".into()), |_, _| {
            Err(RuntimeError::Other("synthetic tokenizer error".into()))
        })
        .unwrap_err();
        assert!(error.to_string().contains("synthetic tokenizer error"));
    }

    #[test]
    #[ignore = "requires MODEL_RUNTIME_PROMPT_COUNT_TEST_MODEL and RAM for one native model load"]
    fn native_count_matches_generation_prompt_tokens() {
        let path = PathBuf::from(
            std::env::var_os("MODEL_RUNTIME_PROMPT_COUNT_TEST_MODEL")
                .expect("set MODEL_RUNTIME_PROMPT_COUNT_TEST_MODEL to a local GGUF"),
        );
        let provider = LocalLlm::load(&path, Some(6144), Some(0)).unwrap();
        let mut slot = None;
        let messages = vec![
            ChatMessage {
                role: MessageRole::User,
                content: "Explain the synthetic example 中文 🦀.".into(),
            },
            ChatMessage {
                role: MessageRole::Assistant,
                content: "Synthetic history: café, пример.".into(),
            },
            ChatMessage {
                role: MessageRole::User,
                content: "Summarize it in English.".into(),
            },
        ];
        for system in [None, Some("Answer the synthetic question concisely.")] {
            let options = CompletionOptions {
                system_prompt: system.map(str::to_string),
                ..Default::default()
            };
            let count = count_prompt_tokens_in_process(
                &mut slot,
                &path,
                provider.context_size,
                provider.gpu_layers,
                &messages,
                &options,
            )
            .unwrap();
            let model = &slot.as_ref().unwrap().model;
            let rendered = build_chat_prompt(model, &messages, &options).unwrap();
            let expected = model.str_to_token(&rendered, AddBos::Always).unwrap();
            let generation_input = prepare_prompt_tokens(model, &messages, &options).unwrap();
            assert_eq!(generation_input, expected);
            assert_eq!(count, generation_input.len());
        }
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
    let backend_log = crate::log_tap::snapshot_since_targets(cutoff, &["llama", "ggml"]);
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
