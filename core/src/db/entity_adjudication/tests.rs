use super::*;
use crate::ai::traits::BoxFuture;
use std::sync::atomic::AtomicUsize;

struct FakeLlm {
    answer: String,
    window: usize,
    calls: AtomicUsize,
    counts: AtomicUsize,
    count_error: bool,
    hang: bool,
}

impl FakeLlm {
    fn new(answer: &str) -> Self {
        Self {
            answer: answer.into(),
            window: 4096,
            calls: AtomicUsize::new(0),
            counts: AtomicUsize::new(0),
            count_error: false,
            hang: false,
        }
    }
}

impl LlmProvider for FakeLlm {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::Relaxed);
            assert_eq!(options.max_tokens, Some(OUTPUT_TOKENS));
            assert_eq!(options.temperature, Some(0.0));
            assert!(messages[0].content.len() < 6000);
            if self.hang {
                std::future::pending::<()>().await;
            }
            Ok(self.answer.clone())
        })
    }

    fn count_prompt_tokens<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<Option<usize>>> {
        Box::pin(async move {
            self.counts.fetch_add(1, Ordering::Relaxed);
            if self.count_error {
                return Err(CoreError::Other("native tokenizer failed".into()));
            }
            Ok(Some(
                (messages[0].content.len() + options.system_prompt.as_ref().unwrap().len()) / 4,
            ))
        })
    }

    fn name(&self) -> &str {
        "synthetic"
    }
    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn context_window(&self) -> Option<usize> {
        Some(self.window)
    }
}

fn uncertain() -> EntityResolution {
    EntityResolution {
        source_phrase: "Mercury".into(),
        display_slug: "mercury".into(),
        target_title: "Mercury".into(),
        target_page_id: None,
        decision: EntityDecision::Ambiguous,
        candidates: vec![
            EntityCandidate {
                id: "planet".into(),
                title: "Mercury (planet)".into(),
            },
            EntityCandidate {
                id: "element".into(),
                title: "Mercury (element)".into(),
            },
        ],
        reason: "Needs sense".into(),
    }
}

#[tokio::test]
async fn exact_match_and_optional_absence_skip_the_model() -> Result<()> {
    let model = FakeLlm::new("not JSON");
    let mut exact = uncertain();
    exact.decision = EntityDecision::Reuse;
    exact.target_page_id = Some("planet".into());
    let result =
        adjudicate_entity_resolution(&exact, "", &[], Some(&model), &CancellationToken::new())
            .await?;
    assert_eq!(result.target_page_id, Some("planet".into()));
    assert_eq!(model.calls.load(Ordering::Relaxed), 0);
    assert_eq!(model.counts.load(Ordering::Relaxed), 0);
    let unresolved =
        adjudicate_entity_resolution(&uncertain(), "", &[], None, &CancellationToken::new())
            .await?;
    assert_eq!(unresolved.decision, EntityDecision::Ambiguous);
    Ok(())
}

#[tokio::test]
async fn contextual_shortlist_supports_reuse_new_and_abstention_without_scores() -> Result<()> {
    for (decision, target) in [
        ("reuse", Some("planet")),
        ("new", None),
        ("ambiguous", None),
    ] {
        let answer = serde_json::json!({"decision":decision,"target_id":target,"reason":"Orbital context identifies the intended sense."});
        let model = FakeLlm::new(&answer.to_string());
        let result = adjudicate_entity_resolution(
            &uncertain(),
            "Mercury orbits the Sun.",
            &[],
            Some(&model),
            &CancellationToken::new(),
        )
        .await?;
        assert_eq!(result.decision.as_str(), decision);
        assert_eq!(result.target_page_id.as_deref(), target);
        if decision == "reuse" {
            assert_eq!(result.target_title, "Mercury (planet)");
        } else {
            assert_eq!(result.target_title, "Mercury");
        }
        assert_eq!(model.calls.load(Ordering::Relaxed), 1);
    }
    Ok(())
}

#[tokio::test]
async fn fabricated_ids_conflicting_decisions_and_model_confidence_are_rejected() {
    for answer in [
        r#"{"decision":"reuse","target_id":"invented","reason":"guess"}"#,
        r#"{"decision":"new","target_id":"planet","reason":"guess"}"#,
        r#"{"decision":"reuse","target_id":"planet","reason":"guess","confidence":0.99}"#,
        r#"{"decision":"merge","target_id":"planet","reason":"guess"}"#,
        r#"{"decision":"reuse","target_id":null,"reason":"guess"}"#,
        "invalid JSON",
    ] {
        assert!(adjudicate_entity_resolution(
            &uncertain(),
            "",
            &[],
            Some(&FakeLlm::new(answer)),
            &CancellationToken::new()
        )
        .await
        .is_err());
    }
}

