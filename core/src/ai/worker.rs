//! Supervised subprocess boundary for native llama.cpp and whisper.cpp work.
//!
//! At most one worker child runs at a time. It persists across requests so the
//! resident native model is reused when the next request wants the same one.
//! The worker is evicted on model-key mismatch, idle timeout, memory pressure,
//! IPC failure, timeout, cancellation, or parent shutdown, and native crashes
//! remain contained inside the child.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

#[cfg(feature = "llm-local")]
use crate::ai::traits::{ChatMessage, CompletionOptions};
use crate::error::{CoreError, Result};
#[cfg(feature = "media")]
use crate::media::Transcript;

#[cfg(feature = "media")]
pub use crate::media::TranscribeProgress as WorkerProgress;
/// Without transcription support, progress frames have no producible payload.
#[cfg(not(feature = "media"))]
#[derive(Debug, Serialize, Deserialize)]
pub enum WorkerProgress {}

pub const WORKER_ARGUMENT: &str = "--grafium-native-ai-worker";
const MAX_REQUEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
const SHUTDOWN_POLL: Duration = Duration::from_millis(50);
const CANCEL_POLL: Duration = Duration::from_millis(50);

static WORKER_EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();
static POOL: OnceLock<Mutex<Option<LiveWorker>>> = OnceLock::new();
static INFLIGHT: OnceLock<InflightQueue> = OnceLock::new();
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
    /// Context length and embedding width, read from the model in the child.
    ///
    /// The parent needs both to size its vector store, and they are only
    /// knowable from the loaded model — so asking the child is what keeps the
    /// parent free of native code entirely.
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
    progress: Option<WorkerProgress>,
}

impl WorkerResponse {
    fn is_progress(&self) -> bool {
        self.progress.is_some() && self.output.is_none() && self.error.is_none()
    }

    fn into_output(self) -> Result<WorkerOutput> {
        match (self.output, self.error, self.progress) {
            (Some(output), None, None) => Ok(output),
            (None, Some(error), None) => Err(CoreError::Other(error)),
            _ => Err(CoreError::Other(
                "native AI worker returned an invalid response".to_string(),
            )),
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
    on_progress: &mut dyn FnMut(WorkerProgress),
) -> Result<WorkerOutput> {
    let cancel = request_cancel(&request);
    check_cancelled(cancel)?;
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
    // One worker, one request at a time. The queue is FIFO so the order the
    // UI shows ("2nd in line") is the order requests actually run in — a
    // plain mutex gave no such promise and could starve a waiting chat.
    let inflight = inflight();
    let _inflight = enter_inflight(inflight, cancel, |position| {
        if position > 0 {
            tracing::debug!(position, "AI request waiting for the model");
        }
    })?;
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
            response.into_output()
        }
        Err(error) => {
            // Transport-level failure invalidates the worker; its Drop kills it.
            drop(worker);
            Err(error)
        }
    };
    response
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
        return Err(CoreError::Other("AI request cancelled".to_string()));
    }
    Ok(())
}

/// A fair queue in front of the single worker.
///
/// The old guard here was a bare `Mutex<()>`. It was correct in the sense that
/// only one request ran at a time, but it had two properties that showed
/// through to the user. A `std::sync::Mutex` makes no ordering promise, so a
/// waiting chat could be skipped over indefinitely; and there was no way to
/// ask "how many are ahead of me", so a queued chat could only be shown a
/// spinner that was indistinguishable from a hang.
///
/// This is a ticket lock instead: take a number, wait for it to come up. That
/// makes the order first-come-first-served and the position a real number the
/// UI can show. Waiters block on a condvar with a short timeout rather than
/// spinning, so cancellation stays responsive without burning a core.
#[derive(Debug, Default)]
struct QueueState {
    next_ticket: u64,
    waiting: VecDeque<u64>,
    running: Option<u64>,
}

