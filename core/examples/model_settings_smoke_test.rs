//! Manual smoke test for the settings-driven model resolution flow (not run
//! in CI — needs real files on disk). Simulates: user downloads a Whisper
//! model from Hugging Face to some arbitrary folder, "imports" it into
//! Grafium's managed models directory, and transcription then just works
//! with zero explicit path — exactly the flow a Settings screen would
//! trigger via `model_library::import_model` + `WhisperTranscriber::from_config`.
//!
//! Usage:
//!   cargo run -p grafium-core --features media --example model_settings_smoke_test \
//!       -- <downloaded-model-file> <data-dir> <video-or-audio-source>
use std::path::PathBuf;

use grafium_core::media::{fetch_audio, MediaConfig, MediaSource, Transcriber, WhisperTranscriber};
use grafium_core::model_library;

fn main() {
    let mut args = std::env::args().skip(1);
    let downloaded_model = PathBuf::from(args.next().expect("usage: <downloaded-model> <data-dir> <source>"));
    let data_dir = PathBuf::from(args.next().expect("usage: <downloaded-model> <data-dir> <source>"));
    let source_arg = args.next().expect("usage: <downloaded-model> <data-dir> <source>");

    // Step 1: "import" — copy the model the user downloaded wherever they
    // downloaded it to into Grafium's managed models directory.
    let models_dir = model_library::default_models_dir(&data_dir);
    let info = model_library::import_model(&downloaded_model, &models_dir).expect("import failed");
    println!("Imported: {} ({} bytes, kind={:?})", info.file_name, info.size_bytes, info.kind);

    // Step 2: settings has *nothing* configured for `whisper.model` — this
    // is the zero-config path: with exactly one Whisper model imported,
    // it's just picked automatically.
    let config = MediaConfig::default();
    println!("Settings: whisper.model = {:?} (nothing set)", config.whisper.model_ref.model);

    let transcriber = WhisperTranscriber::from_config(&config, &data_dir).expect("model resolution/load failed");
    println!("Resolved + loaded model successfully from settings alone.");

    // Step 3: run the same ingestion + transcription pipeline as before.
    let source = MediaSource::parse(&source_arg);
    let wav_path = fetch_audio(&source, &data_dir.join("work")).expect("ingestion failed");
    let transcript = transcriber.transcribe(&wav_path).expect("transcription failed");

    println!("\n--- full text ---\n{}", transcript.full_text);
}
