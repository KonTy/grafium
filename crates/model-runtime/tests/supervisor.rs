use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use model_runtime::error::{Result, RuntimeError};
use model_runtime::recovery::{GpuAttempt, RecoveryLease, RecoveryState, RecoveryStore};
use model_runtime::supervisor::{
    Admission, RequestOptions, Supervisor, SupervisorConfig, Timeouts, WorkerLease,
};

fn config() -> SupervisorConfig {
    let mut config = SupervisorConfig::new(
        env!("CARGO_BIN_EXE_model-runtime-fixture").into(),
        &"shutdown",
    )
    .unwrap();
    config.arguments.push("--worker".into());
    config.poll_interval = Duration::from_millis(5);
    config.monitor_interval = Duration::from_millis(10);
    config.shutdown_grace = Duration::from_millis(80);
    config.reap_timeout = Duration::from_millis(500);
    config.restart.cooldown = Duration::from_millis(120);
    config
}

fn options(cancel: Option<&AtomicBool>, idle_ms: u64) -> RequestOptions<'_> {
    RequestOptions {
        cancel,
        timeouts: Timeouts {
            idle: Duration::from_millis(idle_ms),
            total: Some(Duration::from_secs(2)),
        },
        estimated_bytes: 10,
    }
}

fn request(supervisor: &Supervisor<u32>, key: u32, op: &str) -> Result<u32> {
    supervisor.execute(key, &op, options(None, 1_000), || Ok(()), &mut |_: u32| {})
}

#[derive(Default)]
struct LeaseLog {
    pid: AtomicU32,
    events: Mutex<Vec<bool>>,
    attempts: AtomicUsize,
    fail: AtomicBool,
}

struct RecordingLease(Arc<LeaseLog>);

impl WorkerLease for RecordingLease {
    fn confirmed_exit(&self, expected: bool) -> Result<()> {
        let pid = self.0.pid.load(Ordering::Acquire);
        if pid != 0 {
            assert!(
                !alive(pid),
                "lease was settled before the owned worker died"
            );
        }
        self.0.attempts.fetch_add(1, Ordering::SeqCst);
        if self.0.fail.load(Ordering::Acquire) {
            return Err(RuntimeError::Other(
                "synthetic recovery write failure".into(),
            ));
        }
        self.0.events.lock().unwrap().push(expected);
        Ok(())
    }
}

fn leased(log: &Arc<LeaseLog>) -> Admission {
    Admission {
        lease: Some(Box::new(RecordingLease(Arc::clone(log)))),
        ..Default::default()
    }
}

#[test]
fn admission_environment_overrides_config_and_one_lease_covers_resident_reuse() {
    let mut config = config();
    config
        .environment
        .push(("MODEL_RUNTIME_FIXTURE_VALUE".into(), "7".into()));
    let supervisor = Supervisor::new(config).unwrap();
    let lease = Arc::new(LeaseLog::default());
    let output: u32 = supervisor
        .execute_admitted(
            0,
            &"env",
            options(None, 1_000),
            || {
                let mut admission = leased(&lease);
                admission
                    .environment
                    .push(("MODEL_RUNTIME_FIXTURE_VALUE".into(), "19".into()));
                Ok(admission)
            },
            &mut |_: u32| {},
        )
        .unwrap();
    assert_eq!(output, 19);
    let pid: u32 = supervisor
        .execute_admitted(
            0,
            &"pid",
            options(None, 1_000),
            || panic!("resident reuse must not acquire a second lease"),
            &mut |_: u32| {},
        )
        .unwrap();
    lease.pid.store(pid, Ordering::Release);
    assert!(lease.events.lock().unwrap().is_empty());
    assert_eq!(request(&supervisor, 0, "env").unwrap(), 19);
    supervisor.evict_idle().unwrap();
    assert_eq!(*lease.events.lock().unwrap(), [true]);
    supervisor.shutdown().unwrap();
    assert_eq!(lease.attempts.load(Ordering::SeqCst), 1);
}

struct RenewableCpuLease(Arc<AtomicBool>);

