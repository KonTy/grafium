//! An application-neutral, single-resident-worker supervisor.
//!
//! Requests are FIFO, never automatically replayed, and successful requests
//! reuse a child only for the same host-provided model key. Native crashes can
//! be isolated here; OS failures, system OOM and GPU-driver resets cannot.
//! Hosts own admission policy, native kernels, logging and durable data.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{de::DeserializeOwned, Serialize};

use crate::error::{Result, RuntimeError};
use crate::protocol::{self, Event, Response};

mod containment;
mod lifetime;
pub use lifetime::{exit_without_native_cleanup, start_parent_watchdog};

/// A host-owned recovery marker. Implementations must finish promptly and must
/// leave recovery state intact on Drop or notification failure. Notification
/// happens only after proven process death (or a proven failure to spawn),
/// never merely after sending kill.
pub trait WorkerLease: Send + Sync {
    fn confirmed_exit(&self, expected: bool) -> Result<()>;
}

#[derive(Default)]
pub struct Admission {
    /// Linux cgroup resident/charged-memory limit or Windows process commit
    /// limit. Never an address-space rlimit or a GPU-memory guarantee.
    pub memory_limit_bytes: Option<u64>,
    /// Per-worker values override `SupervisorConfig::environment`.
    pub environment: Vec<(OsString, OsString)>,
    pub lease: Option<Box<dyn WorkerLease>>,
}

pub type DiagnosticCallback = Arc<dyn Fn(&str) + Send + Sync>;
pub type PressureCheck = Arc<dyn Fn(u32) -> Result<Option<String>> + Send + Sync>;

#[derive(Clone)]
pub struct RestartPolicy {
    pub max_failures: usize,
    pub window: Duration,
    pub cooldown: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_failures: 3,
            window: Duration::from_secs(60),
            cooldown: Duration::from_secs(30),
        }
    }
}

pub struct SupervisorConfig {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
    /// JSON payload understood by the host's child loop as graceful shutdown.
    pub shutdown_request: Vec<u8>,
    pub request_limit: u64,
    pub response_limit: u64,
    pub idle_timeout: Duration,
    pub monitor_interval: Duration,
    pub shutdown_grace: Duration,
    pub reap_timeout: Duration,
    pub poll_interval: Duration,
    pub restart: RestartPolicy,
    /// Requires the child to call [`start_parent_watchdog`] before dispatch.
    pub watch_parent: bool,
    /// Called only while idle, outside all supervisor locks.
    pub memory_pressure: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Bounded host callback; runs without pool/process locks. Diagnostics do
    /// not count as progress and never extend request timeouts.
    pub on_diagnostic: Option<DiagnosticCallback>,
    /// Bounded host RAM measurement. Some(reason) stops the active worker.
    /// Called on the requesting thread, never by an unbounded helper pool.
    pub pressure_check: Option<PressureCheck>,
    /// Defaults to 500 ms; request cancellation is polled independently.
    pub pressure_check_interval: Duration,
    #[cfg(test)]
    fail_setup: Option<u8>,
    #[cfg(test)]
    fail_cleanup: bool,
    #[cfg(test)]
    containment_unavailable: bool,
}

impl SupervisorConfig {
    pub fn new(executable: PathBuf, shutdown_request: &impl Serialize) -> Result<Self> {
        Ok(Self {
            executable,
            arguments: Vec::new(),
            environment: Vec::new(),
            shutdown_request: serde_json::to_vec(shutdown_request)?,
            request_limit: protocol::DEFAULT_REQUEST_LIMIT,
            response_limit: protocol::DEFAULT_RESPONSE_LIMIT,
            idle_timeout: Duration::from_secs(10 * 60),
            monitor_interval: Duration::from_secs(30),
            shutdown_grace: Duration::from_secs(3),
            reap_timeout: Duration::from_secs(1),
            poll_interval: Duration::from_millis(25),
            restart: RestartPolicy::default(),
            watch_parent: true,
            memory_pressure: None,
            on_diagnostic: None,
            pressure_check: None,
            pressure_check_interval: Duration::from_millis(500),
            #[cfg(test)]
            fail_setup: None,
            #[cfg(test)]
            fail_cleanup: false,
            #[cfg(test)]
            containment_unavailable: false,
        })
    }

    fn diagnose(&self, message: &str) {
        tracing::info!("{message}");
        if let Some(report) = &self.on_diagnostic {
            report(message);
        }
    }
}

#[derive(Clone, Copy)]
pub struct Timeouts {
    /// Maximum silence, including a blocked request write. Progress resets it.
    pub idle: Duration,
    /// Includes writing and all progress; starts immediately before sending.
    pub total: Option<Duration>,
}

pub struct RequestOptions<'a> {
    pub cancel: Option<&'a AtomicBool>,
    pub timeouts: Timeouts,
    /// Host estimate, not a virtual-address-space or Windows Job memory cap.
    /// A larger request evicts before fresh admission, avoiding double loading.
    pub estimated_bytes: u64,
}

struct Slot<K> {
    key: K,
    estimated_bytes: u64,
    worker: Arc<Worker>,
    last_used: Instant,
}

#[derive(Default)]
struct Failures {
    recent: VecDeque<Instant>,
    blocked_until: Option<Instant>,
}

struct Inner<K> {
    config: SupervisorConfig,
    shutdown: AtomicBool,
    queue: Queue,
    slot: Mutex<Option<Slot<K>>>,
    failures: Mutex<Failures>,
}

pub struct Supervisor<K: Eq + Send + 'static> {
    inner: Arc<Inner<K>>,
}

