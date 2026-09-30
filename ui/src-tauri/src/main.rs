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
        grafium_core::ai::worker::run_from_stdio();
    }
    configure_gui_identity();
    grafium_lib::run();
}

fn configure_gui_identity() {
    #[cfg(target_os = "linux")]
    {
        // Wayland identifies standalone GTK windows by the program name.
        // Keep it stable when installers rename the executable to grafium-bin.
        gtk::glib::set_prgname(Some(env!("CARGO_PKG_NAME")));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn desktop_identity_is_set_without_initializing_gtk() {
        assert!(!gtk::is_initialized());
        super::configure_gui_identity();
        assert_eq!(gtk::glib::prgname().as_deref(), Some("grafium"));
        assert!(!gtk::is_initialized());
    }
}