impl WorkerLease for RenewableCpuLease {
    fn confirmed_exit(&self, _expected: bool) -> Result<()> {
        Ok(())
    }

    fn reusable(&self) -> bool {
        !self.0.load(Ordering::Acquire)
    }
}

#[test]
fn cached_cpu_lease_can_require_readmission_on_the_next_request() {
    let supervisor = Supervisor::new(config()).unwrap();
    let eligible = Arc::new(AtomicBool::new(false));
    let first: u32 = supervisor
        .execute_admitted(
            0,
            &"pid",
            options(None, 1_000),
            || {
                Ok(Admission {
                    lease: Some(Box::new(RenewableCpuLease(Arc::clone(&eligible)))),
                    ..Default::default()
                })
            },
            &mut |_: u32| {},
        )
        .unwrap();
    assert_eq!(request(&supervisor, 0, "pid").unwrap(), first);
    eligible.store(true, Ordering::Release);
    let admissions = AtomicUsize::new(0);
    let second: u32 = supervisor
        .execute_admitted(
            0,
            &"pid",
            options(None, 1_000),
            || {
                admissions.fetch_add(1, Ordering::SeqCst);
                assert!(!alive(first), "CPU must be stopped before GPU admission");
                Ok(Admission::default())
            },
            &mut |_: u32| {},
        )
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(admissions.load(Ordering::SeqCst), 1);
    assert_eq!(request(&supervisor, 0, "pid").unwrap(), second);
    supervisor.shutdown().unwrap();
}

struct GpuRecoveryLease(RecoveryLease);

impl WorkerLease for GpuRecoveryLease {
    fn confirmed_exit(&self, expected: bool) -> Result<()> {
        self.0.confirm_exit(expected)
    }
}

#[test]
fn automatic_recovery_failure_is_not_replayed_and_later_requests_use_cpu() {
    let directory = tempfile::tempdir().unwrap();
    let store = RecoveryStore::open(directory.path().join("synthetic-recovery")).unwrap();
    let supervisor = Supervisor::new(config()).unwrap();
    let attempts = AtomicUsize::new(0);
    let admit = || {
        attempts.fetch_add(1, Ordering::SeqCst);
        let (lease, gpu): (Option<Box<dyn WorkerLease>>, &str) =
            match store.prepare("synthetic-model", "fixture")? {
                GpuAttempt::Allowed(lease) => (Some(Box::new(GpuRecoveryLease(lease))), "1"),
                GpuAttempt::CpuOnly { .. } => (None, "0"),
            };
        Ok(Admission {
            lease,
            environment: vec![("MODEL_RUNTIME_FIXTURE_VALUE".into(), gpu.into())],
            ..Default::default()
        })
    };
    for (attempt, state) in [
        (1, RecoveryState::RetryPending),
        (2, RecoveryState::CpuOnly),
    ] {
        let mut partial_output = Vec::new();
        let result: Result<u32> = supervisor.execute_admitted(
            0,
            &"crash",
            options(None, 1_000),
            &admit,
            &mut |output: u32| partial_output.push(output),
        );
        assert!(result.is_err());
        assert_eq!(
            partial_output.len(),
            1,
            "partial output must not be replayed"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), attempt);
        assert_eq!(store.blocked().unwrap()[0].state, state);
    }
    let cpu: u32 = supervisor
        .execute_admitted(0, &"env", options(None, 1_000), &admit, &mut |_: u32| {})
        .unwrap();
    assert_eq!(cpu, 0);
    assert_eq!(request(&supervisor, 0, "env").unwrap(), 0);
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    supervisor.shutdown().unwrap();
    let reopened = RecoveryStore::open(directory.path().join("synthetic-recovery")).unwrap();
    assert_eq!(reopened.blocked().unwrap()[0].state, RecoveryState::CpuOnly);
}

