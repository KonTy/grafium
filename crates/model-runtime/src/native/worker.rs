//! Shared native dispatch, recovery and application-wide worker lifecycle.
//!
//! Hosts supply the executable, worker argument, policy and private state
//! directory; kernels, framing, admission and native diagnostics live here.
//! A child boundary cannot protect against system OOM or GPU-driver failures.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use super::model_file::{canonical_model_path, lock_for_admission, ModelFileLease};
use crate::protocol::{self, Response};
use crate::recovery::{BlockedModel, GpuAttempt, RecoveryLease, RecoveryStore};
use crate::supervisor::{
    Admission, RequestOptions, Supervisor, SupervisorConfig, Timeouts, WorkerLease,
};
use serde::{Deserialize, Serialize};

use crate::error::{Result, RuntimeError};
#[cfg(feature = "media")]
use crate::transcription::Transcript;
#[cfg(feature = "llm-local")]
use crate::types::{ChatMessage, CompletionOptions};

pub use crate::supervisor::exit_without_native_cleanup;
#[cfg(feature = "media")]
pub use crate::transcription::TranscribeProgress as WorkerProgress;
#[cfg(not(feature = "media"))]
#[derive(Debug, Serialize, Deserialize)]
pub enum WorkerProgress {}

pub const WORKER_ARGUMENT: &str = "--model-runtime-native-worker";
const PROTOCOL_VERSION: &str = "4";
static HOST: OnceLock<NativeHostConfig> = OnceLock::new();
static SUPERVISOR: OnceLock<Supervisor<WorkerKey>> = OnceLock::new();
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static RUNTIME_WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static RECOVERY: OnceLock<RecoveryStore> = OnceLock::new();
static RESIDENT: Mutex<Option<ResidentStatus>> = Mutex::new(None);

pub struct NativeHostConfig {
    pub executable: PathBuf,
    pub worker_argument: OsString,
    pub recovery_state_dir: Option<PathBuf>,
    /// Optional bounded external sink. Warnings are retained here already;
    /// the callback must not re-enter `emit_runtime_warning`.
    pub on_diagnostic: Option<crate::supervisor::DiagnosticCallback>,
    pub force_cpu: bool,
    pub inference_threads: Option<usize>,
}

