//! Opt-in OCR fallback for scanned PDFs, designed to *degrade gracefully*.
//!
//! Some of the best sources a student finds — a scanned historical paper, a
//! photographed lecture handout, a poster PDF — carry no extractable text
//! layer, so [`crate::scraping::extract`] returns (almost) nothing for them.
//! Rather than silently drop those sources, Deep Research can rasterize the PDF
//! and read the pixels with `tesseract`.
//!
//! The overriding design rule here is that OCR is a *bonus, never a
//! requirement*:
//!
//! - It is **off by default** ([`crate::research::config::ResearchConfig::ocr_enabled`]).
//!   OCR is slow and needs an external engine, so paying that cost has to be a
//!   deliberate choice, not a surprise on every run.
//! - It shells out to already-present tools (`pdftoppm` to rasterize, the way
//!   the rest of Grafium renders PDFs, and `tesseract` to recognize) instead of
//!   pulling in a heavyweight OCR crate. `tesseract` in particular is a large
//!   native dependency that many machines won't have, and hard-linking it would
//!   make it a build/runtime requirement for a feature most users never enable.
//! - **If `tesseract` isn't on PATH, we do not error and do not fail the run.**
//!   [`ocr_pdf`] returns `Ok(None)`, the caller records the source as unreadable
//!   and moves on. A missing optional tool must be indistinguishable, from the
//!   agent loop's perspective, from a PDF that simply had no text — either way
//!   the round survives and the next source (or next round) is tried.
//!
//! The "friendly missing-tool message" shape mirrors [`crate::media`]'s
//! `tooling::missing_tool_error`; that helper lives in a private module and so
//! can't be called from here, but the intent — turn a raw `NotFound` spawn
//! error into something actionable, and never hide a *different* failure — is
//! reproduced by [`spawn_error`]. In practice the availability pre-check means
//! the agent never even reaches an error path for the common "not installed"
//! case; the mapping only matters if a tool vanishes mid-run.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{CoreError, Result};

/// Rasterization resolution. 200 DPI is the usual sweet spot for OCR accuracy
/// versus image size — high enough for `tesseract` to resolve body text,
/// without producing needlessly large bitmaps that slow recognition down.
const RENDER_DPI: &str = "200";

/// Is `tesseract` reachable on `PATH`? The whole graceful-degradation promise
/// hinges on answering this *before* trying to run anything, so a machine
/// without it takes the clean "skip OCR" path rather than an error path.
pub fn tesseract_available() -> bool {
    tool_on_path("tesseract")
}

/// Is `pdftoppm` (poppler-utils) reachable? Grafium already relies on poppler
/// for PDF handling, but OCR needs the *rasterizer* specifically, so we confirm
/// it independently before rendering.
pub fn pdftoppm_available() -> bool {
    tool_on_path("pdftoppm")
}

/// Can we OCR at all? Both the rasterizer and the recognizer must be present;
/// missing either means [`ocr_pdf`] short-circuits to `Ok(None)`.
pub fn ocr_available() -> bool {
    tesseract_available() && pdftoppm_available()
}

