use serde::{Deserialize, Serialize};

pub type ReaderResult<T> = Result<T, String>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EpubLocation {
    pub kind: String,
    pub cfi: String,
    pub renderer_version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReaderPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
    pub offset_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<EpubLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_id: Option<String>,
}

impl ReaderPosition {
    pub fn validate(&self, book: &ReaderBook) -> ReaderResult<()> {
        if self.offset_ms > 31_536_000_000 {
            return Err("Reader position is out of range".into());
        }
        if self.voice_id.as_ref().is_some_and(|id| {
            id.is_empty()
                || id.len() > 96
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        }) {
            return Err("Reader position voice ID is invalid".into());
        }
        match book.kind {
            ReaderKind::Audio => {
                if self.locator.is_some()
                    || self.voice_id.is_some()
                    || !book
                        .tracks
                        .iter()
                        .any(|t| Some(&t.id) == self.track_id.as_ref())
                {
                    return Err("Position must name a registered audio track".into());
                }
            }
            ReaderKind::Epub => {
                let loc = self
                    .locator
                    .as_ref()
                    .ok_or("EPUB position needs a locator")?;
                if self.track_id.is_some()
                    || loc.kind != "epub"
                    || !loc.cfi.starts_with("epubcfi(")
                    || !loc.cfi.ends_with(')')
                    || loc.cfi.len() > 8192
                    || loc.cfi.chars().any(char::is_control)
                    || loc.renderer_version.trim().is_empty()
                    || loc.renderer_version.len() > 128
                    || loc.renderer_version.chars().any(char::is_control)
                {
                    return Err("Invalid EPUB locator".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReaderKind {
    Audio,
    Epub,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReaderTrack {
    pub id: String,
    pub title: String,
    pub relative_path: String,
    pub available: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReaderBookmark {
    pub id: String,
    pub book_id: String,
    pub position: ReaderPosition,
    pub created_at: u64,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReaderBook {
    pub id: String,
    pub title: String,
    pub kind: ReaderKind,
    pub available: bool,
    pub tracks: Vec<ReaderTrack>,
    pub position: Option<ReaderPosition>,
    pub bookmarks: Vec<ReaderBookmark>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderSnapshot {
    pub library_path: Option<String>,
    pub books: Vec<ReaderBook>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub(crate) fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epub_locator_matches_the_isolated_reader_wire_contract() {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/private-reader-position.json"
        ))
        .unwrap();
        let position: ReaderPosition = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(position).unwrap(), value);

        let mut incompatible = value;
        let locator = incompatible["locator"].as_object_mut().unwrap();
        let kind = locator.remove("kind").unwrap();
        locator.insert("type".into(), kind);
        assert!(serde_json::from_value::<ReaderPosition>(incompatible).is_err());
    }

    #[test]
    fn voice_identity_is_optional_bounded_and_preserved() {
        let mut wire: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/private-reader-position.json"
        ))
        .unwrap();
        wire["voiceId"] = "local-voice_1".into();
        let position: ReaderPosition = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&position).unwrap(), wire);
        let book = ReaderBook {
            id: "book".into(),
            title: "Synthetic EPUB".into(),
            kind: ReaderKind::Epub,
            available: true,
            tracks: vec![],
            position: None,
            bookmarks: vec![],
            error: None,
        };
        position.validate(&book).unwrap();
        for invalid in ["", "../voice", "voice\n", "voice.id", &"a".repeat(97)] {
            let mut invalid_position = position.clone();
            invalid_position.voice_id = Some(invalid.into());
            assert!(invalid_position.validate(&book).is_err());
        }
        let mut audio = book;
        audio.kind = ReaderKind::Audio;
        audio.tracks.push(ReaderTrack {
            id: "track".into(),
            title: "Track".into(),
            relative_path: "track.mp3".into(),
            available: true,
        });
        let mut audio_position = position;
        audio_position.locator = None;
        audio_position.track_id = Some("track".into());
        assert!(audio_position.validate(&audio).is_err());
        audio_position.voice_id = None;
        audio_position.validate(&audio).unwrap();
    }
}