#[test]
fn crash_timeout_and_protocol_errors_settle_failed_only_after_reaping() {
    for op in ["crash", "hang", "error", "malformed"] {
        let supervisor = Supervisor::new(config()).unwrap();
        let lease = Arc::new(LeaseLog::default());
        let start = Instant::now();
        let result: Result<u32> = supervisor.execute_admitted(
            0,
            &op,
            options(None, 150),
            || Ok(leased(&lease)),
            &mut |pid| lease.pid.store(pid, Ordering::Release),
        );
        assert!(result.is_err(), "{op}");
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(*lease.events.lock().unwrap(), [false], "{op}");
        assert!(!alive(lease.pid.load(Ordering::Acquire)));
    }
}

#[test]
fn intentional_cancel_and_idle_shutdown_escalation_are_expected_exits() {
    for op in ["pid", "ignore-shutdown", "hang"] {
        let supervisor = Supervisor::new(config()).unwrap();
        let lease = Arc::new(LeaseLog::default());
        let cancel = AtomicBool::new(false);
        let result: Result<u32> = supervisor.execute_admitted(
            0,
            &op,
            options(Some(&cancel), 1_000),
            || Ok(leased(&lease)),
            &mut |pid| {
                lease.pid.store(pid, Ordering::Release);
                cancel.store(true, Ordering::Release);
            },
        );
        if op == "hang" {
            assert!(matches!(result, Err(RuntimeError::Cancelled)));
        } else {
            lease.pid.store(result.unwrap(), Ordering::Release);
            assert!(lease.events.lock().unwrap().is_empty());
            supervisor.shutdown().unwrap();
        }
        assert_eq!(*lease.events.lock().unwrap(), [true], "{op}");
    }
}

#[test]
fn unsolicited_exit_between_requests_is_not_confirmed_as_clean() {
    for op in ["exit-after-output", "crash-on-shutdown"] {
        let supervisor = Supervisor::new(config()).unwrap();
        let lease = Arc::new(LeaseLog::default());
        let pid: u32 = supervisor
            .execute_admitted(
                0,
                &op,
                options(None, 1_000),
                || Ok(leased(&lease)),
                &mut |_: u32| {},
            )
            .unwrap();
        lease.pid.store(pid, Ordering::Release);
        if op == "exit-after-output" {
            wait_until(|| !alive(pid));
        }
        supervisor.evict_idle().unwrap();
        assert_eq!(*lease.events.lock().unwrap(), [false]);
    }
}

#[test]
fn cancellation_after_admission_and_failure_to_spawn_settle_without_a_child() {
    let supervisor = Supervisor::new(config()).unwrap();
    let lease = Arc::new(LeaseLog::default());
    let cancel = AtomicBool::new(false);
    let result: Result<u32> = supervisor.execute_admitted(
        0,
        &"pid",
        options(Some(&cancel), 1_000),
        || {
            cancel.store(true, Ordering::Release);
            Ok(leased(&lease))
        },
        &mut |_: u32| {},
    );
    assert!(matches!(result, Err(RuntimeError::Cancelled)));
    assert_eq!(*lease.events.lock().unwrap(), [true]);
    assert!(supervisor.worker_pid().is_none());

    let directory = tempfile::tempdir().unwrap();
    let mut missing = config();
    missing.executable = directory.path().join("not-a-worker");
    let supervisor = Supervisor::new(missing).unwrap();
    let lease = Arc::new(LeaseLog::default());
    let result: Result<u32> = supervisor.execute_admitted(
        0,
        &"pid",
        options(None, 1_000),
        || Ok(leased(&lease)),
        &mut |_: u32| {},
    );
    assert!(result.is_err());
    assert_eq!(*lease.events.lock().unwrap(), [false]);
    assert!(supervisor.worker_pid().is_none());
}

