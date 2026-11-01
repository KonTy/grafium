use crate::{error::Result, CoreError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IqItem {
    pub id: String,
    pub question: String,
    pub answers: Vec<String>,
    pub explanation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IqNorms {
    pub citation: String,
    pub population: String,
    pub sample_size: u32,
    pub age_min: u8,
    pub age_max: u8,
    pub language: String,
    pub test_version: String,
    pub item_set_hash: String,
    pub time_limit_seconds: Option<u32>,
    pub instructions: String,
    pub z_scores: Vec<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IqPack {
    pub schema_version: u8,
    pub kind: String,
    pub id: String,
    pub title: String,
    pub test_version: String,
    pub source: String,
    pub license: String,
    pub items: Vec<IqItem>,
    pub norms: Option<IqNorms>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IqPackSummary {
    pub id: String,
    pub title: String,
    pub item_count: usize,
    pub norms: Option<IqNorms>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IqParticipant {
    pub age: u8,
    pub language: String,
    pub unseen: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IqSession {
    pub id: String,
    pub graph_path: String,
    pub title: String,
    pub questions: Vec<String>,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IqItemResult {
    pub question: String,
    pub answer: String,
    pub accepted_answers: Vec<String>,
    pub explanation: String,
    pub correct: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IqResult {
    pub session_id: String,
    pub title: String,
    pub correct: usize,
    pub total: usize,
    pub unanswered: usize,
    pub percentage: f64,
    pub elapsed_seconds: u64,
    pub estimated_iq: Option<f64>,
    pub iq_unavailable_reason: Option<String>,
    pub norm_citation: Option<String>,
    pub items: Vec<IqItemResult>,
}

pub fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::Other(format!("Invalid IQ pack or assessment: {}", message.into()))
}

pub fn normalized_answer(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl IqPack {
    pub fn item_set_hash(&self) -> Result<String> {
        let items: Vec<_> = self
            .items
            .iter()
            .map(|i| (&i.id, &i.question, &i.answers))
            .collect();
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&items)?)))
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 || self.kind != "iq" {
            return Err(invalid("expected schemaVersion 1 and kind iq"));
        }
        if self.id.is_empty()
            || self.id.len() > 128
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(invalid(
                "id must be 1-128 ASCII letters, digits, dots, hyphens or underscores",
            ));
        }
        for value in [&self.title, &self.test_version, &self.source, &self.license] {
            if value.trim().is_empty() || value.len() > 8192 {
                return Err(invalid(
                    "title, testVersion, source and license are required",
                ));
            }
        }
        if self.items.is_empty() || self.items.len() > 10_000 {
            return Err(invalid("expected 1-10000 ordered items"));
        }
        let mut ids = HashSet::new();
        let mut questions = HashSet::new();
        for item in &self.items {
            if item.id.trim().is_empty()
                || item.id.len() > 128
                || !ids.insert(&item.id)
                || item.question.trim().is_empty()
                || item.question.len() > 32_768
                || !questions.insert(item.question.trim())
                || item.explanation.len() > 32_768
                || item.answers.is_empty()
                || item.answers.len() > 32
                || item
                    .answers
                    .iter()
                    .any(|a| a.len() > 8192 || normalized_answer(a).is_empty())
            {
                return Err(invalid(
                    "items need unique IDs/questions, bounded text and nonempty accepted answers",
                ));
            }
        }
        if let Some(norms) = &self.norms {
            let url = url::Url::parse(&norms.citation)
                .map_err(|_| invalid("norm citation must be an HTTP(S) URL"))?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || [
                    &norms.citation,
                    &norms.population,
                    &norms.instructions,
                    &norms.language,
                ]
                .iter()
                .any(|s| s.len() > 8192)
                || norms.population.trim().is_empty()
                || norms.instructions.trim().is_empty()
                || norms.language.trim().is_empty()
                || norms.sample_size == 0
                || norms.age_min > norms.age_max
                || norms.age_max > 120
                || norms.test_version != self.test_version
                || norms.item_set_hash != self.item_set_hash()?
                || norms
                    .time_limit_seconds
                    .is_some_and(|s| s == 0 || s > 86_400)
                || norms.z_scores.len() != self.items.len() + 1
                || norms
                    .z_scores
                    .iter()
                    .any(|z| !z.is_finite() || z.abs() > 10.0)
                || norms.z_scores.windows(2).any(|pair| pair[0] > pair[1])
            {
                return Err(invalid("norms must document population, instructions, age, language, exact version/item set, timing and an ordered z-score for every raw score"));
            }
        }
        Ok(())
    }

    pub fn summary(&self) -> IqPackSummary {
        IqPackSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            item_count: self.items.len(),
            norms: self.norms.clone(),
        }
    }

    pub fn score(
        &self,
        session_id: &str,
        answers: &[String],
        elapsed_ms: i64,
        participant: &IqParticipant,
        repeated: bool,
    ) -> Result<IqResult> {
        self.validate()?;
        if answers.len() > self.items.len() || elapsed_ms < 0 {
            return Err(invalid("answer count or session clock"));
        }
        let items: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let answer = answers.get(index).cloned().unwrap_or_default();
                let normalized = normalized_answer(&answer);
                IqItemResult {
                    question: item.question.clone(),
                    answer,
                    accepted_answers: item.answers.clone(),
                    explanation: item.explanation.clone(),
                    correct: !normalized.is_empty()
                        && item
                            .answers
                            .iter()
                            .any(|a| normalized_answer(a) == normalized),
                }
            })
            .collect();
        let correct = items.iter().filter(|i| i.correct).count();
        let reason = match &self.norms {
            None => Some("This pack has no population norms; percentage correct is not IQ.".to_string()),
            Some(_) if !participant.unseen || repeated =>
                Some("Previous exposure to these questions makes this a practice score, not a norm-backed IQ estimate.".to_string()),
            Some(n) if participant.age < n.age_min || participant.age > n.age_max =>
                Some("Your age is outside this norm group's eligibility.".to_string()),
            Some(n) if participant.language != n.language =>
                Some("Your test language does not match these norms.".to_string()),
            Some(n) if answers.len() != self.items.len()
                && n.time_limit_seconds.is_none_or(|s| elapsed_ms < i64::from(s) * 1000) =>
                Some("An unfinished assessment cannot produce an IQ estimate.".to_string()),
            Some(_) => None,
        };
        let estimated_iq = if reason.is_none() {
            self.norms
                .as_ref()
                .map(|n| 100.0 + 15.0 * n.z_scores[correct])
        } else {
            None
        };
        Ok(IqResult {
            session_id: session_id.into(),
            title: self.title.clone(),
            correct,
            total: items.len(),
            unanswered: items
                .iter()
                .filter(|i| normalized_answer(&i.answer).is_empty())
                .count(),
            percentage: 100.0 * correct as f64 / items.len() as f64,
            elapsed_seconds: (elapsed_ms / 1000) as u64,
            estimated_iq,
            iq_unavailable_reason: reason,
            norm_citation: self.norms.as_ref().map(|n| n.citation.clone()),
            items,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn pack() -> IqPack {
        IqPack {
            schema_version: 1,
            kind: "iq".into(),
            id: "test-pack".into(),
            title: "Test fixture".into(),
            test_version: "1".into(),
            source: "Test fixture only".into(),
            license: "CC0".into(),
            items: vec![
                IqItem {
                    id: "a".into(),
                    question: "2 + 2?".into(),
                    answers: vec!["4".into()],
                    explanation: "Four.".into(),
                },
                IqItem {
                    id: "b".into(),
                    question: "Color?".into(),
                    answers: vec!["Dark blue".into()],
                    explanation: "Blue.".into(),
                },
            ],
            norms: None,
        }
    }

    pub fn normed_pack() -> IqPack {
        let mut p = pack();
        p.norms = Some(IqNorms {
            citation: "https://example.org/test-fixture-not-real-norms".into(),
            population: "Synthetic fixture only".into(),
            sample_size: 100,
            age_min: 18,
            age_max: 65,
            language: "en".into(),
            test_version: "1".into(),
            item_set_hash: p.item_set_hash().unwrap(),
            time_limit_seconds: None,
            instructions: "Unaided and unseen.".into(),
            z_scores: vec![-1.0, 0.0, 1.0],
        });
        p
    }

    pub fn participant() -> IqParticipant {
        IqParticipant {
            age: 30,
            language: "en".into(),
            unseen: true,
        }
    }

    #[test]
    fn deviation_iq_uses_norm_z_not_percentage() {
        let p = normed_pack();
        for (answers, iq) in [
            (vec!["wrong".into(), "wrong".into()], 85.0),
            (vec!["4".into(), "".into()], 100.0),
            (vec![" 4 ".into(), " DARK  blue ".into()], 115.0),
        ] {
            assert_eq!(
                p.score("s", &answers, 1000, &participant(), false)
                    .unwrap()
                    .estimated_iq,
                Some(iq)
            );
        }
        assert_eq!(
            pack()
                .score("s", &["4".into()], 1000, &participant(), false)
                .unwrap()
                .estimated_iq,
            None
        );
    }

    #[test]
    fn rejects_mismatched_incomplete_and_invalid_norms() {
        let mut p = normed_pack();
        p.items[0].question.push('!');
        assert!(p.validate().is_err());
        p = normed_pack();
        p.norms.as_mut().unwrap().z_scores.pop();
        assert!(p.validate().is_err());
        p = normed_pack();
        p.norms.as_mut().unwrap().z_scores[0] = f64::NAN;
        assert!(p.validate().is_err());
        p = normed_pack();
        p.norms.as_mut().unwrap().z_scores = vec![1.0, 0.0, -1.0];
        assert!(p.validate().is_err());
        p = normed_pack();
        p.norms.as_mut().unwrap().test_version = "different".into();
        assert!(p.validate().is_err());
    }

    #[test]
    fn eligibility_repetition_and_completion_gate_estimates() {
        let p = normed_pack();
        let answers = vec!["4".into(), "".into()];
        for participant in [
            IqParticipant {
                unseen: false,
                ..participant()
            },
            IqParticipant {
                age: 10,
                ..participant()
            },
            IqParticipant {
                language: "fr".into(),
                ..participant()
            },
        ] {
            assert!(p
                .score("s", &answers, 1000, &participant, false)
                .unwrap()
                .estimated_iq
                .is_none());
        }
        assert!(p
            .score("s", &answers, 1000, &participant(), true)
            .unwrap()
            .estimated_iq
            .is_none());
        assert!(p
            .score("s", &answers[..1], 1000, &participant(), false)
            .unwrap()
            .estimated_iq
            .is_none());
        let mut timed = p;
        timed.norms.as_mut().unwrap().time_limit_seconds = Some(10);
        let result = timed
            .score("s", &answers[..1], 10_000, &participant(), false)
            .unwrap();
        assert_eq!(result.estimated_iq, Some(100.0));
        assert_eq!(result.unanswered, 1);
    }
}
