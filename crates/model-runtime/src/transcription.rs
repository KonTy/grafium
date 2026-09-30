//! Backend-independent transcript data and transcription contracts.

use crate::error::Result;
use std::path::Path;

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
    /// trimming and skipping empty segments — the same normalization
    /// both caption readers and native transcription should apply so
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

/// Anything that can turn a host-prepared 16kHz mono WAV file into a [`Transcript`].
pub trait Transcriber {
    fn transcribe(&self, wav_path: &Path) -> Result<Transcript> {
        self.transcribe_with_progress(wav_path, &mut |_| {})
    }

    /// Same as [`Self::transcribe`], but reports periodic progress
    /// (0-100 percent + a human-readable message) so a caller can show
    /// live status during what can be a multi-minute inference pass.
    ///
    /// Implementations must supply this operation; the convenience method
    /// above delegates here without mutually recursive default methods.
    fn transcribe_with_progress(
        &self,
        wav_path: &Path,
        on_progress: &mut dyn FnMut(TranscribeProgress),
    ) -> Result<Transcript>;
}

/// Progress update from a running transcription pass. `percent` is a
/// best-effort 0-100 completion estimate, and `message` is a
/// human-readable label the UI can show verbatim.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TranscribeProgress {
    pub percent: u8,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convenience_method_preserves_transcriber_failures() {
        struct Failed;
        impl Transcriber for Failed {
            fn transcribe_with_progress(
                &self,
                _: &Path,
                _: &mut dyn FnMut(TranscribeProgress),
            ) -> Result<Transcript> {
                Err(crate::error::RuntimeError::Other(
                    "synthetic transcription failure".into(),
                ))
            }
        }
        assert!(Failed
            .transcribe(Path::new("synthetic.wav"))
            .unwrap_err()
            .to_string()
            .contains("synthetic transcription failure"));
    }

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
}