#[test]
fn failed_lease_settlement_is_not_retried_as_success_or_allowed_to_spawn_replacement() {
    let supervisor = Supervisor::new(config()).unwrap();
    let lease = Arc::new(LeaseLog::default());
    let pid: u32 = supervisor
        .execute_admitted(
            0,
            &"pid",
            options(None, 1_000),
            || Ok(leased(&lease)),
            &mut |_: u32| {},
        )
        .unwrap();
    lease.pid.store(pid, Ordering::Release);
    lease.fail.store(true, Ordering::Release);
    assert!(supervisor.evict_idle().is_err());
    assert!(!alive(pid));
    assert!(lease.events.lock().unwrap().is_empty());
    let next: Result<u32> = supervisor.execute_admitted(
        1,
        &"pid",
        options(None, 1_000),
        || panic!("failed cleanup must block a replacement"),
        &mut |_: u32| {},
    );
    assert!(next.is_err());
    assert_eq!(lease.attempts.load(Ordering::SeqCst), 1);
}

#[test]
fn implicit_supervisor_drop_reaps_but_leaves_recovery_marker_unconfirmed() {
    let lease = Arc::new(LeaseLog::default());
    let pid = {
        let supervisor = Supervisor::new(config()).unwrap();
        let pid: u32 = supervisor
            .execute_admitted(
                0,
                &"pid",
                options(None, 1_000),
                || Ok(leased(&lease)),
                &mut |_: u32| {},
            )
            .unwrap();
        lease.pid.store(pid, Ordering::Release);
        pid
    };
    assert!(!alive(pid));
    assert!(lease.events.lock().unwrap().is_empty());
    assert_eq!(lease.attempts.load(Ordering::SeqCst), 0);
}

#[test]
fn active_pressure_reaps_worker_marks_failure_and_releases_queued_request() {
    let lease = Arc::new(LeaseLog::default());
    let trip = Arc::new(AtomicBool::new(false));
    let mut config = config();
    let probe_lease = Arc::clone(&lease);
    let probe_trip = Arc::clone(&trip);
    config.pressure_check_interval = Duration::from_millis(10);
    config.pressure_check = Some(Arc::new(move |pid| {
        Ok(
            (probe_trip.load(Ordering::Acquire) && probe_lease.pid.load(Ordering::Acquire) == pid)
                .then(|| "synthetic external RAM pressure".into()),
        )
    }));
    let supervisor = Arc::new(Supervisor::new(config).unwrap());
    let running = Arc::clone(&supervisor);
    let active_lease = Arc::clone(&lease);
    let (started, ready) = mpsc::channel();
    let active = thread::spawn(move || {
        running.execute_admitted::<_, u32, u32>(
            0,
            &"hang",
            options(None, 60_000),
            || Ok(leased(&active_lease)),
            &mut |pid| {
                active_lease.pid.store(pid, Ordering::Release);
                started.send(pid).unwrap();
            },
        )
    });
    let pid = ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let following = Arc::clone(&supervisor);
    let queued = thread::spawn(move || request(&following, 1, "pid"));
    wait_until(|| supervisor.queued_requests() == 1);
    let start = Instant::now();
    trip.store(true, Ordering::Release);
    assert!(matches!(
        active.join().unwrap(),
        Err(RuntimeError::ResourcePressure(_))
    ));
    assert!(!alive(pid));
    assert_eq!(*lease.events.lock().unwrap(), [false]);
    let replacement = queued.join().unwrap().unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    supervisor.shutdown().unwrap();
    assert!(!alive(replacement));
}

#[test]
fn active_pressure_checks_cover_blocked_writes_without_extending_deadlines() {
    for pressure_trip in [true, false] {
        let lease = Arc::new(LeaseLog::default());
        let probe_lease = Arc::clone(&lease);
        let checks = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&checks);
        let mut config = config();
        config.arguments = vec!["--unread".into()];
        config.pressure_check_interval = Duration::from_millis(20);
        config.pressure_check = Some(Arc::new(move |pid| {
            probe_lease.pid.store(pid, Ordering::Release);
            let count = counter.fetch_add(1, Ordering::SeqCst);
            Ok((pressure_trip && count > 0).then(|| "synthetic write-time pressure".into()))
        }));
        let supervisor = Supervisor::new(config).unwrap();
        let start = Instant::now();
        let result: Result<u32> = supervisor.execute_admitted(
            0,
            &"x".repeat(1024 * 1024),
            options(None, 150),
            || Ok(leased(&lease)),
            &mut |_: u32| {},
        );
        if pressure_trip {
            assert!(matches!(result, Err(RuntimeError::ResourcePressure(_))));
        } else {
            assert!(result.unwrap_err().to_string().contains("time limit"));
        }
        assert!(checks.load(Ordering::SeqCst) >= 2);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(*lease.events.lock().unwrap(), [false]);
    }
}

