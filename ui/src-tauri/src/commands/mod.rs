pub mod assets;
pub mod assistant;
pub mod assistant_workflows;
pub mod blocks;
pub mod books;
pub mod chat;
pub mod favorites;
pub mod flashcards;
pub mod graph;
pub mod help;
pub mod jobs;
pub mod knowledge;
pub mod layout;
pub mod library_index;
pub mod links;
pub mod media;
pub mod model_library;
pub mod pages;
pub mod private_reader;
pub mod private_voice;
pub mod print;
pub mod query;
pub mod reading_notes;
pub mod research;
pub mod startup;
pub mod studies;
pub mod study_link;
pub mod study_player;
pub mod sync;
pub mod tasks;
pub mod theme;
#[cfg(all(test, target_os = "linux"))]
mod theme_native_tests;
pub mod trees;
pub mod writing;
pub mod writing_edits;

/// Directory holding Grafium's own preference files (layout, theme, chat).
///
/// Android has no XDG config directory, so `dirs::config_dir()` returns `None`
/// there: every preference read failed with a user-visible error and every
/// write silently went to a `/tmp` path the app cannot keep. The desktop
/// location is deliberately unchanged so preferences saved by earlier builds
/// keep loading.
pub fn app_config_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager as _;
        app.path()
            .app_data_dir()
            .map(|dir| dir.join("config"))
            .map_err(|err| format!("Could not locate the app configuration directory: {err}"))
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        dirs::config_dir()
            .map(|dir| dir.join("grafium"))
            .ok_or_else(|| "Could not locate the app configuration directory.".to_string())
    }
}

/// Bridges frontend diagnostics into the process log, so a WebKitGTK build's
/// `console.log` (which never reaches stdout) can still be captured when
/// debugging UI behaviour from a terminal or log file.
#[tauri::command]
pub fn ui_log(message: String) {
    // Truncate so a runaway frontend loop can't flood the log. The cut has to
    // land on a character boundary: `&message[..2000]` panics outright when
    // byte 2000 falls inside a multi-byte character, which any note containing
    // CJK, emoji or accents can produce.
    let end = grafium_core::ai::text::char_boundary_prefix_end(&message, 2000);
    let message = &message[..end];
    // Deliberately only `tracing`: this used to also `eprintln!` the same line,
    // printing every frontend message twice.
    tracing::info!(target: "grafium::ui", "{message}");
}
