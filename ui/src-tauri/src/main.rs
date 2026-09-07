fn main() {
    #[cfg(not(target_os = "android"))]
    if grafium_core::ai::worker::is_worker_invocation() {
        std::process::exit(grafium_core::ai::worker::run_from_stdio());
    }
    #[cfg(not(target_os = "android"))]
    if let Err(error) = grafium_core::ai::worker::configure_current_executable() {
        eprintln!("Failed to configure native AI isolation: {error}");
        std::process::exit(1);
    }
    grafium_lib::run();
}
