use super::*;
use crate::ai::traits::BoxFuture;
use crate::knowledge::engine::reading_scope::{ReadingScope, ResearchTarget};
use crate::research::{
    EngineCategory, EngineKind, JsonPaths, ResearchConfig, ResearchPrompts, SearchEngineDef,
};
use crate::scraping::browser::FetchedResource;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

#[derive(Default)]
struct CheckedModel {
    calls: Mutex<Vec<(Vec<ChatMessage>, CompletionOptions)>>,
    assessed: AtomicUsize,
    counted: AtomicUsize,
    hang_count: bool,
    hang_complete: bool,
    measured: bool,
}

impl CheckedModel {
    fn charge(messages: &[ChatMessage], options: &CompletionOptions) -> usize {
        32 + options.system_prompt.as_ref().map_or(0, String::len)
            + messages.iter().map(|m| m.content.len() + 5).sum::<usize>()
    }
}

impl LlmProvider for Arc<CheckedModel> {
    fn name(&self) -> &str {
        "synthetic-reading-model"
    }
    fn health_check(&self) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn context_window(&self) -> Option<usize> {
        Some(6144)
    }
    fn count_prompt_tokens<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<Option<usize>>> {
        Box::pin(async move {
            self.counted.fetch_add(1, Ordering::Relaxed);
            assert!(options.cancel.is_some());
            if self.hang_count {
                return std::future::pending().await;
            }
            Ok(self
                .measured
                .then(|| CheckedModel::charge(messages, options)))
        })
    }
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let tokens = if self.measured {
                CheckedModel::charge(messages, options)
            } else {
                crate::ai::prompt_budget::conservative_prompt_tokens(messages, options)
            };
            assert!(tokens + options.max_tokens.unwrap() as usize + 16 <= 6144);
            self.calls
                .lock()
                .unwrap()
                .push((messages.to_vec(), options.clone()));
            if self.hang_complete {
                return std::future::pending().await;
            }
            let system = options.system_prompt.as_deref().unwrap_or_default();
            Ok(if system.starts_with("PLAN") {
                r#"{"queries":["cobalt therapy lantern memory evidence"]}"#.into()
            } else if system.starts_with("SELECT") {
                r#"{"picks":[0]}"#.into()
            } else if system.starts_with("ASSESS") {
                let sufficient = self.assessed.fetch_add(1, Ordering::Relaxed) > 0;
                format!(r#"{{"sufficient":{sufficient},"missing":"replication evidence"}}"#)
            } else if system.starts_with("REFINE") {
                r#"{"queries":["cobalt therapy lantern memory replication"]}"#.into()
            } else if system.starts_with("SYNTH") {
                r#"{"title_answer":"Independent evidence qualifies the claim [1].","topics":[]}"#
                    .into()
            } else {
                "The reading source claims cobalt therapy improves lantern memory [1].".into()
            })
        })
    }
}

#[derive(Default)]
struct FakeBrowser {
    fetches: AtomicUsize,
    hang: bool,
}

impl BrowserDriver for FakeBrowser {
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<FetchedResource>> {
        Box::pin(async move {
            let index = self.fetches.fetch_add(1, Ordering::Relaxed);
            if self.hang {
                return std::future::pending().await;
            }
            let (kind, body) = if url.starts_with("https://search.test/") {
                (
                    "application/json",
                    format!(
                        r#"{{"results":[{{"url":"https://article.test/{index}","title":"Cobalt therapy study","snippet":"Lantern memory trial evidence"}}]}}"#
                    ),
                )
            } else {
                ("text/html", format!("<html><body><article><h1>Cobalt therapy</h1><p>{}</p></article></body></html>", "An independent trial evaluates cobalt therapy and lantern memory. ".repeat(180)))
            };
            Ok(FetchedResource {
                url: url.into(),
                content_type: Some(kind.into()),
                bytes: body.into_bytes(),
            })
        })
    }
}