#[test]
fn failed_pressure_probe_is_explicit_and_host_hooks_run_without_registry_or_process_locks() {
    let lease = Arc::new(LeaseLog::default());
    let owner = Arc::new(Mutex::new(std::sync::Weak::<Supervisor<u32>>::new()));
    let mut config = config();
    let diagnostic_owner = Arc::clone(&owner);
    config.on_diagnostic = Some(Arc::new(move |_| {
        if let Some(supervisor) = diagnostic_owner.lock().unwrap().upgrade() {
            supervisor.worker_pid();
        }
    }));
    let pressure_owner = Arc::clone(&owner);
    let pressure_lease = Arc::clone(&lease);
    config.pressure_check = Some(Arc::new(move |pid| {
        pressure_lease.pid.store(pid, Ordering::Release);
        let supervisor = pressure_owner.lock().unwrap().upgrade().unwrap();
        assert_eq!(supervisor.worker_pid(), Some(pid));
        Err(RuntimeError::Other("synthetic measurement failure".into()))
    }));
    let supervisor = Arc::new(Supervisor::new(config).unwrap());
    *owner.lock().unwrap() = Arc::downgrade(&supervisor);
    let result: Result<u32> = supervisor.execute_admitted(
        0,
        &"hang",
        options(None, 1_000),
        || Ok(leased(&lease)),
        &mut |_: u32| {},
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("pressure check failed"));
    assert_eq!(*lease.events.lock().unwrap(), [false]);
}

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "condition did not become true before deadline"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // Orphans may await their external init reaper; a zombie has no resident
    // model or executable code and is not a live process.
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        if stat
            .rsplit_once(") ")
            .is_some_and(|(_, rest)| rest.starts_with('Z'))
        {
            return false;
        }
    }
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let alive = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
        CloseHandle(handle);
        alive
    }
}

