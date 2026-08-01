//! Media ingestion: turn a remote URL or local file into a normalized
//! 16kHz mono WAV ready for transcription.
//!
//! Both steps shell out to external tools rather than reimplementing them in
//! Rust — both are the de facto standard for their job and are already
//! present in this environment:
//!   * `yt-dlp` — downloads/extracts audio from YouTube and hundreds of
//!     other sites, handling all their quirks so we don't have to.
//!   * `ffmpeg` — normalizes *any* input (downloaded or local, whatever
//!     container/codec it's in) to the exact PCM format whisper.cpp expects,
//!     so `transcribe` never has to deal with format differences between a
//!     remote video and a local file.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{CoreError, Result};

/// Where the source media came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaSource {
    /// A remote URL (YouTube or any site `yt-dlp` supports).
    Url(String),
    /// A file already on disk.
    LocalFile(PathBuf),
}

impl MediaSource {
    /// Classifies `input` as a URL or local path, whichever it looks like.
    pub fn parse(input: &str) -> Self {
        let trimmed = input.trim();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            MediaSource::Url(trimmed.to_string())
        } else {
            MediaSource::LocalFile(PathBuf::from(trimmed))
        }
    }
}

/// Downloads (if remote) and normalizes `source` into a 16kHz mono 16-bit
/// PCM WAV file inside `workdir`, returning its path.
///
/// `workdir` is created if it doesn't exist and is safe to reuse across
/// multiple calls — each call gets a unique filename so concurrent/repeated
/// ingestions never collide.
pub fn fetch_audio(source: &MediaSource, workdir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(workdir)?;
    let job_id = uuid::Uuid::new_v4();

    let raw_path = match source {
        MediaSource::Url(url) => download_audio(url, workdir, &job_id.to_string())?,
        MediaSource::LocalFile(path) => {
            if !path.exists() {
                return Err(CoreError::NotFound(format!(
                    "Media file not found: {}",
                    path.display()
                )));
            }
            path.clone()
        }
    };

    let wav_path = workdir.join(format!("{job_id}.wav"));
    normalize_to_wav(&raw_path, &wav_path)?;

    // Clean up the intermediate download — never the caller's own local file.
    if matches!(source, MediaSource::Url(_)) {
        let _ = std::fs::remove_file(&raw_path);
    }

    Ok(wav_path)
}

/// Downloads the best available audio stream for `url` via `yt-dlp`, letting
/// it choose the container (m4a/opus/webm/...); `normalize_to_wav` handles
/// turning whatever comes out into whisper's expected format, so this
/// function doesn't need to care which one it got.
fn download_audio(url: &str, workdir: &Path, job_id: &str) -> Result<PathBuf> {
    let output_template = workdir.join(format!("{job_id}.%(ext)s"));
    let output = Command::new("yt-dlp")
        .args([
            "-f",
            "bestaudio/best",
            "--no-playlist",
            "-o",
            &output_template.to_string_lossy(),
            url,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            CoreError::Other(format!("failed to launch yt-dlp (is it installed?): {e}"))
        })?;
    if !output.status.success() {
        return Err(CoreError::Other(format!(
            "yt-dlp failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    // yt-dlp picks the extension itself; find whatever file it produced.
    let prefix = format!("{job_id}.");
    std::fs::read_dir(workdir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(&prefix))
                .unwrap_or(false)
        })
        .ok_or_else(|| CoreError::Other("yt-dlp did not produce an output file".to_string()))
}

/// Re-encodes `input` (any format ffmpeg understands) to 16kHz mono 16-bit
/// PCM WAV at `output` — the exact format whisper.cpp requires.
fn normalize_to_wav(input: &Path, output: &Path) -> Result<()> {
    let result = Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            &input.to_string_lossy(),
            "-ar",
            "16000",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            &output.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            CoreError::Other(format!("failed to launch ffmpeg (is it installed?): {e}"))
        })?;
    if !result.status.success() {
        return Err(CoreError::Other(format!(
            "ffmpeg failed ({}): {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_classifies_urls_vs_local_paths() {
        assert_eq!(
            MediaSource::parse("https://www.youtube.com/watch?v=abc123"),
            MediaSource::Url("https://www.youtube.com/watch?v=abc123".to_string())
        );
        assert_eq!(
            MediaSource::parse("http://example.com/video.mp4"),
            MediaSource::Url("http://example.com/video.mp4".to_string())
        );
        assert_eq!(
            MediaSource::parse("/home/user/video.mp4"),
            MediaSource::LocalFile(PathBuf::from("/home/user/video.mp4"))
        );
        assert_eq!(
            MediaSource::parse("  ./relative/clip.mov  "),
            MediaSource::LocalFile(PathBuf::from("./relative/clip.mov"))
        );
    }

    #[test]
    fn fetch_audio_reports_missing_local_file() {
        let workdir = tempfile::tempdir().unwrap();
        let source = MediaSource::LocalFile(PathBuf::from("/nonexistent/path/video.mp4"));
        let result = fetch_audio(&source, workdir.path());
        assert!(matches!(result, Err(CoreError::NotFound(_))));
    }
}
