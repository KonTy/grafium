//! Grafium transcript provenance; shared transcript values live in model-runtime.
pub use model_runtime::transcription::{Transcript, TranscriptSegment};

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