#[test]
fn reuse_and_model_switch_never_overlap_resident_workers() {
    let supervisor = Supervisor::new(config()).unwrap();
    let admissions = AtomicUsize::new(0);
    let first: u32 = supervisor
        .execute(
            1,
            &"pid",
            options(None, 1_000),
            || {
                admissions.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            &mut |_: u32| {},
        )
        .unwrap();
    assert_eq!(request(&supervisor, 1, "pid").unwrap(), first);
    let second: u32 = supervisor
        .execute(
            2,
            &"pid",
            options(None, 1_000),
            || {
                assert!(!alive(first), "admission began before old child was reaped");
                admissions.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            &mut |_: u32| {},
        )
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(admissions.load(Ordering::SeqCst), 2);
    supervisor.shutdown().unwrap();
    assert!(!alive(second));
}

#[test]
fn ending_the_spawning_thread_does_not_end_the_worker_process() {
    let supervisor = Arc::new(Supervisor::new(config()).unwrap());
    let spawning_thread = Arc::clone(&supervisor);
    let pid = thread::spawn(move || request(&spawning_thread, 0, "pid").unwrap())
        .join()
        .unwrap();
    assert!(alive(pid));
    assert_eq!(request(&supervisor, 0, "pid").unwrap(), pid);
    supervisor.shutdown().unwrap();
    assert!(!alive(pid));
}

#[test]
fn increasing_estimate_requires_eviction_and_new_admission() {
    let supervisor = Supervisor::new(config()).unwrap();
    let pid = request(&supervisor, 0, "pid").unwrap();
    let mut larger = options(None, 1_000);
    larger.estimated_bytes = 20;
    let next: u32 = supervisor
        .execute(
            0,
            &"pid",
            larger,
            || {
                assert!(!alive(pid));
                Ok(())
            },
            &mut |_: u32| {},
        )
        .unwrap();
    assert_ne!(next, pid);
    supervisor.shutdown().unwrap();
    assert!(!alive(next));
}

#[test]
fn explicit_idle_eviction_unloads_without_permanently_stopping_admission() {
    let supervisor = Supervisor::new(config()).unwrap();
    let first = request(&supervisor, 0, "ignore-shutdown").unwrap();
    let start = Instant::now();
    supervisor.evict_idle().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!alive(first));
    assert!(supervisor.worker_pid().is_none());
    supervisor.evict_idle().unwrap();
    let replacement = request(&supervisor, 0, "pid").unwrap();
    assert_ne!(replacement, first);
    supervisor.shutdown().unwrap();
    assert!(!alive(replacement));
}

#[test]
fn explicit_idle_eviction_does_not_interrupt_active_requests() {
    let supervisor = Arc::new(Supervisor::new(config()).unwrap());
    let running = Arc::clone(&supervisor);
    let (started, ready) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let active_cancel = Arc::clone(&cancel);
    let active = thread::spawn(move || {
        running.execute::<_, u32, u32>(
            0,
            &"hang",
            options(Some(&active_cancel), 60_000),
            || Ok(()),
            &mut |pid| {
                started.send(pid).unwrap();
            },
        )
    });
    let pid = ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let start = Instant::now();
    assert!(supervisor
        .evict_idle()
        .unwrap_err()
        .to_string()
        .contains("busy"));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(alive(pid));
    cancel.store(true, Ordering::Release);
    assert!(active.join().unwrap().is_err());
    supervisor.evict_idle().unwrap();
    assert!(!alive(pid));
}

#[test]
fn progress_preserves_reuse_but_does_not_extend_total_deadline_forever() {
    let supervisor = Supervisor::new(config()).unwrap();
    let mut progress: Vec<u32> = Vec::new();
    let pid: u32 = supervisor
        .execute(
            0,
            &"progress",
            options(None, 100),
            || Ok(()),
            &mut |value| progress.push(value),
        )
        .unwrap();
    assert_eq!(progress, [pid, 1, 2, 3]);
    assert_eq!(request(&supervisor, 0, "pid").unwrap(), pid);
    let mut limited = options(None, 100);
    limited.timeouts.total = Some(Duration::from_millis(100));
    let start = Instant::now();
    let result: Result<u32> =
        supervisor.execute(0, &"progress-forever", limited, || Ok(()), &mut |_: u32| {});
    assert!(result.unwrap_err().to_string().contains("time limit"));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!alive(pid));
}

#[test]
fn malformed_error_crash_and_truncated_responses_evict_without_replay() {
    for op in [
        "malformed",
        "oversized",
        "truncated-header",
        "truncated-body",
        "invalid",
        "error",
        "crash",
    ] {
        let supervisor = Supervisor::new(config()).unwrap();
        let mut pid = None;
        let attempts = AtomicUsize::new(0);
        let start = Instant::now();
        let result: Result<u32> = supervisor.execute(
            0,
            &op,
            options(None, 500),
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            &mut |value| pid = Some(value),
        );
        assert!(result.is_err(), "{op}");
        assert!(start.elapsed() < Duration::from_secs(2), "{op}");
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "a failed request was replayed"
        );
        let pid = pid.expect("fixture reports its owned PID before failing");
        assert!(!alive(pid), "{op}: owned child remains alive");
        let replacement = request(&supervisor, 0, "pid").unwrap();
        assert_ne!(replacement, pid);
        supervisor.shutdown().unwrap();
        assert!(!alive(replacement));
    }
}

#[test]
fn crash_budget_blocks_respawns_until_cooldown() {
    let mut config = config();
    config.restart.max_failures = 2;
    let supervisor = Supervisor::new(config).unwrap();
    let mut crashed = Vec::new();
    for _ in 0..2 {
        let result: Result<u32> =
            supervisor.execute(0, &"crash", options(None, 500), || Ok(()), &mut |pid| {
                crashed.push(pid)
            });
        assert!(result.is_err());
    }
    assert_eq!(crashed.len(), 2);
    assert!(crashed.into_iter().all(|pid| !alive(pid)));
    assert!(request(&supervisor, 0, "pid")
        .unwrap_err()
        .to_string()
        .contains("cooldown"));
    thread::sleep(Duration::from_millis(150));
    let pid = request(&supervisor, 0, "pid").unwrap();
    supervisor.shutdown().unwrap();
    assert!(!alive(pid));
}

