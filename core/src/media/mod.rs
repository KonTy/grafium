//! Video/audio ingestion + local transcription pipeline.
//!
//! Turns a YouTube URL, any other `yt-dlp`-supported URL, or a local media
//! file into a plain-text transcript that the rest of Grafium (summarization,
//! fact-checking, or just saving the transcript as a page) can consume.
//!
//! Three concerns are kept deliberately separate, mirroring `ai::traits`'
//! provider-agnostic design:
//!   * [`captions`] — scrape a transcript that already exists (YouTube
//!     creator captions or auto-captions) via `yt-dlp`, with no
//!     transcription step at all. Always available (only needs the
//!     `yt-dlp` binary); by far the cheapest path when it's available, so
//!     [`fetch_transcript`] always tries this first.
//!   * [`ingest`] — normalize *any* source (remote URL or local file) into a
//!     16kHz mono WAV on disk. Always available; only shells out to the
//!     already-installed `yt-dlp`/`ffmpeg` binaries, no heavy Rust deps.
//!   * [`transcribe`] — turn that WAV into text via a local Whisper model.
//!     Gated behind the `media` Cargo feature since it pulls in a C++ build
//!     of whisper.cpp that most consumers of this crate (e.g. the Android
//!     build) don't need. Used as the fallback when no captions exist.
//!
//! [`notes`] then turns whichever [`types::Transcript`] came out of that
//! into a markdown page, tagged with [`types::TranscriptSource`] so a
//! reader (or a future re-fetch) knows how much to trust it.

pub mod captions;
pub mod config;
pub mod ingest;
pub mod notes;
mod tooling;
#[cfg(feature = "media")]
pub mod transcribe;
pub mod types;

pub use captions::{fetch_captions, fetch_captions_with_progress, fetch_metadata, VideoMetadata};
pub use config::{MediaConfig, WhisperSettings};
pub use ingest::{fetch_audio, fetch_audio_with_progress, MediaSource};
pub use notes::transcript_to_markdown;
#[cfg(feature = "media")]
pub use transcribe::{
    TranscribeProgress, Transcriber, WhisperBackend, WhisperTranscriber, WorkerTranscriber,
};
pub use types::{Transcript, TranscriptSegment, TranscriptSource};

use crate::error::Result;
use std::path::Path;

/// Fetches a transcript for `url`, trying the cheapest option first:
///   1. Creator-uploaded YouTube captions.
///   2. YouTube's auto-generated captions.
///   3. (only when built with the `media` feature) Downloading audio and
///      running it through `transcriber` locally.
///
/// Returns `Ok(None)` — not an error — when no captions exist and this
/// build doesn't have local Whisper transcription compiled in, so the
/// caller can show a clear "no transcript available in this build" message
/// rather than a raw error.
#[cfg(not(feature = "media"))]
pub fn fetch_transcript(
    url: &str,
    workdir: &Path,
    lang: &str,
) -> Result<Option<(Transcript, TranscriptSource)>> {
    captions::fetch_captions(url, workdir, lang)
}

#[cfg(not(feature = "media"))]
pub fn fetch_transcript_with_progress(
    url: &str,
    workdir: &Path,
    lang: &str,
    on_progress: &mut dyn FnMut(&str),
) -> Result<Option<(Transcript, TranscriptSource)>> {
    captions::fetch_captions_with_progress(url, workdir, lang, on_progress)
}

/// Same as the non-`media` version, but falls back to local Whisper
/// transcription (via `transcriber`) when no captions exist, so this
/// always returns `Some` for any reachable, non-empty audio/video source.
#[cfg(feature = "media")]
pub fn fetch_transcript(
    url: &str,
    workdir: &Path,
    lang: &str,
    transcriber: &dyn Transcriber,
) -> Result<(Transcript, TranscriptSource)> {
    fetch_transcript_with_progress(url, workdir, lang, transcriber, &mut |_| {})
}

/// Same as [`fetch_transcript`], but reports each stage (checking captions,
/// downloading audio, transcribing) through `on_progress` so a caller can
/// surface live status instead of a silent multi-second/minute wait.
#[cfg(feature = "media")]
pub fn fetch_transcript_with_progress(
    url: &str,
    workdir: &Path,
    lang: &str,
    transcriber: &dyn Transcriber,
    on_progress: &mut dyn FnMut(&str),
) -> Result<(Transcript, TranscriptSource)> {
    if let Some(result) = captions::fetch_captions_with_progress(url, workdir, lang, on_progress)? {
        return Ok(result);
    }
    on_progress("No captions available — falling back to local transcription.");
    let source = MediaSource::parse(url);
    let wav_path = ingest::fetch_audio_with_progress(&source, workdir, on_progress)?;
    // Route whisper.cpp's own periodic percent updates through the
    // caller's plain-text progress channel so the UI can show
    // "Transcribing 3:24 of audio with Whisper… 42%" rather than a
    // single-message "…(this can take a while)…" followed by minutes
    // of silence.
    let transcript = transcriber.transcribe_with_progress(&wav_path, &mut |progress| {
        on_progress(&progress.message);
    })?;
    on_progress("Transcription complete.");
    let _ = std::fs::remove_file(&wav_path);
    Ok((transcript, TranscriptSource::Whisper))
}