fn config() -> ResearchConfig {
    ResearchConfig {
        max_rounds: 3,
        max_sources: 3,
        results_per_query: 2,
        engines: vec![SearchEngineDef {
            id: "fake".into(),
            name: "Fake".into(),
            kind: EngineKind::Json,
            url_template: "https://search.test/?q={query}".into(),
            enabled: true,
            builtin: false,
            category: EngineCategory::Academic,
            selectors: None,
            json_paths: Some(JsonPaths {
                results: "results".into(),
                url: "url".into(),
                title: "title".into(),
                snippet: "snippet".into(),
                url_prefix: None,
            }),
        }],
        prompts: ResearchPrompts {
            plan_queries: "PLAN".into(),
            select_sources: "SELECT".into(),
            assess_sufficiency: "ASSESS".into(),
            refine_queries: "REFINE".into(),
            synthesize: "SYNTH".into(),
        },
        ..Default::default()
    }
}

fn fixture(model: Arc<CheckedModel>) -> (tempfile::TempDir, KnowledgeEngine, ReadingSource) {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = KnowledgeEngine::new(
        dir.path(),
        AiConfig {
            enabled: false,
            ..Default::default()
        },
    )
    .unwrap();
    engine.llm = Some(Box::new(model));
    let db = crate::db::Database::new(dir.path().join("reading.db")).unwrap();
    let page = db.create_page("Books/Synthetic long book", false).unwrap();
    let other = db.create_page("Other page", false).unwrap();
    let text = format!(
        "{}\n\nCobalt therapy improves lantern memory, claims the source.",
        "A synthetic neutral passage about reading. ".repeat(2200)
    );
    assert!(text.len() > 60_000);
    db.create_block_with_id(
        "book",
        &page.id,
        None,
        0,
        &text,
        crate::models::BlockType::Text,
        serde_json::json!({}),
    )
    .unwrap();
    db.create_block_with_id(
        "outside",
        &other.id,
        None,
        0,
        "FORBIDDEN-OTHER-PAGE",
        crate::models::BlockType::Text,
        serde_json::json!({}),
    )
    .unwrap();
    let source = ReadingSource::capture(
        &db,
        &ResearchTarget {
            page_id: page.id,
            scope: ReadingScope::Page,
            block_id: None,
            selection: None,
        },
    )
    .unwrap();
    (dir, engine, source)
}

fn history() -> Vec<ChatTurn> {
    vec![
        ChatTurn {
            role: "user".into(),
            content: format!(
                "Previous question about cobalt therapy and lantern memory. {}",
                "Older discussion. ".repeat(180)
            ),
        },
        ChatTurn {
            role: "assistant".into(),
            content: "We discussed cobalt therapy. RECENT-CONTEXT".into(),
        },
    ]
}

