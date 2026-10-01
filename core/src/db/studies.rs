//! Durable, graph-local study state: intentionally no foreign keys to the
//! rebuildable pages, blocks, tasks, or flashcard indexes.
use super::{immediate_transaction, Database};
use crate::error::{CoreError, Result};
use chrono::{NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StudyKind {
    Page,
    Book,
    Flashcards,
    Audio,
    Video,
    Youtube,
    Website,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StudyProgress {
    pub position: f64,
    pub total: f64,
    pub anchor: String,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StudyItem {
    pub id: String,
    pub title: String,
    pub topic: String,
    pub kind: StudyKind,
    pub source: String,
    pub progress: StudyProgress,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StudyDay {
    pub item_id: String,
    pub topic: String,
    pub day: String,
    pub seconds: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StudySnapshot {
    pub items: Vec<StudyItem>,
    pub days: Vec<StudyDay>,
    pub topics: Vec<String>,
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    let tx = immediate_transaction(conn)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS study_items (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            topic TEXT NOT NULL,
            kind TEXT NOT NULL,
            source TEXT NOT NULL,
            progress TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS study_days (
            item_id TEXT NOT NULL REFERENCES study_items(id) ON DELETE CASCADE,
            topic TEXT NOT NULL,
            day TEXT NOT NULL,
            seconds REAL NOT NULL CHECK(seconds >= 0),
            PRIMARY KEY(item_id, topic, day)
        );
        CREATE TABLE IF NOT EXISTS study_activity_receipts (
            request_id TEXT PRIMARY KEY,
            item_id TEXT NOT NULL REFERENCES study_items(id) ON DELETE CASCADE,
            payload TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_study_receipts_item
            ON study_activity_receipts(item_id);
        CREATE TABLE IF NOT EXISTS study_topics (
            name TEXT PRIMARY KEY NOT NULL CHECK(name != '')
        );
        INSERT OR IGNORE INTO study_topics(name)
            SELECT topic FROM study_items WHERE topic != ''
            UNION SELECT topic FROM study_days WHERE topic != '';",
    )?;
    tx.commit()?;
    Ok(())
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::Other(format!("Invalid study: {}", message.into()))
}

fn bounded(value: &str, name: &str, max: usize, required: bool) -> Result<()> {
    if value.len() > max
        || value.chars().any(char::is_control)
        || (required && value.trim().is_empty())
    {
        return Err(invalid(format!(
            "{name} must be {}at most {max} bytes, without control characters",
            if required { "nonempty and " } else { "" }
        )));
    }
    Ok(())
}

fn validate_progress(progress: &StudyProgress) -> Result<()> {
    for value in [progress.position, progress.total] {
        if !value.is_finite() || !(0.0..=1e12).contains(&value) {
            return Err(invalid(
                "progress must be finite, nonnegative, and at most 1e12",
            ));
        }
    }
    bounded(&progress.anchor, "progress anchor", 8192, false)?;
    bounded(&progress.label, "progress label", 1024, false)
}

fn normalize_source(kind: StudyKind, source: &str) -> Result<String> {
    bounded(source, "source", 8192, kind != StudyKind::Flashcards)?;
    match kind {
        StudyKind::Website => Ok(web_url(source)?.to_string()),
        StudyKind::Youtube => {
            let mut url = web_url(source)?;
            url.set_scheme("https")
                .map_err(|_| invalid("invalid URL scheme"))?;
            Ok(url.to_string())
        }
        StudyKind::Audio | StudyKind::Video => {
            if source.contains(':') {
                return Ok(web_url(source)?.to_string());
            }
            // The frontend supplies decoded paths. Reject percent escapes so
            // later asset decoding cannot introduce traversal or separators.
            if source.contains(['\\', '?', '#', '%'])
                || source
                    .split('/')
                    .any(|part| matches!(part, "" | "." | ".."))
                || source.trim().is_empty()
            {
                return Err(invalid("media paths must stay relative to the graph"));
            }
            Ok(source.to_string())
        }
        _ => Ok(source.to_string()),
    }
}

fn web_url(source: &str) -> Result<url::Url> {
    let url = url::Url::parse(source).map_err(|_| invalid("source must be an HTTP(S) URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid("source must be an HTTP(S) URL without credentials"));
    }
    Ok(url)
}

fn row_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<StudyItem> {
    fn json<T: serde::de::DeserializeOwned>(
        row: &rusqlite::Row<'_>,
        col: usize,
    ) -> rusqlite::Result<T> {
        let text: String = row.get(col)?;
        serde_json::from_str(&text).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                col,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    }
    Ok(StudyItem {
        id: row.get(0)?,
        title: row.get(1)?,
        topic: row.get(2)?,
        kind: json(row, 3)?,
        source: row.get(4)?,
        progress: json(row, 5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn find_item(conn: &Connection, id: &str) -> Result<Option<StudyItem>> {
    Ok(conn.query_row(
        "SELECT id,title,topic,kind,source,progress,created_at,updated_at FROM study_items WHERE id=?1",
        [id], row_item,
    ).optional()?)
}

impl Database {
    pub fn list_studies(&self) -> Result<StudySnapshot> {
        let conn = self.conn()?;
        let tx = conn.unchecked_transaction()?;
        let items = tx
            .prepare(
                "SELECT id,title,topic,kind,source,progress,created_at,updated_at
             FROM study_items ORDER BY created_at, id",
            )?
            .query_map([], row_item)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let days = tx
            .prepare("SELECT item_id,topic,day,seconds FROM study_days ORDER BY day,item_id,topic")?
            .query_map([], |row| {
                Ok(StudyDay {
                    item_id: row.get(0)?,
                    topic: row.get(1)?,
                    day: row.get(2)?,
                    seconds: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let topics = tx
            .prepare("SELECT name FROM study_topics ORDER BY name")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        tx.commit()?;
        Ok(StudySnapshot {
            items,
            days,
            topics,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> StudyItem {
        StudyItem {
            id: "study-one".into(),
            title: "Reading".into(),
            topic: "Physics".into(),
            kind: StudyKind::Page,
            source: "source-page".into(),
            progress: StudyProgress::default(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn request() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    #[test]
    fn studies_contract_serializes_all_kinds_and_camel_case_fields() -> Result<()> {
        for (kind, spelling) in [
            (StudyKind::Page, "page"),
            (StudyKind::Book, "book"),
            (StudyKind::Flashcards, "flashcards"),
            (StudyKind::Audio, "audio"),
            (StudyKind::Video, "video"),
            (StudyKind::Youtube, "youtube"),
            (StudyKind::Website, "website"),
        ] {
            let mut value = item();
            value.kind = kind;
            let json = serde_json::to_value(&value)?;
            assert_eq!(json["kind"], spelling);
            assert!(json["createdAt"].is_string());
            assert!(json["updatedAt"].is_string());
            assert_eq!(serde_json::from_value::<StudyItem>(json)?, value);
        }
        let json = serde_json::to_value(StudyDay {
            item_id: "one".into(),
            topic: "".into(),
            day: "2026-09-30".into(),
            seconds: 1.5,
        })?;
        assert_eq!(json["itemId"], "one");
        assert!(json.get("item_id").is_none());
        Ok(())
    }

    #[test]
    fn studies_metadata_preserves_progress_and_source_identity() -> Result<()> {
        let db = Database::in_memory()?;
        let original = db.save_study(item())?;
        let progress = StudyProgress {
            position: 32.0,
            total: 100.0,
            anchor: "chapter-3".into(),
            label: "Chapter 3".into(),
        };
        db.record_study_activity(&original.id, 0.0, "2026-09-30", Some(&progress), &request())?;
        let mut stale = original.clone();
        stale.title = "New title".into();
        stale.topic = String::new();
        let updated = db.save_study(stale)?;
        assert_eq!(updated.progress, progress);
        assert_eq!(updated.created_at, original.created_at);
        assert_eq!(updated.title, "New title");
        assert!(db.list_studies()?.days.is_empty());
        for changed in ["kind", "source"] {
            let mut invalid = updated.clone();
            if changed == "kind" {
                invalid.kind = StudyKind::Book;
            } else {
                invalid.source = "other".into();
            }
            assert!(db.save_study(invalid).is_err());
        }
        assert_eq!(db.list_studies()?.items, vec![updated]);
        Ok(())
    }

    #[test]
    fn studies_receipts_are_atomic_idempotent_and_keep_historical_topics() -> Result<()> {
        let db = Database::in_memory()?;
        let mut saved = db.save_study(item())?;
        let token = request();
        let old_progress = StudyProgress {
            position: 1.0,
            ..Default::default()
        };
        db.record_study_activity(&saved.id, 30.5, "2026-09-30", Some(&old_progress), &token)?;
        let next_progress = StudyProgress {
            position: 2.0,
            ..Default::default()
        };
        db.record_study_activity(
            &saved.id,
            15.0,
            "2026-09-30",
            Some(&next_progress),
            &request(),
        )?;
        db.record_study_activity(&saved.id, 30.5, "2026-09-30", Some(&old_progress), &token)?;
        assert!(db
            .record_study_activity(&saved.id, 31.0, "2026-09-30", None, &token)
            .is_err());
        assert_eq!(db.list_studies()?.items[0].progress, next_progress);
        assert_eq!(db.list_studies()?.days[0].seconds, 45.5);
        saved.topic = "Astronomy".into();
        db.save_study(saved.clone())?;
        db.record_study_activity(&saved.id, 20.0, "2026-09-30", None, &request())?;
        let snapshot = db.list_studies()?;
        assert_eq!(snapshot.days.len(), 2);
        assert_eq!(
            snapshot.days.iter().map(|day| day.seconds).sum::<f64>(),
            65.5
        );
        assert!(db
            .record_study_activity("absent", 10.0, "2026-09-30", None, &request())
            .is_err());
        assert_eq!(db.list_studies()?, snapshot);
        let conn = db.conn()?;
        // Force an error after the receipt insert to prove the transaction does
        // not retain a receipt or partial time/progress on failure.
        conn.execute_batch(
            "CREATE TRIGGER fail_study_activity BEFORE INSERT ON study_days
                                BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        )?;
        drop(conn);
        let failed_token = request();
        assert!(db
            .record_study_activity(&saved.id, 10.0, "2026-10-01", None, &failed_token)
            .is_err());
        let conn = db.conn()?;
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM study_activity_receipts WHERE request_id=?1",
                [&failed_token],
                |row| row.get::<_, i64>(0)
            )?,
            0
        );
        drop(conn);
        assert_eq!(db.list_studies()?, snapshot);
        Ok(())
    }

    #[test]
    fn studies_validate_activity_metadata_and_portable_sources() -> Result<()> {
        let db = Database::in_memory()?;
        db.save_study(item())?;
        for seconds in [-1.0, 120.001, f64::NAN, f64::INFINITY] {
            assert!(db
                .record_study_activity("study-one", seconds, "2026-09-30", None, &request())
                .is_err());
        }
        for day in [
            "2026-02-30",
            "2026-9-30",
            "",
            "2026-09-30T00:00:00",
            "+2026-09-30",
        ] {
            assert!(db
                .record_study_activity("study-one", 10.0, day, None, &request())
                .is_err());
        }
        for value in [-1.0, f64::NAN, f64::INFINITY, 1e13] {
            for total in [false, true] {
                let mut progress = StudyProgress::default();
                if total {
                    progress.total = value;
                } else {
                    progress.position = value;
                }
                assert!(db
                    .record_study_activity(
                        "study-one",
                        0.0,
                        "2026-09-30",
                        Some(&progress),
                        &request()
                    )
                    .is_err());
                let mut candidate = item();
                candidate.progress = progress;
                assert!(db.save_study(candidate).is_err());
            }
        }
        let mut bad = item();
        bad.title = " ".into();
        assert!(db.save_study(bad).is_err());
        let mut bad = item();
        bad.topic = "x".repeat(513);
        assert!(db.save_study(bad).is_err());
        for kind in [
            StudyKind::Website,
            StudyKind::Youtube,
            StudyKind::Audio,
            StudyKind::Video,
        ] {
            for source in [
                "javascript:alert(1)",
                "file:///etc/passwd",
                "https://user:pass@example.com",
                "data:text/plain,x",
            ] {
                assert!(
                    normalize_source(kind, source).is_err(),
                    "{kind:?}: {source}"
                );
            }
        }
        for source in [
            "/outside.mp3",
            "../outside.mp3",
            "assets/../../outside.mp3",
            r"C:\outside.mp3",
            r"\\host\share.mp3",
            r"assets\..\outside.mp3",
            r"assets\lecture.mp3",
            "assets/lecture.mp3?download",
            "assets/lecture.mp3#start",
            "assets//lecture.mp3",
            "assets/./lecture.mp3",
            "assets/",
            "./lecture.mp3",
            "assets/%2e%2e/lecture.mp3",
            "assets/%252e%252e/lecture.mp3",
            "assets/%2flecture.mp3",
            "assets/%5clecture.mp3",
        ] {
            assert!(
                normalize_source(StudyKind::Audio, source).is_err(),
                "{source}"
            );
        }
        assert_eq!(
            normalize_source(StudyKind::Website, "http://EXAMPLE.COM/read")?,
            "http://example.com/read"
        );
        assert_eq!(
            normalize_source(StudyKind::Website, "https://EXAMPLE.COM/read")?,
            "https://example.com/read"
        );
        assert_eq!(
            normalize_source(
                StudyKind::Youtube,
                "http://www.youtube.com/watch?v=dQw4w9WgXcQ"
            )?,
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert_eq!(
            normalize_source(StudyKind::Audio, "assets/lecture one.mp3")?,
            "assets/lecture one.mp3"
        );
        assert_eq!(normalize_source(StudyKind::Flashcards, "")?, "");
        assert_eq!(normalize_source(StudyKind::Flashcards, "*")?, "*");
        assert_eq!(
            normalize_source(StudyKind::Flashcards, "exact/tag")?,
            "exact/tag"
        );
        assert!(db.list_studies()?.days.is_empty());
        Ok(())
    }

    #[test]
    fn studies_migration_is_additive_and_corruption_is_reported() -> Result<()> {
        let db = Database::in_memory()?;
        let page = db.create_page("Synthetic source", false)?;
        let conn = db.conn()?;
        // Recreate an old schema without Studies, then run its additive migration.
        conn.execute_batch(
            "DROP TABLE study_activity_receipts; DROP TABLE study_days; DROP TABLE study_items;",
        )?;
        initialize(&conn)?;
        initialize(&conn)?;
        drop(conn);
        assert_eq!(db.get_page_by_id(&page.id)?.title, "Synthetic source");
        db.save_study(item())?;
        let conn = db.conn()?;
        conn.execute("UPDATE study_items SET progress='broken JSON'", [])?;
        drop(conn);
        assert!(db.list_studies().is_err());
        assert!(db.save_study(item()).is_err());
        Ok(())
    }

    #[test]
    fn studies_persist_across_reopen_reindex_and_removal_preserves_sources() -> Result<()> {
        let root = std::env::current_dir()?
            .join("target")
            .join(format!("study-fixture-{}", request()));
        std::fs::create_dir_all(&root)?;
        let result = (|| -> Result<()> {
            let graph = crate::Graph::open(&root)?;
            let page =
                graph.create_page_with_content("Synthetic source", false, "- Original words\n")?;
            let source_path = root.join(page.file_path.as_ref().unwrap());
            let original_bytes = std::fs::read(&source_path)?;
            let mut candidate = item();
            candidate.source = page.id;
            let saved = graph.db.save_study(candidate)?;
            let token = request();
            graph
                .db
                .record_study_activity(&saved.id, 60.0, "2026-09-30", None, &token)?;
            let expected = graph.db.list_studies()?;
            graph.reindex_all()?;
            assert_eq!(graph.db.list_studies()?, expected);
            drop(graph);
            let graph = crate::Graph::open(&root)?;
            assert_eq!(graph.db.list_studies()?, expected);
            graph
                .db
                .record_study_activity(&saved.id, 60.0, "2026-09-30", None, &token)?;
            assert_eq!(graph.db.list_studies()?, expected);
            graph.db.remove_study(&saved.id)?;
            assert!(graph.db.list_studies()?.items.is_empty());
            assert!(graph.db.list_studies()?.days.is_empty());
            assert_eq!(std::fs::read(&source_path)?, original_bytes);
            assert_eq!(
                graph.db.get_page_by_id(&saved.source)?.title,
                "Synthetic source"
            );
            let conn = graph.db.conn()?;
            assert_eq!(
                conn.query_row("SELECT count(*) FROM study_activity_receipts", [], |row| {
                    row.get::<_, i64>(0)
                })?,
                0
            );
            drop(conn);
            drop(graph);
            let graph = crate::Graph::open(&root)?;
            assert!(graph.db.list_studies()?.items.is_empty());
            assert_eq!(graph.db.list_studies()?.topics, vec!["Physics"]);
            Ok(())
        })();
        std::fs::remove_dir_all(&root)?;
        result
    }

    #[test]
    fn studies_concurrent_activity_is_not_lost() -> Result<()> {
        let db = Database::in_memory()?;
        db.save_study(item())?;
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let db = db.clone();
                std::thread::spawn(move || {
                    db.record_study_activity("study-one", 10.0, "2026-09-30", None, &request())
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap()?;
        }
        assert_eq!(db.list_studies()?.days[0].seconds, 80.0);
        Ok(())
    }

    #[test]
    fn studies_are_isolated_and_survive_source_index_deletion() -> Result<()> {
        let first = Database::in_memory()?;
        let second = Database::in_memory()?;
        let page = first.create_page("Source", false)?;
        let mut first_item = item();
        first_item.source = page.id.clone();
        first.save_study(first_item)?;
        first.record_study_activity("study-one", 12.0, "2026-09-30", None, &request())?;
        let expected = first.list_studies()?;
        assert!(second.list_studies()?.items.is_empty());
        let mut second_item = item();
        second_item.title = "Different graph".into();
        second.save_study(second_item)?;
        second.remove_study("study-one")?;
        first.delete_page(&page.id)?;
        assert_eq!(first.list_studies()?, expected);
        Ok(())
    }

    #[test]
    fn studies_topic_migration_preserves_history_after_retag_and_removal() -> Result<()> {
        let db = Database::in_memory()?;
        let mut study = db.save_study(item())?;
        db.record_study_activity(&study.id, 10.0, "2026-09-30", None, &request())?;
        study.topic = "Astronomy".into();
        study = db.save_study(study)?;
        let mut empty = item();
        empty.id = "untagged".into();
        empty.topic.clear();
        db.save_study(empty)?;
        let conn = db.conn()?;
        // Simulate an existing graph before topic-history storage. Physics
        // exists only in activity, while Astronomy exists only on the item.
        conn.execute_batch("DROP TABLE study_topics;")?;
        initialize(&conn)?;
        initialize(&conn)?;
        drop(conn);
        assert_eq!(db.list_studies()?.topics, vec!["Astronomy", "Physics"]);
        study.topic = "Chemistry".into();
        db.save_study(study.clone())?;
        db.remove_study(&study.id)?;
        db.remove_study("untagged")?;
        let snapshot = db.list_studies()?;
        assert!(snapshot.items.is_empty());
        assert!(snapshot.days.is_empty());
        assert_eq!(snapshot.topics, vec!["Astronomy", "Chemistry", "Physics"]);
        let conn = db.conn()?;
        initialize(&conn)?;
        drop(conn);
        assert_eq!(db.list_studies()?, snapshot);
        assert_eq!(
            serde_json::to_value(snapshot)?["topics"],
            serde_json::json!(["Astronomy", "Chemistry", "Physics"])
        );
        assert!(Database::in_memory()?.list_studies()?.topics.is_empty());
        Ok(())
    }

    #[test]
    fn studies_topic_history_and_metadata_are_saved_atomically() -> Result<()> {
        let db = Database::in_memory()?;
        let original = db.save_study(item())?;
        let conn = db.conn()?;
        conn.execute_batch(
            "CREATE TRIGGER fail_topic_save BEFORE INSERT ON study_topics
             BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        )?;
        drop(conn);
        let mut changed = original.clone();
        changed.topic = "Unsaved topic".into();
        changed.title = "Unsaved title".into();
        assert!(db.save_study(changed).is_err());
        let snapshot = db.list_studies()?;
        assert_eq!(snapshot.items, vec![original]);
        assert_eq!(snapshot.topics, vec!["Physics"]);
        Ok(())
    }

    #[test]
    fn studies_local_media_is_included_in_orphan_cleanup_references() -> Result<()> {
        let db = Database::in_memory()?;
        for (id, kind, source) in [
            ("audio", StudyKind::Audio, "assets/lesson one.mp3"),
            ("video", StudyKind::Video, "pages/Course/assets/lesson.mp4"),
            (
                "remote-audio",
                StudyKind::Audio,
                "http://example.com/remote.mp3",
            ),
            (
                "remote-video",
                StudyKind::Video,
                "https://example.com/remote.mp4",
            ),
        ] {
            let mut study = item();
            study.id = id.into();
            study.kind = kind;
            study.source = source.into();
            db.save_study(study)?;
        }
        assert_eq!(db.count_pages()?, 0);
        let references = db.get_all_media_references()?;
        assert_eq!(references.len(), 2);
        assert!(references.contains(&"assets/lesson one.mp3".to_string()));
        assert!(references.contains(&"pages/Course/assets/lesson.mp4".to_string()));
        db.remove_study("audio")?;
        assert_eq!(
            db.get_all_media_references()?,
            vec!["pages/Course/assets/lesson.mp4"]
        );
        Ok(())
    }
}

impl Database {
    /// Existing items only accept metadata edits. A stale form can never
    /// replace progress written by the reader or timer in another window.
    pub fn save_study(&self, mut item: StudyItem) -> Result<StudyItem> {
        bounded(&item.id, "ID", 128, true)?;
        bounded(&item.title, "title", 512, true)?;
        bounded(&item.topic, "topic", 512, false)?;
        validate_progress(&item.progress)?;
        item.title = item.title.trim().to_string();
        item.source = normalize_source(item.kind, &item.source)?;
        let conn = self.conn()?;
        let tx = immediate_transaction(&conn)?;
        let now = Utc::now().to_rfc3339();
        if let Some(existing) = find_item(&tx, &item.id)? {
            if existing.kind != item.kind || existing.source != item.source {
                return Err(invalid(
                    "kind and source cannot change; create a new study instead",
                ));
            }
            tx.execute(
                "UPDATE study_items SET title=?2, topic=?3, updated_at=?4 WHERE id=?1",
                params![item.id, item.title, item.topic, now],
            )?;
            item.progress = existing.progress;
            item.created_at = existing.created_at;
        } else {
            item.created_at = now.clone();
            tx.execute(
                "INSERT INTO study_items(id,title,topic,kind,source,progress,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",
                params![
                    item.id,
                    item.title,
                    item.topic,
                    serde_json::to_string(&item.kind)?,
                    item.source,
                    serde_json::to_string(&item.progress)?,
                    now
                ],
            )?;
        }
        item.updated_at = now;
        if !item.topic.is_empty() {
            tx.execute(
                "INSERT OR IGNORE INTO study_topics(name) VALUES(?1)",
                [&item.topic],
            )?;
        }
        tx.commit()?;
        Ok(item)
    }

    pub fn remove_study(&self, id: &str) -> Result<()> {
        bounded(id, "ID", 128, true)?;
        // Foreign-key cascades remove only this item's time and retry receipts.
        self.conn()?
            .execute("DELETE FROM study_items WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn record_study_activity(
        &self,
        id: &str,
        seconds: f64,
        day: &str,
        progress: Option<&StudyProgress>,
        request_id: &str,
    ) -> Result<()> {
        bounded(id, "ID", 128, true)?;
        if !seconds.is_finite() || !(0.0..=120.0).contains(&seconds) {
            return Err(invalid(
                "activity seconds must be finite and between 0 and 120",
            ));
        }
        if day.len() != 10
            || NaiveDate::parse_from_str(day, "%Y-%m-%d")
                .map(|date| date.format("%Y-%m-%d").to_string() != day)
                .unwrap_or(true)
        {
            return Err(invalid(
                "activity day must be a valid local YYYY-MM-DD date",
            ));
        }
        uuid::Uuid::parse_str(request_id).map_err(|_| invalid("request ID must be a UUID"))?;
        if let Some(progress) = progress {
            validate_progress(progress)?;
        }
        let payload = serde_json::to_string(&(id, seconds, day, progress))?;
        let conn = self.conn()?;
        let tx = immediate_transaction(&conn)?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT payload FROM study_activity_receipts WHERE request_id=?1",
                [request_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            if previous != payload {
                return Err(invalid(
                    "request ID was already used for different activity",
                ));
            }
            return Ok(());
        }
        let item = find_item(&tx, id)?.ok_or_else(|| CoreError::NotFound(format!("Study {id}")))?;
        tx.execute(
            "INSERT INTO study_activity_receipts(request_id,item_id,payload) VALUES(?1,?2,?3)",
            params![request_id, id, payload],
        )?;
        if seconds > 0.0 {
            tx.execute(
                "INSERT INTO study_days(item_id,topic,day,seconds) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(item_id,topic,day) DO UPDATE SET seconds=study_days.seconds+excluded.seconds",
                params![id, item.topic, day, seconds],
            )?;
        }
        let now = Utc::now().to_rfc3339();
        if let Some(progress) = progress {
            tx.execute(
                "UPDATE study_items SET progress=?2,updated_at=?3 WHERE id=?1",
                params![id, serde_json::to_string(progress)?, now],
            )?;
        } else {
            tx.execute(
                "UPDATE study_items SET updated_at=?2 WHERE id=?1",
                params![id, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
