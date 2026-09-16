use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn assistant_rejects_graph_mismatch_even_for_none_before_reading_sources() {
    let db = grafium_core::db::Database::in_memory().unwrap();
    let root = std::path::Path::new("/synthetic/active");
    for context in [
        AssistantContext::None {},
        AssistantContext::Graph {},
        AssistantContext::Page {
            page_id: "missing".into(),
        },
    ] {
        let error = capture_context(&db, root, "/synthetic/other", &context)
            .err()
            .unwrap();
        assert!(error.contains("graph changed"));
    }
    assert!(matches!(
        capture_context(&db, root, "/synthetic/active", &AssistantContext::None {}).unwrap(),
        AssistantSource::None
    ));
}

#[tokio::test]
async fn assistant_cancel_during_engine_wait_never_acquires_engine_and_releases_registration() {
    let state = KnowledgeState {
        engine: Arc::new(tokio::sync::RwLock::new(None)),
        cancels: Arc::new(Mutex::new(std::collections::HashMap::new())),
    };
    let busy = state.engine.write().await;
    let operation =
        ReadingOperation::register(&state, "assistant-cancel-before-model".into()).unwrap();
    state.cancels.lock().unwrap()["assistant-cancel-before-model"].store(true, Ordering::Release);
    tokio::time::timeout(std::time::Duration::from_millis(100), async {
        tokio::select! {
            biased;
            _ = operation.cancelled() => {},
            _ = state.engine.read() => panic!("stopped Chat acquired the engine"),
        }
    })
    .await
    .unwrap();
    drop(operation);
    assert!(state.cancels.lock().unwrap().is_empty());
    drop(busy);
}