impl<K: Eq + Send + 'static> Supervisor<K> {
    pub fn new(config: SupervisorConfig) -> Result<Self> {
        if config.poll_interval.is_zero()
            || config.monitor_interval.is_zero()
            || config.pressure_check_interval.is_zero()
            || config.restart.max_failures == 0
            || config.shutdown_request.len() as u64 > config.request_limit
        {
            return Err(RuntimeError::Other(
                "invalid worker supervisor configuration".into(),
            ));
        }
        let inner = Arc::new(Inner {
            config,
            shutdown: AtomicBool::new(false),
            queue: Queue::default(),
            slot: Mutex::new(None),
            failures: Mutex::new(Failures::default()),
        });
        let weak = Arc::downgrade(&inner);
        thread::Builder::new()
            .name("model-worker-idle".into())
            .spawn(move || {
                while let Some(inner) = weak.upgrade() {
                    let state = lock(&inner.queue.state);
                    if inner.shutdown.load(Ordering::Acquire) {
                        return;
                    }
                    let (state, _) = inner
                        .queue
                        .ready
                        .wait_timeout(state, inner.config.monitor_interval)
                        .unwrap_or_else(PoisonError::into_inner);
                    drop(state);
                    if inner.shutdown.load(Ordering::Acquire) {
                        return;
                    }
                    // Maintenance uses the same gate as requests: no child can
                    // be spawned while a previous child is still being reaped.
                    if let Some(_ticket) = inner.queue.try_idle() {
                        inner.evict_idle();
                    }
                }
            })?;
        Ok(Self { inner })
    }

    /// Admission is host code and runs after eviction, immediately before a
    /// new spawn. A response error invalidates the child just like bad framing.
    /// No request is retried, including requests that might mutate host state.
    pub fn execute<Q: Serialize, O: DeserializeOwned, P: DeserializeOwned>(
        &self,
        key: K,
        request: &Q,
        options: RequestOptions<'_>,
        admission: impl FnOnce() -> Result<()>,
        on_progress: &mut dyn FnMut(P),
    ) -> Result<O> {
        self.execute_admitted(
            key,
            request,
            options,
            || {
                admission()?;
                Ok(Admission::default())
            },
            on_progress,
        )
    }

    /// Like `execute`, with spawn-specific containment, environment and a
    /// recovery lease. Admission is not repeated for a reused resident worker.
    /// Keep the request key aligned with device/configuration identity, or
    /// explicitly evict before changing it. A requested hard limit that cannot
    /// be enforced requires `pressure_check` for monitored fallback.
    pub fn execute_admitted<Q: Serialize, O: DeserializeOwned, P: DeserializeOwned>(
        &self,
        key: K,
        request: &Q,
        options: RequestOptions<'_>,
        admission: impl FnOnce() -> Result<Admission>,
        on_progress: &mut dyn FnMut(P),
    ) -> Result<O> {
        let inner = &self.inner;
        check(&inner.shutdown, options.cancel)?;
        let payload = serde_json::to_vec(request)?;
        if payload.len() as u64 > inner.config.request_limit {
            return Err(RuntimeError::Other(
                "native request exceeds IPC size limit".into(),
            ));
        }
        let _ticket =
            inner
                .queue
                .enter(&inner.shutdown, options.cancel, inner.config.poll_interval)?;
        check(&inner.shutdown, options.cancel)?;
        inner.check_cooldown()?;
        let previous = {
            let slot = lock(&inner.slot);
            slot.as_ref().map(|slot| {
                (
                    Arc::clone(&slot.worker),
                    slot.key == key && options.estimated_bytes <= slot.estimated_bytes,
                )
            })
        };
        let reuse = if let Some((worker, eligible)) = previous {
            if eligible && !worker.stopping.load(Ordering::Acquire) && worker.process.is_alive()? {
                Some(worker)
            } else {
                let dead = !worker.process.is_alive()?;
                inner.evict(&worker, inner.config.shutdown_grace, options.cancel, !dead)?;
                if dead {
                    inner.record_failure();
                    inner.check_cooldown()?;
                }
                None
            }
        } else {
            None
        };
        let worker = match reuse {
            Some(worker) => worker,
            None => {
                check(&inner.shutdown, options.cancel)?;
                let mut prepared = PreparedSpawn::new(&inner.config, admission()?);
                let setup = check(&inner.shutdown, options.cancel)
                    .and_then(|()| prepared.prepare(&inner.config))
                    .and_then(|()| check(&inner.shutdown, options.cancel));
                if let Err(error) = setup {
                    return Err(prepared.failed(error, &inner.config));
                }
                if let Some(message) = prepared.unavailable_diagnostic() {
                    inner.config.diagnose(message);
                }
                // Registration and spawning share this short critical section.
                // Shutdown never waits on IPC or native dispatch behind it.
                let mut slot = lock(&inner.slot);
                let spawned = check(&inner.shutdown, options.cancel)
                    .and_then(|()| Worker::spawn(&inner.config, &mut prepared));
                let worker = match spawned {
                    Ok(worker) => Arc::new(worker),
                    Err(error) => {
                        drop(slot);
                        if !intentional(&error) {
                            inner.record_failure();
                        }
                        return Err(prepared.failed(error, &inner.config));
                    }
                };
                *slot = Some(Slot {
                    key,
                    estimated_bytes: options.estimated_bytes,
                    worker: Arc::clone(&worker),
                    last_used: Instant::now(),
                });
                worker
            }
        };
        // Unlike a checked-out pool, the registry retains the active child, so
        // shutdown can terminate it even if a host callback is slow or stuck.
        let mut active = ActiveRequest {
            inner,
            worker: &worker,
            finished: false,
        };
        if let Some(message) = worker.process.enforced_diagnostic() {
            inner.config.diagnose(&message);
        }
        let outcome = worker.round_trip(
            payload,
            &options,
            &inner.shutdown,
            &inner.config,
            on_progress,
        );
        active.finished = true;
        match outcome {
            Ok(output) => {
                check(&inner.shutdown, options.cancel).inspect_err(|_| {
                    if let Err(error) = inner.evict(&worker, Duration::ZERO, None, true) {
                        tracing::error!(%error, "cannot evict cancelled native worker");
                    }
                })?;
                if let Some(slot) = lock(&inner.slot).as_mut() {
                    slot.last_used = Instant::now();
                }
                Ok(output)
            }
            Err(error) => {
                let expected = intentional(&error);
                if !expected {
                    inner.record_failure();
                }
                if let Err(cleanup) = inner.evict(&worker, Duration::ZERO, None, expected) {
                    tracing::error!(%cleanup, "native worker cleanup failed; replacement blocked");
                    inner
                        .config
                        .diagnose(&format!("Native worker cleanup is incomplete: {cleanup}"));
                }
                Err(error)
            }
        }
    }

    pub fn queued_requests(&self) -> usize {
        lock(&self.inner.queue.state).waiting.len()
    }

    /// Diagnostic identity of this supervisor's registered child, if any.
    pub fn worker_pid(&self) -> Option<u32> {
        lock(&self.inner.slot)
            .as_ref()
            .and_then(|slot| lock(&slot.worker.process.child).as_ref().map(Child::id))
    }

    /// Reversibly unload an idle child, for example before changing providers.
    /// Active or queued requests are not cancelled: callers must drain them
    /// first. A concurrent maintenance eviction is waited for within a bounded
    /// deadline, and shutdown interrupts that wait.
    pub fn evict_idle(&self) -> Result<()> {
        let inner = &self.inner;
        let _ticket = inner.queue.enter_idle(
            &inner.shutdown,
            inner.config.poll_interval,
            inner.config.shutdown_grace + inner.config.reap_timeout,
        )?;
        let worker = lock(&inner.slot)
            .as_ref()
            .map(|slot| Arc::clone(&slot.worker));
        if let Some(worker) = worker {
            inner.evict(&worker, inner.config.shutdown_grace, None, true)?;
        }
        Ok(())
    }

    /// Stops admission, wakes every queued request and stops the registered
    /// child, whether idle or actively executing. Does not join IO threads.
    /// Successful explicit cleanup settles its lease; implicit Drop reaps but
    /// leaves the recovery marker unconfirmed.
    pub fn shutdown(&self) -> Result<()> {
        let inner = &self.inner;
        inner.shutdown.store(true, Ordering::Release);
        inner.queue.ready.notify_all();
        let deadline = Instant::now() + inner.config.shutdown_grace + inner.config.reap_timeout;
        let worker = loop {
            match inner.slot.try_lock() {
                Ok(slot) => break slot.as_ref().map(|slot| Arc::clone(&slot.worker)),
                Err(std::sync::TryLockError::Poisoned(error)) => {
                    break error
                        .into_inner()
                        .as_ref()
                        .map(|slot| Arc::clone(&slot.worker));
                }
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(
                        inner
                            .config
                            .poll_interval
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Err(_) => {
                    return Err(RuntimeError::Other(
                        "worker spawn is still finishing during shutdown".into(),
                    ))
                }
            }
        };
        if let Some(worker) = worker {
            inner.evict(
                &worker,
                inner
                    .config
                    .shutdown_grace
                    .min(deadline.saturating_duration_since(Instant::now())),
                None,
                true,
            )?;
        }
        Ok(())
    }
}

