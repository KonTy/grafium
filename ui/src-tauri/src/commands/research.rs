//! Tauri commands for Deep Research: the user-editable engine/prompt
//! configuration, an engine smoke test, and the multi-round research run
//! itself.
//!
//! The configuration lives in the current graph's `knowledge/config/` folder
//! rather than in `AiConfig`, because it is edited on a different cadence and
//! by a different person: a student tuning prompts and search engines for one
//! subject shouldn't risk their provider/model setup, and a malformed research
//! config must never be able to stop the AI engine from starting.
//!
//! Research runs stream over the *same* `ai://chat_stream` / `ai://chat_sources`
//! channels as ordinary Chat answers, and produce the same two-part
//! notes/web shape, so the Chat pane renders a deep-research answer with the
//! code it already has — the difference is how the answer was obtained, not
//! how it looks.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

use grafium_core::knowledge::conversation::{self, ChatTurn};
use grafium_core::knowledge::engine::reading_scope::{
    self, ReadingSource, ResearchScopeInfo, ResearchTarget,
};
use grafium_core::knowledge::engine::AskStreamEvent;
use grafium_core::knowledge::engine::ResearchWebMode;
use grafium_core::research::{ResearchConfig, ResearchPrompts, SearchEngineDef};
use grafium_core::scraping::browser::HttpBrowserDriver;

use super::knowledge::{
    AskSourcesPayload, AskStreamChunk, ChatScope, KnowledgeState, SourceDto, WebSourceDto,
};

