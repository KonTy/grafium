//! Read-only, cancellable writing assistance, separate from Chat and graph mutation.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use grafium_core::ai::config::{AiMode, ProviderType};
use grafium_core::ai::writing::{
    self, WritingAnalysis, WritingCancellation, WritingInputBlock, WritingRewriteDiagnostic,
    WritingRewriteResult,
};
use grafium_core::knowledge::KnowledgeEngine;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use super::knowledge::KnowledgeState;

static OPERATIONS: LazyLock<Mutex<HashMap<String, WritingCancellation>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct Operation {
    id: String,
    cancel: WritingCancellation,
}

impl Operation {
    fn register(id: String) -> Result<Self, String> {
        if id.trim().is_empty() || id.len() > 256 {
            return Err("Writing operation ID must contain 1 to 256 bytes.".into());
        }
        let mut operations = OPERATIONS.lock().unwrap_or_else(|p| p.into_inner());
        if operations.contains_key(&id) {
            return Err("This writing operation is already running.".into());
        }
        if !operations.is_empty() {
            return Err(
                "Another writing operation is running. Cancel it or wait for it to finish.".into(),
            );
        }
        let cancel = WritingCancellation::default();
        operations.insert(id.clone(), cancel.clone());
        Ok(Self { id, cancel })
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        self.cancel.cancel();
        OPERATIONS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.id);
    }
}

fn configured_context(engine: &KnowledgeEngine) -> Option<usize> {
    let config = engine.config();
    // Native providers do not yet report context_window(). Their configured
    // runtime context, not a model's advertised training limit, is the budget.
    config
        .local
        .as_ref()
        .filter(|local| config.mode == AiMode::Local && local.provider == ProviderType::HuggingFace)
        .map(|local| local.local_llm.context_size.unwrap_or(4096) as usize)
}

fn writing_source_inputs(
    db: &grafium_core::db::Database,
    blocks: &[WritingInputBlock],
) -> Result<Vec<WritingInputBlock>, String> {
    let ids = blocks.iter().map(|block| block.id.clone()).collect::<Vec<_>>();
    let metadata = db.get_blocks_with_page_meta(&ids).map_err(|e| e.to_string())?;
    let mut source_ids = std::collections::HashSet::new();
    let pages: std::collections::HashSet<_> = metadata.iter().map(|block| &block.page_id).collect();
    for page in pages {
        let page_blocks = db.list_blocks_for_page(page).map_err(|e| e.to_string())?;
        source_ids.extend(grafium_core::knowledge::source_projection::without_reading_notes(&page_blocks)
            .into_iter().map(|block| block.id));
    }
    let mut source = Vec::new();
    for block in blocks {
        if !metadata.iter().any(|saved| saved.block_id == block.id && saved.content == block.content) {
            return Err("Writing source changed or is no longer available. Refresh before retrying.".into());
        }
        if source_ids.contains(&block.id) {
            // Mutation inputs retain the exact original markers and content.
            source.push(block.clone());
        }
    }
    Ok(source)
}

fn restore_unwritten_blocks(original: &[WritingInputBlock], mut result: WritingRewriteResult) -> WritingRewriteResult {
    let mut rewritten: HashMap<_, _> = result.blocks.into_iter().map(|block| (block.id.clone(), block)).collect();
    result.blocks = original.iter().enumerate().map(|(index, block)| {
        rewritten.remove(&block.id).unwrap_or_else(|| {
            result.skipped.push(writing::WritingRewriteIssue {
                block_id: block.id.clone(), block_ordinal: index + 1, line_ordinal: None,
                reason: "Reading annotation retained unchanged; it is not author/source text.".into(),
            });
            block.clone()
        })
    }).collect();
    result
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WritingProgress<'a> {
    operation_id: &'a str,
    message: String,
}

fn rewrite_progress<'a>(
    operation_id: &'a str,
    block_count: usize,
    event: &WritingRewriteDiagnostic,
) -> Option<WritingProgress<'a>> {
    if event.stage != "request" {
        return None;
    }
    Some(WritingProgress {
        operation_id,
        message: format!(
            "{} text block {} of {}, line {}...",
            if event.attempt > 1 {
                "Retrying"
            } else {
                "Rewriting"
            },
            event.block_ordinal,
            block_count,
            event.line_ordinal,
        ),
    })
}

