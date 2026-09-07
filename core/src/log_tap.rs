//! A small in-memory ring buffer for the app's `tracing` events, so
//! diagnostics-friendly UI paths (e.g. the media-import progress dialog)
//! can surface the actual whisper.cpp / GGML / llama.cpp error output
//! *verbatim* instead of just knowing that "something failed".
//!
//! # Why this exists
//! whisper.cpp and llama.cpp both emit their real error and status
//! messages through `whisper_rs::install_logging_hooks()` /
//! `llama_cpp_2` hooks that route native log lines through the Rust
//! `tracing` crate. Those messages contain the information we most
//! need for a good user experience — e.g. "ggml_vulkan: no supported
//! devices found" is what tells us to warn the user that we're about
//! to fall back to CPU inference, and "whisper_backend_init_gpu: using
//! Vulkan backend" tells us the GPU is live. Without a way to inspect
//! those events at the call site, the UI can only ever show generic
//! "transcription failed" / "please wait" messages.
//!
//! The tap is deliberately just a *buffer*: it doesn't format, filter,
//! or interpret anything — that's the job of the caller
//! (`media::transcribe`, `ai::providers::local_llm`, etc.), which knows
//! which log lines are relevant to its own error surface.
//!
//! # Wiring
//! `ui/src-tauri/src/lib.rs` installs a `tracing_subscriber::Layer`
//! that calls [`record`] on every event, alongside the existing `fmt`
//! layer that still writes to stderr. That means every event goes to
//! stderr *and* to the tap in parallel — no observability loss for
//! developers, plus the ability to surface it to the user.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// One captured `tracing` event. Kept intentionally small so we can
/// hold a rolling window of the last several hundred events without a
/// meaningful memory hit.
#[derive(Debug, Clone)]
pub struct TapEvent {
    pub at: Instant,
    pub level: TapLevel,
    /// The `tracing` target the event came from — e.g. `"whisper_rs"`,
    /// `"llama_cpp_2"`, `"grafium_core::ai::references"`.
    pub target: String,
    pub message: String,
}

/// Mirror of `tracing::Level` that doesn't require `tracing` as a
/// public dependency of any downstream API using `TapEvent`. The
/// mapping is stable (`ERROR` > `WARN` > `INFO` > `DEBUG` > `TRACE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Cap on the number of events retained. Old events are dropped from
/// the front once this many are in the buffer. Sized to comfortably
/// hold the entire chatty init of both whisper.cpp *and* llama.cpp
/// without truncating any of it — those two together emit a few
/// dozen lines at load time, plus a handful per generation.
const MAX_EVENTS: usize = 2048;

fn buffer() -> &'static Mutex<VecDeque<TapEvent>> {
    static BUFFER: OnceLock<Mutex<VecDeque<TapEvent>>> = OnceLock::new();
    BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_EVENTS)))
}

/// Appends one event to the tap buffer, evicting the oldest entry
/// when the buffer is full. Cheap enough to call from a `tracing`
/// `on_event` hook that fires on every native log line.
pub fn record(level: TapLevel, target: &str, message: &str) {
    if let Ok(mut buf) = buffer().lock() {
        if buf.len() == MAX_EVENTS {
            buf.pop_front();
        }
        buf.push_back(TapEvent {
            at: Instant::now(),
            level,
            target: target.to_string(),
            message: message.to_string(),
        });
    }
}

/// Returns a snapshot of every event captured at or after `since`.
/// Order is oldest-first (matches emission order), which is what
/// callers want when interpreting a step-by-step init log.
pub fn snapshot_since(since: Instant) -> Vec<TapEvent> {
    match buffer().lock() {
        Ok(buf) => buf.iter().filter(|e| e.at >= since).cloned().collect(),
        Err(_) => Vec::new(),
    }
}

/// Returns a snapshot of every event captured at or after `since`
/// whose target starts with any of `target_prefixes`. Useful for
/// isolating just whisper.cpp events (target prefix `"whisper"`),
/// just GGML events (target prefix `"ggml"`), etc., without pulling
/// in unrelated infra chatter (e.g. `"reqwest"`, `"h2"`).
pub fn snapshot_since_targets(since: Instant, target_prefixes: &[&str]) -> Vec<TapEvent> {
    match buffer().lock() {
        Ok(buf) => buf
            .iter()
            .filter(|e| {
                e.at >= since
                    && target_prefixes
                        .iter()
                        .any(|p| e.target.starts_with(p))
            })
            .cloned()
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn snapshot_since_returns_only_events_after_the_cutoff() {
        // Rely on a lower cutoff than the first record's timestamp to
        // include it, then a higher cutoff to exclude it — proves the
        // `since` filter is inclusive and time-monotonic.
        let cutoff_low = Instant::now() - Duration::from_secs(60);
        record(TapLevel::Info, "grafium_core::test", "hello");
        std::thread::sleep(Duration::from_millis(2));
        let mid = Instant::now();
        std::thread::sleep(Duration::from_millis(2));
        record(TapLevel::Warn, "grafium_core::test", "world");

        let all = snapshot_since(cutoff_low);
        // At least both records; other tests running in parallel may
        // have added more, so match by "contains" rather than exact eq.
        assert!(all.iter().any(|e| e.message == "hello"));
        assert!(all.iter().any(|e| e.message == "world"));

        let after_mid = snapshot_since(mid);
        assert!(after_mid.iter().all(|e| e.message != "hello"));
        assert!(after_mid.iter().any(|e| e.message == "world"));
    }

    #[test]
    fn snapshot_since_targets_filters_by_prefix() {
        let cutoff = Instant::now() - Duration::from_secs(60);
        record(TapLevel::Info, "whisper_rs::init", "vulkan ok");
        record(TapLevel::Info, "reqwest::client", "GET /");
        let events = snapshot_since_targets(cutoff, &["whisper"]);
        assert!(events.iter().any(|e| e.message == "vulkan ok"));
        assert!(events.iter().all(|e| e.message != "GET /"));
    }
}