#[derive(Debug, Default)]
struct InflightQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl InflightQueue {
    fn lock_state(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn take_ticket(&self) -> u64 {
        let mut state = self.lock_state();
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.waiting.push_back(ticket);
        ticket
    }

    /// Requests ahead of `ticket`: 0 means it is running or next to run.
    ///
    /// Production code doesn't ask — the UI mirrors the queue itself, and can
    /// do so correctly precisely because this queue is FIFO. This exists so
    /// the ordering guarantee is directly testable.
    #[cfg(test)]
    fn position(&self, ticket: u64) -> usize {
        let state = self.lock_state();
        Self::position_in(&state, ticket)
    }

    fn position_in(state: &QueueState, ticket: u64) -> usize {
        if state.running == Some(ticket) {
            return 0;
        }
        let ahead = state
            .waiting
            .iter()
            .position(|queued| *queued == ticket)
            .unwrap_or(0);
        ahead + usize::from(state.running.is_some())
    }

    /// Give up a ticket that never got its turn.
    fn abandon(&self, ticket: u64) {
        let mut state = self.lock_state();
        state.waiting.retain(|queued| *queued != ticket);
        drop(state);
        self.ready.notify_all();
    }

    fn release(&self, ticket: u64) {
        let mut state = self.lock_state();
        if state.running == Some(ticket) {
            state.running = None;
        }
        drop(state);
        self.ready.notify_all();
    }

    /// Wait for `ticket`'s turn, reporting queue position as it changes.
    ///
    /// Returns `Err` if the caller cancelled while waiting; the ticket is
    /// dropped from the queue in that case so nobody behind it is stuck.
    fn acquire(
        &self,
        ticket: u64,
        cancel: Option<&AtomicBool>,
        mut on_position: impl FnMut(usize),
    ) -> Result<()> {
        let mut last_reported: Option<usize> = None;
        let mut state = self.lock_state();
        loop {
            if cancel.is_some() && check_cancelled(cancel).is_err() {
                state.waiting.retain(|queued| *queued != ticket);
                drop(state);
                self.ready.notify_all();
                return Err(CoreError::Other("AI request cancelled".to_string()));
            }
            let is_turn = state.running.is_none() && state.waiting.front() == Some(&ticket);
            if is_turn {
                state.waiting.pop_front();
                state.running = Some(ticket);
                if last_reported.is_some_and(|position| position > 0) {
                    on_position(0);
                }
                return Ok(());
            }
            let position = Self::position_in(&state, ticket);
            if last_reported != Some(position) {
                last_reported = Some(position);
                // Report outside the lock: a callback that touches the queue
                // (or just takes its time) must not hold up the holder's
                // release.
                drop(state);
                on_position(position);
                state = self.lock_state();
                continue;
            }
            let (next, _) = self
                .ready
                .wait_timeout(state, CANCEL_POLL)
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
        }
    }
}

/// Holds the worker for one request and releases it on drop, including on an
/// early `?` return or a panic.
#[derive(Debug)]
struct InflightTicket<'a> {
    queue: &'a InflightQueue,
    ticket: u64,
}

impl Drop for InflightTicket<'_> {
    fn drop(&mut self) {
        self.queue.release(self.ticket);
    }
}

/// Join the queue and wait for a turn.
fn enter_inflight<'a>(
    queue: &'a InflightQueue,
    cancel: Option<&AtomicBool>,
    on_position: impl FnMut(usize),
) -> Result<InflightTicket<'a>> {
    let ticket = queue.take_ticket();
    match queue.acquire(ticket, cancel, on_position) {
        Ok(()) => Ok(InflightTicket { queue, ticket }),
        Err(error) => {
            queue.abandon(ticket);
            Err(error)
        }
    }
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

fn inflight() -> &'static InflightQueue {
    INFLIGHT.get_or_init(InflightQueue::default)
}

