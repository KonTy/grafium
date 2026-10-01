use grafium_core::db::studies::{StudyItem, StudyProgress, StudySnapshot};
use grafium_core::{db::Database, Graph};
use std::{path::Path, sync::Mutex};
use tauri::State;

fn with_study_graph<T>(
    graph: &Mutex<Graph>,
    graph_path: &str,
    run: impl FnOnce(&Database) -> grafium_core::error::Result<T>,
) -> Result<T, String> {
    let graph = graph.lock().map_err(|e| e.to_string())?;
    let mismatch = || "The active graph changed; studies were not accessed".to_string();
    if graph_path.trim().is_empty()
        || graph.root_dir.canonicalize().map_err(|_| mismatch())?
            != Path::new(graph_path)
                .canonicalize()
                .map_err(|_| mismatch())?
    {
        return Err(mismatch());
    }
    // Keep both the identity check and the complete DB transaction under the
    // same lock used by graph switching.
    run(&graph.db).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_studies(
    state: State<crate::AppState>,
    graph_path: String,
) -> Result<StudySnapshot, String> {
    with_study_graph(&state.graph, &graph_path, Database::list_studies)
}

#[tauri::command(rename_all = "camelCase")]
pub fn save_study(
    state: State<crate::AppState>,
    graph_path: String,
    item: StudyItem,
) -> Result<StudyItem, String> {
    with_study_graph(&state.graph, &graph_path, |db| db.save_study(item))
}

#[tauri::command(rename_all = "camelCase")]
pub fn remove_study(
    state: State<crate::AppState>,
    graph_path: String,
    id: String,
) -> Result<(), String> {
    with_study_graph(&state.graph, &graph_path, |db| db.remove_study(&id))
}

#[tauri::command(rename_all = "camelCase")]
pub fn record_study_activity(
    state: State<crate::AppState>,
    graph_path: String,
    id: String,
    seconds: f64,
    day: String,
    progress: Option<StudyProgress>,
    request_id: String,
) -> Result<(), String> {
    with_study_graph(&state.graph, &graph_path, |db| {
        db.record_study_activity(&id, seconds, &day, progress.as_ref(), &request_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use grafium_core::db::studies::StudyKind;

    #[test]
    fn studies_graph_switch_rejects_stale_reads_and_writes() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("study-command-fixture-{}", uuid::Uuid::new_v4()));
        let a = root.join("a");
        let b = root.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        {
            let state = Mutex::new(Graph::open(&a).unwrap());
            let a_path = a.to_str().unwrap();
            let b_path = b.to_str().unwrap();
            let item = StudyItem {
                id: "shared-id".into(),
                title: "First graph".into(),
                topic: "".into(),
                kind: StudyKind::Page,
                source: "source-page".into(),
                progress: StudyProgress::default(),
                created_at: "".into(),
                updated_at: "".into(),
            };
            with_study_graph(&state, a_path, |db| db.save_study(item.clone())).unwrap();
            *state.lock().unwrap() = Graph::open(&b).unwrap();
            for stale_path in [a_path, "", "/nonexistent-study-graph"] {
                assert!(with_study_graph(&state, stale_path, Database::list_studies).is_err());
                assert!(
                    with_study_graph(&state, stale_path, |db| db.save_study(item.clone())).is_err()
                );
                assert!(
                    with_study_graph(&state, stale_path, |db| db.remove_study(&item.id)).is_err()
                );
                assert!(
                    with_study_graph(&state, stale_path, |db| db.record_study_activity(
                        &item.id,
                        10.0,
                        "2026-09-30",
                        None,
                        &uuid::Uuid::new_v4().to_string()
                    ))
                    .is_err()
                );
            }
            assert!(with_study_graph(&state, b_path, Database::list_studies)
                .unwrap()
                .items
                .is_empty());
            let mut other = item.clone();
            other.title = "Second graph".into();
            with_study_graph(&state, b_path, |db| db.save_study(other)).unwrap();
            *state.lock().unwrap() = Graph::open(&a).unwrap();
            assert_eq!(
                with_study_graph(&state, a_path, Database::list_studies)
                    .unwrap()
                    .items[0]
                    .title,
                "First graph"
            );
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
