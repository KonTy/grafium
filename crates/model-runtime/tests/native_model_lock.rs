//! Exercise the real file-lease implementation without enabling native engines.
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fs2::FileExt;
use model_runtime::{error, supervisor};
use supervisor::{Admission, RequestOptions, Supervisor, SupervisorConfig, Timeouts, WorkerLease};

#[path = "../src/native/model_file.rs"]
mod model_file;

fn exclusive_available(path: &Path) -> bool {
    let file = File::open(path).unwrap();
    let available = FileExt::try_lock_exclusive(&file).is_ok();
    if available {
        FileExt::unlock(&file).unwrap();
    }
    available
}

fn config() -> SupervisorConfig {
    let mut config = SupervisorConfig::new(
        env!("CARGO_BIN_EXE_model-runtime-fixture").into(),
        &"shutdown",
    )
    .unwrap();
    config.arguments.push("--worker".into());
    config.poll_interval = Duration::from_millis(5);
    config.shutdown_grace = Duration::from_millis(40);
    config.reap_timeout = Duration::from_millis(500);
    config
}

fn options(cancel: Option<&AtomicBool>) -> RequestOptions<'_> {
    RequestOptions {
        cancel,
        estimated_bytes: 0,
        timeouts: Timeouts {
            idle: Duration::from_millis(200),
            total: Some(Duration::from_secs(2)),
        },
    }
}

fn admission(path: &Path, recovery: Option<Box<dyn WorkerLease>>) -> error::Result<Admission> {
    let locked = model_file::lock_for_admission(path)?;
    assert_eq!(locked.path(), path.canonicalize().unwrap());
    Ok(Admission {
        lease: Some(Box::new(model_file::ModelFileLease::new(locked, recovery))),
        ..Default::default()
    })
}

#[test]
fn cpu_worker_retains_file_lock_across_reuse_and_releases_after_idle_eviction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.gguf");
    std::fs::write(&path, b"not a real model").unwrap();
    let supervisor = Supervisor::new(config()).unwrap();
    let first: u32 = supervisor
        .execute_admitted(
            0,
            &"pid",
            options(None),
            || admission(&path, None),
            &mut |_: u32| {},
        )
        .unwrap();
    assert!(!exclusive_available(&path));
    let reused: u32 = supervisor
        .execute_admitted(
            0,
            &"pid",
            options(None),
            || panic!("reuse must preserve its existing inode lease"),
            &mut |_: u32| {},
        )
        .unwrap();
    assert_eq!(first, reused);
    assert!(!exclusive_available(&path));
    supervisor.evict_idle().unwrap();
    assert!(exclusive_available(&path));
    let _: u32 = supervisor
        .execute_admitted(
            0,
            &"pid",
            options(None),
            || admission(&path, None),
            &mut |_: u32| {},
        )
        .unwrap();
    assert!(!exclusive_available(&path));
    supervisor.shutdown().unwrap();
    assert!(exclusive_available(&path));
}

struct RecoveryProbe {
    pid: Arc<AtomicU32>,
    expected: Arc<Mutex<Vec<bool>>>,
    fail: bool,
}

