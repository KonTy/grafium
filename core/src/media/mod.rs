//! Video/audio ingestion + local transcription pipeline.
//!
//! Turns a YouTube URL, any other `yt-dlp`-supported URL, or a local media
//! file into a plain-text transcript that the rest of Grafium (summarization,
//! fact-checking, or just saving the transcript as a page) can consume.
//!
//! Two concerns are kept deliberately separate, mirroring `ai::traits`'
//! provider-agnostic design:
//!   * [`ingest`] — normalize *any* source (remote URL or local file) into a
//!     16kHz mono WAV on disk. Always available; only shells out to the
//!     already-installed `yt-dlp`/`ffmpeg` binaries, no heavy Rust deps.
//!   * [`transcribe`] — turn that WAV into text via a local Whisper model.
//!     Gated behind the `media` Cargo feature since it pulls in a C++ build
//!     of whisper.cpp that most consumers of this crate (e.g. the Android
//!     build) don't need.
//!
//! Neither module knows about the other — `fetch_audio`'s output (a path to
//! a normalized WAV) is exactly what `Transcriber::transcribe` expects as
//! input, so a caller composes them, but each is independently testable and
//! swappable (e.g. a future cloud STT `Transcriber` impl needs no ingestion
//! changes at all).

pub mod config;
pub mod ingest;
#[cfg(feature = "media")]
pub mod transcribe;

pub use config::{MediaConfig, WhisperSettings};
pub use ingest::{fetch_audio, MediaSource};
#[cfg(feature = "media")]
pub use transcribe::{Transcriber, Transcript, TranscriptSegment, WhisperTranscriber};
