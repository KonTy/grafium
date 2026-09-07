//! Supervised subprocess boundary for native llama.cpp and whisper.cpp work.
//!
//! At most one worker child runs at a time. It persists across requests so the
//! resident native model is reused when the next request wants the same one.
//! The worker is evicted on model-key mismatch, idle timeout, memory pressure,
//! IPC failure, timeout, cancellation, or parent shutdown, and native crashes
//! remain contained inside the child.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

#[cfg(feature = "llm-local")]
use crate::ai::traits::{ChatMessage, CompletionOptions};
use crate::error::{CoreError, Result};
#[cfg(feature = "media")]
use crate::media::Transcript;

pub const WORKER_ARGUMENT: &str = "--grafium-native-ai-worker";
const MAX_REQUEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
const SHUTDOWN_POLL: Duration = Duration::from_millis(50);

static WORKER_EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();
static POOL: OnceLock<Mutex<Option<LiveWorker>>> = OnceLock::new();
static INFLIGHT: OnceLock<Mutex<()>> = OnceLock::new();
static IDLE_MONITOR: OnceLock<()> = OnceLock::new();
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

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
    /// Context length and embedding width, read from the model in the child.
    ///
    /// The parent needs both to size its vector store, and they are only
    /// knowable from the loaded model — so asking the child is what keeps the
    /// parent free of native code entirely.
    #[cfg(feature = "llm-local")]
    EmbedderInfo { model_path: PathBuf },
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
    Ready,
    #[cfg(feature = "llm-local")]
    Embed(Vec<Vec<f32>>),
    #[cfg(feature = "llm-local")]
    EmbedderInfo { context_size: u32, dimension: usize },
    #[cfg(feature = "media")]
    Whisper(Transcript),
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkerResponse {
    output: Option<WorkerOutput>,
    error: Option<String>,
    /// An interim progress report rather than the final answer.
    ///
    /// Transcribing an hour of audio takes minutes, and the caller shows a
    /// live percentage while it runs. A strict one-request-one-response
    /// protocol would have forced a choice between isolating whisper and
    /// keeping that feedback, so a request may now be answered by any number
    /// of progress frames followed by exactly one terminal frame.
    #[serde(default)]
    progress: Option<crate::media::TranscribeProgress>,
}

impl WorkerResponse {
    fn is_progress(&self) -> bool {
        self.progress.is_some() && self.output.is_none() && self.error.is_none()
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
                "shutdown requests do not identify a worker".to_string(),
            )),
        }
    }
}

pub fn execute(request: WorkerRequest, timeout: Duration) -> Result<WorkerOutput> {
    execute_with_progress(request, timeout, &mut |_| {})
}