impl<K: Eq + Send + 'static> Drop for Supervisor<K> {
    fn drop(&mut self) {
        // An implicit destructor is not the host's explicit confirmation path.
        // Reap the child, but preserve its recovery marker if shutdown/eviction
        // was never completed explicitly.
        if let Some(worker) = lock(&self.inner.slot)
            .as_ref()
            .map(|slot| Arc::clone(&slot.worker))
        {
            worker.process.suppress_lease.store(true, Ordering::Release);
        }
        if let Err(error) = self.shutdown() {
            tracing::error!(%error, "native supervisor shutdown failed");
        }
    }
}

struct ActiveRequest<'a, K> {
    inner: &'a Inner<K>,
    worker: &'a Arc<Worker>,
    finished: bool,
}

impl<K> Drop for ActiveRequest<'_, K> {
    fn drop(&mut self) {
        if !self.finished {
            self.inner.record_failure();
            if let Err(error) = self.inner.evict(self.worker, Duration::ZERO, None, false) {
                tracing::error!(%error, "cannot evict native worker after host callback panic");
            }
        }
    }
}

impl<K> Inner<K> {
    fn check_cooldown(&self) -> Result<()> {
        let mut failures = lock(&self.failures);
        if let Some(until) = failures.blocked_until {
            if Instant::now() < until {
                return Err(RuntimeError::Other(
                    "native worker repeatedly failed; restart cooldown is active".into(),
                ));
            }
            failures.blocked_until = None;
            failures.recent.clear();
        }
        Ok(())
    }

    fn record_failure(&self) {
        let now = Instant::now();
        let mut failures = lock(&self.failures);
        while failures
            .recent
            .front()
            .is_some_and(|at| now.duration_since(*at) > self.config.restart.window)
        {
            failures.recent.pop_front();
        }
        failures.recent.push_back(now);
        if failures.recent.len() >= self.config.restart.max_failures {
            failures.blocked_until = Some(now + self.config.restart.cooldown);
        }
    }

    fn evict(
        &self,
        worker: &Arc<Worker>,
        grace: Duration,
        cancel: Option<&AtomicBool>,
        expected: bool,
    ) -> Result<()> {
        worker.stop(grace, &self.shutdown, cancel, &self.config, expected)?;
        let mut slot = lock(&self.slot);
        if slot
            .as_ref()
            .is_some_and(|slot| Arc::ptr_eq(&slot.worker, worker))
        {
            slot.take();
        }
        Ok(())
    }

