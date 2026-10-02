//! Grafium bootstrap/error compatibility over the shared native runtime.

use crate::error::Result;
use model_runtime::native::worker as runtime;
use std::path::Path;
use std::time::Duration;

pub use runtime::{
    emit_runtime_warning, exit_without_native_cleanup, recovery_status, resident_status,
    run_from_stdio, runtime_warnings, shutdown_pool, status, NativeHostConfig, NativeModelKind,
    NativeStatus, ResidentModel, ResidentStatus, WorkerOutput, WorkerProgress, WorkerRequest,
};

pub const WORKER_ARGUMENT: &str = "--grafium-native-ai-worker";

pub fn execute(request: WorkerRequest, timeout: Duration) -> Result<WorkerOutput> {
    Ok(runtime::execute(request, timeout)?)
}

pub fn execute_with_progress(
    request: WorkerRequest,
    timeout: Duration,
    on_progress: &mut dyn FnMut(WorkerProgress),
) -> Result<WorkerOutput> {
    Ok(runtime::execute_with_progress(
        request,
        timeout,
        on_progress,
    )?)
}

pub fn evict_idle() -> Result<()> {
    Ok(runtime::evict_idle()?)
}
pub fn allow_gpu_retry(key: &str) -> Result<()> {
    Ok(runtime::allow_gpu_retry(key)?)
}
pub fn use_cpu(key: &str) -> Result<()> {
    Ok(runtime::use_cpu(key)?)
}
pub fn gpu_risk_key(workload: &str, path: &Path) -> Result<String> {
    Ok(runtime::gpu_risk_key(workload, path)?)
}

pub fn is_worker_invocation() -> bool {
    std::env::args_os()
        .nth(1)
        .is_some_and(|argument| argument == WORKER_ARGUMENT)
        || runtime::is_worker_invocation()
}

pub fn configure_current_executable() -> Result<()> {
    configure(None)
}
pub fn configure_current_executable_with_state(state_dir: &Path) -> Result<()> {
    configure(Some(state_dir))
}

fn configure(state_dir: Option<&Path>) -> Result<()> {
    let mut options = NativeHostConfig::new(std::env::current_exe()?, WORKER_ARGUMENT);
    options.recovery_state_dir = state_dir.map(Path::to_path_buf);
    options.force_cpu = std::env::var_os("GRAFIUM_DISABLE_GPU_OFFLOAD").is_some()
        || std::env::var_os("GRAFIUM_NATIVE_FORCE_CPU").is_some();
    options.inference_threads = std::env::var("GRAFIUM_LLM_THREADS").ok().and_then(|value| {
        let threads = value.trim().parse().ok();
        if threads.is_none() {
            tracing::warn!("Invalid GRAFIUM_LLM_THREADS; using bounded automatic threads");
        }
        threads
    });
    Ok(runtime::configure_with_options(options)?)
}
