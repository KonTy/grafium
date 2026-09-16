use super::*;
use crate::ai::traits::BoxFuture;
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct InspectModel {
    pub requests: Mutex<Vec<(Vec<ChatMessage>, CompletionOptions)>>,
}

impl InspectModel {
    fn tokens(messages: &[ChatMessage], options: &CompletionOptions) -> usize {
        32 + options.system_prompt.as_ref().map_or(0, String::len)
            + messages.iter().map(|m| m.content.len() + 5).sum::<usize>()
    }
}

impl LlmProvider for InspectModel {
    fn name(&self) -> &str {
        "synthetic-provenance-model"
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
        Box::pin(async move { Ok(Some(Self::tokens(messages, options))) })
    }
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            assert!(
                Self::tokens(messages, options) + options.max_tokens.unwrap() as usize + 16 <= 6144
            );
            self.requests
                .lock()
                .unwrap()
                .push((messages.to_vec(), options.clone()));
            let system = options.system_prompt.as_deref().unwrap_or_default();
            Ok(
                if system.starts_with("PLAN") || system.starts_with("REFINE") {
                    r#"{"queries":["source claim verification"]}"#
                } else if system.starts_with("SELECT") {
                    r#"{"picks":[0]}"#
                } else if system.starts_with("ASSESS") {
                    r#"{"sufficient":true,"missing":""}"#
                } else if system.starts_with("SYNTH") {
                    r#"{"title_answer":"A cited answer [1].","topics":[]}"#
                } else {
                    "A cited answer [1]."
                }
                .into(),
            )
        })
    }
}

pub(crate) fn body(number: usize) -> String {
    let head = format!("BEGIN-SOURCE-{number}; ");
    let tail = format!(" END-SOURCE-{number};");
    format!("{head}{}{tail}", "x".repeat(2800 - head.len() - tail.len()))
}

pub(crate) fn assert_provenance(payload: &str, labels: &[usize], prefix: &str) {
    for &number in labels {
        let label = format!("{prefix}[{number}]:\n");
        let start = payload
            .find(&label)
            .unwrap_or_else(|| panic!("Missing original label {label} in {payload}"))
            + label.len();
        let section = payload[start..].split("\n\n").next().unwrap();
        assert!(
            section.contains(&format!("BEGIN-SOURCE-{number}")),
            "{section}"
        );
        assert!(
            section.contains(&format!("END-SOURCE-{number}")),
            "{section}"
        );
        for &other in labels.iter().filter(|&&other| other != number) {
            assert!(
                !section.contains(&format!("SOURCE-{other};")),
                "Source {other} was attributed to label {number}"
            );
        }
    }
}

#[tokio::test]
async fn research_budget_preserves_each_numbered_source_and_the_entire_question() {
    for with_reading_source in [false, true] {
        let model = InspectModel::default();
        let question = format!(
            "QUESTION-BEGIN {} QUESTION-END",
            "meaningful qualifier ".repeat(100)
        );
        assert!(
            question.len() > 2000,
            "exercise the old question truncation limit"
        );
        let instructions = "REQUIRED-INSTRUCTIONS: cite original source numbers; distinguish independent evidence from the reading source.";
        let mut input = StepInput::new(&question, instructions);
        input.evidence = [1, 7, 13]
            .into_iter()
            .map(|number| Evidence::numbered(number, body(number)))
            .collect();
        let reading = vec![(27, body(27))];
        complete(
            &model,
            "Keep provenance.",
            &input,
            with_reading_source.then_some(reading.as_slice()),
            &[],
            2048,
            0.2,
            None,
        )
        .await
        .unwrap();
        let requests = model.requests.lock().unwrap();
        let (messages, options) = &requests[0];
        if !with_reading_source {
            assert_eq!(options.max_tokens, Some(2048));
        }
        let payload = &messages.last().unwrap().content;
        assert!(payload.starts_with(&format!("Question: {question}\n\n")));
        assert!(payload.ends_with(instructions));
        assert!(payload.contains("excerpt shortened"));
        assert_provenance(payload, &[1, 7, 13], "");
        if with_reading_source {
            assert_provenance(&messages[0].content, &[27], "Reading excerpt ");
        }
    }
}

#[tokio::test]
async fn research_budget_never_truncates_required_instructions_or_oversized_questions() {
    for (system, question, instructions) in [
        (
            "system".into(),
            "q".repeat(7000),
            "Required instruction".into(),
        ),
        ("system".into(), "A short question".into(), "i".repeat(7000)),
        (
            "s".repeat(7000),
            "A short question".into(),
            "Required instruction".into(),
        ),
    ] {
        let model = InspectModel::default();
        let mut input = StepInput::new(&question, instructions);
        input.evidence.push(Evidence::numbered(31, body(31)));
        assert!(
            complete(&model, &system, &input, None, &[], 2048, 0.2, None)
                .await
                .is_err()
        );
        assert!(model.requests.lock().unwrap().is_empty());
    }
}
