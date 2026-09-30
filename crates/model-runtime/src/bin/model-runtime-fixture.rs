//! Deterministic subprocess fixture. It never loads a model or accesses data.
use std::io::{self, Write};
use std::time::Duration;

use model_runtime::protocol::{self, Response};
use model_runtime::supervisor::{
    exit_without_native_cleanup, start_parent_watchdog, RequestOptions, Supervisor,
    SupervisorConfig, Timeouts,
};

fn emit(output: Option<u32>, error: Option<&str>, progress: Option<u32>) {
    let frame = Response {
        output,
        error: error.map(str::to_owned),
        progress,
    };
    protocol::write_frame(
        &mut io::stdout().lock(),
        &frame,
        protocol::DEFAULT_RESPONSE_LIMIT,
    )
    .unwrap();
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "--parent" {
        let mut config =
            SupervisorConfig::new(std::env::current_exe().unwrap(), &"shutdown").unwrap();
        config.arguments.push("--worker".into());
        let supervisor = Supervisor::new(config).unwrap();
        let pid: u32 = supervisor
            .execute(
                0,
                &"pid",
                RequestOptions {
                    cancel: None,
                    timeouts: Timeouts {
                        idle: Duration::from_secs(2),
                        total: None,
                    },
                    estimated_bytes: 0,
                },
                || Ok(()),
                &mut |_: u32| {},
            )
            .unwrap();
        println!("{pid}");
        io::stdout().flush().unwrap();
        // The watchdog must work while the child is busy, not merely at EOF.
        let _: Result<u32, _> = supervisor.execute(
            0,
            &"hang",
            RequestOptions {
                cancel: None,
                timeouts: Timeouts {
                    idle: Duration::from_secs(60),
                    total: None,
                },
                estimated_bytes: 0,
            },
            || Ok(()),
            &mut |_: u32| {},
        );
        return;
    }
    if !matches!(mode.as_str(), "--worker" | "--unread") {
        return;
    }
    if start_parent_watchdog().is_err() {
        exit_without_native_cleanup(3);
    }
    if mode == "--unread" {
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    let mut ignore_shutdown = false;
    let mut crash_on_shutdown = false;
    loop {
        let request: String =
            match protocol::read_frame(&mut io::stdin().lock(), protocol::DEFAULT_REQUEST_LIMIT) {
                Ok(Some(request)) => request,
                _ => exit_without_native_cleanup(0),
            };
        let pid = std::process::id();
        if request == "shutdown" {
            if crash_on_shutdown {
                exit_without_native_cleanup(42);
            }
            if ignore_shutdown {
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                }
            }
            exit_without_native_cleanup(0);
        }
        if request == "pid" || request == "ignore-shutdown" || request == "crash-on-shutdown" {
            ignore_shutdown |= request == "ignore-shutdown";
            crash_on_shutdown |= request == "crash-on-shutdown";
            emit(Some(pid), None, None);
            continue;
        }
        if request == "env" {
            let value = std::env::var("MODEL_RUNTIME_FIXTURE_VALUE")
                .unwrap()
                .parse()
                .unwrap();
            emit(Some(value), None, None);
            continue;
        }
        if request == "exit-after-output" {
            emit(Some(pid), None, None);
            exit_without_native_cleanup(42);
        }
        emit(None, None, Some(pid));
        match request.as_str() {
            "progress" | "delay" => {
                for value in 1..=3 {
                    std::thread::sleep(Duration::from_millis(20));
                    emit(None, None, Some(value));
                }
                emit(Some(pid), None, None);
            }
            "crash" => exit_without_native_cleanup(42),
            "hang" => loop {
                std::thread::sleep(Duration::from_secs(60));
            },
            "progress-forever" => loop {
                std::thread::sleep(Duration::from_millis(10));
                emit(None, None, Some(pid));
            },
            "error" => emit(None, Some("synthetic native failure"), None),
            "invalid" => emit(Some(pid), Some("conflicting terminal fields"), None),
            "malformed" => {
                protocol::write_bytes(&mut io::stdout().lock(), b"not json", 100).unwrap();
            }
            "oversized" => {
                io::stdout()
                    .write_all(&(protocol::DEFAULT_RESPONSE_LIMIT + 1).to_le_bytes())
                    .unwrap();
                io::stdout().flush().unwrap();
            }
            "truncated-header" => {
                io::stdout().write_all(&[1, 2, 3]).unwrap();
                io::stdout().flush().unwrap();
                exit_without_native_cleanup(0);
            }
            "truncated-body" => {
                io::stdout().write_all(&99_u64.to_le_bytes()).unwrap();
                io::stdout().write_all(b"{").unwrap();
                io::stdout().flush().unwrap();
                exit_without_native_cleanup(0);
            }
            _ => emit(Some(pid), None, None),
        }
    }
}
