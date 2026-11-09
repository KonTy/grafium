use crate::AppState;
use grafium_core::iq::{IqPackSummary, IqParticipant, IqResult, IqSession};
use grafium_core::models::{Flashcard, FlashcardTopic};
use tauri::State;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlashcardPackImport {
    anki: Option<grafium_core::import::anki::AnkiImportSummary>,
    iq: Option<IqPackSummary>,
}

#[tauri::command(rename_all = "camelCase")]
pub fn import_flashcard_pack(
    app: tauri::AppHandle,
    state: State<AppState>,
    path: String,
) -> Result<FlashcardPackImport, String> {
    if let Some(pack) = grafium_core::import::iq::read_iq_pack(std::path::Path::new(&path))
        .map_err(|e| e.to_string())?
    {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        let summary = graph.db.import_iq_pack(&pack).map_err(|e| e.to_string())?;
        Ok(FlashcardPackImport {
            anki: None,
            iq: Some(summary),
        })
    } else {
        Ok(FlashcardPackImport {
            anki: Some(import_anki_apkg(app, state, path)?),
            iq: None,
        })
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_iq_packs(state: State<AppState>) -> Result<Vec<IqPackSummary>, String> {
    state
        .graph
        .lock()
        .map_err(|e| e.to_string())?
        .db
        .list_iq_packs()
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn start_iq_session(
    state: State<AppState>,
    pack_id: String,
    participant: IqParticipant,
) -> Result<IqSession, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .start_iq_session(&pack_id, &participant, &graph.root_dir.to_string_lossy())
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn answer_iq_item(
    state: State<AppState>,
    graph_path: String,
    session_id: String,
    index: usize,
    answer: String,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    if graph.root_dir != std::path::Path::new(&graph_path) {
        return Err("The active graph changed; this assessment belongs to another graph.".into());
    }
    graph
        .db
        .answer_iq_item(&session_id, index, &answer)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn finish_iq_session(
    state: State<AppState>,
    graph_path: String,
    session_id: String,
) -> Result<IqResult, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    if graph.root_dir != std::path::Path::new(&graph_path) {
        return Err("The active graph changed; this assessment belongs to another graph.".into());
    }
    graph
        .db
        .finish_iq_session(&session_id)
        .map_err(|e| e.to_string())
}
#[tauri::command(rename_all = "camelCase")]
pub fn list_flashcards_due(
    state: State<AppState>,
    limit: Option<i64>,
    topic: Option<String>,
) -> Result<Vec<Flashcard>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .list_flashcards_due(topic.as_deref(), limit.unwrap_or(20))
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_flashcard_topics(state: State<AppState>) -> Result<Vec<FlashcardTopic>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.flashcard_topics().map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_all_flashcards(
    state: State<AppState>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Flashcard>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .list_flashcards(limit.unwrap_or(100), offset.unwrap_or(0))
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn update_flashcard_review(
    state: State<AppState>,
    id: String,
    ease_factor: f64,
    interval_days: i32,
    next_review_at: i64,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .update_flashcard_review(&id, ease_factor, interval_days, next_review_at)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn grade_flashcard(
    state: State<AppState>,
    id: String,
    quality: i32,
) -> Result<Flashcard, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .grade_flashcard(&id, quality)
        .map_err(|e| e.to_string())
}

/// Import an Anki `.apkg` deck into the active graph. The deck is converted into
/// a single markdown page of `Front :: Back` flashcards tagged with the deck's
/// topic, and any referenced media (audio/images) is copied into the graph's
/// assets directory so it renders in the flashcard reviewer.
///
/// Emits `anki-import-progress` events (phase/current/total) throughout so the
/// UI can show a progress bar while the (potentially long) import runs.
#[tauri::command(rename_all = "camelCase")]
pub fn import_anki_apkg(
    app: tauri::AppHandle,
    state: State<AppState>,
    path: String,
) -> Result<grafium_core::import::anki::AnkiImportSummary, String> {
    use tauri::Emitter;
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let mut on_progress = |p: grafium_core::import::anki::ImportProgress| {
        let _ = app.emit("anki-import-progress", p);
    };
    grafium_core::import::anki::import_apkg_with_progress(
        &graph,
        std::path::Path::new(&path),
        &mut on_progress,
    )
    .map_err(|e| e.to_string())
}
