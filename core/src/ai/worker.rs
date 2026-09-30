//! Grafium's native-kernel adapter to the reusable model-runtime supervisor.
//!
//! The runtime owns framing, FIFO admission and subprocess lifetime. Grafium
//! owns request types, memory estimates, model dispatch and native logging.
//! A child boundary cannot protect against system OOM or GPU-driver failures.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use model_runtime::protocol::{self, Response};
use model_runtime::supervisor::{Admission, RequestOptions, Supervisor, SupervisorConfig, Timeouts, WorkerLease};
use model_runtime::recovery::{BlockedModel, GpuAttempt, RecoveryLease, RecoveryStore};
use serde::{Deserialize, Serialize};

#[cfg(feature = "llm-local")]
use crate::ai::traits::{ChatMessage, CompletionOptions};
use crate::error::{CoreError, Result};
#[cfg(feature = "media")]
use crate::media::Transcript;

#[cfg(feature = "media")]
pub use crate::media::TranscribeProgress as WorkerProgress;
pub use model_runtime::supervisor::exit_without_native_cleanup;
#[cfg(not(feature = "media"))]
#[derive(Debug, Serialize, Deserialize)]
pub enum WorkerProgress {}

pub const WORKER_ARGUMENT: &str = "--grafium-native-ai-worker";
static SUPERVISOR: OnceLock<Supervisor<WorkerKey>> = OnceLock::new();
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static RUNTIME_WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static RECOVERY: OnceLock<RecoveryStore> = OnceLock::new();

struct RecoveryWorkerLease(RecoveryLease);

impl WorkerLease for RecoveryWorkerLease {
    fn confirmed_exit(&self, expected: bool) -> model_runtime::error::Result<()> {
        self.0.confirm_exit(expected)
    }
}

pub fn recovery_status() -> Vec<BlockedModel> {
    match RECOVERY.get().map(RecoveryStore::blocked) {
        Some(Ok(blocked)) => blocked,
        Some(Err(error)) => {
            remember_warning(&format!("GPU recovery state cannot be read; GPU attempts are disabled: {error}"));
            Vec::new()
        }
        None => Vec::new(),
    }
}

pub fn allow_gpu_retry(key: &str) -> Result<()> {
    evict_idle()?;
    RECOVERY.get().ok_or_else(|| CoreError::Other("Persistent GPU recovery is unavailable".into()))?
        .allow_once(key)?;
    Ok(())
}

pub fn gpu_risk_key(workload: &str, path: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let path = path.canonicalize()?;
    let metadata = std::fs::metadata(&path)?;
    let modified = metadata.modified()?.duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| CoreError::Other(format!("Invalid model timestamp: {e}")))?;
    let identity = serde_json::to_vec(&(workload, path, metadata.len(), modified.as_nanos(), "native-vulkan"))?;
    Ok(format!("{:x}", Sha256::digest(identity)))
}

fn gpu_request_identity(request: &WorkerRequest) -> Result<Option<(String, String)>> {
    let (workload, path, requested) = match request {
        #[cfg(feature = "llm-local")]
        WorkerRequest::Llm { model_path, gpu_layers, .. }
        | WorkerRequest::CountPrompt { model_path, gpu_layers, .. }
        | WorkerRequest::ValidateLlm { model_path, gpu_layers, .. } =>
            ("chat", model_path, cfg!(feature = "llm-local-vulkan") && *gpu_layers > 0),
        #[cfg(feature = "llm-local")]
        WorkerRequest::Embed { model_path, .. } | WorkerRequest::EmbedderInfo { model_path } =>
            ("embeddings", model_path, cfg!(feature = "llm-local-vulkan")),
        #[cfg(feature = "media")]
        WorkerRequest::Whisper { model_path, .. } =>
            ("transcription", model_path, cfg!(feature = "media-vulkan")),
        WorkerRequest::Shutdown => return Ok(None),
    };
    if !requested || std::env::var_os("GRAFIUM_DISABLE_GPU_OFFLOAD").is_some() {
        return Ok(None);
    }
    let label = format!("{workload}: {}", path.file_name().unwrap_or_default().to_string_lossy());
    Ok(Some((gpu_risk_key(workload, path)?, crate::ai::truncate_to_char_boundary(&label, 256).to_string())))
}