#[tauri::command(rename_all = "camelCase")]
pub async fn ai_analyze_writing(
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    blocks: Vec<WritingInputBlock>,
    operation_id: String,
) -> Result<WritingAnalysis, String> {
    let operation = Operation::register(operation_id)?;
    let blocks = {
        let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
        writing_source_inputs(&graph.db, &blocks)?
    };
    let guard = tokio::select! {
        biased;
        _ = operation.cancel.cancelled() => return Err("Operation cancelled".into()),
        guard = state.engine.read() => guard,
    };
    let engine = guard.as_ref().ok_or("AI is not initialized.")?;
    if !engine.is_llm_ready() {
        return Err("AI is disabled or the configured writing model is not ready.".into());
    }
    let llm = engine
        .llm_provider()
        .ok_or("No writing model is configured.")?;
    writing::analyze_writing(llm, &blocks, configured_context(engine), &operation.cancel)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn ai_rewrite_writing(
    app: AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    blocks: Vec<WritingInputBlock>,
    operation_id: String,
) -> Result<WritingRewriteResult, String> {
    let operation = Operation::register(operation_id)?;
    let source_blocks = {
        let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
        writing_source_inputs(&graph.db, &blocks)?
    };
    let guard = tokio::select! {
        biased;
        _ = operation.cancel.cancelled() => return Err("Operation cancelled".into()),
        guard = state.engine.read() => guard,
    };
    let engine = guard.as_ref().ok_or("AI is not initialized.")?;
    if !engine.is_llm_ready() {
        return Err("AI is disabled or the configured writing model is not ready.".into());
    }
    let llm = engine
        .llm_provider()
        .ok_or("No writing model is configured.")?;
    let rewritten = writing::rewrite_writing_with_diagnostics(
        llm,
        &source_blocks,
        configured_context(engine),
        &operation.cancel,
        &mut |event| {
            if let Some(progress) = rewrite_progress(&operation.id, source_blocks.len(), &event) {
                if let Err(error) = app.emit("ai-writing-progress", progress) {
                    eprintln!("Could not emit writing progress: {error}");
                }
            }
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(restore_unwritten_blocks(&blocks, rewritten))
}

#[tauri::command(rename_all = "camelCase")]
pub fn ai_cancel_writing(operation_id: String) -> Result<(), String> {
    if let Some(cancel) = OPERATIONS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&operation_id)
    {
        cancel.cancel();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_annotation_inputs_are_preserved_unwritten_in_original_order() {
        let directory = tempfile::tempdir_in(".").unwrap();
        let graph = grafium_core::Graph::open(directory.path()).unwrap();
        let page = graph.create_page_with_content("Book", false, "- We utilize tools.[^1]\n").unwrap();
        graph.reading_note_create("d39f3c66-55aa-407a-aecb-04ac4c50d05f", &page.id, None, "USER-ANNOTATION: contradictory claim.").unwrap();
        let original: Vec<_> = graph.db.list_blocks_for_page(&page.id).unwrap().into_iter()
            .map(|block| WritingInputBlock { id: block.id, content: block.content }).collect();
        let source = writing_source_inputs(&graph.db, &original).unwrap();
        assert_eq!(source.len() + 1, original.len());
        assert!(source.iter().all(|b| !b.content.contains("USER-ANNOTATION")));
        assert!(source.iter().any(|b| b.content.contains("[^grafium-note-")));
        let restored = restore_unwritten_blocks(&original, WritingRewriteResult {
            blocks: source.into_iter().map(|mut b| { b.content = b.content.replace("utilize", "use"); b }).collect(),
            skipped: Vec::new(),
        });
        assert_eq!(restored.blocks.len(), original.len());
        for (before, after) in original.iter().zip(&restored.blocks) {
            assert_eq!(before.id, after.id);
            assert_eq!(after.content, before.content.replace("utilize", "use"));
        }
        assert_eq!(restored.skipped.len(), 1);
        let mut stale = original.clone();
        stale[0].content.push_str(" unsaved");
        assert!(writing_source_inputs(&graph.db, &stale).is_err());
    }

    #[test]
    fn writing_progress_reports_only_request_ordinals_for_the_current_operation() {
        let mut event = WritingRewriteDiagnostic {
            block_ordinal: 3,
            line_ordinal: 2,
            attempt: 1,
            stage: "request",
            preservation: None,
        };
        let progress = rewrite_progress("operation", 8, &event).unwrap();
        assert_eq!(
            serde_json::to_value(progress).unwrap(),
            serde_json::json!({
                "operationId": "operation",
                "message": "Rewriting text block 3 of 8, line 2...",
            })
        );
        event.attempt = 2;
        assert_eq!(
            rewrite_progress("operation", 8, &event).unwrap().message,
            "Retrying text block 3 of 8, line 2..."
        );
        event.stage = "preservation";
        assert!(rewrite_progress("operation", 8, &event).is_none());
    }

    #[test]
    fn writing_operation_registration_releases_on_success_error_cancel_and_panic() {
        assert!(Operation::register(String::new()).is_err());
        assert!(Operation::register("x".repeat(257)).is_err());
        let operation = Operation::register("writing-test".into()).unwrap();
        let cancel = operation.cancel.clone();
        assert!(Operation::register("writing-test".into()).is_err());
        assert!(Operation::register("another-operation".into()).is_err());
        ai_cancel_writing("unknown-id".into()).unwrap();
        assert!(cancel.check().is_ok());
        ai_cancel_writing("writing-test".into()).unwrap();
        assert!(cancel.check().is_err());
        drop(operation);
        assert!(OPERATIONS.lock().unwrap().is_empty());

        let failed = (|| {
            let _operation = Operation::register("writing-test".into())?;
            Err::<(), String>("Simulated failure".into())
        })();
        assert!(failed.is_err());
        assert!(OPERATIONS.lock().unwrap().is_empty());

        let panicked = std::panic::catch_unwind(|| {
            let _operation = Operation::register("writing-test".into()).unwrap();
            panic!("Simulated unwinding");
        });
        assert!(panicked.is_err());
        assert!(OPERATIONS.lock().unwrap().is_empty());

        let operation = Operation::register("writing-test".into()).unwrap();
        let cancel = operation.cancel.clone();
        drop(operation);
        assert!(cancel.check().is_err());
        assert!(OPERATIONS.lock().unwrap().is_empty());
    }
}