/// As [`execute`], reporting interim progress as the child emits it.
///
/// Transcription takes minutes and shows a live percentage, so isolating it
/// would otherwise have meant losing that feedback.
pub fn execute_with_progress(
    request: WorkerRequest,
    timeout: Duration,
    on_progress: &mut dyn FnMut(crate::media::TranscribeProgress),
) -> Result<WorkerOutput> {
    if matches!(request, WorkerRequest::Shutdown) {
        return Err(CoreError::Other(
            "shutdown requests cannot be dispatched by callers".to_string(),
        ));
    }
    if SHUTDOWN.load(Ordering::Acquire) {
        return Err(CoreError::Other(
            "Grafium is shutting down; native AI is unavailable".to_string(),
        ));
    }
    let key = WorkerKey::from_request(&request)?;
    let executable = WORKER_EXECUTABLE
        .get()
        .ok_or_else(|| {
            CoreError::Other(
                "native AI worker is not configured by this application host".to_string(),
            )
        })?
        .clone();
    // Serialize concurrent callers so they observe/mutate the pool one at a time
    // and no two workers ever run at once.
    let inflight = inflight();
    let _inflight = inflight.lock().unwrap_or_else(PoisonError::into_inner);
    if SHUTDOWN.load(Ordering::Acquire) {
        return Err(CoreError::Other(
            "Grafium is shutting down; native AI is unavailable".to_string(),
        ));
    }

    // Reuse-or-spawn under a brief POOL critical section, then check the worker
    // out for IPC. This keeps `shutdown_pool` from blocking behind long IPC.
    let mut worker = {
        let pool = pool();
        let mut guard = pool.lock().unwrap_or_else(PoisonError::into_inner);
        // Reuse-eligibility uses the raw (uncapped) working-set estimate rather
        // than an admission-checked limit, so a resident model's own RSS cannot
        // falsely reject reuse of an already-large-enough worker. Admission
        // still runs before any *spawn* below.
        let estimated_working_set = estimated_working_set(&request).ok();
        let reuse = match guard.as_mut() {
            Some(existing) => {
                existing.key == key
                    && existing.is_alive()
                    && estimated_working_set
                        .map(|needed| existing.memory_limit >= needed)
                        .unwrap_or(false)
            }
            None => false,
        };
        if !reuse {
            drop(guard.take());
            let memory_limit = required_memory_limit(&request)?;
            let spawned = LiveWorker::spawn(key, memory_limit, &executable)?;
            *guard = Some(spawned);
        }
        guard
            .take()
            .expect("worker installed in the pool for check-out")
    };

    let outcome = worker.round_trip(&request, timeout, on_progress);
    let now = Instant::now();

    let response = match outcome {
        Ok(response) => {
            worker.last_used = now;
            // Return the worker to the pool for reuse unless shutdown ran while
            // we were busy. Do NOT evict solely because free RAM dropped after
            // loading a large model — its own RSS accounts for most of that
            // change. The idle monitor evicts on real external pressure below.
            let pool = pool();
            let mut guard = pool.lock().unwrap_or_else(PoisonError::into_inner);
            if SHUTDOWN.load(Ordering::Acquire) {
                drop(worker);
            } else if guard.is_none() {
                *guard = Some(worker);
            } else {
                // A racing execute (across a poisoned mutex recovery) reinstalled
                // a worker while we were out; discard ours to avoid two children.
                drop(worker);
            }
            match (response.output, response.error) {
                (Some(output), None) => Ok(output),
                (None, Some(error)) => Err(CoreError::Other(error)),
                _ => Err(CoreError::Other(
                    "native AI worker returned an invalid response".to_string(),
                )),
            }
        }
        Err(error) => {
            // Transport-level failure invalidates the worker; its Drop kills it.
            drop(worker);
            Err(error)
        }
    };
    response
}

pub fn shutdown_pool() {
    SHUTDOWN.store(true, Ordering::Release);
    let Some(pool) = POOL.get() else {
        return;
    };
    let mut guard = match pool.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    drop(guard.take());
}

pub fn is_worker_invocation() -> bool {
    std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == WORKER_ARGUMENT)
}

pub fn configure_current_executable() -> Result<()> {
    let executable = std::env::current_exe().map_err(|e| {
        CoreError::Other(format!("cannot locate Grafium's AI worker executable: {e}"))
    })?;
    WORKER_EXECUTABLE.set(executable).map_err(|_| {
        CoreError::Other("native AI worker executable was already configured".to_string())
    })
}

fn pool() -> &'static Mutex<Option<LiveWorker>> {
    let pool = POOL.get_or_init(|| Mutex::new(None));
    IDLE_MONITOR.get_or_init(|| {
        thread::Builder::new()
            .name("grafium-ai-idle-monitor".into())
            .spawn(monitor_loop)
            .expect("failed to start native AI idle monitor");
    });
    pool
}

fn inflight() -> &'static Mutex<()> {
    INFLIGHT.get_or_init(|| Mutex::new(()))
}

fn required_memory_limit(request: &WorkerRequest) -> Result<u64> {
    memory_limit_for_request(request)
}

