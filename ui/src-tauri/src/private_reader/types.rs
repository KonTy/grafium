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
            ReaderKind::Audio | ReaderKind::Video | ReaderKind::Youtube => {
                if self.locator.is_some()
                    || self.voice_id.is_some()
                    || if book.source_url.is_some() {
                        self.track_id.is_some()
                    } else {
                        !book
                            .tracks
                            .iter()
                            .any(|t| Some(&t.id) == self.track_id.as_ref())
                    }
                {
                    return Err("Position must name a registered local media track, or an offset-only external item".into());
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
    Video,
    Youtube,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReaderProgress {
    pub position: f64,
    pub total: f64,
    pub anchor: String,
    pub label: String,
}

impl ReaderProgress {
    pub fn validate(&self) -> ReaderResult<()> {
        if !self.position.is_finite()
            || !self.total.is_finite()
            || self.position < 0.0
            || self.total < 0.0
            || self.position > 31_536_000_000.0
            || self.total > 31_536_000_000.0
            || (self.total > 0.0 && self.position > self.total)
            || self.anchor.len() > 8192
            || self.label.len() > 1024
            || self.anchor.chars().any(char::is_control)
            || self.label.chars().any(char::is_control)
        {
            return Err("Invalid Library progress".into());
        }
        Ok(())
    }
}

pub fn normalize_link(kind: ReaderKind, value: &str) -> ReaderResult<String> {
    if kind == ReaderKind::Epub
        || !(value.to_ascii_lowercase().starts_with("http://")
            || value.to_ascii_lowercase().starts_with("https://"))
        || value.len() > 8192
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
        || value.contains('\\')
    {
        return Err("Invalid Library link".into());
    }
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid Library URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || value
            .split("://")
            .nth(1)
            .unwrap_or("")
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .contains('@')
    {
        return Err("Library links require HTTP(S) without credentials".into());
    }
    if kind != ReaderKind::Youtube {
        return Ok(url.to_string());
    }
    if url.port().is_some() {
        return Err("Invalid YouTube URL port".into());
    }
    let host = url.host_str().unwrap_or("");
    let segments: Vec<_> = url.path_segments().into_iter().flatten().collect();
    let video = if matches!(host, "youtu.be" | "www.youtu.be") && segments.len() == 1 {
        Some(segments[0].to_owned())
    } else if matches!(
        host,
        "youtube.com"
            | "www.youtube.com"
            | "m.youtube.com"
            | "music.youtube.com"
            | "youtube-nocookie.com"
            | "www.youtube-nocookie.com"
    ) {
        if url.path() == "/watch" {
            url.query_pairs()
                .find(|(key, _)| key == "v")
                .map(|(_, v)| v.into_owned())
        } else if segments.len() == 2 && matches!(segments[0], "embed" | "shorts" | "live") {
            Some(segments[1].to_owned())
        } else {
            None
        }
    } else {
        None
    }
    .ok_or("Use a known YouTube video URL")?;
    if video.len() != 11
        || !video
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return Err("Invalid YouTube video ID".into());
    }
    Ok(format!("https://www.youtube.com/watch?v={video}"))
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
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub last_used_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<ReaderProgress>,
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
            favorite: false,
            last_used_at: 0,
            source_url: None,
            progress: None,
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
