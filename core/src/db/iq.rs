use super::{immediate_transaction, Database};
use crate::error::Result;
use crate::iq::*;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;
use uuid::Uuid;

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS iq_packs(id TEXT PRIMARY KEY, payload TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS iq_sessions(
            id TEXT PRIMARY KEY, pack TEXT NOT NULL, participant TEXT NOT NULL,
            started_at INTEGER NOT NULL, answers TEXT NOT NULL, result TEXT
         );",
    )?;
    Ok(())
}

impl Database {
    pub fn import_iq_pack(&self, pack: &IqPack) -> Result<IqPackSummary> {
        pack.validate()?;
        let payload = serde_json::to_string(pack)?;
        let conn = self.conn()?;
        let tx = immediate_transaction(&conn)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload FROM iq_packs WHERE id=?1",
                [&pack.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing != payload {
                return Err(invalid("this pack ID already exists with different content; import a new ID instead of overwriting it"));
            }
        } else {
            tx.execute(
                "INSERT INTO iq_packs(id,payload) VALUES(?1,?2)",
                params![pack.id, payload],
            )?;
        }
        tx.commit()?;
        Ok(pack.summary())
    }

    pub fn list_iq_packs(&self) -> Result<Vec<IqPackSummary>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT payload FROM iq_packs ORDER BY id")?;
        let payloads = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        payloads
            .iter()
            .map(|payload| {
                let pack: IqPack = serde_json::from_str(payload)?;
                pack.validate()?;
                Ok(pack.summary())
            })
            .collect()
    }

    pub fn start_iq_session(
        &self,
        pack_id: &str,
        participant: &IqParticipant,
        graph_path: &str,
    ) -> Result<IqSession> {
        if participant.age > 120 || participant.language.len() > 128 {
            return Err(invalid("participant age or language"));
        }
        let conn = self.conn()?;
        let payload: String =
            conn.query_row("SELECT payload FROM iq_packs WHERE id=?1", [pack_id], |r| {
                r.get(0)
            })?;
        let pack: IqPack = serde_json::from_str(&payload)?;
        pack.validate()?;
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO iq_sessions(id,pack,participant,started_at,answers) VALUES(?1,?2,?3,?4,'[]')",
            params![id, payload, serde_json::to_string(participant)?, now],
        )?;
        Ok(IqSession {
            id,
            graph_path: graph_path.into(),
            title: pack.title,
            questions: pack.items.into_iter().map(|i| i.question).collect(),
            expires_at: pack
                .norms
                .and_then(|n| n.time_limit_seconds)
                .map(|s| now + i64::from(s) * 1000),
        })
    }

    pub fn answer_iq_item(&self, session_id: &str, index: usize, answer: &str) -> Result<()> {
        if answer.len() > 8192 {
            return Err(invalid("answer is too long"));
        }
        let conn = self.conn()?;
        let tx = immediate_transaction(&conn)?;
        let (payload, started, stored, result): (String, i64, String, Option<String>) = tx
            .query_row(
                "SELECT pack,started_at,answers,result FROM iq_sessions WHERE id=?1",
                [session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
        let pack: IqPack = serde_json::from_str(&payload)?;
        let mut answers: Vec<String> = serde_json::from_str(&stored)?;
        // A retry of an already committed response must not count it twice.
        if answers.get(index).is_some_and(|saved| saved == answer) {
            return Ok(());
        }
        let elapsed = Utc::now().timestamp_millis() - started;
        if result.is_some()
            || index != answers.len()
            || index >= pack.items.len()
            || elapsed < 0
            || pack
                .norms
                .as_ref()
                .and_then(|n| n.time_limit_seconds)
                .is_some_and(|s| elapsed >= i64::from(s) * 1000)
        {
            return Err(invalid(
                "assessment finished, answer out of order, or time limit reached",
            ));
        }
        answers.push(answer.into());
        tx.execute(
            "UPDATE iq_sessions SET answers=?2 WHERE id=?1",
            params![session_id, serde_json::to_string(&answers)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn finish_iq_session(&self, session_id: &str) -> Result<IqResult> {
        let conn = self.conn()?;
        let tx = immediate_transaction(&conn)?;
        let (payload, participant, started, stored, existing): (
            String,
            String,
            i64,
            String,
            Option<String>,
        ) = tx.query_row(
            "SELECT pack,participant,started_at,answers,result FROM iq_sessions WHERE id=?1",
            [session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        if let Some(existing) = existing {
            return Ok(serde_json::from_str(&existing)?);
        }
        let pack: IqPack = serde_json::from_str(&payload)?;
        let answers: Vec<String> = serde_json::from_str(&stored)?;
        let participant: IqParticipant = serde_json::from_str(&participant)?;
        let mut stmt = tx.prepare("SELECT pack FROM iq_sessions WHERE id!=?1")?;
        let prior = stmt
            .query_map([session_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut repeated = false;
        let questions: HashSet<_> = pack
            .items
            .iter()
            .map(|i| normalized_answer(&i.question))
            .collect();
        for payload in prior {
            let prior_pack: IqPack = serde_json::from_str(&payload)?;
            if prior_pack
                .items
                .iter()
                .any(|old| questions.contains(&normalized_answer(&old.question)))
            {
                repeated = true;
                break;
            }
        }
        let elapsed = Utc::now().timestamp_millis() - started;
        let result = pack.score(session_id, &answers, elapsed, &participant, repeated)?;
        tx.execute(
            "UPDATE iq_sessions SET result=?2 WHERE id=?1",
            params![session_id, serde_json::to_string(&result)?],
        )?;
        drop(stmt);
        tx.commit()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iq::tests::{normed_pack, pack, participant};

    #[test]
    fn persisted_answers_are_idempotent_and_never_grade_review_cards() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Review fixture", false).unwrap();
        let block = db
            .create_block(
                &page.id,
                None,
                0,
                "Review :: Answer",
                crate::models::BlockType::Flashcard,
                serde_json::json!({}),
            )
            .unwrap();
        let card = db
            .upsert_flashcard(&block.id, "Review", "Answer", &[])
            .unwrap();
        db.grade_flashcard(&card.id, 4).unwrap();
        let before = serde_json::to_value(db.list_flashcards(10, 0).unwrap()).unwrap();
        db.import_iq_pack(&normed_pack()).unwrap();
        let s = db
            .start_iq_session("test-pack", &participant(), "/graph")
            .unwrap();
        db.answer_iq_item(&s.id, 0, "4").unwrap();
        db.answer_iq_item(&s.id, 0, "4").unwrap();
        assert!(db.answer_iq_item(&s.id, 0, "5").is_err());
        db.answer_iq_item(&s.id, 1, "dark blue").unwrap();
        let result = db.finish_iq_session(&s.id).unwrap();
        assert_eq!(result.estimated_iq, Some(115.0));
        assert_eq!(db.finish_iq_session(&s.id).unwrap().correct, 2);
        assert_eq!(
            serde_json::to_value(db.list_flashcards(10, 0).unwrap()).unwrap(),
            before
        );
        let repeated = db
            .start_iq_session("test-pack", &participant(), "/graph")
            .unwrap();
        db.answer_iq_item(&repeated.id, 0, "4").unwrap();
        db.answer_iq_item(&repeated.id, 1, "dark blue").unwrap();
        assert!(db
            .finish_iq_session(&repeated.id)
            .unwrap()
            .estimated_iq
            .is_none());
        let mut changed = normed_pack();
        changed.title = "Changed".into();
        assert!(db.import_iq_pack(&changed).is_err());
    }

    #[test]
    fn expired_answers_are_rejected_and_omissions_score_zero() {
        let db = Database::in_memory().unwrap();
        let mut p = normed_pack();
        p.norms.as_mut().unwrap().time_limit_seconds = Some(1);
        db.import_iq_pack(&p).unwrap();
        let s = db
            .start_iq_session(&p.id, &participant(), "/graph")
            .unwrap();
        db.conn()
            .unwrap()
            .execute(
                "UPDATE iq_sessions SET started_at=started_at-2000 WHERE id=?1",
                [&s.id],
            )
            .unwrap();
        assert!(db.answer_iq_item(&s.id, 0, "4").is_err());
        let result = db.finish_iq_session(&s.id).unwrap();
        assert_eq!(result.unanswered, 2);
        assert_eq!(result.estimated_iq, Some(85.0));
    }

    #[test]
    fn packs_and_attempts_survive_reopening_database() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.sqlite");
        let db = Database::new(&path).unwrap();
        db.import_iq_pack(&pack()).unwrap();
        let s = db
            .start_iq_session("test-pack", &participant(), "/graph")
            .unwrap();
        db.answer_iq_item(&s.id, 0, "4").unwrap();
        drop(db);
        let db = Database::new(&path).unwrap();
        assert_eq!(db.list_iq_packs().unwrap().len(), 1);
        assert_eq!(db.finish_iq_session(&s.id).unwrap().correct, 1);
    }

    #[test]
    fn opening_overlapping_packs_blocks_estimate_even_without_submitted_answers() {
        let db = Database::in_memory().unwrap();
        let p = normed_pack();
        db.import_iq_pack(&p).unwrap();
        let s = db
            .start_iq_session(&p.id, &participant(), "/graph")
            .unwrap();
        let mut overlapping = p.clone();
        overlapping.id = "different-id".into();
        db.import_iq_pack(&overlapping).unwrap();
        db.start_iq_session(&overlapping.id, &participant(), "/graph")
            .unwrap();
        db.answer_iq_item(&s.id, 0, "4").unwrap();
        db.answer_iq_item(&s.id, 1, "dark blue").unwrap();
        assert!(db.finish_iq_session(&s.id).unwrap().estimated_iq.is_none());
    }
}