fn estimated_working_set(request: &WorkerRequest) -> Result<u64> {
    match request {
        #[cfg(feature = "llm-local")]
        WorkerRequest::Llm {
            model_path,
            context_size,
            ..
        }
        | WorkerRequest::ValidateLlm {
            model_path,
            context_size,
            ..
        } => crate::ai::resources::estimate_worker_working_set(
            model_path,
            crate::ai::resources::ModelWorkload::Llm {
                context_tokens: *context_size,
            },
            0,
        ),
        #[cfg(feature = "llm-local")]
        WorkerRequest::Embed {
            model_path,
            context_size,
            texts,
        } => {
            // The batch itself counts: indexing sends thousands of chunks, and
            // ignoring their size is how an admission check passes right
            // before the allocation it was meant to prevent.
            let input_bytes = texts.iter().map(|t| t.len() as u64).sum();
            crate::ai::resources::estimate_worker_working_set(
                model_path,
                crate::ai::resources::ModelWorkload::Llm {
                    context_tokens: *context_size,
                },
                input_bytes,
            )
        }
        #[cfg(feature = "llm-local")]
        WorkerRequest::EmbedderInfo { model_path } => {
            crate::ai::resources::estimate_worker_working_set(
                model_path,
                crate::ai::resources::ModelWorkload::Llm { context_tokens: 0 },
                0,
            )
        }
        #[cfg(feature = "media")]
        WorkerRequest::Whisper {
            model_path,
            wav_path,
            ..
        } => {
            let input_bytes = std::fs::metadata(wav_path)
                .map_err(|e| {
                    CoreError::Other(format!("Cannot inspect audio {}: {e}", wav_path.display()))
                })?
                .len();
            crate::ai::resources::estimate_worker_working_set(
                model_path,
                crate::ai::resources::ModelWorkload::Whisper,
                input_bytes,
            )
        }
        WorkerRequest::Shutdown => Err(CoreError::Other(
            "shutdown requests do not have a working-set estimate".to_string(),
        )),
    }
}

fn monitor_loop() {
    loop {
        thread::sleep(IDLE_CHECK_INTERVAL);
        if SHUTDOWN.load(Ordering::Acquire) {
            continue;
        }
        let Some(pool) = POOL.get() else {
            continue;
        };
        let Ok(mut guard) = pool.try_lock() else {
            // A request is checking out or putting back; try again next tick.
            continue;
        };
        let evict = match guard.as_mut() {
            Some(worker) => {
                !worker.is_alive()
                    || worker.last_used.elapsed() >= IDLE_TIMEOUT
                    || crate::ai::resources::is_memory_pressure_high()
            }
            None => false,
        };
        if evict {
            drop(guard.take());
        }
    }
}

fn memory_limit_for_request(request: &WorkerRequest) -> Result<u64> {
    match request {
        #[cfg(feature = "llm-local")]
        WorkerRequest::Llm {
            model_path,
            context_size,
            ..
        }
        | WorkerRequest::ValidateLlm {
            model_path,
            context_size,
            ..
        } => crate::ai::resources::worker_memory_limit(
            model_path,
            crate::ai::resources::ModelWorkload::Llm {
                context_tokens: *context_size,
            },
            0,
        ),
        #[cfg(feature = "llm-local")]
        WorkerRequest::Embed {
            model_path,
            context_size,
            texts,
        } => {
            // The batch itself counts: indexing sends thousands of chunks, and
            // ignoring their size is how an admission check passes right
            // before the allocation it was meant to prevent.
            let input_bytes = texts.iter().map(|t| t.len() as u64).sum();
            crate::ai::resources::estimate_worker_working_set(
                model_path,
                crate::ai::resources::ModelWorkload::Llm {
                    context_tokens: *context_size,
                },
                input_bytes,
            )
        }
        #[cfg(feature = "llm-local")]
        WorkerRequest::EmbedderInfo { model_path } => {
            crate::ai::resources::estimate_worker_working_set(
                model_path,
                crate::ai::resources::ModelWorkload::Llm { context_tokens: 0 },
                0,
            )
        }
        #[cfg(feature = "media")]
        WorkerRequest::Whisper {
            model_path,
            wav_path,
            ..
        } => {
            let input_bytes = std::fs::metadata(wav_path)
                .map_err(|e| {
                    CoreError::Other(format!("Cannot inspect audio {}: {e}", wav_path.display()))
                })?
                .len();
            crate::ai::resources::worker_memory_limit(
                model_path,
                crate::ai::resources::ModelWorkload::Whisper,
                input_bytes,
            )
        }
        WorkerRequest::Shutdown => Err(CoreError::Other(
            "shutdown requests do not have a memory limit".to_string(),
        )),
    }
}

