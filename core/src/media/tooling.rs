//! Friendly "please install X" errors for the external CLI tools the
//! `media` module shells out to (`yt-dlp`, `ffmpeg`).
//!
//! [`std::process::Command::output`] returns a raw
//! [`std::io::Error`] when the binary itself can't be found (as opposed to
//! the binary running and failing, which is a different error path). That
//! raw OS error (`No such file or directory (os error 2)`) is not helpful to
//! a Grafium user who has no idea `yt-dlp`/`ffmpeg` is a CLI tool at all —
//! [`missing_tool_error`] turns it into a one-line, actionable message,
//! falling back to the raw OS error for any other spawn failure (e.g.
//! permission denied) so nothing is hidden.

use crate::error::CoreError;

/// Builds the error returned when spawning `tool` fails. When `err` is
/// specifically "binary not found" (the common case: the tool just isn't
/// installed), returns a message telling the user how to install it instead
/// of a raw OS error string.
pub(crate) fn missing_tool_error(tool: &str, install_hint: &str, err: std::io::Error) -> CoreError {
    if err.kind() == std::io::ErrorKind::NotFound {
        CoreError::Other(format!(
            "{tool} is not installed (or not on your PATH). {install_hint}"
        ))
    } else {
        CoreError::Other(format!("failed to launch {tool}: {err}"))
    }
}

pub(crate) const YT_DLP_INSTALL_HINT: &str =
    "Install it and try again — see https://github.com/yt-dlp/yt-dlp#installation \
     (e.g. `pip install yt-dlp`, `brew install yt-dlp`, or your OS package manager).";

pub(crate) const FFMPEG_INSTALL_HINT: &str =
    "Install it and try again — see https://ffmpeg.org/download.html \
     (e.g. `apt install ffmpeg`, `brew install ffmpeg`, or your OS package manager).";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tool_error_gives_install_instructions_when_binary_not_found() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "No such file or directory");
        let msg = missing_tool_error("yt-dlp", YT_DLP_INSTALL_HINT, err).to_string();

        assert!(msg.contains("yt-dlp is not installed"), "{msg}");
        assert!(msg.contains("yt-dlp#installation"), "{msg}");
    }

    #[test]
    fn missing_tool_error_falls_back_to_raw_error_for_other_failures() {
        let err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let msg = missing_tool_error("ffmpeg", FFMPEG_INSTALL_HINT, err).to_string();

        assert!(msg.contains("failed to launch ffmpeg"), "{msg}");
        assert!(msg.contains("denied"), "{msg}");
        assert!(!msg.contains("is not installed"), "{msg}");
    }
}