#[tokio::test]
async fn assistant_graph_and_book_web_modes_preserve_budgets_citations_and_scope() {
    use crate::knowledge::assistant_scope::AssistantContext;
    use crate::models::{BlockType, LinkType};
    for graph_wide in [false, true] {
        for mode in [AssistantMode::Web, AssistantMode::Deep] {
            let dir = tempfile::tempdir_in(".").unwrap();
            let model = Arc::new(CheckedModel {
                measured: true,
                ..Default::default()
            });
            let mut engine = KnowledgeEngine::new(
                dir.path(),
                AiConfig {
                    enabled: false,
                    ..Default::default()
                },
            )
            .unwrap();
            engine.llm = Some(Box::new(model.clone()));
            let db = crate::db::Database::in_memory().unwrap();
            let book = db.create_page("Recognized whole book", false).unwrap();
            db.update_page(
                &book.id,
                None,
                Some(&serde_json::json!({"collection":"book"})),
            )
            .unwrap();
            let mut member_ids = vec![book.id.clone()];
            for number in 0..4 {
                let member = db.create_page(&format!("Member {number}"), false).unwrap();
                let link = db
                    .create_block(
                        &book.id,
                        None,
                        number,
                        &format!("[[{}]]", member.title),
                        BlockType::Text,
                        serde_json::json!({}),
                    )
                    .unwrap();
                db.insert_link(&link.id, &member.id, LinkType::Page)
                    .unwrap();
                db.create_block(
                    &member.id,
                    None,
                    0,
                    &format!(
                        "Cobalt therapy improves lantern memory. {}",
                        "Neutral source passage. ".repeat(1500)
                    ),
                    BlockType::Text,
                    serde_json::json!({}),
                )
                .unwrap();
                member_ids.push(member.id);
            }
            let outside = db.create_page("Outside the book", false).unwrap();
            db.create_block(
                &outside.id,
                None,
                0,
                "Cobalt therapy FORBIDDEN-BOOK-OUTSIDER",
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
            let source = AssistantSource::capture(
                &db,
                dir.path(),
                &if graph_wide {
                    AssistantContext::Graph {}
                } else {
                    AssistantContext::Book { page_id: book.id }
                },
            )
            .unwrap();
            let browser = FakeBrowser::default();
            let mut notes = Vec::new();
            let mut text = String::new();
            let outcome = engine
                .assistant_chat_using(
                    &source,
                    "Evaluate cobalt therapy lantern memory",
                    &history(),
                    "this-graph",
                    mode,
                    &config(),
                    &browser,
                    Some(Arc::new(AtomicBool::new(false))),
                    &mut |event| match event {
                        AskStreamEvent::Delta(delta) => text.push_str(delta),
                        AskStreamEvent::Note(note) => notes.push(note.to_string()),
                        _ => {}
                    },
                )
                .await
                .unwrap();
            assert!(text.contains("From your notes") && text.contains("From the web"));
            assert!(!outcome.sources.is_empty() && !outcome.web_citations.is_empty());
            assert!(notes.iter().any(|note| note.contains("partial")));
            let calls = model.calls.lock().unwrap();
            assert!(calls.iter().any(|(_, options)| options
                .system_prompt
                .as_deref()
                .is_some_and(|p| p.starts_with("PLAN"))));
            if !graph_wide {
                assert!(outcome
                    .sources
                    .iter()
                    .all(|source| member_ids.contains(&source.page_id)));
                for (messages, options) in calls.iter() {
                    let payload = format!("{messages:?} {:?}", options.system_prompt);
                    assert!(!payload.contains("FORBIDDEN-BOOK-OUTSIDER"));
                }
            }
        }
    }
}

struct SemanticQueryEmbedder;

impl crate::ai::traits::Embedder for SemanticQueryEmbedder {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move { Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect()) })
    }
    fn dimension(&self) -> usize {
        2
    }
    fn model_name(&self) -> &str {
        "synthetic-semantic-query"
    }
}