// ─── Child process ───────────────────────────────────────────────────────────

pub fn run_from_stdio() -> i32 {
    start_parent_watchdog();
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut state = ChildState::default();
    loop {
        let request: WorkerRequest = {
            let mut stdin = stdin.lock();
            match read_framed(&mut stdin) {
                Ok(Some(request)) => request,
                Ok(None) => return 0,
                Err(error) => {
                    eprintln!("native AI worker: cannot read request: {error}");
                    return 2;
                }
            }
        };
        if matches!(request, WorkerRequest::Shutdown) {
            return 0;
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
        let mut stdout = stdout.lock();
        if let Err(error) = write_framed(&mut stdout, &response) {
            eprintln!("native AI worker: cannot write response: {error}");
            return 2;
        }
    }
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

/// Send an interim progress frame to the parent.
///
/// Written directly rather than buffered: the whole point is that the reader
/// sees a percentage advance during a transcription that takes minutes, and a
/// buffered report arriving at the end would be worthless.
#[cfg(feature = "media")]
fn emit_progress(progress: crate::media::TranscribeProgress) {
    let frame = WorkerResponse {
        output: None,
        error: None,
        progress: Some(progress),
    };
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    // Best effort: losing a progress line must never fail the work itself.
    let _ = write_framed(&mut stdout, &frame);
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
            &mut |progress| emit_progress(progress),
        )
        .map(WorkerOutput::Whisper),
        WorkerRequest::Shutdown => unreachable!("shutdown handled by the caller"),
    }
}

// ─── Framing ─────────────────────────────────────────────────────────────────

fn write_framed<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    if (bytes.len() as u64) > MAX_RESPONSE_BYTES {
        return Err(io::Error::other(
            "worker response exceeds 32 MiB frame limit",
        ));
    }
    let len = (bytes.len() as u64).to_le_bytes();
    writer.write_all(&len)?;
    writer.write_all(&bytes)?;
    writer.flush()
}

fn read_framed<R: Read, T: DeserializeOwned>(reader: &mut R) -> io::Result<Option<T>> {
    let mut len_buf = [0u8; 8];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let len = u64::from_le_bytes(len_buf);
    if len > MAX_REQUEST_BYTES.max(MAX_RESPONSE_BYTES) {
        return Err(io::Error::other("worker frame exceeds size limit"));
    }
    let mut buffer = vec![0u8; len as usize];
    reader.read_exact(&mut buffer)?;
    serde_json::from_slice(&buffer)
        .map(Some)
        .map_err(io::Error::other)
}

fn read_framed_bytes<R: Read>(reader: &mut R, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 8];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let len = u64::from_le_bytes(len_buf);
    if len > limit {
        return Err(io::Error::other("worker frame exceeds size limit"));
    }
    let mut buffer = vec![0u8; len as usize];
    reader.read_exact(&mut buffer)?;
    Ok(Some(buffer))
}

// ─── Parent-side live worker ─────────────────────────────────────────────────

struct LiveWorker {
    key: WorkerKey,
    memory_limit: u64,
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Receiver<io::Result<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
    last_used: Instant,
    #[cfg(windows)]
    _job: WindowsJob,
}