fn admission(request: &WorkerRequest) -> model_runtime::error::Result<Admission> {
    let mut admission = Admission {
        memory_limit_bytes: Some(request_memory_limit(request).map_err(|e| model_runtime::error::RuntimeError::Other(e.to_string()))?),
        ..Default::default()
    };
    let identity = gpu_request_identity(request)
        .map_err(|e| model_runtime::error::RuntimeError::Other(e.to_string()))?;
    if let Some((key, label)) = identity {
        match RECOVERY.get().map(|store| store.prepare(&key, &label)) {
            Some(Ok(GpuAttempt::Allowed(lease))) => {
                admission.lease = Some(Box::new(RecoveryWorkerLease(lease)));
                admission.environment.push(("GRAFIUM_GPU_LEASE_ACTIVE".into(), "1".into()));
            }
            other => {
                let reason = match other {
                    Some(Ok(GpuAttempt::CpuOnly { reason })) => reason,
                    Some(Err(error)) => format!("Persistent GPU recovery failed: {error}"),
                    None => "Persistent GPU recovery was not configured by this host".into(),
                    Some(Ok(GpuAttempt::Allowed(_))) => unreachable!(),
                };
                remember_warning(&format!("{reason}. Using CPU; GPU retry requires explicit approval."));
                admission.environment.push(("GRAFIUM_NATIVE_FORCE_CPU".into(), "1".into()));
            }
        }
    }
    Ok(admission)
}