#[tokio::test]
async fn assistant_book_later_semantic_match_survives_global_excerpt_limit_in_all_modes() {
    use crate::ai::traits::ChunkEmbedding;
    use crate::knowledge::assistant_scope::AssistantContext;
    use crate::models::{BlockType, LinkType};

    for mode in [
        AssistantMode::Answer,
        AssistantMode::Web,
        AssistantMode::Deep,
    ] {
        let dir = tempfile::tempdir_in(".").unwrap();
        let model = Arc::new(CheckedModel {
            measured: true,
            ..Default::default()
        });
        let mut engine = KnowledgeEngine::new(
            dir.path(),
            AiConfig {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
        engine.llm = Some(Box::new(model.clone()));
        engine.embedder = Some(Box::new(SemanticQueryEmbedder));
        let db = crate::db::Database::in_memory().unwrap();
        let book = db.create_page("Author ordered book", false).unwrap();
        db.update_page(
            &book.id,
            None,
            Some(&serde_json::json!({"collection":"book"})),
        )
        .unwrap();
        let first = db.create_page("First member", false).unwrap();
        let later = db.create_page("Later member", false).unwrap();
        for (member, order) in [(&first, 0), (&later, 1)] {
            let link = db
                .create_block(
                    &book.id,
                    None,
                    order,
                    &format!("[[{}]]", member.title),
                    BlockType::Text,
                    serde_json::json!({}),
                )
                .unwrap();
            db.insert_link(&link.id, &member.id, LinkType::Page)
                .unwrap();
        }
        for index in 0..16 {
            db.create_block(
                &first.id,
                None,
                index,
                &format!("Unrelated ornamental passage {index}."),
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
        }
        let evidence = "Cobalt therapy improves lantern memory. SEMANTIC-EVIDENCE-LATER";
        let target = db
            .create_block(
                &later.id,
                None,
                0,
                evidence,
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
        let store = Arc::new(crate::knowledge::SqliteVectorStore::in_memory().unwrap());
        store
            .upsert(&[ChunkEmbedding {
                chunk_id: "semantic-later".into(),
                graph_id: "this-graph".into(),
                page_id: later.id.clone(),
                block_id: Some(target.id),
                page_title: later.title.clone(),
                content: evidence.into(),
                embedding: vec![1.0, 0.0],
                metadata: serde_json::json!({}),
            }])
            .await
            .unwrap();
        engine.vector_store = Some(store);
        let question = "Explain illumination and recollection";
        assert!(crate::db::chat_salient_terms(question)
            .iter()
            .all(|term| !evidence.to_lowercase().contains(term.as_str())));
        let source = AssistantSource::capture(
            &db,
            dir.path(),
            &AssistantContext::Book { page_id: book.id },
        )
        .unwrap();
        let browser = FakeBrowser::default();
        let outcome = engine
            .assistant_chat_using(
                &source,
                question,
                &[],
                "this-graph",
                mode,
                &config(),
                &browser,
                Some(Arc::new(AtomicBool::new(false))),
                &mut |_| {},
            )
            .await
            .unwrap();
        let calls = model.calls.lock().unwrap();
        let first_request = &calls[0].0;
        assert!(
            first_request
                .iter()
                .any(|message| message.content.contains("SEMANTIC-EVIDENCE-LATER")),
            "later semantic-only evidence was replaced by earlier fallback excerpts"
        );
        assert!(outcome
            .sources
            .iter()
            .any(|source| source.page_id == later.id));
        if mode == AssistantMode::Answer {
            assert_eq!(browser.fetches.load(Ordering::Relaxed), 0);
        } else {
            let planning = calls
                .iter()
                .find(|(_, options)| {
                    options
                        .system_prompt
                        .as_deref()
                        .is_some_and(|prompt| prompt.starts_with("PLAN"))
                })
                .unwrap();
            assert!(planning
                .0
                .iter()
                .any(|message| message.content.contains("SEMANTIC-EVIDENCE-LATER")));
        }
    }
}

struct ForbiddenEmbedder;

impl crate::ai::traits::Embedder for ForbiddenEmbedder {
    fn embed<'a>(&'a self, _texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        panic!("No notes must never call an embedder");
    }
    fn dimension(&self) -> usize {
        2
    }
    fn model_name(&self) -> &str {
        "forbidden"
    }
}

#[tokio::test]
async fn assistant_none_bypasses_notes_embeddings_and_note_prompts_in_every_mode() {
    for mode in [
        AssistantMode::Answer,
        AssistantMode::Web,
        AssistantMode::Deep,
    ] {
        let model = Arc::new(CheckedModel {
            measured: true,
            ..Default::default()
        });
        let (dir, mut engine, _) = fixture(model.clone());
        engine.embedder = Some(Box::new(ForbiddenEmbedder));
        engine.vector_store = Some(Arc::new(
            crate::knowledge::SqliteVectorStore::open(&dir.path().join("vectors.db")).unwrap(),
        ));
        let db = crate::db::Database::in_memory().unwrap();
        let page = db.create_page("cobalt therapy", false).unwrap();
        db.create_block(
            &page.id,
            None,
            0,
            "cobalt therapy NOTE-SECRET",
            crate::models::BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();
        let source = AssistantSource::capture(
            &db,
            dir.path(),
            &crate::knowledge::assistant_scope::AssistantContext::None {},
        )
        .unwrap();
        let browser = FakeBrowser::default();
        let mut text = String::new();
        let outcome = engine
            .assistant_chat_using(
                &source,
                "Research and verify cobalt therapy lantern memory",
                &history(),
                "synthetic",
                mode,
                &config(),
                &browser,
                Some(Arc::new(AtomicBool::new(false))),
                &mut |ev| {
                    if let AskStreamEvent::Delta(delta) = ev {
                        text.push_str(delta)
                    }
                },
            )
            .await
            .unwrap();
        assert!(outcome.sources.is_empty());
        assert!(!text.contains("From your notes"));
        let calls = model.calls.lock().unwrap();
        assert!(!calls.is_empty());
        for (messages, options) in calls.iter() {
            let payload = format!("{messages:?} {:?}", options.system_prompt);
            assert!(!payload.contains("NOTE-SECRET"));
            assert!(!payload.contains("Selected reading excerpts"));
            assert!(!payload.contains("Reading excerpt ["));
            assert!(!payload.contains("selected reading source"));
        }
        if mode == AssistantMode::Answer {
            assert_eq!(browser.fetches.load(Ordering::Relaxed), 0);
            assert_eq!(calls.len(), 1);
            assert!(outcome.web_citations.is_empty());
        } else {
            assert!(browser.fetches.load(Ordering::Relaxed) >= 2);
            assert!(!outcome.web_citations.is_empty());
        }
    }
}

#[tokio::test]
async fn assistant_answer_is_never_promoted_to_web_for_graph_or_scoped_sources() {
    for graph_wide in [false, true] {
        let model = Arc::new(CheckedModel {
            measured: true,
            ..Default::default()
        });
        let (dir, engine, scoped) = fixture(model.clone());
        let source = if graph_wide {
            let db = crate::db::Database::in_memory().unwrap();
            let page = db.create_page("Cobalt therapy", false).unwrap();
            db.create_block(
                &page.id,
                None,
                0,
                "Cobalt therapy improves lantern memory.",
                crate::models::BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
            AssistantSource::capture(
                &db,
                dir.path(),
                &crate::knowledge::assistant_scope::AssistantContext::Graph {},
            )
            .unwrap()
        } else {
            AssistantSource::Reading(vec![scoped])
        };
        let browser = FakeBrowser::default();
        engine
            .assistant_chat_using(
                &source,
                "Research and verify cobalt therapy lantern memory",
                &[],
                "synthetic",
                AssistantMode::Answer,
                &config(),
                &browser,
                Some(Arc::new(AtomicBool::new(false))),
                &mut |_| {},
            )
            .await
            .unwrap();
        assert_eq!(browser.fetches.load(Ordering::Relaxed), 0);
        assert_eq!(model.calls.lock().unwrap().len(), 1);
        let calls = model.calls.lock().unwrap();
        let payload = format!("{:?}", calls[0].0);
        assert!(payload.contains("Cobalt therapy"));
        assert!(!payload.contains("FORBIDDEN-OTHER-PAGE"));
    }
}

/// Only the app-side FakeBrowser has search/article data. The API-compatible
/// server supports plain chat completions and cannot execute browsing tools.
#[tokio::test]
async fn assistant_remote_api_web_and_deep_receive_host_fetched_plain_messages() {
    use crate::ai::providers::openai_compatible::OpenAiCompatibleLlm;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    for mode in [AssistantMode::Web, AssistantMode::Deep] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let recorded = requests.clone();
        let transport = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut raw = Vec::new();
                let (headers, body_start) = loop {
                    let mut buffer = [0u8; 4096];
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    raw.extend_from_slice(&buffer[..n]);
                    if let Some(start) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        break (String::from_utf8(raw[..start].to_vec()).unwrap(), start + 4);
                    }
                };
                assert!(headers.starts_with("POST /v1/chat/completions "));
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                while raw.len() < body_start + length {
                    let mut buffer = [0u8; 4096];
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    raw.extend_from_slice(&buffer[..n]);
                }
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw[body_start..body_start + length]).unwrap();
                assert_eq!(payload["model"], "dgx-synthetic");
                assert!(payload.get("tools").is_none() && payload.get("tool_choice").is_none());
                let messages = payload["messages"].as_array().unwrap();
                assert!(messages.iter().all(|m| ["system", "user", "assistant"]
                    .contains(&m["role"].as_str().unwrap())
                    && m["content"].is_string()));
                let system = messages.iter().find(|m| m["role"] == "system").unwrap()["content"]
                    .as_str()
                    .unwrap();
                let answer = if system.starts_with("PLAN") {
                    r#"{"queries":["cobalt therapy lantern memory trial"]}"#
                } else if system.starts_with("SELECT") {
                    r#"{"picks":[0]}"#
                } else if system.starts_with("ASSESS") {
                    r#"{"sufficient":true,"missing":""}"#
                } else if system.starts_with("SYNTH") {
                    assert!(messages.iter().any(|m| m["content"]
                        .as_str()
                        .unwrap()
                        .contains("An independent trial evaluates cobalt therapy")));
                    r#"{"title_answer":"The host-retrieved trial qualifies the claim [1].","topics":[]}"#
                } else {
                    panic!("unexpected model request: {system}")
                };
                recorded.lock().unwrap().push(payload);
                let response = serde_json::to_string(&serde_json::json!({"choices":[{"message":{"role":"assistant","content":answer}}]})).unwrap();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).as_bytes()).await.unwrap();
            }
        });
        let dir = tempfile::tempdir_in(".").unwrap();
        let mut engine = KnowledgeEngine::new(
            dir.path(),
            AiConfig {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
        engine.llm = Some(Box::new(OpenAiCompatibleLlm::new(
            &endpoint,
            "dgx-synthetic",
            None,
        )));
        let browser = FakeBrowser::default();
        let mut text = String::new();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            engine.assistant_chat_using(
                &AssistantSource::None,
                "Evaluate cobalt therapy lantern memory",
                &[],
                "synthetic",
                mode,
                &config(),
                &browser,
                Some(Arc::new(AtomicBool::new(false))),
                &mut |ev| {
                    if let AskStreamEvent::Delta(delta) = ev {
                        text.push_str(delta)
                    }
                },
            ),
        )
        .await;
        transport.abort();
        let outcome = result.unwrap().unwrap();
        assert!(text.contains("host-retrieved trial"));
        assert!(outcome.sources.is_empty());
        assert_eq!(outcome.web_citations.len(), 1);
        assert!(browser.fetches.load(Ordering::Relaxed) >= 2);
        assert!(requests.lock().unwrap().len() >= 3);
    }
}

