//! Shared transcript data shapes — deliberately independent of *how* a
//! transcript was produced (scraped YouTube captions via `captions`, or
//! run through whisper.cpp via the feature-gated `transcribe`) so both
//! producers, and every consumer (the markdown note writer in `notes`,
//! summarization, fact-checking, ...) agree on one shape. Kept in its own
//! unconditional module (no Cargo feature gate) specifically so
//! `captions.rs` — which needs zero heavy dependencies beyond the
//! already-required `yt-dlp` binary — never has to pull in whisper.cpp
//! just to describe the text it scraped.

/// One timestamped span of the transcript.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TranscriptSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// Full transcription result. Summarization/fact-checking typically only
/// need `full_text`; `segments` are kept for anything that wants to jump to
/// a moment in the source video (e.g. citing "at 3:42 they claim...").
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Transcript {
    pub segments: Vec<TranscriptSegment>,
    pub full_text: String,
}

impl Transcript {
    /// Builds `full_text` by joining segment text with single spaces,
    /// trimming, and collapsing runs of whitespace — the same normalization
    /// both `captions::parse_vtt` and whisper.cpp output should apply so
    /// `full_text` reads as continuous prose regardless of which producer
    /// made it.
    pub fn from_segments(segments: Vec<TranscriptSegment>) -> Self {
        let full_text = segments
            .iter()
            .map(|s| s.text.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            segments,
            full_text,
        }
    }
}

/// Where a `Transcript` came from — surfaced in the generated markdown note
/// so a reader knows how much to trust it (creator-written captions are
/// generally more accurate than YouTube's auto-generated ones or a local
/// Whisper run) and so re-fetching can prefer upgrading a lower-confidence
/// source later (e.g. re-run Whisper over an auto-caption-only note).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TranscriptSource {
    /// Captions written/uploaded by the video's creator.
    CreatorCaptions,
    /// YouTube's own auto-generated captions.
    AutoCaptions,
    /// Transcribed locally via whisper.cpp (see `transcribe::WhisperTranscriber`).
    Whisper,
}

impl TranscriptSource {
    /// A short, human-readable label used in the generated note's
    /// frontmatter (`transcript_source: ...`).
    pub fn label(&self) -> &'static str {
        match self {
            TranscriptSource::CreatorCaptions => "youtube_captions",
            TranscriptSource::AutoCaptions => "youtube_auto_captions",
            TranscriptSource::Whisper => "whisper",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_segments_joins_trims_and_skips_empty_text() {
        let transcript = Transcript::from_segments(vec![
            TranscriptSegment {
                start_ms: 0,
                end_ms: 1000,
                text: "  Hello ".to_string(),
            },
            TranscriptSegment {
                start_ms: 1000,
                end_ms: 1500,
                text: "".to_string(),
            },
            TranscriptSegment {
                start_ms: 1500,
                end_ms: 2500,
                text: "world.".to_string(),
            },
        ]);
        assert_eq!(transcript.full_text, "Hello world.");
        assert_eq!(transcript.segments.len(), 3);
    }

    #[test]
    fn transcript_source_labels_are_stable() {
        assert_eq!(
            TranscriptSource::CreatorCaptions.label(),
            "youtube_captions"
        );
        assert_eq!(
            TranscriptSource::AutoCaptions.label(),
            "youtube_auto_captions"
        );
        assert_eq!(TranscriptSource::Whisper.label(), "whisper");
    }
}