#[derive(Debug, Serialize, Deserialize)]
pub enum WorkerRequest {
    #[cfg(feature = "llm-local")]
    Llm {
        model_path: PathBuf,
        context_size: u32,
        gpu_layers: u32,
        messages: Vec<ChatMessage>,
        options: CompletionOptions,
    },
    #[cfg(feature = "llm-local")]
    CountPrompt {
        model_path: PathBuf,
        context_size: u32,
        gpu_layers: u32,
        messages: Vec<ChatMessage>,
        options: CompletionOptions,
    },
    #[cfg(feature = "llm-local")]
    ValidateLlm {
        model_path: PathBuf,
        context_size: u32,
        gpu_layers: u32,
    },
    #[cfg(feature = "llm-local")]
    Embed {
        model_path: PathBuf,
        context_size: u32,
        texts: Vec<String>,
    },
    #[cfg(feature = "llm-local")]
    EmbedderInfo {
        model_path: PathBuf,
    },
    #[cfg(feature = "media")]
    Whisper {
        model_path: PathBuf,
        language: Option<String>,
        wav_path: PathBuf,
    },
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum WorkerOutput {
    #[cfg(feature = "llm-local")]
    Llm(String),
    #[cfg(feature = "llm-local")]
    PromptTokenCount(usize),
    #[cfg(feature = "llm-local")]
    Ready,
    #[cfg(feature = "llm-local")]
    Embed(Vec<Vec<f32>>),
    #[cfg(feature = "llm-local")]
    EmbedderInfo { context_size: u32, dimension: usize },
    #[cfg(feature = "media")]
    Whisper(Transcript),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum WorkerEvent {
    Progress(WorkerProgress),
    Warning { warning: String },
    GpuInfo { gpu_info: crate::gpu_info::GpuInfo },
}

type WorkerResponse = Response<WorkerOutput, WorkerEvent>;

pub fn runtime_warnings() -> Vec<String> {
    RUNTIME_WARNINGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn remember_warning(warning: &str) {
    let mut warnings = RUNTIME_WARNINGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    push_warning(&mut warnings, warning);
}

fn push_warning(warnings: &mut Vec<String>, warning: &str) {
    let warning: String = warning.chars().take(2048).collect();
    warnings.retain(|previous| previous != &warning);
    if warnings.len() == 8 {
        warnings.remove(0);
    }
    warnings.push(warning);
}

/// Native fallback diagnostics cross the same typed IPC boundary as progress,
/// without changing the transcription callback's public payload.
pub fn emit_runtime_warning(warning: &str) {
    tracing::warn!("{warning}");
    remember_warning(warning);
    if is_worker_invocation() {
        let response = WorkerResponse {
            output: None,
            error: None,
            progress: Some(WorkerEvent::Warning {
                warning: warning.chars().take(2048).collect(),
            }),
        };
        if let Err(error) = protocol::write_frame(
            &mut io::stdout().lock(),
            &response,
            protocol::DEFAULT_RESPONSE_LIMIT,
        ) {
            tracing::warn!(%error, "native worker could not report a runtime warning");
        }

    }
}

#[cfg(feature = "llm-local")]
pub(crate) fn emit_gpu_info(info: crate::gpu_info::GpuInfo) {
    crate::gpu_info::remember_native_gpu(info.clone());
    if is_worker_invocation() {
        let response = WorkerResponse {
            output: None, error: None, progress: Some(WorkerEvent::GpuInfo { gpu_info: info }),
        };
        if let Err(error) = protocol::write_frame(&mut io::stdout().lock(), &response, protocol::DEFAULT_RESPONSE_LIMIT) {
            tracing::warn!(%error, "Could not report selected GPU");
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WorkerKey {
    #[cfg(feature = "llm-local")]
    Llm {
        model_path: PathBuf,
        context_size: u32,
        gpu_layers: u32,
    },
    #[cfg(feature = "llm-local")]
    Embedder { model_path: PathBuf },
    #[cfg(feature = "media")]
    Whisper {
        model_path: PathBuf,
        language: Option<String>,
    },
}

impl WorkerKey {
    fn from_request(request: &WorkerRequest) -> Result<Self> {
        match request {
            #[cfg(feature = "llm-local")]
            WorkerRequest::Llm {
                model_path,
                context_size,
                gpu_layers,
                ..
            }
            | WorkerRequest::CountPrompt {
                model_path,
                context_size,
                gpu_layers,
                ..
            }
            | WorkerRequest::ValidateLlm {
                model_path,
                context_size,
                gpu_layers,
            } => Ok(Self::Llm {
                model_path: model_path.clone(),
                context_size: *context_size,
                gpu_layers: *gpu_layers,
            }),
            #[cfg(feature = "llm-local")]
            WorkerRequest::Embed { model_path, .. }
            | WorkerRequest::EmbedderInfo { model_path } => Ok(Self::Embedder {
                model_path: model_path.clone(),
            }),
            #[cfg(feature = "media")]
            WorkerRequest::Whisper {
                model_path,
                language,
                ..
            } => Ok(Self::Whisper {
                model_path: model_path.clone(),
                language: language.clone(),
            }),
            WorkerRequest::Shutdown => Err(CoreError::Other(
                "shutdown requests do not identify a worker".into(),
            )),
        }
    }
}

pub fn execute(request: WorkerRequest, timeout: Duration) -> Result<WorkerOutput> {
    execute_with_progress(request, timeout, &mut |_| {})
}

pub fn execute_with_progress(
    request: WorkerRequest,
    timeout: Duration,
    on_progress: &mut dyn FnMut(WorkerProgress),
) -> Result<WorkerOutput> {
    let cancel = request_cancel(&request);
    check_cancelled(cancel)?;
    if SHUTDOWN.load(Ordering::Acquire) {
        return Err(CoreError::Other(
            "Grafium is shutting down; native AI is unavailable".into(),
        ));
    }
    let key = WorkerKey::from_request(&request)?;
    let supervisor = SUPERVISOR.get().ok_or_else(|| {
        CoreError::Other("native AI worker is not configured by this application host".into())
    })?;
    let estimated_bytes = estimated_working_set(&request)?;
    supervisor
        .execute_admitted(
            key,
            &request,
            RequestOptions {
                cancel,
                // Preserve progress-sensitive transcription timeouts, but prevent
                // a faulty child from extending a request forever with heartbeats.
                timeouts: Timeouts {
                    idle: timeout,
                    total: Some(timeout.saturating_mul(6)),
                },
                estimated_bytes,
            },
            || admission(&request),
            &mut |event| match event {
                WorkerEvent::Progress(progress) => on_progress(progress),
                WorkerEvent::Warning { warning } => remember_warning(&warning),
                WorkerEvent::GpuInfo { gpu_info } => crate::gpu_info::remember_native_gpu(gpu_info),
            },
        )
        .map_err(CoreError::from)
}

fn request_cancel(request: &WorkerRequest) -> Option<&AtomicBool> {
    match request {
        #[cfg(feature = "llm-local")]
        WorkerRequest::CountPrompt { options, .. } | WorkerRequest::Llm { options, .. } => {
            options.cancel.as_deref()
        }
        _ => None,
    }
}

pub(crate) fn check_cancelled(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err(CoreError::Other("AI request cancelled".into()));
    }
    Ok(())
}

pub fn shutdown_pool() {
    SHUTDOWN.store(true, Ordering::Release);
    if let Some(supervisor) = SUPERVISOR.get() {
        if let Err(error) = supervisor.shutdown() {
            tracing::error!(%error, "native AI worker shutdown failed");
        }
    }
}

/// Unload a resident model after the host has drained requests, without
/// permanently stopping admission as application shutdown does.
pub fn evict_idle() -> Result<()> {
    if let Some(supervisor) = SUPERVISOR.get() {
        supervisor.evict_idle()?;
    }
    Ok(())
}

pub fn is_worker_invocation() -> bool {
    std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == WORKER_ARGUMENT)
}

pub fn configure_current_executable() -> Result<()> {
    configure(None)
}

pub fn configure_current_executable_with_state(state_dir: &std::path::Path) -> Result<()> {
    configure(Some(state_dir))
}

fn configure(state_dir: Option<&std::path::Path>) -> Result<()> {
    if let Some(directory) = state_dir {
        match RecoveryStore::open(directory) {
            Ok(store) => RECOVERY.set(store)
                .map_err(|_| CoreError::Other("GPU recovery was already configured".into()))?,
            Err(error) => remember_warning(&format!("Persistent GPU recovery is unavailable; using CPU: {error}")),
        }
    }
    let mut config = SupervisorConfig::new(std::env::current_exe()?, &WorkerRequest::Shutdown)?;
    config.arguments.push(WORKER_ARGUMENT.into());
    config.environment.push(("GRAFIUM_NATIVE_PROTOCOL".into(), "2".into()));
    config.memory_pressure = Some(Arc::new(crate::ai::resources::is_memory_pressure_high));
    config.on_diagnostic = Some(Arc::new(remember_warning));
    config.pressure_check = Some(Arc::new(|_| Ok(model_runtime::resources::critical_memory_pressure())));
    let supervisor = Supervisor::new(config)?;
    SUPERVISOR.set(supervisor).map_err(|_| {
        CoreError::Other("native AI worker executable was already configured".into())
    })?;
    if SHUTDOWN.load(Ordering::Acquire) {
        shutdown_pool();
    }
    Ok(())
}

fn estimated_working_set(request: &WorkerRequest) -> Result<u64> {
    request_memory(request, false)
}

fn request_memory_limit(request: &WorkerRequest) -> Result<u64> {
    request_memory(request, true)
}

fn request_memory(request: &WorkerRequest, admit: bool) -> Result<u64> {
    use crate::ai::resources::{estimate_worker_working_set, ModelWorkload};
    let estimate = if admit {
        model_runtime::resources::containment_limit
    } else {
        estimate_worker_working_set
    };
    let result = match request {
        #[cfg(feature = "llm-local")]
        WorkerRequest::Llm {
            model_path,
            context_size,
            ..
        }
        | WorkerRequest::CountPrompt {
            model_path,
            context_size,
            ..
        }
        | WorkerRequest::ValidateLlm {
            model_path,
            context_size,
            ..
        } => estimate(
            model_path,
            ModelWorkload::Llm {
                context_tokens: *context_size,
            },
            0,
        ),
        #[cfg(feature = "llm-local")]
        WorkerRequest::Embed {
            model_path,
            context_size,
            texts,
        } => estimate(
            model_path,
            ModelWorkload::Llm {
                context_tokens: *context_size,
            },
            texts.iter().map(|text| text.len() as u64).sum(),
        ),
        #[cfg(feature = "llm-local")]
        WorkerRequest::EmbedderInfo { model_path } => {
            estimate(model_path, ModelWorkload::Llm { context_tokens: 0 }, 0)
        }
        #[cfg(feature = "media")]
        WorkerRequest::Whisper {
            model_path,
            wav_path,
            ..
        } => {
            let input_bytes = std::fs::metadata(wav_path)
                .map_err(|error| {
                    CoreError::Other(format!(
                        "Cannot inspect audio {}: {error}",
                        wav_path.display()
                    ))
                })?
                .len();
            estimate(model_path, ModelWorkload::Whisper, input_bytes)
        }
        WorkerRequest::Shutdown => {
            return Err(CoreError::Other(
                "shutdown requests do not have a working-set estimate".into(),
            ))
        }
    };
    Ok(result?)
}

fn init_worker_logging() {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(io::stderr))
        .with(crate::log_tap::NativeLogLayer)
        .try_init();
}

/// The child owns no durable state. Explicit shutdown and parent disappearance
/// both bypass process-wide vendor destructors after flushing protocol output.
pub fn run_from_stdio() -> ! {
    if std::env::var("GRAFIUM_NATIVE_PROTOCOL").as_deref() != Ok("2") {
        eprintln!("Native worker requires a matching Grafium runtime. Restart Grafium after upgrading.");
        exit_without_native_cleanup(2);
    }
    init_worker_logging();
    if let Err(error) = model_runtime::supervisor::start_parent_watchdog() {
        eprintln!("native AI worker: cannot monitor parent: {error}");
        exit_without_native_cleanup(3);
    }
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut state = ChildState::default();
    let code = loop {
        let request: WorkerRequest =
            match protocol::read_frame(&mut stdin.lock(), protocol::DEFAULT_REQUEST_LIMIT) {
                Ok(Some(request)) => request,
                Ok(None) => break 0,
                Err(error) => {
                    eprintln!("native AI worker: cannot read request: {error}");
                    break 2;
                }
            };
        if matches!(request, WorkerRequest::Shutdown) {
            break 0;
        }
        let response = match dispatch(&mut state, request) {
            Ok(output) => WorkerResponse {
                output: Some(output),
                error: None,
                progress: None,
            },
            Err(error) => WorkerResponse {
                output: None,
                error: Some(error.to_string()),
                progress: None,
            },
        };
        if let Err(error) = protocol::write_frame(
            &mut stdout.lock(),
            &response,
            protocol::DEFAULT_RESPONSE_LIMIT,
        ) {
            eprintln!("native AI worker: cannot write response: {error}");
            break 2;
        }
    };
    std::mem::forget(state);
    exit_without_native_cleanup(code);
}

#[derive(Default)]
struct ChildState {
    #[cfg(feature = "llm-local")]
    llm: Option<crate::ai::providers::local_llm::LlmSlot>,
    #[cfg(feature = "llm-local")]
    embedder: Option<crate::ai::providers::local_embedder::EmbedderSlot>,
    #[cfg(feature = "media")]
    whisper: Option<crate::media::transcribe::WhisperSlot>,
}

#[cfg(feature = "media")]
fn emit_progress(progress: crate::media::TranscribeProgress) {
    let frame = WorkerResponse {
        output: None,
        error: None,
        progress: Some(WorkerEvent::Progress(progress)),
    };
    if let Err(error) = protocol::write_frame(
        &mut io::stdout().lock(),
        &frame,
        protocol::DEFAULT_RESPONSE_LIMIT,
    ) {
        tracing::warn!(%error, "native AI worker could not report progress");
    }
}

fn dispatch(state: &mut ChildState, request: WorkerRequest) -> Result<WorkerOutput> {
    match request {
        #[cfg(feature = "llm-local")]
        WorkerRequest::Llm {
            model_path,
            context_size,
            gpu_layers,
            messages,
            options,
        } => crate::ai::providers::local_llm::complete_in_process(
            &mut state.llm,
            &model_path,
            context_size,
            gpu_layers,
            &messages,
            &options,
        )
        .map(WorkerOutput::Llm),
        #[cfg(feature = "llm-local")]
        WorkerRequest::CountPrompt {
            model_path,
            context_size,
            gpu_layers,
            messages,
            options,
        } => crate::ai::providers::local_llm::count_prompt_tokens_in_process(
            &mut state.llm,
            &model_path,
            context_size,
            gpu_layers,
            &messages,
            &options,
        )
        .map(WorkerOutput::PromptTokenCount),
        #[cfg(feature = "llm-local")]
        WorkerRequest::ValidateLlm {
            model_path,
            context_size,
            gpu_layers,
        } => crate::ai::providers::local_llm::validate_in_process(
            &mut state.llm,
            &model_path,
            context_size,
            gpu_layers,
        )
        .map(|()| WorkerOutput::Ready),
        #[cfg(feature = "llm-local")]
        WorkerRequest::Embed {
            model_path,
            context_size,
            texts,
        } => crate::ai::providers::local_embedder::embed_in_process(
            &mut state.embedder,
            &model_path,
            context_size,
            &texts,
        )
        .map(WorkerOutput::Embed),
        #[cfg(feature = "llm-local")]
        WorkerRequest::EmbedderInfo { model_path } => {
            crate::ai::providers::local_embedder::info_in_process(&mut state.embedder, &model_path)
                .map(|(context_size, dimension)| WorkerOutput::EmbedderInfo {
                    context_size,
                    dimension,
                })
        }
        #[cfg(feature = "media")]
        WorkerRequest::Whisper {
            model_path,
            language,
            wav_path,
        } => crate::media::transcribe::transcribe_in_process(
            &mut state.whisper,
            &model_path,
            language.as_deref(),
            &wav_path,
            &mut emit_progress,
        )
        .map(WorkerOutput::Whisper),
        WorkerRequest::Shutdown => unreachable!("shutdown handled by the caller"),
    }
}

#[cfg(all(test, feature = "llm-local"))]
mod tests {
    use super::*;
    use crate::ai::traits::MessageRole;

    fn count_request() -> WorkerRequest {
        WorkerRequest::CountPrompt {
            model_path: PathBuf::from("synthetic-model.gguf"),
            context_size: 6144,
            gpu_layers: 0,
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: "中文 🦀".into(),
            }],
            options: CompletionOptions {
                system_prompt: Some("Synthetic system".into()),
                cancel: Some(Arc::new(AtomicBool::new(false))),
                ..Default::default()
            },
        }
    }

    #[test]
    fn cancellation_is_checked_before_configuration_or_loading_models() {
        let request = count_request();
        request_cancel(&request)
            .unwrap()
            .store(true, Ordering::Release);
        assert!(execute(request, Duration::from_secs(60))
            .unwrap_err()
            .to_string()
            .contains("cancelled"));
    }

    #[test]
    fn count_request_preserves_prompt_and_shares_generation_pool_key() {
        let count = count_request();
        let mut frame = Vec::new();
        protocol::write_frame(&mut frame, &count, protocol::DEFAULT_REQUEST_LIMIT).unwrap();
        let decoded: WorkerRequest =
            protocol::read_frame(&mut frame.as_slice(), protocol::DEFAULT_REQUEST_LIMIT)
                .unwrap()
                .unwrap();
        assert_eq!(
            serde_json::to_value(&count).unwrap(),
            serde_json::to_value(&decoded).unwrap()
        );
        let WorkerRequest::CountPrompt {
            model_path,
            context_size,
            gpu_layers,
            messages,
            options,
        } = decoded
        else {
            panic!("wrong request type")
        };
        assert!(options.cancel.is_none(), "live handles cannot cross IPC");
        let generation = WorkerRequest::Llm {
            model_path: model_path.clone(),
            context_size,
            gpu_layers,
            messages,
            options,
        };
        let validation = WorkerRequest::ValidateLlm {
            model_path,
            context_size,
            gpu_layers,
        };
        assert_eq!(
            WorkerKey::from_request(&count).unwrap(),
            WorkerKey::from_request(&generation).unwrap()
        );
        assert_eq!(
            WorkerKey::from_request(&count).unwrap(),
            WorkerKey::from_request(&validation).unwrap()
        );
    }

    #[test]
    fn grafium_terminal_payloads_use_the_shared_protocol() {
        let mut frame = Vec::new();
        protocol::write_frame(
            &mut frame,
            &WorkerResponse {
                output: Some(WorkerOutput::PromptTokenCount(8536)),
                error: None,
                progress: None,
            },
            protocol::DEFAULT_RESPONSE_LIMIT,
        )
        .unwrap();
        let decoded: WorkerResponse =
            protocol::read_frame(&mut frame.as_slice(), protocol::DEFAULT_RESPONSE_LIMIT)
                .unwrap()
                .unwrap();
        assert!(matches!(
            decoded.into_output().unwrap(),
            WorkerOutput::PromptTokenCount(8536)
        ));
        for payload in [
            br#"{"output":null,"error":null}"#.as_slice(),
            br#"{"output":{"PromptTokenCount":4},"error":"conflict"}"#,
            br#"{"output":null,"error":"synthetic failure"}"#,
        ] {
            assert!(serde_json::from_slice::<WorkerResponse>(payload)
                .unwrap()
                .into_output()
                .is_err());
        }
    }

    #[test]
    fn gpu_fallback_warning_crosses_ipc_without_becoming_transcription_progress() {
        let response = WorkerResponse {
            output: None,
            error: None,
            progress: Some(WorkerEvent::Warning {
                warning: "Synthetic CPU fallback".into(),
            }),
        };
        let response: WorkerResponse =
            serde_json::from_slice(&serde_json::to_vec(&response).unwrap()).unwrap();
        let protocol::Event::Progress(WorkerEvent::Warning { warning }) =
            response.into_event().unwrap()
        else {
            panic!("runtime warning was not preserved")
        };
        assert_eq!(warning, "Synthetic CPU fallback");
        remember_warning(&warning);
        assert!(runtime_warnings().contains(&warning));
    }

    #[test]
    fn runtime_warnings_are_bounded_deduplicated_and_ordered_by_latest_occurrence() {
        let mut warnings = Vec::new();
        for number in 0..8 {
            push_warning(&mut warnings, &format!("warning {number}"));
        }
        push_warning(&mut warnings, "warning 0");
        assert_eq!(warnings.len(), 8);
        assert_eq!(warnings.last().unwrap(), "warning 0");
        push_warning(&mut warnings, "new warning");
        assert_eq!(warnings.len(), 8);
        assert!(!warnings.iter().any(|warning| warning == "warning 1"));
        let long_warning = "🦀".repeat(3000);
        push_warning(&mut warnings, &long_warning);
        push_warning(&mut warnings, &long_warning);
        assert_eq!(warnings.len(), 8);
        assert_eq!(warnings.last().unwrap().chars().count(), 2048);
    }

    #[cfg(feature = "media")]
    #[test]
    fn grafium_progress_payload_uses_shared_envelope() {
        let response = WorkerResponse {
            output: None,
            error: None,
            progress: Some(WorkerEvent::Progress(crate::media::TranscribeProgress {
                percent: 50,
                message: "Synthetic progress".into(),
            })),
        };
        let response: WorkerResponse =
            serde_json::from_slice(&serde_json::to_vec(&response).unwrap()).unwrap();
        assert!(response.is_progress());
    }
}