    fn evict_idle(&self) {
        let candidate = lock(&self.slot)
            .as_ref()
            .map(|slot| (Arc::clone(&slot.worker), slot.last_used));
        let Some((worker, last_used)) = candidate else {
            return;
        };
        let dead = match worker.process.is_alive() {
            Ok(alive) => !alive,
            Err(error) => {
                tracing::warn!(%error, "cannot inspect idle worker");
                true
            }
        };
        let pressure = self
            .config
            .memory_pressure
            .as_ref()
            .is_some_and(|probe| probe());
        if dead || last_used.elapsed() >= self.config.idle_timeout || pressure {
            if dead {
                self.record_failure();
            }
            if let Err(error) = self.evict(&worker, self.config.shutdown_grace, None, !dead) {
                tracing::error!(%error, "cannot unload idle native worker");
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn check(shutdown: &AtomicBool, cancel: Option<&AtomicBool>) -> Result<()> {
    if shutdown.load(Ordering::Acquire) {
        return Err(RuntimeError::ShuttingDown);
    }
    if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err(RuntimeError::Cancelled);
    }
    Ok(())
}

fn intentional(error: &RuntimeError) -> bool {
    matches!(error, RuntimeError::Cancelled | RuntimeError::ShuttingDown)
}

#[derive(Default)]
struct QueueState {
    next: u64,
    waiting: VecDeque<u64>,
    running: Option<u64>,
    maintenance: bool,
}

#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

struct Ticket<'a> {
    queue: &'a Queue,
    id: u64,
}

impl Queue {
    fn enter<'a>(
        &'a self,
        shutdown: &AtomicBool,
        cancel: Option<&AtomicBool>,
        poll: Duration,
    ) -> Result<Ticket<'a>> {
        let mut state = lock(&self.state);
        check(shutdown, cancel)?;
        let id = state.next;
        state.next = state.next.wrapping_add(1);
        state.waiting.push_back(id);
        loop {
            if let Err(error) = check(shutdown, cancel) {
                state.waiting.retain(|ticket| *ticket != id);
                self.ready.notify_all();
                return Err(error);
            }
            if state.running.is_none() && state.waiting.front() == Some(&id) {
                state.waiting.pop_front();
                state.running = Some(id);
                state.maintenance = false;
                return Ok(Ticket { queue: self, id });
            }
            let (next, _) = self
                .ready
                .wait_timeout(state, poll)
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
        }
    }

    fn try_idle(&self) -> Option<Ticket<'_>> {
        let mut state = lock(&self.state);
        if state.running.is_some() || !state.waiting.is_empty() {
            return None;
        }
        let id = state.next;
        state.next = state.next.wrapping_add(1);
        state.running = Some(id);
        state.maintenance = true;
        Some(Ticket { queue: self, id })
    }

    fn enter_idle<'a>(
        &'a self,
        shutdown: &AtomicBool,
        poll: Duration,
        timeout: Duration,
    ) -> Result<Ticket<'a>> {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.state);
        loop {
            check(shutdown, None)?;
            if !state.waiting.is_empty() || (state.running.is_some() && !state.maintenance) {
                return Err(RuntimeError::WorkerBusy);
            }
            if state.running.is_none() {
                let id = state.next;
                state.next = state.next.wrapping_add(1);
                state.running = Some(id);
                state.maintenance = true;
                return Ok(Ticket { queue: self, id });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RuntimeError::Other(
                    "native worker maintenance did not finish before the eviction deadline".into(),
                ));
            }
            let (next, _) = self
                .ready
                .wait_timeout(state, poll.min(remaining))
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
        }
    }
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.queue.state);
        if state.running == Some(self.id) {
            state.running = None;
            state.maintenance = false;
        }
        drop(state);
        self.queue.ready.notify_all();
    }
}

enum Settlement {
    Pending,
    Running,
    Confirmed,
    Failed(String),
}

struct LeaseState {
    lease: Option<Box<dyn WorkerLease>>,
    state: Mutex<Settlement>,
    ready: Condvar,
}

impl Default for LeaseState {
    fn default() -> Self {
        Self::new(None)
    }
}

impl LeaseState {
    fn new(lease: Option<Box<dyn WorkerLease>>) -> Self {
        Self {
            lease,
            state: Mutex::new(Settlement::Pending),
            ready: Condvar::new(),
        }
    }

    fn confirmed_exit(&self, expected: bool, timeout: Duration) -> Result<()> {
        let Some(lease) = &self.lease else {
            return Ok(());
        };
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.state);
        loop {
            match &*state {
                Settlement::Confirmed => return Ok(()),
                Settlement::Failed(error) => return Err(RuntimeError::Other(error.clone())),
                Settlement::Pending => {
                    *state = Settlement::Running;
                    break;
                }
                Settlement::Running => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(RuntimeError::Other(
                            "native worker lease settlement is still in progress".into(),
                        ));
                    }
                    state = self
                        .ready
                        .wait_timeout(state, remaining)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
        }
        drop(state);
        let result = lease.confirmed_exit(expected);
        *lock(&self.state) = match &result {
            Ok(()) => Settlement::Confirmed,
            Err(error) => {
                Settlement::Failed(format!("native worker lease settlement failed: {error}"))
            }
        };
        self.ready.notify_all();
        result
    }
}

struct PreparedSpawn {
    command: Command,
    lifetime: Option<lifetime::ParentLifetime>,
    containment: Option<containment::Containment>,
    lease: LeaseState,
    process: Option<OwnedProcess>,
    memory_limit: Option<u64>,
}