/// OCR up to `max_pages` pages of a PDF, returning the recognized text.
///
/// Returns:
/// - `Ok(Some(text))` — OCR ran and produced non-empty text.
/// - `Ok(None)` — OCR is unavailable (tooling not installed) *or* produced no
///   text. Both are "couldn't read this source", which the agent treats
///   identically to an empty extraction: record it and carry on. This is the
///   branch that keeps a missing `tesseract` from ever breaking a run.
/// - `Err(_)` — the tools were present but a spawn/exec failed unexpectedly
///   mid-run. The agent swallows this too (records the source unreadable), but
///   it is surfaced rather than masked so a genuinely broken install is
///   diagnosable.
///
/// Only the first `max_pages` pages are rendered: OCR cost is roughly linear in
/// pages, the goal is a *usable* excerpt to feed selection/synthesis (not a
/// faithful full transcription), and the per-source text is truncated by the
/// caller anyway. Rendering the whole of a 200-page scan would be a large,
/// pointless cost.
pub fn ocr_pdf(pdf_bytes: &[u8], max_pages: usize) -> Result<Option<String>> {
    // The availability pre-check is what makes "tesseract absent" a no-op
    // rather than an error: we never spawn anything if we can't complete.
    if !ocr_available() {
        return Ok(None);
    }

    let workdir = scratch_dir()?;
    // Ensure the scratch directory is always removed, even on the early-return
    // and `?` paths below, so OCR never leaks rasterized pages onto disk.
    let _guard = DirGuard(&workdir);

    let input = workdir.join("input.pdf");
    std::fs::write(&input, pdf_bytes)?;

    // pdftoppm writes `<prefix>-<n>.png` per page; `-r` sets DPI, `-f`/`-l`
    // bound the page range so we never render more than requested.
    let prefix = workdir.join("page");
    let render = Command::new("pdftoppm")
        .arg("-png")
        .arg("-r")
        .arg(RENDER_DPI)
        .arg("-f")
        .arg("1")
        .arg("-l")
        .arg(max_pages.max(1).to_string())
        .arg(&input)
        .arg(&prefix)
        .output()
        .map_err(|e| spawn_error("pdftoppm", e))?;
    if !render.status.success() {
        return Err(CoreError::Other(format!(
            "pdftoppm failed to rasterize PDF: {}",
            String::from_utf8_lossy(&render.stderr).trim()
        )));
    }

    let mut pages = collect_page_images(&workdir)?;
    pages.sort();

    let mut text = String::new();
    for page in pages {
        // `tesseract <image> stdout` prints recognized text to stdout; we take
        // whatever each page yields and concatenate. A single unreadable page
        // shouldn't discard the others, so a per-page failure is skipped.
        let recognized = Command::new("tesseract")
            .arg(&page)
            .arg("stdout")
            .output()
            .map_err(|e| spawn_error("tesseract", e))?;
        if recognized.status.success() {
            let page_text = String::from_utf8_lossy(&recognized.stdout);
            let trimmed = page_text.trim();
            if !trimmed.is_empty() {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(trimmed);
            }
        }
    }

    let text = text.trim().to_string();
    Ok((!text.is_empty()).then_some(text))
}

/// Turn a spawn `io::Error` into a friendly message, distinguishing "the tool
/// isn't installed" (the actionable common case) from any other spawn failure
/// (surfaced verbatim so nothing is hidden). Mirrors [`crate::media`]'s
/// `tooling::missing_tool_error`, which is private to that module.
fn spawn_error(tool: &str, err: std::io::Error) -> CoreError {
    if err.kind() == std::io::ErrorKind::NotFound {
        CoreError::Other(format!(
            "{tool} is not installed (or not on your PATH); skipping OCR for this source."
        ))
    } else {
        CoreError::Other(format!("failed to launch {tool}: {err}"))
    }
}

/// Scan `PATH` for an executable named `tool`. Deliberately does not *run*
/// anything (no `--version` probe): a pure lookup has no side effects and can't
/// be tripped up by a tool that lacks the flag we'd probe with.
fn tool_on_path(tool: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(tool).is_file())
}

fn collect_page_images(workdir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut images = Vec::new();
    for entry in std::fs::read_dir(workdir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("png") {
            images.push(path);
        }
    }
    Ok(images)
}

/// A uniquely-named scratch directory under the OS temp location. We can't use
/// the `tempfile` crate here (it is a dev-dependency only), so uniqueness is
/// hand-rolled from the process id and a monotonic counter — enough to avoid
/// collisions between concurrent research runs in the same process.
fn scratch_dir() -> Result<std::path::PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("grafium-ocr-{}-{}", std::process::id(), unique));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Removes a scratch directory (best-effort) when dropped, so rasterized pages
/// don't outlive the OCR call regardless of which path returns.
struct DirGuard<'a>(&'a Path);

impl Drop for DirGuard<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_on_path_is_false_for_a_nonexistent_binary() {
        assert!(!tool_on_path("grafium-definitely-not-a-real-tool-xyz"));
    }

    #[test]
    fn spawn_error_distinguishes_missing_tool_from_other_failures() {
        let not_found = std::io::Error::new(std::io::ErrorKind::NotFound, "nope");
        let msg = spawn_error("tesseract", not_found).to_string();
        assert!(msg.contains("tesseract is not installed"), "{msg}");
        assert!(msg.contains("skipping OCR"), "{msg}");

        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let msg = spawn_error("tesseract", denied).to_string();
        assert!(msg.contains("failed to launch tesseract"), "{msg}");
        assert!(!msg.contains("is not installed"), "{msg}");
    }

    #[test]
    fn ocr_pdf_skips_gracefully_when_tooling_is_unavailable() {
        // On any machine without the OCR tools (this CI environment has no
        // `tesseract`), `ocr_pdf` must return Ok(None) — never an error, never
        // a panic, and without writing any scratch files. This is the exact
        // "degrade gracefully" contract the agent relies on.
        if !ocr_available() {
            let result = ocr_pdf(b"%PDF-1.4 not really a pdf", 3);
            assert!(
                matches!(result, Ok(None)),
                "expected graceful skip, got {result:?}"
            );
        }
    }
}