impl LiveWorker {
    fn spawn(key: WorkerKey, memory_limit: u64, executable: &std::path::Path) -> Result<Self> {
        let mut command = Command::new(executable);
        command
            .arg(WORKER_ARGUMENT)
            .env("GRAFIUM_AI_PARENT_PID", std::process::id().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        configure_child(&mut command, memory_limit)?;

        let mut child = command
            .spawn()
            .map_err(|e| CoreError::Other(format!("cannot start native AI worker: {e}")))?;
        #[cfg(windows)]
        let job = WindowsJob::assign(&child, memory_limit)?;
        let stdin = child.stdin.take().ok_or_else(|| {
            CoreError::Other("native AI worker stdin was not captured".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            CoreError::Other("native AI worker stdout was not captured".to_string())
        })?;
        let (tx, rx) = mpsc::channel();
        let reader = thread::Builder::new()
            .name("grafium-ai-worker-reader".into())
            .spawn(move || read_response_loop(stdout, tx))
            .map_err(|e| CoreError::Other(format!("cannot start native AI worker reader: {e}")))?;
        Ok(Self {
            key,
            memory_limit,
            child,
            stdin: Some(stdin),
            responses: rx,
            reader: Some(reader),
            last_used: Instant::now(),
            #[cfg(windows)]
            _job: job,
        })
    }

    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn round_trip(
        &mut self,
        request: &WorkerRequest,
        timeout: Duration,
        on_progress: &mut dyn FnMut(crate::media::TranscribeProgress),
    ) -> Result<WorkerResponse> {
        let payload = serde_json::to_vec(request)
            .map_err(|e| CoreError::Other(format!("cannot encode native AI request: {e}")))?;
        if (payload.len() as u64) > MAX_REQUEST_BYTES {
            return Err(CoreError::Other(
                "native AI request exceeds the 4 MiB IPC limit".to_string(),
            ));
        }

        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| CoreError::Other("native AI worker stdin was closed".to_string()))?;
        let len = (payload.len() as u64).to_le_bytes();
        stdin
            .write_all(&len)
            .and_then(|()| stdin.write_all(&payload))
            .and_then(|()| stdin.flush())
            .map_err(|e| {
                CoreError::Other(format!("cannot send request to native AI worker: {e}"))
            })?;

        loop {
        match self.responses.recv_timeout(timeout) {
            Ok(Ok(bytes)) => {
                let response: WorkerResponse = serde_json::from_slice(&bytes).map_err(|e| {
                    CoreError::Other(format!("native AI worker returned invalid data: {e}"))
                })?;
                if response.is_progress() {
                    if let Some(progress) = response.progress {
                        on_progress(progress);
                    }
                    // Keep waiting: the timeout covers silence, not the whole
                    // job, so a long transcription that is visibly advancing
                    // is never cut off.
                    continue;
                }
                return Ok(response);
            }
            Ok(Err(error)) => return Err(CoreError::Other(format!(
                "native AI worker connection failed: {error}"
            ))),
            Err(RecvTimeoutError::Timeout) => return Err(CoreError::Other(format!(
                "native AI worker exceeded its {} minute time limit and was stopped",
                timeout.as_secs() / 60
            ))),
            Err(RecvTimeoutError::Disconnected) => return Err(CoreError::Other(
                "native AI worker exited unexpectedly".to_string(),
            )),
        }
        }
    }
}

fn read_response_loop(mut stdout: ChildStdout, tx: Sender<io::Result<Vec<u8>>>) {
    loop {
        match read_framed_bytes(&mut stdout, MAX_RESPONSE_BYTES) {
            Ok(Some(bytes)) => {
                if tx.send(Ok(bytes)).is_err() {
                    return;
                }
            }
            Ok(None) => return,
            Err(error) => {
                let _ = tx.send(Err(error));
                return;
            }
        }
    }
}

impl Drop for LiveWorker {
    fn drop(&mut self) {
        // Ask the child to exit cleanly by sending a shutdown frame (best effort),
        // then close stdin so it sees EOF. Kill after a short grace period.
        if let Some(mut stdin) = self.stdin.take() {
            if let Ok(payload) = serde_json::to_vec(&WorkerRequest::Shutdown) {
                let len = (payload.len() as u64).to_le_bytes();
                let _ = stdin.write_all(&len);
                let _ = stdin.write_all(&payload);
                let _ = stdin.flush();
            }
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(SHUTDOWN_POLL),
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

// ─── Platform-specific child sandboxing ──────────────────────────────────────

#[cfg(unix)]
fn configure_child(command: &mut Command, memory_limit: u64) -> Result<()> {
    use std::os::unix::process::CommandExt;

    if memory_limit > libc::rlim_t::MAX as u64 {
        return Err(CoreError::Other(
            "native AI worker memory limit is not representable".to_string(),
        ));
    }
    let limit = memory_limit as libc::rlim_t;
    // SAFETY: only async-signal-safe libc calls run between fork and exec.
    // Note: PR_SET_PDEATHSIG is deliberately NOT used because it is bound to
    // the *thread* that forked, and Grafium calls execute() from tokio's
    // short-lived blocking pool. The `start_parent_watchdog` thread inside the
    // child polls getppid() instead, tying lifetime to the parent process.
    unsafe {
        command.pre_exec(move || {
            let mut inherited = std::mem::zeroed::<libc::rlimit>();
            if libc::getrlimit(libc::RLIMIT_AS, &mut inherited) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let effective_limit = limit.min(inherited.rlim_cur).min(inherited.rlim_max);
            let resource_limit = libc::rlimit {
                rlim_cur: effective_limit,
                rlim_max: effective_limit,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &resource_limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setpriority(libc::PRIO_PROCESS, 0, 10) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(windows)]
fn configure_child(command: &mut Command, _memory_limit: u64) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::BELOW_NORMAL_PRIORITY_CLASS;

    command.creation_flags(BELOW_NORMAL_PRIORITY_CLASS);
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn configure_child(_command: &mut Command, _memory_limit: u64) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn start_parent_watchdog() {
    let Some(parent) = std::env::var("GRAFIUM_AI_PARENT_PID")
        .ok()
        .and_then(|value| value.parse::<libc::pid_t>().ok())
    else {
        return;
    };
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        // SAFETY: signal zero probes process existence without delivering a signal.
        let alive = unsafe { libc::kill(parent, 0) == 0 }
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
        if !alive {
            std::process::exit(3);
        }
    });
}

#[cfg(windows)]
fn start_parent_watchdog() {}

#[cfg(not(any(unix, windows)))]
fn start_parent_watchdog() {}

#[cfg(windows)]
struct WindowsJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl WindowsJob {
    fn assign(child: &Child, memory_limit: u64) -> Result<Self> {
        use std::mem::size_of;
        use std::os::windows::io::AsRawHandle;
        use std::ptr::null;
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        };

        if memory_limit > usize::MAX as u64 {
            return Err(CoreError::Other(
                "native AI worker memory limit is not representable".to_string(),
            ));
        }
        // SAFETY: null security/name pointers request an anonymous Job Object.
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        if handle.is_null() {
            return Err(CoreError::Other(format!(
                "cannot create native AI worker Job Object: {}",
                std::io::Error::last_os_error()
            )));
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        limits.ProcessMemoryLimit = memory_limit as usize;
        // SAFETY: both pointers reference live values with the exact API-declared sizes.
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            // SAFETY: handle was returned by CreateJobObjectW and is still owned here.
            unsafe { CloseHandle(handle) };
            return Err(CoreError::Other(format!(
                "cannot configure native AI worker Job Object: {}",
                std::io::Error::last_os_error()
            )));
        }
        let process = child.as_raw_handle() as HANDLE;
        // SAFETY: process is the live Child process handle; handle is a configured Job Object.
        if unsafe { AssignProcessToJobObject(handle, process) } == 0 {
            // SAFETY: handle was returned by CreateJobObjectW and is still owned here.
            unsafe { CloseHandle(handle) };
            return Err(CoreError::Other(format!(
                "cannot contain native AI worker in Job Object: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(Self(handle))
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        // SAFETY: this type exclusively owns the Job Object handle.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