#[tokio::test]
async fn native_token_budget_shrinks_context_and_rejects_an_unfittable_shortlist() -> Result<()> {
    let mut model = FakeLlm::new(
        r#"{"decision":"ambiguous","target_id":null,"reason":"Insufficient context."}"#,
    );
    model.window = 800;
    let descriptions = vec![EntityCandidateContext {
        id: "planet".into(),
        title: "Mercury (planet)".into(),
        description: "星".repeat(800),
    }];
    adjudicate_entity_resolution(
        &uncertain(),
        &"文".repeat(4000),
        &descriptions,
        Some(&model),
        &CancellationToken::new(),
    )
    .await?;
    assert!(model.counts.load(Ordering::Relaxed) > 1);
    model.window = 64;
    assert!(adjudicate_entity_resolution(
        &uncertain(),
        "",
        &[],
        Some(&model),
        &CancellationToken::new()
    )
    .await
    .is_err());
    assert_eq!(model.calls.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test]
async fn tokenizer_errors_and_cancellation_are_not_silently_downgraded() {
    let mut broken = FakeLlm::new("{}");
    broken.count_error = true;
    assert!(adjudicate_entity_resolution(
        &uncertain(),
        "",
        &[],
        Some(&broken),
        &CancellationToken::new()
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("tokenizer"));
    let mut hanging = FakeLlm::new("{}");
    hanging.hang = true;
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let operation = async {
        adjudicate_entity_resolution(&uncertain(), "", &[], Some(&hanging), &cancel).await
    };
    let cancellation = async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        trigger.cancel();
    };
    let (result, ()) = tokio::join!(operation, cancellation);
    assert!(matches!(result, Err(CoreError::Cancelled)));
}

#[tokio::test]
async fn contextual_proposals_revalidate_source_and_shortlist_without_creating_pages() -> Result<()>
{
    use crate::db::Database;
    use crate::models::{BlockType, LinkCandidateStatus};
    use crate::parser::TagTerm;

    let db = Database::in_memory()?;
    let target = db.create_page("Niacin", false)?;
    let source = db.create_page("Synthetic source", false)?;
    let block = db.create_block(
        &source.id,
        None,
        0,
        "Nicin is a nutrient.",
        BlockType::Text,
        serde_json::json!({}),
    )?;
    let tags = [TagTerm::from("Nicin")];
    let resolution = db.resolve_tag_terms(&tags)?.remove(0);
    let model = FakeLlm::new(&serde_json::json!({
        "decision":"reuse","target_id":target.id,"reason":"Nutrient context supports the spelling correction."
    }).to_string());
    let resolved = adjudicate_entity_resolution(
        &resolution,
        &block.content,
        &db.entity_candidate_contexts(&resolution)?,
        Some(&model),
        &CancellationToken::new(),
    )
    .await?;
    let snapshot = db.list_blocks_for_page(&source.id)?;
    assert_eq!(
        db.discover_resolved_semantic_concept_candidates(
            &source.id,
            &tags,
            &[resolved.clone()],
            &snapshot,
            10,
            &CancellationToken::new()
        )?,
        1
    );
    let candidate = db
        .list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Pending), 10)?
        .remove(0);
    assert_eq!(candidate.to_page_id.as_deref(), Some(target.id.as_str()));
    assert_eq!(candidate.confidence, 0.0);
    assert_eq!(db.count_pages()?, 2);
    db.update_block(&block.id, "Nicin changed context.", None)?;
    assert!(db
        .discover_resolved_semantic_concept_candidates(
            &source.id,
            &tags,
            &[resolved.clone()],
            &snapshot,
            10,
            &CancellationToken::new()
        )
        .is_err());
    let current = db.list_blocks_for_page(&source.id)?;
    db.conn()?.execute(
        "UPDATE pages SET title = 'Different nutrient' WHERE id = ?1",
        [&target.id],
    )?;
    assert!(db
        .discover_resolved_semantic_concept_candidates(
            &source.id,
            &tags,
            &[resolved],
            &current,
            10,
            &CancellationToken::new()
        )
        .is_err());
    assert_eq!(db.count_pages()?, 2);
    Ok(())
}