/// Admission check for a request. Returns the address-space cap for the child,
/// which is always "no cap" — see below.
fn required_memory_limit(request: &WorkerRequest) -> Result<u64> {
    // Admission still runs, for every request: this compares the model's
    // estimated working set against real available memory and refuses loads
    // that cannot fit, with an error naming the shortfall. That check is
    // measured against actual free memory, so it is worth keeping.
    memory_limit_for_request(request)?;

    // The estimate is deliberately not turned into an `RLIMIT_AS` cap.
    // `RLIMIT_AS` bounds virtual address space, not resident memory, and
    // llama.cpp's address space bears almost no relation to its working set:
    // the model file is mmapped in full, compute buffers are reserved up
    // front, and a GPU driver reserves tens of gigabytes it never touches.
    // Any working-set-derived cap therefore refuses loads that would have been
    // perfectly fine, and llama.cpp reports the refusal as nothing more than
    // "null reference from llama.cpp" — a 639 MB embedding model died this way
    // on a 1.4 GB cap, and so did every CPU completion, since gpu_layers
    // defaults to 0 and the CPU path is what most installs get.
    //
    // What actually contains a blow-up now is the isolation itself: an
    // out-of-memory kill or a driver reset ends this child and the next
    // request starts a fresh one.
    Ok(u64::MAX)
}