impl PreparedSpawn {
    fn new(config: &SupervisorConfig, admission: Admission) -> Self {
        let mut command = Command::new(&config.executable);
        command
            .args(&config.arguments)
            .envs(config.environment.iter().cloned())
            .envs(admission.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        Self {
            command,
            lifetime: None,
            containment: None,
            lease: LeaseState::new(admission.lease),
            process: None,
            memory_limit: admission.memory_limit_bytes,
        }
    }

    fn prepare(&mut self, config: &SupervisorConfig) -> Result<()> {
        #[cfg(test)]
        if config.containment_unavailable {
            self.containment = Some(containment::Containment::unavailable(
                "synthetic undelegated controller (no system cgroup access)".into(),
            ));
        }
        if self.containment.is_none() {
            self.containment = Some(containment::Containment::prepare(
                &mut self.command,
                self.memory_limit,
            )?);
        }
        if self.memory_limit.is_some()
            && self.unavailable_diagnostic().is_some()
            && config.pressure_check.is_none()
        {
            return Err(RuntimeError::Other(
                "hard RAM containment is unavailable and no active pressure monitor was configured"
                    .into(),
            ));
        }
        self.lifetime = Some(lifetime::ParentLifetime::configure(
            &mut self.command,
            config.watch_parent,
        )?);
        Ok(())
    }

    fn unavailable_diagnostic(&self) -> Option<&str> {
        self.containment
            .as_ref()
            .filter(|c| !c.enforced())
            .map(|c| c.diagnostic())
    }

    fn failed(&mut self, error: RuntimeError, config: &SupervisorConfig) -> RuntimeError {
        if let Some(diagnostic) = self.unavailable_diagnostic() {
            config.diagnose(diagnostic);
        }
        let cleanup = if let Some(process) = &self.process {
            process.finish_exit(intentional(&error))
        } else {
            self.containment
                .as_mut()
                .map_or(Ok(()), |containment| containment.confirmed_exit())
                .map_err(RuntimeError::from)
                .and_then(|()| {
                    self.lease
                        .confirmed_exit(intentional(&error), config.reap_timeout)
                })
        };
        match cleanup {
            Ok(()) => error,
            Err(cleanup) => {
                tracing::error!(%cleanup, "native worker setup cleanup is incomplete");
                config.diagnose(&format!(
                    "Native worker setup cleanup is incomplete: {cleanup}"
                ));
                RuntimeError::Other(format!("{error}; native worker cleanup failed: {cleanup}"))
            }
        }
    }
}

struct WriteJob {
    bytes: Vec<u8>,
    done: mpsc::Sender<io::Result<()>>,
}

struct Worker {
    process: OwnedProcess,
    writes: SyncSender<WriteJob>,
    responses: Mutex<Receiver<io::Result<Vec<u8>>>>,
    stopping: AtomicBool,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
}

impl Worker {
    fn spawn(config: &SupervisorConfig, prepared: &mut PreparedSpawn) -> Result<Self> {
        let child = prepared.command.spawn()?;
        // Establish RAII ownership before any fallible post-spawn setup.
        prepared.process = Some(OwnedProcess {
            child: Mutex::new(Some(child)),
            _lifetime: prepared
                .lifetime
                .take()
                .expect("spawn preparation established parent lifetime"),
            containment: Mutex::new(prepared.containment.take()),
            lease: std::mem::take(&mut prepared.lease),
            stop_requested: AtomicBool::new(false),
            kill_requested: AtomicBool::new(false),
            unexpected: AtomicBool::new(false),
            diagnostic_reported: AtomicBool::new(false),
            suppress_lease: AtomicBool::new(false),
            #[cfg(windows)]
            job: None,
            reap_timeout: config.reap_timeout,
            poll: config.poll_interval,
            #[cfg(test)]
            fail_cleanup: config.fail_cleanup,
        });
        let process = prepared.process.as_mut().unwrap();
        process._lifetime.spawned();
        if let Some(containment) = lock(&process.containment).as_mut() {
            containment.spawned();
        }
        #[cfg(test)]
        if config.fail_setup == Some(1) {
            return Err(RuntimeError::Other(
                lock(&process.child).as_ref().unwrap().id().to_string(),
            ));
        }
        #[cfg(windows)]
        {
            process.job = Some(lifetime::WindowsJob::assign(
                lock(&process.child).as_ref().unwrap(),
                prepared.memory_limit,
            )?);
        }
        let stdin = lock(&process.child)
            .as_mut()
            .unwrap()
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("worker stdin was not captured"))?;
        let stdout = lock(&process.child)
            .as_mut()
            .unwrap()
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("worker stdout was not captured"))?;
        let (tx, responses) = mpsc::sync_channel(1);
        let response_limit = config.response_limit;
        let reader = thread::Builder::new()
            .name("model-worker-reader".into())
            .spawn(move || read_responses(stdout, tx, response_limit))?;
        #[cfg(test)]
        if config.fail_setup == Some(2) {
            return Err(RuntimeError::Other(
                lock(&process.child).as_ref().unwrap().id().to_string(),
            ));
        }
        let (writes, jobs) = mpsc::sync_channel::<WriteJob>(1);
        let request_limit = config.request_limit;
        let writer = thread::Builder::new()
            .name("model-worker-writer".into())
            .spawn(move || {
                let mut stdin = stdin;
                while let Ok(job) = jobs.recv() {
                    let result = protocol::write_bytes(&mut stdin, &job.bytes, request_limit);
                    let failed = result.is_err();
                    let _ = job.done.send(result);
                    if failed {
                        return;
                    }
                }
            })?;
        Ok(Self {
            process: prepared.process.take().unwrap(),
            writes,
            responses: Mutex::new(responses),
            stopping: AtomicBool::new(false),
            reader: Some(reader),
            writer: Some(writer),
        })
    }

    fn round_trip<O: DeserializeOwned, P: DeserializeOwned>(
        &self,
        payload: Vec<u8>,
        options: &RequestOptions<'_>,
        shutdown: &AtomicBool,
        config: &SupervisorConfig,
        on_progress: &mut dyn FnMut(P),
    ) -> Result<O> {
        let result = self.round_trip_inner(payload, options, shutdown, config, on_progress);
        if result.as_ref().is_err_and(|error| !intentional(error)) {
            self.process.unexpected.store(true, Ordering::Release);
        }
        result
    }

    fn round_trip_inner<O: DeserializeOwned, P: DeserializeOwned>(
        &self,
        payload: Vec<u8>,
        options: &RequestOptions<'_>,
        shutdown: &AtomicBool,
        config: &SupervisorConfig,
        on_progress: &mut dyn FnMut(P),
    ) -> Result<O> {
        let started = Instant::now();
        let mut progress_at = started;
        let mut pressure = ActivePressure::new();
        check(shutdown, options.cancel)?;
        let (done, sent) = mpsc::channel();
        self.writes
            .try_send(WriteJob {
                bytes: payload,
                done,
            })
            .map_err(|_| RuntimeError::Other("native worker input is unavailable".into()))?;
        loop {
            let wait = self.request_wait(
                started,
                progress_at,
                options,
                shutdown,
                config,
                &mut pressure,
            )?;
            match sent.recv_timeout(wait) {
                Ok(result) => {
                    result?;
                    break;
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(RuntimeError::Other("native worker writer exited".into()))
                }
            }
        }
        let responses = lock(&self.responses);
        loop {
            let wait = self.request_wait(
                started,
                progress_at,
                options,
                shutdown,
                config,
                &mut pressure,
            )?;
            match responses.recv_timeout(wait) {
                Ok(Ok(bytes)) => {
                    check(shutdown, options.cancel)?;
                    let response: Response<O, P> = serde_json::from_slice(&bytes)?;
                    match response.into_event()? {
                        Event::Output(output) => return Ok(output),
                        Event::Progress(progress) => {
                            on_progress(progress);
                            progress_at = Instant::now();
                        }
                    }
                }
                Ok(Err(error)) => return Err(error.into()),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(RuntimeError::Other(
                        "native worker exited unexpectedly".into(),
                    ))
                }
            }
        }
    }

    fn request_wait(
        &self,
        started: Instant,
        progress_at: Instant,
        options: &RequestOptions<'_>,
        shutdown: &AtomicBool,
        config: &SupervisorConfig,
        pressure: &mut ActivePressure,
    ) -> Result<Duration> {
        wait_duration(
            started,
            progress_at,
            options,
            shutdown,
            config.poll_interval,
        )?;
        pressure.check(&self.process, config)?;
        // Host diagnostics and measurements are not progress. Recalculate
        // both deadlines after the bounded callback, rather than extending it.
        Ok(wait_duration(
            started,
            progress_at,
            options,
            shutdown,
            config.poll_interval,
        )?
        .min(pressure.until_next(config)))
    }

    fn stop(
        &self,
        grace: Duration,
        shutdown: &AtomicBool,
        cancel: Option<&AtomicBool>,
        config: &SupervisorConfig,
        expected: bool,
    ) -> Result<()> {
        let result = self.stop_inner(grace, shutdown, cancel, config, expected);
        if result.is_err() {
            self.process.unexpected.store(true, Ordering::Release);
        }
        result
    }

    fn stop_inner(
        &self,
        grace: Duration,
        shutdown: &AtomicBool,
        cancel: Option<&AtomicBool>,
        config: &SupervisorConfig,
        expected: bool,
    ) -> Result<()> {
        self.process.begin_stop(expected)?;
        let started_shutdown = shutdown.load(Ordering::Acquire);
        if !self.stopping.swap(true, Ordering::AcqRel) {
            let (done, _) = mpsc::channel();
            // Never block trying to write shutdown into a full native pipe.
            let _ = self.writes.try_send(WriteJob {
                bytes: config.shutdown_request.clone(),
                done,
            });
        }
        let deadline = Instant::now() + grace;
        while self.process.is_alive()? && Instant::now() < deadline {
            // Shutdown interrupts an idle/model-switch eviction already waiting.
            if (!started_shutdown && shutdown.load(Ordering::Acquire))
                || cancel.is_some_and(|flag| flag.load(Ordering::Acquire))
            {
                break;
            }
            thread::sleep(
                config
                    .poll_interval
                    .min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        self.process.finish_exit(expected)
    }
}

struct ActivePressure {
    checked_at: Option<Instant>,
}

impl ActivePressure {
    fn new() -> Self {
        Self { checked_at: None }
    }

    fn check(&mut self, process: &OwnedProcess, config: &SupervisorConfig) -> Result<()> {
        let Some(probe) = &config.pressure_check else {
            return Ok(());
        };
        if self
            .checked_at
            .is_some_and(|at| at.elapsed() < config.pressure_check_interval)
        {
            return Ok(());
        }
        self.checked_at = Some(Instant::now());
        let pid = lock(&process.child)
            .as_ref()
            .map(Child::id)
            .ok_or_else(|| {
                RuntimeError::Other("native worker no longer has an owned process".into())
            })?;
        match probe(pid) {
            Ok(None) => Ok(()),
            Ok(Some(reason)) => {
                process.unexpected.store(true, Ordering::Release);
                config.diagnose(&format!(
                    "Native worker stopped under active memory pressure: {reason}"
                ));
                Err(RuntimeError::ResourcePressure(reason))
            }
            Err(error) => Err(RuntimeError::Other(format!(
                "native worker pressure check failed: {error}"
            ))),
        }
    }

    fn until_next(&self, config: &SupervisorConfig) -> Duration {
        if config.pressure_check.is_none() {
            return Duration::MAX;
        }
        self.checked_at.map_or(Duration::ZERO, |at| {
            config.pressure_check_interval.saturating_sub(at.elapsed())
        })
    }
}

fn wait_duration(
    started: Instant,
    progress_at: Instant,
    options: &RequestOptions<'_>,
    shutdown: &AtomicBool,
    poll: Duration,
) -> Result<Duration> {
    check(shutdown, options.cancel)?;
    let idle = options.timeouts.idle.saturating_sub(progress_at.elapsed());
    let total = options
        .timeouts
        .total
        .map(|limit| limit.saturating_sub(started.elapsed()));
    let remaining = total.map_or(idle, |total| idle.min(total));
    if remaining.is_zero() {
        return Err(RuntimeError::Other(
            "native worker exceeded its time limit and was stopped".into(),
        ));
    }
    Ok(poll.min(remaining))
}

fn read_responses(mut stdout: ChildStdout, responses: SyncSender<io::Result<Vec<u8>>>, limit: u64) {
    loop {
        match protocol::read_bytes(&mut stdout, limit) {
            Ok(Some(bytes)) => {
                if responses.send(Ok(bytes)).is_err() {
                    return;
                }
            }
            Ok(None) => return,
            Err(error) => {
                let _ = responses.send(Err(error));
                return;
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Joining a reader whose pipe is held by a descendant is unbounded.
        // Dropping a JoinHandle detaches it; channel closure or OS pipe closure
        // lets it finish. Never let vendor shutdown code hold up the host.
        for handle in [&mut self.reader, &mut self.writer] {
            if handle.as_ref().is_some_and(JoinHandle::is_finished)
                && handle.take().unwrap().join().is_err()
            {
                tracing::warn!("native worker IO thread panicked");
            }
        }
    }
}

struct OwnedProcess {
    child: Mutex<Option<Child>>,
    _lifetime: lifetime::ParentLifetime,
    containment: Mutex<Option<containment::Containment>>,
    lease: LeaseState,
    stop_requested: AtomicBool,
    kill_requested: AtomicBool,
    unexpected: AtomicBool,
    diagnostic_reported: AtomicBool,
    suppress_lease: AtomicBool,
    #[cfg(windows)]
    job: Option<lifetime::WindowsJob>,
    reap_timeout: Duration,
    poll: Duration,
    #[cfg(test)]
    fail_cleanup: bool,
}

impl OwnedProcess {
    fn observe_exit(&self, status: ExitStatus) {
        let killed = self.kill_requested.load(Ordering::Acquire) && supervisor_kill(status);
        if !self.stop_requested.load(Ordering::Acquire) || (!status.success() && !killed) {
            self.unexpected.store(true, Ordering::Release);
        }
    }

    fn enforced_diagnostic(&self) -> Option<String> {
        if self.diagnostic_reported.swap(true, Ordering::AcqRel) {
            return None;
        }
        lock(&self.containment)
            .as_ref()
            .filter(|c| c.enforced())
            .map(|c| c.diagnostic().to_owned())
    }

    fn is_alive(&self) -> io::Result<bool> {
        match lock(&self.child).as_mut() {
            Some(child) => match child.try_wait()? {
                Some(status) => {
                    self.observe_exit(status);
                    Ok(false)
                }
                None => Ok(true),
            },
            None => Ok(false),
        }
    }

    fn begin_stop(&self, expected: bool) -> Result<()> {
        if !expected {
            self.unexpected.store(true, Ordering::Release);
        }
        let mut child = lock(&self.child);
        if let Some(status) = child.as_mut().map(Child::try_wait).transpose()?.flatten() {
            self.observe_exit(status);
        }
        self.stop_requested.store(true, Ordering::Release);
        Ok(())
    }

    fn finish_exit(&self, expected: bool) -> Result<()> {
        let result = (|| {
            self.begin_stop(expected)?;
            self.kill_and_reap()?;
            #[cfg(test)]
            if self.fail_cleanup {
                return Err(RuntimeError::Other("synthetic ambiguous cleanup".into()));
            }
            if let Some(containment) = lock(&self.containment).as_mut() {
                containment.confirmed_exit()?;
            }
            if self.suppress_lease.load(Ordering::Acquire) {
                Ok(())
            } else {
                self.lease
                    .confirmed_exit(!self.unexpected.load(Ordering::Acquire), self.reap_timeout)
            }
        })();
        if result.is_err() {
            self.unexpected.store(true, Ordering::Release);
        }
        result
    }

    fn kill_and_reap(&self) -> Result<()> {
        {
            let mut child = lock(&self.child);
            let Some(child) = child.as_mut() else {
                return Ok(());
            };
            if let Some(status) = child.try_wait()? {
                self.observe_exit(status);
                return Ok(());
            }
            self.kill_requested.store(true, Ordering::Release);
            if let Err(error) = child.kill() {
                match child.try_wait()? {
                    Some(status) => self.observe_exit(status),
                    None => return Err(error.into()),
                }
            }
        }
        let deadline = Instant::now() + self.reap_timeout;
        loop {
            if !self.is_alive()? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(RuntimeError::Other(
                    "native worker did not exit after termination; replacement blocked".into(),
                ));
            }
            thread::sleep(
                self.poll
                    .min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if let Err(error) = self.begin_stop(false) {
            tracing::error!(%error, "cannot inspect native child during final cleanup");
        }
        if let Err(error) = self.kill_and_reap() {
            // No unbounded Child::wait in a destructor. On pathological kernel
            // stalls retain ownership in a reaper rather than enabling overlap.
            tracing::error!(%error, "native child still needs reaping");
            if let Some(mut child) = self
                .child
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            {
                let mut containment = self
                    .containment
                    .get_mut()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                // A pathological kernel wait may never finish, but must not
                // hold up an application destructor. This thread retains the
                // only Child and is never used to authorize a replacement.
                if let Err(error) = thread::Builder::new()
                    .name("model-worker-reaper".into())
                    .spawn(move || {
                        if let Err(error) = child.kill() {
                            tracing::error!(%error, "native child hard termination failed");
                        }
                        match child.wait() {
                            Ok(_) => {
                                if let Some(containment) = containment.as_mut() {
                                    if let Err(error) = containment.confirmed_exit() {
                                        tracing::error!(%error, "deferred native cgroup cleanup failed");
                                    }
                                }
                            }
                            Err(error) => tracing::error!(%error, "native child reaping failed"),
                        }
                    })
                {
                    tracing::error!(%error, "cannot start deferred native child reaper");
                }
            }
        } else if let Some(containment) = self
            .containment
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            if let Err(error) = containment.confirmed_exit() {
                tracing::error!(%error, "final native cgroup cleanup failed");
            }
        }
        // A destructor never settles a recovery lease. If explicit cleanup was
        // interrupted or ambiguous, its host-owned marker must survive.
    }
}

#[cfg(unix)]
fn supervisor_kill(status: ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal() == Some(libc::SIGKILL)
}

#[cfg(windows)]
fn supervisor_kill(status: ExitStatus) -> bool {
    status.code() == Some(1)
}

#[cfg(not(any(unix, windows)))]
fn supervisor_kill(_: ExitStatus) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    struct RecordingLease {
        records: Arc<Mutex<Vec<bool>>>,
        pid: Arc<std::sync::atomic::AtomicU32>,
    }

    #[cfg(unix)]
    impl WorkerLease for RecordingLease {
        fn confirmed_exit(&self, expected: bool) -> Result<()> {
            #[cfg(unix)]
            {
                let pid = self.pid.load(Ordering::Acquire);
                if pid != 0 {
                    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1);
                    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
                }
            }
            self.records.lock().unwrap().push(expected);
            Ok(())
        }
    }

    #[cfg(unix)]
    #[test]
    fn unavailable_limit_is_reported_and_requires_monitored_fallback_without_system_mutation() {
        for monitored in [true, false] {
            let diagnostics = Arc::new(Mutex::new(Vec::new()));
            let capture = Arc::clone(&diagnostics);
            let mut config = SupervisorConfig::new("/bin/sh".into(), &"shutdown").unwrap();
            config.arguments = vec!["-c".into(), "exec sleep 30".into()];
            config.containment_unavailable = true;
            config.on_diagnostic = Some(Arc::new(move |message| {
                capture.lock().unwrap().push(message.to_owned())
            }));
            if monitored {
                config.pressure_check = Some(Arc::new(|_| Ok(None)));
            }
            let supervisor = Supervisor::new(config).unwrap();
            let records = Arc::new(Mutex::new(Vec::new()));
            let admission = Admission {
                memory_limit_bytes: Some(1_000),
                lease: Some(Box::new(RecordingLease {
                    records: Arc::clone(&records),
                    pid: Default::default(),
                })),
                ..Default::default()
            };
            let result: Result<u32> = supervisor.execute_admitted(
                0,
                &"pid",
                RequestOptions {
                    cancel: None,
                    timeouts: Timeouts {
                        idle: Duration::from_millis(30),
                        total: None,
                    },
                    estimated_bytes: 0,
                },
                || Ok(admission),
                &mut |_: u32| {},
            );
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains(if monitored {
                    "time limit"
                } else {
                    "no active pressure monitor"
                }),
                "{error}"
            );
            assert!(diagnostics
                .lock()
                .unwrap()
                .iter()
                .any(|message| message.contains("synthetic undelegated")));
            assert!(diagnostics
                .lock()
                .unwrap()
                .iter()
                .all(|message| !message.contains("limit enforced")));
            assert_eq!(*records.lock().unwrap(), [false]);
            assert!(supervisor.worker_pid().is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn ambiguous_cleanup_and_drop_never_settle_a_lease_as_clean() {
        for retry in [false, true] {
            let mut config = SupervisorConfig::new("/bin/sh".into(), &"shutdown").unwrap();
            config.arguments = vec!["-c".into(), "exec sleep 30".into()];
            config.fail_cleanup = true;
            config.poll_interval = Duration::from_millis(5);
            let records = Arc::new(Mutex::new(Vec::new()));
            let pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let admission = Admission {
                lease: Some(Box::new(RecordingLease {
                    records: Arc::clone(&records),
                    pid: Arc::clone(&pid),
                })),
                ..Default::default()
            };
            let mut prepared = PreparedSpawn::new(&config, admission);
            prepared.prepare(&config).unwrap();
            let mut worker = Worker::spawn(&config, &mut prepared).unwrap();
            pid.store(
                lock(&worker.process.child).as_ref().unwrap().id(),
                Ordering::Release,
            );
            let start = Instant::now();
            assert!(worker
                .stop(Duration::ZERO, &AtomicBool::new(false), None, &config, true)
                .is_err());
            assert!(!worker.process.is_alive().unwrap());
            assert!(start.elapsed() < Duration::from_secs(1));
            assert!(records.lock().unwrap().is_empty());
            if retry {
                worker.process.fail_cleanup = false;
                worker
                    .stop(Duration::ZERO, &AtomicBool::new(false), None, &config, true)
                    .unwrap();
                assert_eq!(*records.lock().unwrap(), [false]);
            }
            drop(worker);
            assert_eq!(
                *records.lock().unwrap(),
                if retry { vec![false] } else { vec![] }
            );
        }
    }

    #[test]
    fn explicit_eviction_waits_only_for_bounded_interruptible_maintenance() {
        let queue = Queue::default();
        let shutdown = AtomicBool::new(false);
        let held = queue.try_idle().unwrap();
        let poll = Duration::from_millis(5);
        let started = Instant::now();
        assert!(queue.enter_idle(&shutdown, poll, poll).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        shutdown.store(true, Ordering::Release);
        assert!(queue
            .enter_idle(&shutdown, poll, Duration::from_secs(60))
            .is_err());
        drop(held);
        shutdown.store(false, Ordering::Release);
        let held = queue.try_idle().unwrap();
        thread::scope(|scope| {
            let waiting = scope.spawn(|| {
                queue
                    .enter_idle(&shutdown, poll, Duration::from_secs(1))
                    .is_ok()
            });
            thread::sleep(poll);
            drop(held);
            assert!(waiting.join().unwrap());
        });
    }

    #[test]
    fn queue_runs_in_fifo_order_and_releases_every_ticket() {
        let queue = Queue::default();
        let shutdown = AtomicBool::new(false);
        let poll = Duration::from_millis(5);
        let held = queue.enter(&shutdown, None, poll).unwrap();
        let order = Mutex::new(Vec::new());
        thread::scope(|scope| {
            for index in 0..4 {
                let queue = &queue;
                let order = &order;
                let shutdown = &shutdown;
                scope.spawn(move || {
                    let _ticket = queue.enter(shutdown, None, poll).unwrap();
                    lock(order).push(index);
                });
                let deadline = Instant::now() + Duration::from_secs(2);
                while lock(&queue.state).waiting.len() != index + 1 {
                    assert!(Instant::now() < deadline);
                    thread::sleep(poll);
                }
            }
            drop(held);
        });
        assert_eq!(*lock(&order), [0, 1, 2, 3]);
        assert!(lock(&queue.state).running.is_none());
        assert!(lock(&queue.state).waiting.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn partial_setup_failure_before_or_after_reader_creation_reaps_child() {
        for stage in [1, 2] {
            let mut config = SupervisorConfig::new("/bin/sh".into(), &"shutdown").unwrap();
            config.arguments = vec!["-c".into(), "exec sleep 30".into()];
            config.reap_timeout = Duration::from_millis(500);
            config.poll_interval = Duration::from_millis(5);
            config.fail_setup = Some(stage);
            let start = Instant::now();
            let records = Arc::new(Mutex::new(Vec::new()));
            let observed_pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let admission = Admission {
                lease: Some(Box::new(RecordingLease {
                    records: Arc::clone(&records),
                    pid: Arc::clone(&observed_pid),
                })),
                ..Default::default()
            };
            let mut prepared = PreparedSpawn::new(&config, admission);
            prepared.prepare(&config).unwrap();
            let error = match Worker::spawn(&config, &mut prepared) {
                Ok(_) => panic!("injected setup failure did not run"),
                Err(error) => {
                    observed_pid.store(error.to_string().parse().unwrap(), Ordering::Release);
                    prepared.failed(error, &config)
                }
            };
            let pid: libc::pid_t = error.to_string().parse().unwrap();
            assert!(start.elapsed() < Duration::from_secs(2));
            // Only probe the PID our own failed spawn returned; never signal
            // unrelated processes while checking lifecycle correctness.
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
            assert_eq!(*records.lock().unwrap(), [false]);
        }
    }
}
