fn main() {
    // Answers "which build is installed?" without launching the GUI, and is
    // what `scripts/deploy-local.sh` calls to verify a binary before
    // installing it. Checked first because it must work even if the AI
    // worker setup below would fail.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--version" || arg == "-V")
    {
        println!("{}", grafium_lib::build_info::full());
        return;
    }

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