impl NativeHostConfig {
    pub fn new(executable: PathBuf, worker_argument: impl Into<OsString>) -> Self {
        Self {
            executable,
            worker_argument: worker_argument.into(),
            recovery_state_dir: None,
            on_diagnostic: None,
            force_cpu: false,
            inference_threads: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeModelKind {
    Chat,
    Embeddings,
    Transcription,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidentModel {
    pub kind: NativeModelKind,
    pub model_path: PathBuf,
    pub context_size: Option<u32>,
    pub on_gpu: bool,
    pub gpu_layers: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidentStatus {
    pub worker_pid: u32,
    pub model: ResidentModel,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeStatus {
    pub configured: bool,
    pub shutting_down: bool,
    pub worker_pid: Option<u32>,
    pub queued_requests: usize,
    pub resident: Option<ResidentStatus>,
}

/// A load confirmed by the child, tied to the currently registered process.
/// Preparing a provider or selecting GPU settings never populates this value.
pub fn resident_status() -> Option<ResidentStatus> {
    let pid = SUPERVISOR.get()?.worker_pid()?;
    let resident = RESIDENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()?;
    if resident.worker_pid != pid {
        return None;
    }
    let process_id = sysinfo::Pid::from_u32(pid);
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[process_id]), true);
    let process = system.process(process_id)?;
    if matches!(
        process.status(),
        sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
    ) {
        return None;
    }
    Some(resident)
}

pub fn status() -> NativeStatus {
    NativeStatus {
        configured: SUPERVISOR.get().is_some(),
        shutting_down: SHUTDOWN.load(Ordering::Acquire),
        worker_pid: SUPERVISOR.get().and_then(Supervisor::worker_pid),
        queued_requests: SUPERVISOR.get().map_or(0, Supervisor::queued_requests),
        resident: resident_status(),
    }
}

pub(crate) fn configured_threads() -> Option<usize> {
    HOST.get().and_then(|host| host.inference_threads)
}

pub(crate) fn cpu_disabled() -> bool {
    HOST.get().is_some_and(|host| host.force_cpu)
        || std::env::var_os("MODEL_RUNTIME_DISABLE_GPU_OFFLOAD").is_some()
}

pub(crate) fn emit_resident(model: ResidentModel) {
    let response = WorkerResponse {
        output: None,
        error: None,
        progress: Some(WorkerEvent::Resident { resident: model }),
    };
    if is_worker_invocation() {
        if let Err(error) = protocol::write_frame(
            &mut io::stdout().lock(),
            &response,
            protocol::DEFAULT_RESPONSE_LIMIT,
        ) {
            tracing::warn!(%error, "Could not report native model residency");
        }
    }
}

struct RecoveryWorkerLease(RecoveryLease);

impl WorkerLease for RecoveryWorkerLease {
    fn confirmed_exit(&self, expected: bool) -> crate::error::Result<()> {
        self.0.confirm_exit(expected)
    }
}

struct CpuRecoveryLease {
    store: RecoveryStore,
    key: String,
}

impl WorkerLease for CpuRecoveryLease {
    fn confirmed_exit(&self, _expected: bool) -> Result<()> {
        // A CPU worker's outcome must not change the GPU retry budget.
        Ok(())
    }

    fn reusable(&self) -> bool {
        // Keep safe CPU fallback on journal/lock errors, but do not pin a
        // formerly denied GPU request to CPU after another host releases it.
        !self.store.gpu_available(&self.key).unwrap_or(false)
    }
}

pub fn recovery_status() -> Vec<BlockedModel> {
    match RECOVERY.get().map(RecoveryStore::blocked) {
        Some(Ok(blocked)) => blocked,
        Some(Err(error)) => {
            remember_warning(&format!(
                "GPU recovery state cannot be read; GPU attempts are disabled: {error}"
            ));
            Vec::new()
        }
        None => Vec::new(),
    }
}

pub fn allow_gpu_retry(key: &str) -> Result<()> {
    evict_idle()?;
    RECOVERY
        .get()
        .ok_or_else(|| RuntimeError::Other("Persistent GPU recovery is unavailable".into()))?
        .allow_once(key)?;
    Ok(())
}

pub fn use_cpu(key: &str) -> Result<()> {
    RECOVERY
        .get()
        .ok_or_else(|| RuntimeError::Other("Persistent GPU recovery is unavailable".into()))?
        .use_cpu(key)
}

pub fn gpu_risk_key(workload: &str, path: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let path = path.canonicalize()?;
    let metadata = std::fs::metadata(&path)?;
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| RuntimeError::Other(format!("Invalid model timestamp: {e}")))?;
    let identity = serde_json::to_vec(&(
        workload,
        path,
        metadata.len(),
        modified.as_nanos(),
        "native-vulkan",
    ))?;
    Ok(format!("{:x}", Sha256::digest(identity)))
}

fn gpu_request_identity(request: &WorkerRequest) -> Result<Option<(String, String)>> {
    let (workload, path, requested) = match request {
        #[cfg(feature = "llm-local")]
        WorkerRequest::Llm {
            model_path,
            gpu_layers,
            ..
        }
        | WorkerRequest::CountPrompt {
            model_path,
            gpu_layers,
            ..
        }
        | WorkerRequest::ValidateLlm {
            model_path,
            gpu_layers,
            ..
        } => (
            "chat",
            model_path,
            cfg!(feature = "llm-local-vulkan") && *gpu_layers > 0,
        ),
        #[cfg(feature = "llm-local")]
        WorkerRequest::Embed { model_path, .. } | WorkerRequest::EmbedderInfo { model_path } => {
            ("embeddings", model_path, cfg!(feature = "llm-local-vulkan"))
        }
        #[cfg(feature = "llm-local")]
        WorkerRequest::EmbedConfigured {
            model_path,
            gpu_layers,
            ..
        } => (
            "embeddings",
            model_path,
            cfg!(feature = "llm-local-vulkan") && *gpu_layers > 0,
        ),
        #[cfg(feature = "media")]
        WorkerRequest::Whisper { model_path, .. } => {
            ("transcription", model_path, cfg!(feature = "media-vulkan"))
        }
        WorkerRequest::Shutdown => return Ok(None),
    };
    if !requested || cpu_disabled() {
        return Ok(None);
    }
    let label = format!(
        "{workload}: {}",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    let mut end = label.len().min(256);
    while !label.is_char_boundary(end) {
        end -= 1;
    }
    Ok(Some((
        gpu_risk_key(workload, path)?,
        label[..end].to_owned(),
    )))
}

fn admission(request: &WorkerRequest) -> crate::error::Result<Admission> {
    let model = lock_for_admission(request.model_path()?)?;
    if model.path() != request.model_path()? {
        return Err(RuntimeError::Other(
            "Model path changed during admission; retry after the file update completes".into(),
        ));
    }
    let mut admission = Admission {
        memory_limit_bytes: Some(
            request_memory_limit(request)
                .map_err(|e| crate::error::RuntimeError::Other(e.to_string()))?,
        ),
        ..Default::default()
    };
    let identity = gpu_request_identity(request)
        .map_err(|e| crate::error::RuntimeError::Other(e.to_string()))?;
    let mut recovery: Option<Box<dyn WorkerLease>> = None;
    if let Some((key, label)) = identity {
        match RECOVERY.get().map(|store| store.prepare(&key, &label)) {
            Some(Ok(GpuAttempt::Allowed(lease))) => {
                recovery = Some(Box::new(RecoveryWorkerLease(lease)));
                admission
                    .environment
                    .push(("MODEL_RUNTIME_GPU_LEASE_ACTIVE".into(), "1".into()));
            }
            other => {
                let reason = match other {
                    Some(Ok(GpuAttempt::CpuOnly { reason })) => reason,
                    Some(Err(error)) => format!("Persistent GPU recovery failed: {error}"),
                    None => "Persistent GPU recovery was not configured by this host".into(),
                    Some(Ok(GpuAttempt::Allowed(_))) => unreachable!(),
                };
                remember_warning(&format!("{reason} Using CPU for this worker."));
                recovery = RECOVERY.get().map(|store| {
                    Box::new(CpuRecoveryLease {
                        store: store.clone(),
                        key,
                    }) as Box<dyn WorkerLease>
                });
                admission
                    .environment
                    .push(("MODEL_RUNTIME_NATIVE_FORCE_CPU".into(), "1".into()));
            }
        }
    }
    admission.lease = Some(Box::new(ModelFileLease::new(model, recovery)));
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
        /// Send generated text as ordered `Text` events before the result.
        stream: bool,
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
    EmbedConfigured {
        model_path: PathBuf,
        context_size: u32,
        gpu_layers: u32,
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

impl WorkerRequest {
    fn model_path(&self) -> Result<&std::path::Path> {
        match self {
            #[cfg(feature = "llm-local")]
            Self::Llm { model_path, .. }
            | Self::CountPrompt { model_path, .. }
            | Self::ValidateLlm { model_path, .. }
            | Self::Embed { model_path, .. }
            | Self::EmbedConfigured { model_path, .. }
            | Self::EmbedderInfo { model_path } => Ok(model_path),
            #[cfg(feature = "media")]
            Self::Whisper { model_path, .. } => Ok(model_path),
            Self::Shutdown => Err(RuntimeError::Other(
                "Shutdown requests have no model file".into(),
            )),
        }
    }

    fn canonicalize_model_path(&mut self) -> Result<()> {
        let path = match self {
            #[cfg(feature = "llm-local")]
            Self::Llm { model_path, .. }
            | Self::CountPrompt { model_path, .. }
            | Self::ValidateLlm { model_path, .. }
            | Self::Embed { model_path, .. }
            | Self::EmbedConfigured { model_path, .. }
            | Self::EmbedderInfo { model_path } => model_path,
            #[cfg(feature = "media")]
            Self::Whisper { model_path, .. } => model_path,
            Self::Shutdown => {
                return Err(RuntimeError::Other(
                    "Shutdown requests have no model file".into(),
                ))
            }
        };
        *path = canonical_model_path(path)?;
        Ok(())
    }
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
    Warning {
        warning: String,
    },
    GpuInfo {
        gpu_info: crate::gpu_info::GpuInfo,
    },
    Resident {
        resident: ResidentModel,
    },
    /// Provisional generated text; discarded by callers if the request fails.
    Text {
        text: String,
    },
}

type WorkerResponse = Response<WorkerOutput, WorkerEvent>;

pub fn runtime_warnings() -> Vec<String> {
    RUNTIME_WARNINGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn remember_warning(warning: &str) {
    {
        let mut warnings = RUNTIME_WARNINGS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        push_warning(&mut warnings, warning);
    }
    if let Some(report) = HOST.get().and_then(|host| host.on_diagnostic.as_ref()) {
        report(warning);
    }
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
            output: None,
            error: None,
            progress: Some(WorkerEvent::GpuInfo { gpu_info: info }),
        };
        if let Err(error) = protocol::write_frame(
            &mut io::stdout().lock(),
            &response,
            protocol::DEFAULT_RESPONSE_LIMIT,
        ) {
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
    Embedder {
        model_path: PathBuf,
        context_size: u32,
        gpu_layers: u32,
    },
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
            WorkerRequest::Embed {
                model_path,
                context_size,
                ..
            } => Ok(Self::Embedder {
                model_path: model_path.clone(),
                context_size: *context_size,
                gpu_layers: if cfg!(feature = "llm-local-vulkan") {
                    super::llama_shared::OFFLOAD_ALL_LAYERS
                } else {
                    0
                },
            }),
            #[cfg(feature = "llm-local")]
            WorkerRequest::EmbedConfigured {
                model_path,
                context_size,
                gpu_layers,
                ..
            } => Ok(Self::Embedder {
                model_path: model_path.clone(),
                context_size: *context_size,
                gpu_layers: *gpu_layers,
            }),
            #[cfg(feature = "llm-local")]
            WorkerRequest::EmbedderInfo { model_path } => Ok(Self::Embedder {
                model_path: model_path.clone(),
                context_size: 0,
                gpu_layers: if cfg!(feature = "llm-local-vulkan") {
                    super::llama_shared::OFFLOAD_ALL_LAYERS
                } else {
                    0
                },
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
            WorkerRequest::Shutdown => Err(RuntimeError::Other(
                "shutdown requests do not identify a worker".into(),
            )),
        }
    }
}

pub fn execute(request: WorkerRequest, timeout: Duration) -> Result<WorkerOutput> {
    execute_with_events(request, timeout, &mut |_| {}, &mut |_| {})
}

pub fn execute_with_progress(
    request: WorkerRequest,
    timeout: Duration,
    on_progress: &mut dyn FnMut(WorkerProgress),
) -> Result<WorkerOutput> {
    execute_with_events(request, timeout, on_progress, &mut |_| {})
}

/// Delivers a streaming request's generated text in order before its output.
/// Text is provisional: discard it if this returns an error.
pub fn execute_streaming(
    request: WorkerRequest,
    timeout: Duration,
    on_text: &mut dyn FnMut(&str),
) -> Result<WorkerOutput> {
    execute_with_events(request, timeout, &mut |_| {}, on_text)
}

fn execute_with_events(
    mut request: WorkerRequest,
    timeout: Duration,
    on_progress: &mut dyn FnMut(WorkerProgress),
    on_text: &mut dyn FnMut(&str),
) -> Result<WorkerOutput> {
    check_cancelled(request_cancel(&request))?;
    if SHUTDOWN.load(Ordering::Acquire) {
        return Err(RuntimeError::Other(
            "host is shutting down; native AI is unavailable".into(),
        ));
    }
    request.canonicalize_model_path()?;
    let cancel = request_cancel(&request);
    let key = WorkerKey::from_request(&request)?;
    let supervisor = SUPERVISOR.get().ok_or_else(|| {
        RuntimeError::Other("native AI worker is not configured by this application host".into())
    })?;
    let estimated_bytes = estimated_working_set(&request)?;
    let mut confirmed_resident = None;
    let result = supervisor.execute_admitted(
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
            WorkerEvent::Text { text } => on_text(&text),
            WorkerEvent::GpuInfo { gpu_info } => crate::gpu_info::remember_native_gpu(gpu_info),
            WorkerEvent::Resident { resident } => {
                confirmed_resident = supervisor.worker_pid().map(|worker_pid| ResidentStatus {
                    worker_pid,
                    model: resident,
                });
            }
        },
    );
    match &result {
        Ok(_) => {
            if let Some(resident) =
                confirmed_resident.filter(|state| Some(state.worker_pid) == supervisor.worker_pid())
            {
                *RESIDENT.lock().unwrap_or_else(PoisonError::into_inner) = Some(resident);
            }
        }
        Err(_) => {
            let pid = supervisor.worker_pid();
            let mut resident = RESIDENT.lock().unwrap_or_else(PoisonError::into_inner);
            if resident
                .as_ref()
                .is_some_and(|state| Some(state.worker_pid) != pid)
            {
                *resident = None;
            }
        }
    }
    result
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
        return Err(RuntimeError::Cancelled);
    }
    Ok(())
}

/// Permanently stop native admission and surface any incomplete cleanup.
/// Use `evict_idle` rather than shutdown when changing settings.
pub fn shutdown() -> Result<()> {
    SHUTDOWN.store(true, Ordering::Release);
    if let Some(supervisor) = SUPERVISOR.get() {
        supervisor.shutdown()?;
    }
    super::model_file::ensure_exit_confirmed()?;
    *RESIDENT.lock().unwrap_or_else(PoisonError::into_inner) = None;
    Ok(())
}

/// Logging compatibility wrapper for application exit hooks.
pub fn shutdown_pool() {
    if let Err(error) = shutdown() {
        tracing::error!(%error, "native AI worker shutdown failed");
    }
}

/// Unload a resident model after the host has drained requests, without
/// permanently stopping admission as application shutdown does.
pub fn evict_idle() -> Result<()> {
    if let Some(supervisor) = SUPERVISOR.get() {
        supervisor.evict_idle()?;
    }
    *RESIDENT.lock().unwrap_or_else(PoisonError::into_inner) = None;
    Ok(())
}

pub fn is_worker_invocation() -> bool {
    std::env::args_os().nth(1).is_some_and(|arg| {
        arg == std::env::var_os("MODEL_RUNTIME_WORKER_ARGUMENT")
            .unwrap_or_else(|| WORKER_ARGUMENT.into())
    })
}

pub fn configure_with_options(options: NativeHostConfig) -> Result<()> {
    if options.worker_argument.is_empty() {
        return Err(RuntimeError::Other(
            "Native worker argument must not be empty".into(),
        ));
    }
    if HOST.get().is_some() || SUPERVISOR.get().is_some() {
        return Err(RuntimeError::Other(
            "Native host was already configured".into(),
        ));
    }
    let mut config = SupervisorConfig::new(options.executable.clone(), &WorkerRequest::Shutdown)?;
    config.arguments.push(options.worker_argument.clone());
    config.environment.push((
        "MODEL_RUNTIME_WORKER_ARGUMENT".into(),
        options.worker_argument.clone(),
    ));
    config.environment.push((
        "MODEL_RUNTIME_NATIVE_PROTOCOL".into(),
        PROTOCOL_VERSION.into(),
    ));
    if options.force_cpu {
        config
            .environment
            .push(("MODEL_RUNTIME_DISABLE_GPU_OFFLOAD".into(), "1".into()));
    }
    if let Some(threads) = options.inference_threads {
        config.environment.push((
            "MODEL_RUNTIME_LLM_THREADS".into(),
            threads.to_string().into(),
        ));
    }
    HOST.set(options)
        .map_err(|_| RuntimeError::Other("Native host was already configured".into()))?;
    if let Some(directory) = HOST.get().and_then(|host| host.recovery_state_dir.as_ref()) {
        match RecoveryStore::open(directory) {
            Ok(store) => RECOVERY
                .set(store)
                .map_err(|_| RuntimeError::Other("GPU recovery was already configured".into()))?,
            Err(error) => remember_warning(&format!(
                "Persistent GPU recovery is unavailable; using CPU: {error}"
            )),
        }
    }
    config.memory_pressure = Some(Arc::new(crate::native::policy::is_memory_pressure_high));
    config.on_diagnostic = Some(Arc::new(remember_warning));
    config.pressure_check = Some(Arc::new(|_| {
        Ok(crate::resources::critical_memory_pressure())
    }));
    let supervisor = Supervisor::new(config)?;
    SUPERVISOR.set(supervisor).map_err(|_| {
        RuntimeError::Other("native AI worker executable was already configured".into())
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
    use crate::native::policy::{estimate_worker_working_set, ModelWorkload};
    let estimate = if admit {
        crate::resources::containment_limit
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
        }
        | WorkerRequest::EmbedConfigured {
            model_path,
            context_size,
            texts,
            ..
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
                    RuntimeError::Other(format!(
                        "Cannot inspect audio {}: {error}",
                        wav_path.display()
                    ))
                })?
                .len();
            estimate(model_path, ModelWorkload::Whisper, input_bytes)
        }
        WorkerRequest::Shutdown => {
            return Err(RuntimeError::Other(
                "shutdown requests do not have a working-set estimate".into(),
            ))
        }
    };
    result
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
    if std::env::var("MODEL_RUNTIME_NATIVE_PROTOCOL").as_deref() != Ok(PROTOCOL_VERSION) {
        eprintln!(
            "Native worker requires a matching model runtime. Restart the host after upgrading."
        );
        exit_without_native_cleanup(2);
    }
    init_worker_logging();
    if let Err(error) = crate::supervisor::start_parent_watchdog() {
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
    llm: Option<crate::native::llm::LlmSlot>,
    #[cfg(feature = "llm-local")]
    embedder: Option<crate::native::embedder::EmbedderSlot>,
    #[cfg(feature = "media")]
    whisper: Option<crate::native::transcribe::WhisperSlot>,
}

#[cfg(feature = "media")]
fn emit_progress(progress: crate::transcription::TranscribeProgress) {
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

#[cfg(feature = "llm-local")]
fn emit_text(text: &str) -> Result<()> {
    let frame = WorkerResponse {
        output: None,
        error: None,
        progress: Some(WorkerEvent::Text { text: text.into() }),
    };
    // A lost piece would silently corrupt the answer assembled by the host.
    protocol::write_frame(
        &mut io::stdout().lock(),
        &frame,
        protocol::DEFAULT_RESPONSE_LIMIT,
    )
    .map_err(|error| RuntimeError::Other(format!("Could not stream generated text: {error}")))
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
            stream,
        } => crate::native::llm::complete_in_process(
            &mut state.llm,
            &model_path,
            context_size,
            gpu_layers,
            &messages,
            &options,
            &mut |text| if stream { emit_text(text) } else { Ok(()) },
        )
        .map(WorkerOutput::Llm),
        #[cfg(feature = "llm-local")]
        WorkerRequest::CountPrompt {
            model_path,
            context_size,
            gpu_layers,
            messages,
            options,
        } => crate::native::llm::count_prompt_tokens_in_process(
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
        } => crate::native::llm::validate_in_process(
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
        } => crate::native::embedder::embed_in_process(
            &mut state.embedder,
            &model_path,
            context_size,
            &texts,
        )
        .map(WorkerOutput::Embed),
        #[cfg(feature = "llm-local")]
        WorkerRequest::EmbedConfigured {
            model_path,
            context_size,
            gpu_layers,
            texts,
        } => crate::native::embedder::embed_configured_in_process(
            &mut state.embedder,
            &model_path,
            context_size,
            gpu_layers,
            &texts,
        )
        .map(WorkerOutput::Embed),
        #[cfg(feature = "llm-local")]
        WorkerRequest::EmbedderInfo { model_path } => {
            crate::native::embedder::info_in_process(&mut state.embedder, &model_path).map(
                |(context_size, dimension)| WorkerOutput::EmbedderInfo {
                    context_size,
                    dimension,
                },
            )
        }
        #[cfg(feature = "media")]
        WorkerRequest::Whisper {
            model_path,
            language,
            wav_path,
        } => crate::native::transcribe::transcribe_in_process(
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
    use crate::types::MessageRole;

    #[test]
    fn cached_cpu_rechecks_eligibility_without_spending_credit_or_changing_gpu_failures() {
        let directory = tempfile::tempdir().unwrap();
        let store = RecoveryStore::open(directory.path().join("synthetic-recovery")).unwrap();
        let prepare = || match store.prepare("synthetic", "fixture").unwrap() {
            GpuAttempt::Allowed(lease) => lease,
            _ => panic!("expected eligible GPU attempt"),
        };
        let first = prepare();
        let cpu = CpuRecoveryLease {
            store: store.clone(),
            key: "synthetic".into(),
        };
        assert!(
            cpu.reusable(),
            "another active worker prevents GPU admission"
        );
        first.confirm_exit(false).unwrap();
        for _ in 0..3 {
            assert!(
                !cpu.reusable(),
                "CPU cache must yield to the automatic attempt"
            );
        }
        let trial = prepare();
        assert!(cpu.reusable(), "active recovery cannot be duplicated");
        trial.confirm_exit(false).unwrap();
        assert!(cpu.reusable(), "exhausted recovery keeps CPU cached");
        cpu.confirmed_exit(false).unwrap();
        assert_eq!(
            store.blocked().unwrap()[0].state,
            crate::recovery::RecoveryState::CpuOnly
        );
        store.allow_once("synthetic").unwrap();
        assert!(!cpu.reusable(), "manual retry must also bypass cached CPU");
        std::fs::write(
            directory.path().join("synthetic-recovery/journal.json"),
            b"{",
        )
        .unwrap();
        assert!(
            cpu.reusable(),
            "damaged recovery must keep safe CPU fallback"
        );
    }
    #[test]
    fn embedding_context_and_gpu_choices_are_part_of_the_reuse_key() {
        let request = |context_size, gpu_layers| WorkerRequest::EmbedConfigured {
            model_path: "synthetic.gguf".into(),
            context_size,
            gpu_layers,
            texts: vec!["synthetic input".into()],
        };
        let cpu = WorkerKey::from_request(&request(512, 0)).unwrap();
        assert_ne!(cpu, WorkerKey::from_request(&request(1024, 0)).unwrap());
        assert_ne!(cpu, WorkerKey::from_request(&request(512, 8)).unwrap());
        assert_eq!(cpu, WorkerKey::from_request(&request(512, 0)).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn model_alias_is_resolved_before_keying_or_serializing_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("actual.gguf");
        let alias = dir.path().join("selected.gguf");
        std::fs::write(&target, b"synthetic data, never loaded").unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let mut request = WorkerRequest::EmbedderInfo { model_path: alias };
        request.canonicalize_model_path().unwrap();
        assert_eq!(
            request.model_path().unwrap(),
            target.canonicalize().unwrap()
        );
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(
            encoded["EmbedderInfo"]["model_path"],
            target.canonicalize().unwrap().to_str().unwrap()
        );
    }

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
            stream: true,
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
    fn native_terminal_payloads_use_the_shared_protocol() {
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
    fn streamed_text_crosses_ipc_in_order_without_becoming_another_event() {
        for text in [
            "Love",
            " is",
            " 🙂 patient",
            "",
            "{\"warning\":\"not a warning\"}",
        ] {
            let response = WorkerResponse {
                output: None,
                error: None,
                progress: Some(WorkerEvent::Text { text: text.into() }),
            };
            let response: WorkerResponse =
                serde_json::from_slice(&serde_json::to_vec(&response).unwrap()).unwrap();
            let protocol::Event::Progress(WorkerEvent::Text { text: decoded }) =
                response.into_event().unwrap()
            else {
                panic!("streamed text was decoded as a different event")
            };
            assert_eq!(decoded, text);
        }
        let warning: WorkerResponse = serde_json::from_slice(
            br#"{"output":null,"error":null,"progress":{"warning":"Synthetic"}}"#,
        )
        .unwrap();
        assert!(matches!(
            warning.into_event().unwrap(),
            protocol::Event::Progress(WorkerEvent::Warning { .. })
        ));
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
    fn native_progress_payload_uses_shared_envelope() {
        let response = WorkerResponse {
            output: None,
            error: None,
            progress: Some(WorkerEvent::Progress(
                crate::transcription::TranscribeProgress {
                    percent: 50,
                    message: "Synthetic progress".into(),
                },
            )),
        };
        let response: WorkerResponse =
            serde_json::from_slice(&serde_json::to_vec(&response).unwrap()).unwrap();
        assert!(response.is_progress());
    }
}