fn estimated_working_set(request: &WorkerRequest) -> Result<u64> {
    match request {
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
        | WorkerRequest::CountPrompt {
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

/// Sends the worker's own diagnostics to stderr.
///
/// Without this the child has no `tracing` subscriber at all, so everything
/// llama.cpp reports through `install_llm_logging` — the load progress, the
/// context parameters, and crucially the reason a context failed to be
/// created — is formatted and then dropped on the floor. The parent inherits
/// stderr, so writing there lands the lines in the application log next to
/// the request that triggered them.
///
/// stdout is deliberately untouched: it carries the length-framed response
/// protocol, and a stray log line written to it would desynchronize framing.
fn init_worker_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .try_init();
}

pub fn run_from_stdio() -> i32 {
    init_worker_logging();
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
        on_progress: &mut dyn FnMut(WorkerProgress),
    ) -> Result<WorkerResponse> {
        let cancel = request_cancel(request);
        check_cancelled(cancel)?;
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

        receive_response(&self.responses, timeout, cancel, on_progress)
    }
}

fn receive_response(
    responses: &Receiver<io::Result<Vec<u8>>>,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    on_progress: &mut dyn FnMut(WorkerProgress),
) -> Result<WorkerResponse> {
    let mut last_progress = Instant::now();
    loop {
        check_cancelled(cancel)?;
        let remaining = timeout.saturating_sub(last_progress.elapsed());
        let wait = if cancel.is_some() {
            remaining.min(CANCEL_POLL)
        } else {
            remaining
        };
        match responses.recv_timeout(wait) {
            Ok(Ok(bytes)) => {
                check_cancelled(cancel)?;
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
                    last_progress = Instant::now();
                    continue;
                }
                return Ok(response);
            }
            Ok(Err(error)) => {
                return Err(CoreError::Other(format!(
                    "native AI worker connection failed: {error}"
                )))
            }
            Err(RecvTimeoutError::Timeout) => {
                if last_progress.elapsed() < timeout {
                    continue;
                }
                return Err(CoreError::Other(format!(
                    "native AI worker exceeded its {} minute time limit and was stopped",
                    timeout.as_secs() / 60
                )));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(CoreError::Other(
                    "native AI worker exited unexpectedly".to_string(),
                ))
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

#[cfg(all(test, feature = "llm-local"))]
mod prompt_count_tests {
    use super::*;
    use crate::ai::traits::MessageRole;
    use std::sync::Arc;

    fn count_request() -> WorkerRequest {
        WorkerRequest::CountPrompt {
            model_path: PathBuf::from("synthetic-model.gguf"),
            context_size: 6144,
            gpu_layers: 0,
            messages: vec![
                ChatMessage {
                    role: MessageRole::User,
                    content: "中文 🦀".into(),
                },
                ChatMessage {
                    role: MessageRole::Assistant,
                    content: "Synthetic history".into(),
                },
            ],
            options: CompletionOptions {
                system_prompt: Some("Synthetic system".into()),
                cancel: Some(Arc::new(AtomicBool::new(false))),
                ..Default::default()
            },
        }
    }

    #[test]
    fn scoped_research_generation_observes_cancellation_without_loading_a_model() {
        let WorkerRequest::CountPrompt {
            model_path, context_size, gpu_layers, messages, options,
        } = count_request() else {
            unreachable!()
        };
        let flag = options.cancel.as_ref().unwrap().clone();
        let request = WorkerRequest::Llm {
            model_path, context_size, gpu_layers, messages, options,
        };
        let cancel = request_cancel(&request).expect("generation shares cancellable worker waits");
        let (_sender, receiver) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(20));
                flag.store(true, Ordering::Release);
            });
            assert!(
                receive_response(&receiver, Duration::from_secs(60), Some(cancel), &mut |_| {})
                    .unwrap_err().to_string().contains("cancelled")
            );
        });
        // This must fail before consulting the executable or any model path.
        assert!(execute(request, Duration::from_secs(60))
            .unwrap_err().to_string().contains("cancelled"));
    }

    #[test]
    fn count_request_preserves_prompt_and_shares_generation_pool_key() {
        let count = count_request();
        let mut frame = Vec::new();
        write_framed(&mut frame, &count).unwrap();
        let decoded: WorkerRequest = read_framed(&mut frame.as_slice()).unwrap().unwrap();
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
    fn prompt_count_terminal_response_round_trips() {
        let mut frame = Vec::new();
        write_framed(
            &mut frame,
            &WorkerResponse {
                output: Some(WorkerOutput::PromptTokenCount(8536)),
                error: None,
                progress: None,
            },
        )
        .unwrap();
        let decoded: WorkerResponse = read_framed(&mut frame.as_slice()).unwrap().unwrap();
        assert!(!decoded.is_progress());
        assert!(matches!(
            decoded.into_output().unwrap(),
            WorkerOutput::PromptTokenCount(8536)
        ));
    }

    fn receive_payload(payload: &[u8]) -> Result<WorkerOutput> {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(payload.to_vec())).unwrap();
        receive_response(&rx, Duration::from_secs(1), None, &mut |_| {})?.into_output()
    }

    #[test]
    fn count_errors_and_invalid_responses_are_not_silently_accepted() {
        let error = receive_payload(
            br#"{"output":null,"error":"failed to tokenize prompt: synthetic error"}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("synthetic error"));
        for payload in [
            br#"{"output":null,"error":null}"#.as_slice(),
            br#"{"output":{"PromptTokenCount":4},"error":"conflict"}"#,
            br#"{"output":{"PromptTokenCount":-1}}"#,
            b"not json",
        ] {
            assert!(receive_payload(payload).is_err());
        }
        assert!(matches!(
            receive_payload(br#"{"output":{"PromptTokenCount":0},"error":null}"#).unwrap(),
            WorkerOutput::PromptTokenCount(0)
        ));
    }

    #[test]
    fn prompt_count_cancellation_interrupts_silent_worker() {
        let (_tx, rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(CANCEL_POLL);
                cancel.store(true, Ordering::Release);
            });
            let error = receive_response(&rx, Duration::from_secs(60), Some(&cancel), &mut |_| {})
                .unwrap_err();
            assert!(error.to_string().contains("cancelled"), "{error}");
        });
    }

    #[test]
    fn silent_worker_timeout_and_disconnect_are_errors() {
        let (tx, rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let error = receive_response(&rx, Duration::ZERO, Some(&cancel), &mut |_| {}).unwrap_err();
        assert!(error.to_string().contains("time limit"), "{error}");
        drop(tx);
        let error = receive_response(&rx, Duration::from_secs(1), None, &mut |_| {}).unwrap_err();
        assert!(error.to_string().contains("exited unexpectedly"), "{error}");
    }

    #[cfg(feature = "media")]
    #[test]
    fn progress_frames_still_precede_terminal_responses() {
        let (tx, rx) = mpsc::channel();
        for response in [
            WorkerResponse {
                output: None,
                error: None,
                progress: Some(crate::media::TranscribeProgress {
                    percent: 50,
                    message: "Synthetic progress".into(),
                }),
            },
            WorkerResponse {
                output: Some(WorkerOutput::PromptTokenCount(42)),
                error: None,
                progress: None,
            },
        ] {
            tx.send(Ok(serde_json::to_vec(&response).unwrap())).unwrap();
        }
        let mut seen = Vec::new();
        let output = receive_response(&rx, Duration::from_secs(1), None, &mut |progress| {
            seen.push(progress.percent);
        })
        .unwrap()
        .into_output()
        .unwrap();
        assert_eq!(seen, [50]);
        assert!(matches!(output, WorkerOutput::PromptTokenCount(42)));
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

    if memory_limit == u64::MAX {
        // Uncapped on purpose — see `required_memory_limit`.
        return Ok(());
    }
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

/// Queue behaviour is plain synchronisation logic, so these run without the
/// native model feature — the failure they guard against (a chat that waits
/// forever behind later arrivals) has nothing to do with llama.cpp.
#[cfg(test)]
mod queue_tests {
    use super::*;

    #[test]
    fn prompt_count_cancellation_interrupts_pool_wait_without_model_load() {
        let queue = InflightQueue::default();
        let held = enter_inflight(&queue, None, |_| {}).unwrap();
        let cancel = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(CANCEL_POLL);
                cancel.store(true, Ordering::Release);
            });
            let error = enter_inflight(&queue, Some(&cancel), |_| {}).unwrap_err();
            assert!(error.to_string().contains("cancelled"), "{error}");
        });
        drop(held);
    }

    /// The whole point of the ticket lock: a plain mutex could hand the worker
    /// to whichever thread happened to wake first, so a chat could sit behind
    /// later arrivals forever.
    #[test]
    fn queued_requests_run_in_the_order_they_arrived() {
        let queue = InflightQueue::default();
        let held = enter_inflight(&queue, None, |_| {}).unwrap();

        let order = Mutex::new(Vec::new());
        thread::scope(|scope| {
            for index in 0..4 {
                // Take tickets in a known order before any of them can run.
                let ticket = queue.take_ticket();
                let queue = &queue;
                let order = &order;
                scope.spawn(move || {
                    queue.acquire(ticket, None, |_| {}).unwrap();
                    order.lock().unwrap().push(index);
                    queue.release(ticket);
                });
            }
            thread::sleep(CANCEL_POLL * 2);
            drop(held);
        });

        assert_eq!(order.into_inner().unwrap(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_waiting_request_learns_how_many_are_ahead_of_it() {
        let queue = InflightQueue::default();
        let held = enter_inflight(&queue, None, |_| {}).unwrap();
        let ahead = queue.take_ticket();
        let mine = queue.take_ticket();

        assert_eq!(queue.position(mine), 2, "one running, one queued ahead");
        assert_eq!(queue.position(ahead), 1, "only the running one is ahead");

        drop(held);
        queue.acquire(ahead, None, |_| {}).unwrap();
        assert_eq!(queue.position(mine), 1);
        queue.release(ahead);
        queue.acquire(mine, None, |_| {}).unwrap();
        assert_eq!(queue.position(mine), 0, "running means nothing is ahead");
        queue.release(mine);
    }

    /// A cancelled request must not leave a gap that blocks everyone behind
    /// it — the classic ticket-lock failure.
    #[test]
    fn cancelling_while_queued_does_not_strand_the_requests_behind_it() {
        let queue = InflightQueue::default();
        let held = enter_inflight(&queue, None, |_| {}).unwrap();
        let cancel = AtomicBool::new(true);

        thread::scope(|scope| {
            let doomed = scope.spawn(|| enter_inflight(&queue, Some(&cancel), |_| {}).unwrap_err());
            let error = doomed.join().unwrap();
            assert!(error.to_string().contains("cancelled"), "{error}");
        });

        drop(held);
        let next = enter_inflight(&queue, None, |_| {}).expect("queue moved on");
        drop(next);
        assert_eq!(queue.position(queue.take_ticket()), 0);
    }

    #[test]
    fn releasing_on_an_early_return_frees_the_worker() {
        let queue = InflightQueue::default();
        {
            let _held = enter_inflight(&queue, None, |_| {}).unwrap();
            assert!(queue.lock_state().running.is_some());
        }
        assert!(
            queue.lock_state().running.is_none(),
            "the guard did not release on scope exit"
        );
    }

    #[test]
    fn depth_is_zero_before_any_request() {
        assert_eq!(
            InflightQueue::default().lock_state().waiting.len(),
            0,
            "a fresh queue should be empty"
        );
    }

}