#[tokio::test]
async fn reading_research_and_graph_retrieval_do_not_attribute_annotations_to_the_author() {
    let (dir, graph, page, _) = crate::knowledge::source_projection::tests::annotated_book();
    let model = Arc::new(CheckedModel {
        measured: true,
        ..Default::default()
    });
    let mut engine = KnowledgeEngine::new(
        dir.path(),
        AiConfig {
            enabled: false,
            ..Default::default()
        },
    )
    .unwrap();
    engine.llm = Some(Box::new(model.clone()));
    let source = ReadingSource::capture(
        &graph.db,
        &ResearchTarget {
            page_id: page.id.clone(),
            scope: ReadingScope::Page,
            block_id: None,
            selection: None,
        },
    )
    .unwrap();
    let browser = FakeBrowser::default();
    engine
        .research_scoped_using(
            &source,
            "What does the author claim about cobalt?",
            &[],
            None,
            ResearchWebMode::Off,
            &config(),
            &browser,
            Some(Arc::new(AtomicBool::new(false))),
            &mut |_| {},
        )
        .await
        .unwrap();
    assert_eq!(browser.fetches.load(Ordering::Relaxed), 0);
    let calls = model.calls.lock().unwrap();
    assert!(!calls.is_empty());
    for (messages, options) in calls.iter() {
        let payload = messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            + options.system_prompt.as_deref().unwrap_or("");
        assert!(payload.contains("cobalt improves memory.[^1]"));
        assert!(!payload.contains("USER-ANNOTATION"));
        assert!(!payload.contains("grafium-note-"));
    }
    drop(calls);
    let mut hits = engine
        .hybrid_search(&graph.db, "cobalt memory objection", 12, None)
        .await
        .unwrap();
    assert!(!hits.is_empty());
    engine.expand_hits(&graph.db, &mut hits);
    for hit in hits {
        assert!(!hit.content.contains("USER-ANNOTATION"));
        assert!(!hit.content.contains("grafium-note-"));
        assert!(hit
            .parents
            .iter()
            .chain(&hit.children)
            .all(
                |b| !b.content.contains("USER-ANNOTATION") && !b.content.contains("grafium-note-")
            ));
    }
}

