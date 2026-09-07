//! Manual smoke test for the `media` pipeline (not run in CI — needs a real
//! whisper model file and network/binary access). Usage:
//!
//!   cargo run -p grafium-core --features media --example media_smoke_test \
//!       -- <model.bin> <video_url_or_local_file>
use std::path::PathBuf;

use grafium_core::media::{fetch_audio, MediaSource, Transcriber, WhisperTranscriber};

fn main() {
    if grafium_core::ai::worker::is_worker_invocation() {
        std::process::exit(grafium_core::ai::worker::run_from_stdio());
    }
    grafium_core::ai::worker::configure_current_executable()
        .expect("failed to configure native AI worker");
    let model_path = std::env::args()
        .nth(1)
        .expect("usage: media_smoke_test <model.bin> <source>");
    let source_arg = std::env::args()
        .nth(2)
        .expect("usage: media_smoke_test <model.bin> <source>");

    let source = MediaSource::parse(&source_arg);
    println!("Source: {source:?}");

    let workdir = PathBuf::from("/tmp/grafium-media-test/work");
    let wav_path = fetch_audio(&source, &workdir).expect("ingestion failed");
    println!("Normalized WAV: {}", wav_path.display());

    let transcriber = WhisperTranscriber::load(&PathBuf::from(model_path), Some("en"))
        .expect("model load failed");
    let transcript = transcriber
        .transcribe(&wav_path)
        .expect("transcription failed");

    println!("\n--- {} segments ---", transcript.segments.len());
    for seg in &transcript.segments {
        println!("[{:>6}ms - {:>6}ms] {}", seg.start_ms, seg.end_ms, seg.text);
    }
    println!("\n--- full text ---\n{}", transcript.full_text);
}
