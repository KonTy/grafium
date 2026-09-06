pub mod assets;
pub mod assistant;
pub mod blocks;
pub mod favorites;
pub mod flashcards;
pub mod graph;
pub mod knowledge;
pub mod links;
pub mod media;
pub mod model_library;
pub mod pages;
pub mod query;
pub mod research;
pub mod sync;
pub mod tasks;
pub mod theme;
pub mod trees;

/// Bridges frontend diagnostics into the process log, so a WebKitGTK build's
/// `console.log` (which never reaches stdout) can still be captured when
/// debugging UI behaviour from a terminal or log file.
#[tauri::command]
pub fn ui_log(message: String) {
    // Truncate so a runaway frontend loop can't flood the log.
    let message = if message.len() > 2000 {
        &message[..2000]
    } else {
        &message
    };
    tracing::info!(target: "grafium::ui", "{message}");
    eprintln!("[UI] {message}");
}