struct SummaryEvidenceModel(Mutex<Vec<Vec<ChatMessage>>>);

impl LlmProvider for Arc<SummaryEvidenceModel> {
    fn name(&self) -> &str {
        "synthetic-summary-evidence"
    }
    fn health_check(&self) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        _: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(messages.to_vec());
            Ok(r#"{"title_answer":null,"topics":[{"topic":"The author's cobalt claim","summary":"The author claims cobalt improves memory and supplies an author citation; the statement remains a claim from this synthetic book.","tags":[{"term":"cobalt"}]}]}"#.into())
        })
    }
}

#[tokio::test]
async fn reading_book_summary_omits_managed_footer_and_retains_author_citations() {
    let (dir, graph, page, _) = crate::knowledge::source_projection::tests::annotated_book();
    let blocks = graph.db.list_blocks_for_page(&page.id).unwrap();
    let raw = crate::parser::serialize_page(&page.properties, &blocks);
    assert!(raw.contains("USER-ANNOTATION"));
    let model = Arc::new(SummaryEvidenceModel(Mutex::new(Vec::new())));
    let mut engine = KnowledgeEngine::new(
        dir.path(),
        AiConfig {
            enabled: false,
            ..Default::default()
        },
    )
    .unwrap();
    engine.llm = Some(Box::new(model.clone()));
    let summary = engine
        .summarize_text(&page.title, &raw, &mut |_| {})
        .await
        .unwrap();
    assert!(!summary.topics.is_empty());
    let calls = model.0.lock().unwrap();
    assert!(!calls.is_empty());
    for messages in calls.iter() {
        let payload = messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!payload.contains("USER-ANNOTATION"));
        assert!(!payload.contains("grafium-reading-note"));
        assert!(!payload.contains("grafium-note-"));
        assert!(payload.contains("cobalt improves memory.[^1]"));
    }
}