/// One search hit, as returned by the Settings "Test" button.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultPayload {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Resolves the graph-portable directory holding `research_config.json`.
fn graph_knowledge_config_dir(
    app: &tauri::AppHandle,
    app_state: &crate::AppState,
) -> Result<PathBuf, String> {
    let snapshot = crate::current_graph_snapshot(app, app_state.graph.as_ref())?;
    let dir = snapshot.root_dir.join("knowledge").join("config");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Legacy machine-local location used before research prompts became graph data.
fn legacy_app_knowledge_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("knowledge");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn research_config_dir(
    app: &tauri::AppHandle,
    app_state: &crate::AppState,
) -> Result<PathBuf, String> {
    let graph_dir = graph_knowledge_config_dir(app, app_state)?;
    let graph_path = ResearchConfig::config_path(&graph_dir);
    if graph_path.exists() {
        return Ok(graph_dir);
    }

    let legacy_dir = legacy_app_knowledge_dir(app)?;
    let legacy_path = ResearchConfig::config_path(&legacy_dir);
    if legacy_path.exists() {
        std::fs::copy(&legacy_path, &graph_path).map_err(|e| e.to_string())?;
    }
    Ok(graph_dir)
}

#[tauri::command]
pub async fn research_get_config(
    app: tauri::AppHandle,
    app_state: State<'_, crate::AppState>,
) -> Result<ResearchConfig, String> {
    let dir = research_config_dir(&app, &app_state)?;
    ResearchConfig::load_or_create(&dir).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn research_set_config(
    app: tauri::AppHandle,
    app_state: State<'_, crate::AppState>,
    payload: ResearchConfig,
) -> Result<(), String> {
    let dir = research_config_dir(&app, &app_state)?;
    payload.save(&dir).map_err(|e| e.to_string())
}

/// Restores the default prompt for every step, leaving the engine registry and
/// the numeric knobs untouched — a student who has broken one prompt shouldn't
/// lose the engines they added to get it back.
#[tauri::command]
pub async fn research_reset_prompts(
    app: tauri::AppHandle,
    app_state: State<'_, crate::AppState>,
) -> Result<ResearchConfig, String> {
    let dir = research_config_dir(&app, &app_state)?;
    let mut config = ResearchConfig::load_or_create(&dir).map_err(|e| e.to_string())?;
    config.prompts = ResearchPrompts::default();
    config.save(&dir).map_err(|e| e.to_string())?;
    Ok(config)
}

/// Runs a single query against one engine definition and returns what it
/// parsed.
///
/// This exists because the two ways an engine can be broken look identical
/// from the Chat pane — a wrong CSS selector and an engine that is blocking us
/// both yield "no results". Running the engine in isolation and showing either
/// the parsed hits or the transport error is what lets someone tell those
/// apart while editing a definition.
#[tauri::command]
pub async fn research_test_engine(
    engine: SearchEngineDef,
    query: String,
) -> Result<Vec<SearchResultPayload>, String> {
    let browser = HttpBrowserDriver::new();
    let results = grafium_core::scraping::engines::search_one(&browser, &engine, &query, 5)
        .await
        .map_err(|e| e.to_string())?;
    Ok(results
        .into_iter()
        .map(|r| SearchResultPayload {
            title: r.title,
            url: r.url,
            snippet: r.snippet,
        })
        .collect())
}

/// Runs the full multi-round research workflow for `question`.
///
/// Deliberately separate from `ai_ask_stream` rather than a flag on it: this is
/// an explicit, expensive action the user opted into with the Research
/// checkbox, so it bypasses the intent classifier entirely instead of asking a
/// model to second-guess a decision the user already made.
#[tauri::command]
pub async fn research_deep(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    question: String,
    request_id: String,
    graph_id: Option<String>,
    history: Option<Vec<ChatTurn>>,
    scope: Option<ChatScope>,
) -> Result<(), String> {
    scope.unwrap_or_default().require_internet()?;
    let history = history.unwrap_or_default();
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(
            "Research needs an AI model — configure and save a Local or Cloud provider in \
             Settings \u{2192} AI / Knowledge Engine first."
                .to_string(),
        );
    }

    let config_dir = research_config_dir(&app, &app_state)?;
    let config = ResearchConfig::load_or_create(&config_dir).map_err(|e| e.to_string())?;

    let snapshot = crate::current_graph_snapshot(&app, app_state.graph.as_ref())?;
    let resolved_graph_id =
        graph_id.unwrap_or_else(|| snapshot.root_dir.to_string_lossy().to_string());
    let graph = crate::open_graph_snapshot(&snapshot)?;

    // Registered under the same key space as ordinary answers so the existing
    // Stop button cancels a research run without the UI needing to know which
    // kind of request is in flight.
    let cancel = Arc::new(AtomicBool::new(false));
    if let Ok(mut map) = state.cancels.lock() {
        map.insert(request_id.clone(), cancel.clone());
    }

    let app_for_events = app.clone();
    let rid = request_id.clone();
    let mut on_event = move |ev: AskStreamEvent<'_>| {
        let (delta, phase, note) = match ev {
            AskStreamEvent::Delta(d) => (d.to_string(), None, None),
            AskStreamEvent::Phase(p) => (String::new(), Some(p.as_str().to_string()), None),
            AskStreamEvent::Note(n) => (String::new(), None, Some(n.to_string())),
        };
        let _ = app_for_events.emit(
            "ai://chat_stream",
            AskStreamChunk {
                request_id: rid.clone(),
                delta,
                phase,
                note,
                done: false,
                error: None,
            },
        );
    };

    let effective_question = conversation::resolve_research_followup(&question, &history);

    let outcome = engine
        .ask_stream_with_deep_research(
            &graph.db,
            &effective_question,
            Some(resolved_graph_id.as_str()),
            &config,
            &history,
            Some(cancel),
            &mut on_event,
        )
        .await;

    if let Ok(mut map) = state.cancels.lock() {
        map.remove(&request_id);
    }

    let outcome = outcome.map_err(|e| e.to_string())?;

    app.emit(
        "ai://chat_sources",
        AskSourcesPayload {
            request_id: request_id.clone(),
            sources: outcome.sources.into_iter().map(SourceDto::from).collect(),
            web_sources: outcome
                .web_citations
                .into_iter()
                .map(WebSourceDto::from)
                .collect(),
        },
    )
    .map_err(|e| e.to_string())?;

    if let Some(message) = outcome.trailing_message {
        app.emit(
            "ai://chat_stream",
            AskStreamChunk {
                request_id: request_id.clone(),
                delta: message,
                phase: None,
                note: None,
                done: false,
                error: None,
            },
        )
        .map_err(|e| e.to_string())?;
    }

    app.emit(
        "ai://chat_stream",
        AskStreamChunk {
            request_id,
            delta: String::new(),
            phase: None,
            note: None,
            done: true,
            error: None,
        },
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

const MAX_EARLY_RESEARCH_CANCELS: usize = 256;
static EARLY_RESEARCH_CANCELS: std::sync::Mutex<std::collections::VecDeque<String>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

fn remember_early_cancel(recent: &mut std::collections::VecDeque<String>, id: &str) {
    if let Some(index) = recent.iter().position(|value| value == id) {
        recent.remove(index);
    }
    if recent.len() == MAX_EARLY_RESEARCH_CANCELS {
        recent.pop_front();
    }
    recent.push_back(id.to_string());
}

fn cancel_research_request(state: &KnowledgeState, request_id: &str) -> Result<(), String> {
    if request_id.trim().is_empty() || request_id.len() > 256 {
        return Err("A research request ID of at most 256 bytes is required".into());
    }
    // Serialize remembering and consuming with registration. Whichever IPC is
    // dispatched first, Stop either finds the live flag or leaves a tombstone.
    let registry = state.cancels.lock().map_err(|e| e.to_string())?;
    if let Some(flag) = registry.get(request_id) {
        flag.store(true, std::sync::atomic::Ordering::Release);
    } else {
        let mut recent = EARLY_RESEARCH_CANCELS.lock().map_err(|e| e.to_string())?;
        remember_early_cancel(&mut recent, request_id);
    }
    Ok(())
}

/// Cancellation shares Chat's live registry and remembers a bounded number of
/// one-shot IDs when Stop wins the race against research IPC dispatch.
#[tauri::command]
pub async fn research_cancel(
    state: State<'_, KnowledgeState>,
    request_id: String,
) -> Result<(), String> {
    cancel_research_request(&state, &request_id)
}

pub(super) fn require_research_graph(root: &std::path::Path, graph_path: &str) -> Result<(), String> {
    if graph_path.is_empty() || root != std::path::Path::new(graph_path) {
        return Err("The graph changed. Reopen Research from the current page.".into());
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn research_scope_info(
    app_state: State<'_, crate::AppState>,
    graph_path: String,
    page_id: String,
    block_id: Option<String>,
) -> Result<ResearchScopeInfo, String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
    require_research_graph(&graph.root_dir, &graph_path)?;
    reading_scope::research_scope_info(&graph.db, &page_id, block_id.as_deref())
        .map_err(|e| e.to_string())
}

pub(super) struct ReadingOperation {
    id: String,
    pub(super) flag: Arc<AtomicBool>,
    registry: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
}

impl ReadingOperation {
    pub(super) fn register(state: &KnowledgeState, id: String) -> Result<Self, String> {
        if id.trim().is_empty() || id.len() > 256 {
            return Err("A research request ID of at most 256 bytes is required".into());
        }
        let mut registry = state.cancels.lock().map_err(|e| e.to_string())?;
        {
            let mut recent = EARLY_RESEARCH_CANCELS.lock().map_err(|e| e.to_string())?;
            if let Some(index) = recent.iter().position(|value| value == &id) {
                recent.remove(index);
                return Err("Research cancelled before starting".into());
            }
        }
        if registry.contains_key(&id) {
            return Err("This research request is already running".into());
        }
        let flag = Arc::new(AtomicBool::new(false));
        registry.insert(id.clone(), flag.clone());
        Ok(Self {
            id,
            flag,
            registry: state.cancels.clone(),
        })
    }

    pub(super) async fn cancelled(&self) {
        while !self.flag.load(std::sync::atomic::Ordering::Acquire) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}

impl Drop for ReadingOperation {
    fn drop(&mut self) {
        self.flag.store(true, std::sync::atomic::Ordering::Release);
        if let Ok(mut registry) = self.registry.lock() {
            if registry
                .get(&self.id)
                .is_some_and(|flag| Arc::ptr_eq(flag, &self.flag))
            {
                registry.remove(&self.id);
            }
        }
    }
}

pub(super) fn reading_config(root: &std::path::Path) -> Result<ResearchConfig, String> {
    use grafium_core::research::config::{
        MAX_RESULTS_PER_QUERY_CEILING, MAX_ROUNDS_CEILING, MAX_SOURCES_CEILING,
    };
    let path = ResearchConfig::config_path(&root.join("knowledge").join("config"));
    let mut config: ResearchConfig = match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|e| e.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ResearchConfig::default(),
        Err(error) => return Err(error.to_string()),
    };
    config.max_rounds = config.max_rounds.clamp(1, MAX_ROUNDS_CEILING);
    config.max_sources = config.max_sources.clamp(1, MAX_SOURCES_CEILING);
    config.results_per_query = config
        .results_per_query
        .clamp(1, MAX_RESULTS_PER_QUERY_CEILING);
    Ok(config)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn research_scoped(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    question: String,
    request_id: String,
    graph_path: String,
    target: ResearchTarget,
    history: Vec<ChatTurn>,
    web_mode: ResearchWebMode,
) -> Result<(), String> {
    let operation = ReadingOperation::register(&state, request_id.clone())?;
    // Capture page text and root together before the first await. Unlike
    // reopening a Graph, these reads never reindex or touch source files.
    let (root, source) = {
        let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
        require_research_graph(&graph.root_dir, &graph_path)?;
        let source = ReadingSource::capture(&graph.db, &target).map_err(|e| e.to_string())?;
        (graph.root_dir.clone(), source)
    };
    let config = if web_mode == ResearchWebMode::Off {
        ResearchConfig::default()
    } else {
        reading_config(&root)?
    };
    let app_for_events = app.clone();
    let rid = request_id.clone();
    let flag = operation.flag.clone();
    let mut on_event = move |event: AskStreamEvent<'_>| {
        if flag.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let (delta, phase, note) = match event {
            AskStreamEvent::Delta(value) => (value.to_string(), None, None),
            AskStreamEvent::Phase(value) => (String::new(), Some(value.as_str().to_string()), None),
            AskStreamEvent::Note(value) => (String::new(), None, Some(value.to_string())),
        };
        let _ = app_for_events.emit(
            "ai://chat_stream",
            AskStreamChunk {
                request_id: rid.clone(),
                delta,
                phase,
                note,
                done: false,
                error: None,
            },
        );
    };
    let run = async {
        let guard = state.engine.read().await;
        let engine = guard.as_ref().ok_or_else(|| {
            grafium_core::CoreError::Other("Research needs a configured AI model".into())
        })?;
        let browser = HttpBrowserDriver::new();
        engine
            .research_scoped_using(
                &source,
                &question,
                &history,
                Some(&graph_path),
                web_mode,
                &config,
                &browser,
                Some(operation.flag.clone()),
                &mut on_event,
            )
            .await
    };
    let outcome = tokio::select! {
        biased;
        _ = operation.cancelled() => Err(grafium_core::CoreError::Other("Research cancelled".into())),
        outcome = run => outcome,
    };
    let cancelled = operation.flag.load(std::sync::atomic::Ordering::Acquire);
    let error = outcome
        .as_ref()
        .err()
        .filter(|_| !cancelled)
        .map(ToString::to_string);
    if let Ok(outcome) = outcome {
        if !cancelled {
            app.emit(
                "ai://chat_sources",
                AskSourcesPayload {
                    request_id: request_id.clone(),
                    sources: outcome.sources.into_iter().map(SourceDto::from).collect(),
                    web_sources: outcome
                        .web_citations
                        .into_iter()
                        .map(WebSourceDto::from)
                        .collect(),
                },
            )
            .map_err(|e| e.to_string())?;
        }
    }
    app.emit(
        "ai://chat_stream",
        AskStreamChunk {
            request_id,
            delta: String::new(),
            phase: None,
            note: None,
            done: true,
            error: error.clone(),
        },
    )
    .map_err(|e| e.to_string())?;
    match error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod reading_tests {
    use super::*;

    fn state() -> KnowledgeState {
        KnowledgeState {
            engine: Arc::new(tokio::sync::RwLock::new(None)),
            cancels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    #[test]
    fn reading_operation_registry_releases_on_error_success_and_cancel() {
        let state = state();
        assert!(ReadingOperation::register(&state, "".into()).is_err());
        let operation = ReadingOperation::register(&state, "request".into()).unwrap();
        let flag = operation.flag.clone();
        assert!(ReadingOperation::register(&state, "request".into()).is_err());
        assert!(!flag.load(std::sync::atomic::Ordering::Acquire));
        drop(operation);
        assert!(flag.load(std::sync::atomic::Ordering::Acquire));
        assert!(state.cancels.lock().unwrap().is_empty());
        let operation = ReadingOperation::register(&state, "request".into()).unwrap();
        operation
            .flag
            .store(true, std::sync::atomic::Ordering::Release);
        drop(operation);
        assert!(state.cancels.lock().unwrap().is_empty());
        let result = (|| -> Result<(), String> {
            let _operation = ReadingOperation::register(&state, "request".into())?;
            Err("synthetic failure".into())
        })();
        assert!(result.is_err());
        assert!(state.cancels.lock().unwrap().is_empty());
    }

    #[test]
    fn reading_stop_before_ipc_dispatch_prevents_registration() {
        let state = state();
        cancel_research_request(&state, "stopped-before-dispatch").unwrap();
        assert!(state.cancels.lock().unwrap().is_empty());
        let result = ReadingOperation::register(&state, "stopped-before-dispatch".into());
        assert!(matches!(result, Err(error) if error.contains("cancelled")));
        assert!(state.cancels.lock().unwrap().is_empty());
        assert!(!EARLY_RESEARCH_CANCELS
            .lock()
            .unwrap()
            .iter()
            .any(|id| id == "stopped-before-dispatch"));
    }

    #[tokio::test]
    async fn reading_stop_while_waiting_for_engine_prevents_generation() {
        let state = state();
        let engine_busy = state.engine.write().await;
        let operation =
            ReadingOperation::register(&state, "stopped-waiting-for-engine".into()).unwrap();
        cancel_research_request(&state, &operation.id).unwrap();
        let ran_model = AtomicBool::new(false);
        tokio::time::timeout(std::time::Duration::from_millis(100), async {
            tokio::select! {
                biased;
                _ = operation.cancelled() => {},
                _ = async {
                    let _engine = state.engine.read().await;
                    ran_model.store(true, std::sync::atomic::Ordering::Release);
                } => panic!("a stopped request must not reach model work"),
            }
        })
        .await
        .unwrap();
        assert!(!ran_model.load(std::sync::atomic::Ordering::Acquire));
        drop(engine_busy);
        drop(operation);
        assert!(state.cancels.lock().unwrap().is_empty());
    }

    #[test]
    fn reading_early_cancellation_memory_is_bounded_and_deduplicated() {
        let mut recent = std::collections::VecDeque::new();
        for index in 0..MAX_EARLY_RESEARCH_CANCELS + 10 {
            remember_early_cancel(&mut recent, &format!("request-{index}"));
        }
        assert_eq!(recent.len(), MAX_EARLY_RESEARCH_CANCELS);
        assert_eq!(recent.front().unwrap(), "request-10");
        remember_early_cancel(&mut recent, "request-10");
        assert_eq!(recent.len(), MAX_EARLY_RESEARCH_CANCELS);
        assert_eq!(recent.back().unwrap(), "request-10");
    }

    #[test]
    fn reading_ipc_contract_rejects_changed_graph_and_uses_camel_case() {
        let root = std::path::Path::new("/synthetic/first");
        assert!(require_research_graph(root, "/synthetic/first").is_ok());
        assert!(require_research_graph(root, "/synthetic/second").is_err());
        assert!(require_research_graph(root, "").is_err());
        let target: ResearchTarget = serde_json::from_value(serde_json::json!({
            "pageId":"day", "scope":"selection", "selection":{"blockIds":["block"],"text":"selected"}
        })).unwrap();
        assert_eq!(target.page_id, "day");
        assert_eq!(target.selection.unwrap().block_ids, vec!["block"]);
        assert_eq!(serde_json::to_value(ResearchWebMode::Off).unwrap(), "off");
        assert_eq!(
            serde_json::to_value(ResearchWebMode::Search).unwrap(),
            "search"
        );
        assert_eq!(
            serde_json::to_value(ResearchWebMode::Research).unwrap(),
            "research"
        );
    }

    #[test]
    fn reading_config_defaults_do_not_create_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!dir.path().join("knowledge").exists());
        let _ = reading_config(dir.path()).unwrap();
        assert!(!dir.path().join("knowledge").exists());
    }
}