#[test]
fn silent_worker_timeout_and_active_cancellation_reap_children() {
    for cancel_request in [false, true] {
        let supervisor = Supervisor::new(config()).unwrap();
        let cancel = AtomicBool::new(false);
        let mut pid = None;
        let started = Instant::now();
        let result: Result<u32> = supervisor.execute(
            0,
            &"hang",
            options(Some(&cancel), if cancel_request { 30_000 } else { 40 }),
            || Ok(()),
            &mut |value| {
                pid = Some(value);
                if cancel_request {
                    cancel.store(true, Ordering::Release);
                }
            },
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!alive(pid.unwrap()));
    }
}

#[test]
fn shutdown_reaps_active_worker_and_wakes_non_cancellable_queue() {
    let supervisor = Arc::new(Supervisor::new(config()).unwrap());
    let (started, ready) = mpsc::channel();
    let running = Arc::clone(&supervisor);
    let active = thread::spawn(move || {
        running.execute::<_, u32, u32>(0, &"hang", options(None, 60_000), || Ok(()), &mut |pid| {
            started.send(pid).unwrap();
        })
    });
    let pid = ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut queued = Vec::new();
    for _ in 0..4 {
        let supervisor = Arc::clone(&supervisor);
        queued.push(thread::spawn(move || request(&supervisor, 0, "pid")));
    }
    wait_until(|| supervisor.queued_requests() == 4);
    let start = Instant::now();
    supervisor.shutdown().unwrap();
    assert!(active.join().unwrap().is_err());
    for queued in queued {
        assert!(queued.join().unwrap().is_err());
    }
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!alive(pid));
    assert!(request(&supervisor, 0, "pid")
        .unwrap_err()
        .to_string()
        .contains("shutting down"));
}