#[tokio::test]
async fn reading_research_long_book_and_followup_fit_exact_context_without_web_or_foreign_notes() {
    for measured in [true, false] {
        let model = Arc::new(CheckedModel {
            measured,
            ..Default::default()
        });
        let (_dir, engine, source) = fixture(model.clone());
        let browser = FakeBrowser::default();
        let mut notes = Vec::new();
        let outcome = engine
            .research_scoped_using(
                &source,
                "What evidence supports that?",
                &history(),
                None,
                ResearchWebMode::Off,
                &config(),
                &browser,
                Some(Arc::new(AtomicBool::new(false))),
                &mut |event| {
                    if let AskStreamEvent::Note(note) = event {
                        notes.push(note.to_string());
                    }
                },
            )
            .await
            .unwrap();
        assert_eq!(browser.fetches.load(Ordering::Relaxed), 0);
        assert_eq!(outcome.sources.len(), 1);
        assert_eq!(outcome.sources[0].page_id, source.page.id);
        assert!(notes.iter().any(|s| s.contains("partially covered")));
        let calls = model.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let prompt = calls[0]
            .0
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(prompt.contains("Cobalt therapy improves lantern memory"));
        assert!(prompt.contains("RECENT-CONTEXT"));
        assert!(!prompt.contains("FORBIDDEN-OTHER-PAGE"));
    }
}

