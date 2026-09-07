pub mod assets;
pub mod assistant;
pub mod blocks;
pub mod favorites;
pub mod flashcards;
pub mod graph;
pub mod jobs;
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