#[test]
fn cancelled_queue_ticket_does_not_strand_requests_behind_it() {
    let supervisor = Arc::new(Supervisor::new(config()).unwrap());
    let (started, ready) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let running = Arc::clone(&supervisor);
    let active = thread::spawn(move || {
        let mut first = true;
        running.execute::<_, u32, u32>(0, &"delay", options(None, 2_000), || Ok(()), &mut |pid| {
            if first {
                first = false;
                started.send(pid).unwrap();
                held.recv_timeout(Duration::from_secs(2)).unwrap();
            }
        })
    });
    let pid = ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let (waiting, flag) = (Arc::clone(&supervisor), Arc::clone(&cancel));
    let abandoned = thread::spawn(move || {
        waiting.execute::<_, u32, u32>(
            0,
            &"pid",
            options(Some(&flag), 1_000),
            || Ok(()),
            &mut |_| {},
        )
    });
    wait_until(|| supervisor.queued_requests() == 1);
    let waiting = Arc::clone(&supervisor);
    let following = thread::spawn(move || request(&waiting, 0, "pid"));
    wait_until(|| supervisor.queued_requests() == 2);
    cancel.store(true, Ordering::Release);
    assert!(abandoned
        .join()
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    release.send(()).unwrap();
    assert_eq!(active.join().unwrap().unwrap(), pid);
    assert_eq!(following.join().unwrap().unwrap(), pid);
    supervisor.shutdown().unwrap();
    assert!(!alive(pid));
}

#[test]
fn blocked_stdin_cannot_defeat_timeout_or_shutdown_deadline() {
    for explicit_shutdown in [false, true] {
        let mut config = config();
        config.arguments = vec!["--unread".into()];
        let supervisor = Arc::new(Supervisor::new(config).unwrap());
        let running = Arc::clone(&supervisor);
        let active = thread::spawn(move || {
            running.execute::<_, u32, u32>(
                0,
                &"x".repeat(1024 * 1024),
                options(None, if explicit_shutdown { 60_000 } else { 150 }),
                || Ok(()),
                &mut |_| {},
            )
        });
        wait_until(|| supervisor.worker_pid().is_some());
        let pid = supervisor.worker_pid().unwrap();
        let start = Instant::now();
        if explicit_shutdown {
            supervisor.shutdown().unwrap();
        }
        assert!(active.join().unwrap().is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(!alive(pid));
    }
}

#[test]
fn model_switch_grace_is_interruptible_and_replacement_is_not_admitted_after_cancel() {
    let mut config = config();
    config.shutdown_grace = Duration::from_secs(5);
    let supervisor = Arc::new(Supervisor::new(config).unwrap());
    let first = request(&supervisor, 0, "ignore-shutdown").unwrap();
    let running = Arc::clone(&supervisor);
    let cancel = Arc::new(AtomicBool::new(false));
    let cancelled = Arc::clone(&cancel);
    let replacement = thread::spawn(move || {
        running.execute::<_, u32, u32>(
            1,
            &"pid",
            options(Some(&cancelled), 1_000),
            || panic!("cancelled replacement must not reach admission"),
            &mut |_| {},
        )
    });
    thread::sleep(Duration::from_millis(30));
    let start = Instant::now();
    cancel.store(true, Ordering::Release);
    assert!(replacement
        .join()
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!alive(first));
}

#[test]
fn idle_pressure_and_hung_graceful_exit_are_bounded() {
    for pressure in [false, true] {
        let mut config = config();
        let pressure_flag = Arc::new(AtomicBool::new(false));
        if pressure {
            let flag = Arc::clone(&pressure_flag);
            config.memory_pressure = Some(Arc::new(move || flag.load(Ordering::Acquire)));
        } else {
            config.idle_timeout = Duration::from_millis(50);
        }
        let supervisor = Supervisor::new(config).unwrap();
        let pid = request(&supervisor, 0, "ignore-shutdown").unwrap();
        pressure_flag.store(true, Ordering::Release);
        let start = Instant::now();
        wait_until(|| !alive(pid));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(supervisor.worker_pid().is_none() || !alive(pid));
        supervisor.shutdown().unwrap();
    }
}

#[test]
fn panicking_host_progress_callback_evicts_and_releases_fifo_gate() {
    let supervisor = Supervisor::new(config()).unwrap();
    let mut pid = None;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<u32> =
            supervisor.execute(0, &"hang", options(None, 1_000), || Ok(()), &mut |value| {
                pid = Some(value);
                panic!("synthetic host progress callback panic");
            });
    }));
    assert!(outcome.is_err());
    assert!(!alive(pid.unwrap()));
    let next = request(&supervisor, 0, "pid").unwrap();
    supervisor.shutdown().unwrap();
    assert!(!alive(next));
}

#[test]
fn shutdown_can_stop_child_even_while_host_progress_callback_is_blocked() {
    let supervisor = Arc::new(Supervisor::new(config()).unwrap());
    let (started, ready) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let running = Arc::clone(&supervisor);
    let active = thread::spawn(move || {
        running.execute::<_, u32, u32>(0, &"hang", options(None, 60_000), || Ok(()), &mut |pid| {
            started.send(pid).unwrap();
            held.recv_timeout(Duration::from_secs(2)).unwrap();
        })
    });
    let pid = ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let start = Instant::now();
    supervisor.shutdown().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!alive(pid));
    release.send(()).unwrap();
    assert!(active.join().unwrap().is_err());
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
fn parent_death_terminates_busy_worker_without_pid_polling() {
    let mut parent = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_model-runtime-fixture"))
            .arg("--parent")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let stdout = parent.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        tx.send(line.trim().parse::<u32>().unwrap()).unwrap();
    });
    let worker_pid = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(alive(worker_pid));
    let start = Instant::now();
    parent.0.kill().unwrap();
    parent.0.wait().unwrap();
    wait_until(|| !alive(worker_pid));
    assert!(start.elapsed() < Duration::from_secs(2));
}