impl WorkerLease for RecoveryProbe {
    fn confirmed_exit(&self, expected: bool) -> error::Result<()> {
        let pid = self.pid.load(Ordering::Acquire);
        #[cfg(not(unix))]
        let _ = pid;
        #[cfg(unix)]
        if pid != 0 {
            assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
        self.expected.lock().unwrap().push(expected);
        if self.fail {
            return Err(error::RuntimeError::Other(
                "synthetic recovery write failure".into(),
            ));
        }
        Ok(())
    }
}

#[test]
fn crash_timeout_and_cancel_release_locks_only_after_confirmed_process_exit() {
    for op in ["crash", "hang", "cancel"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        std::fs::write(&path, b"not a real model").unwrap();
        let supervisor = Supervisor::new(config()).unwrap();
        let pid = Arc::new(AtomicU32::new(0));
        let expected = Arc::new(Mutex::new(Vec::new()));
        let cancel = AtomicBool::new(false);
        let result: error::Result<u32> = supervisor.execute_admitted(
            0,
            &if op == "cancel" { "hang" } else { op },
            options(Some(&cancel)),
            || {
                admission(
                    &path,
                    Some(Box::new(RecoveryProbe {
                        pid: Arc::clone(&pid),
                        expected: Arc::clone(&expected),
                        fail: false,
                    })),
                )
            },
            &mut |value| {
                pid.store(value, Ordering::Release);
                assert!(!exclusive_available(&path));
                if op == "cancel" {
                    cancel.store(true, Ordering::Release);
                }
            },
        );
        assert!(result.is_err());
        assert!(exclusive_available(&path));
        assert_eq!(*expected.lock().unwrap(), [op == "cancel"]);
    }
}

#[test]
fn failed_spawn_releases_an_unused_file_lease() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.gguf");
    std::fs::write(&path, b"not a real model").unwrap();
    let mut config = config();
    config.executable = dir.path().join("missing-worker");
    let supervisor = Supervisor::new(config).unwrap();
    let result: error::Result<u32> = supervisor.execute_admitted(
        0,
        &"pid",
        options(None),
        || admission(&path, None),
        &mut |_: u32| {},
    );
    assert!(result.is_err());
    assert!(exclusive_available(&path));
}

#[test]
fn recovery_io_failure_does_not_keep_a_proven_dead_models_file_locked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.gguf");
    std::fs::write(&path, b"not a real model").unwrap();
    let supervisor = Supervisor::new(config()).unwrap();
    let pid = Arc::new(AtomicU32::new(0));
    let expected = Arc::new(Mutex::new(Vec::new()));
    let actual: u32 = supervisor
        .execute_admitted(
            0,
            &"pid",
            options(None),
            || {
                admission(
                    &path,
                    Some(Box::new(RecoveryProbe {
                        pid: Arc::clone(&pid),
                        expected: Arc::clone(&expected),
                        fail: true,
                    })),
                )
            },
            &mut |_: u32| {},
        )
        .unwrap();
    pid.store(actual, Ordering::Release);
    assert!(!exclusive_available(&path));
    assert!(supervisor
        .evict_idle()
        .unwrap_err()
        .to_string()
        .contains("synthetic recovery"));
    assert!(exclusive_available(&path));
    assert_eq!(*expected.lock().unwrap(), [true]);
}

#[test]
#[ignore = "child-only fixture for unconfirmed_drop_blocks_other_processes_until_host_exit"]
fn unconfirmed_drop_retains_lock_fixture() {
    let path = std::path::PathBuf::from(std::env::var_os("MODEL_RUNTIME_LOCK_TEST_PATH").unwrap());
    let lease =
        model_file::ModelFileLease::new(model_file::lock_for_admission(&path).unwrap(), None);
    drop(lease);
    assert!(!exclusive_available(&path));
    assert!(model_file::lock_for_admission(&path).is_err());
    std::fs::write(path.with_extension("ready"), b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !path.with_extension("done").exists() {
        assert!(
            Instant::now() < deadline,
            "fixture controller did not finish"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn unconfirmed_drop_blocks_other_processes_until_host_exit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.gguf");
    std::fs::write(&path, b"not a real model").unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "unconfirmed_drop_retains_lock_fixture",
            "--ignored",
            "--quiet",
        ])
        .env("MODEL_RUNTIME_LOCK_TEST_PATH", &path)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    while !path.with_extension("ready").exists() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("model lock fixture did not become ready");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let blocked = !exclusive_available(&path);
    std::fs::write(path.with_extension("done"), b"done").unwrap();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("model lock fixture did not exit");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(status.success());
    assert!(
        blocked,
        "unconfirmed cleanup released the inode lock too early"
    );
    assert!(exclusive_available(&path));
}
