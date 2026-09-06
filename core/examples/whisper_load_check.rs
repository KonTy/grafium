//! Checks that a Whisper model still initializes when llama.cpp's Vulkan
//! backend is compiled into the same process.
//!
//! Exists because whisper-rs statically links its own copy of GGML while
//! `llm-local` links llama.cpp's dynamically, so both end up in one binary.
//! Which copy wins symbol resolution decides whether *either* can reach the
//! GPU, and getting it wrong is silent: the app keeps working and merely runs
//! inference on the CPU an order of magnitude slower. This is the cheap
//! canary for the Whisper half of that tradeoff.
//!
//! Usage:
//!   cargo run -p grafium-core --release --features media-vulkan,llm-local-vulkan \
//!       --example whisper_load_check -- <model.bin>
use std::path::PathBuf;

use grafium_core::media::WhisperTranscriber;

fn main() {
    let model_path = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: whisper_load_check <model.bin>"),
    );

    match WhisperTranscriber::load(&model_path, Some("en")) {
        Ok(_) => println!("OK: whisper context created for {}", model_path.display()),
        Err(err) => {
            eprintln!("FAIL: whisper model failed to load: {err}");
            std::process::exit(1);
        }
    }
}