#[tokio::test]
async fn reading_research_planner_refinement_and_synthesis_receive_bounded_source_claims() {
    let model = Arc::new(CheckedModel {
        measured: true,
        ..Default::default()
    });
    let (_dir, engine, source) = fixture(model.clone());
    let browser = FakeBrowser::default();
    let mut answer = String::new();
    let result = engine
        .research_scoped_using(
            &source,
            "Find evidence for this book's claims",
            &history(),
            None,
            ResearchWebMode::Research,
            &config(),
            &browser,
            Some(Arc::new(AtomicBool::new(false))),
            &mut |event| {
                if let AskStreamEvent::Delta(text) = event {
                    answer.push_str(text);
                }
            },
        )
        .await
        .unwrap();
    assert!(answer.contains("## From your notes") && answer.contains("## From the web"));
    assert!(!result.web_citations.is_empty());
    assert!(result.sources.iter().all(|s| s.page_id == source.page.id));
    let calls = model.calls.lock().unwrap();
    for step in ["PLAN", "REFINE", "SYNTH"] {
        let (messages, options) = calls
            .iter()
            .find(|(_, o)| {
                o.system_prompt
                    .as_deref()
                    .is_some_and(|s| s.starts_with(step))
            })
            .unwrap();
        let text = messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("Cobalt therapy improves lantern memory"),
            "{step}: {text}"
        );
        assert!(text.contains("UNTRUSTED SELECTED READING EXCERPTS"));
        assert!(!text.contains("FORBIDDEN-OTHER-PAGE"));
        assert!(options
            .system_prompt
            .as_ref()
            .unwrap()
            .contains("derive concrete searches"));
    }
}

#[tokio::test]
async fn reading_research_search_mode_is_one_round() {
    let model = Arc::new(CheckedModel {
        measured: true,
        ..Default::default()
    });
    let (_dir, engine, source) = fixture(model.clone());
    engine
        .research_scoped_using(
            &source,
            "Check cobalt therapy",
            &[],
            None,
            ResearchWebMode::Search,
            &config(),
            &FakeBrowser::default(),
            Some(Arc::new(AtomicBool::new(false))),
            &mut |_| {},
        )
        .await
        .unwrap();
    assert!(!model.calls.lock().unwrap().iter().any(|(_, o)| o
        .system_prompt
        .as_deref()
        .is_some_and(|s| s.starts_with("REFINE"))));
}

#[tokio::test]
async fn reading_research_default_step_prompts_fit_with_history_excerpts_and_answer_reserve() {
    let model = Arc::new(CheckedModel {
        measured: true,
        ..Default::default()
    });
    let prompts = ResearchPrompts::default();
    let source = vec![(
        1,
        format!(
            "Source claim: cobalt therapy improves lantern memory. {}",
            "Source detail. ".repeat(180)
        ),
    )];
    let mut input = crate::research::budget::StepInput::new(
        "Is the claim supported?",
        "Use only the numbered sources.",
    );
    input.evidence = vec![crate::research::budget::Evidence::numbered(
        1,
        "Independent evidence. ".repeat(600),
    )];
    for system in [
        prompts.plan_queries,
        prompts.select_sources,
        prompts.assess_sufficiency,
        prompts.refine_queries,
        prompts.synthesize,
    ] {
        crate::research::budget::complete(
            &model,
            &system,
            &input,
            Some(&source),
            &[ChatMessage {
                role: MessageRole::User,
                content: "A recent followup.".into(),
            }],
            4096,
            0.2,
            Some(Arc::new(AtomicBool::new(false))),
        )
        .await
        .unwrap();
    }
    assert_eq!(model.calls.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn reading_research_cancellation_interrupts_tokenizer_notes_and_browser() {
    for phase in ["before", "tokenizer", "notes", "browser"] {
        let model = Arc::new(CheckedModel {
            measured: true,
            hang_count: phase == "tokenizer",
            hang_complete: phase == "notes",
            ..Default::default()
        });
        let (_dir, engine, source) = fixture(model.clone());
        let browser = FakeBrowser {
            hang: phase == "browser",
            ..Default::default()
        };
        let flag = Arc::new(AtomicBool::new(phase == "before"));
        let stop = flag.clone();
        let setter = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            stop.store(true, Ordering::Release);
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            engine.research_scoped_using(
                &source,
                "Check cobalt therapy",
                &[],
                None,
                ResearchWebMode::Research,
                &config(),
                &browser,
                Some(flag),
                &mut |_| {},
            ),
        )
        .await
        .expect("cancellation must interrupt pending work");
        assert!(result.is_err(), "{phase}");
        setter.await.unwrap();
        if phase != "browser" {
            assert_eq!(browser.fetches.load(Ordering::Relaxed), 0);
        } else {
            assert_eq!(browser.fetches.load(Ordering::Relaxed), 1);
        }
    }
}
