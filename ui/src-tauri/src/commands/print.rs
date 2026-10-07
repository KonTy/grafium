//! Native printing.
//!
//! The frontend builds the print document into the page and then asks for it
//! to be printed. On Linux that goes through WebKitGTK's own print operation
//! rather than `window.print()`, which is unreliable under WebKitGTK; the same
//! operation also writes the PDF, so printing and "Save as PDF" are one code
//! path and share one rendering.

use serde::{Deserialize, Serialize};
use tauri::WebviewWindow;

/// Where the printed document should go.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PrintTarget {
    /// Hand over to the system print dialog, so the user picks the printer.
    Dialog,
    /// Write a PDF to a path the user has already chosen.
    Pdf { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PrintOutcome {
    Printed,
    Cancelled,
    /// This platform has no native print path; the caller falls back to
    /// `window.print()`.
    Unsupported,
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub async fn print_document(
    window: WebviewWindow,
    target: PrintTarget,
) -> Result<PrintOutcome, String> {
    use gtk::prelude::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use webkit2gtk::{PrintOperation, PrintOperationExt, PrintOperationResponse};

    // Resolved before touching GTK so a bad path fails with a clear message
    // instead of a silently empty print job.
    let pdf_uri = match &target {
        PrintTarget::Pdf { path } => Some(
            gtk::glib::filename_to_uri(path, None)
                .map_err(|error| format!("Could not use that file path: {error}"))?
                .to_string(),
        ),
        PrintTarget::Dialog => None,
    };

    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |webview| {
            let operation = PrintOperation::new(&webview.inner());
            // Both signals can fire for one job, and `finished` always follows
            // `failed`, so the first result wins and the rest are ignored.
            let reply = Rc::new(RefCell::new(Some(send)));
            // The operation must outlive this closure or the job is dropped
            // mid-print. Clearing it when the job ends breaks the reference
            // cycle the signal handlers would otherwise keep alive.
            let keep_alive = Rc::new(RefCell::new(Some(operation.clone())));

            let on_failed = reply.clone();
            let failed_keep_alive = keep_alive.clone();
            operation.connect_failed(move |_, error| {
                if let Some(send) = on_failed.borrow_mut().take() {
                    let _ = send.send(Err(error.to_string()));
                }
                failed_keep_alive.borrow_mut().take();
            });

            let on_finished = reply.clone();
            let finished_keep_alive = keep_alive.clone();
            operation.connect_finished(move |_| {
                if let Some(send) = on_finished.borrow_mut().take() {
                    let _ = send.send(Ok(PrintOutcome::Printed));
                }
                finished_keep_alive.borrow_mut().take();
            });

            match pdf_uri {
                Some(uri) => {
                    let settings = gtk::PrintSettings::new();
                    // GTK's built-in file backend. Its printer name is not
                    // translated, so this works in any locale.
                    settings.set_printer("Print to File");
                    settings.set(gtk::PRINT_SETTINGS_OUTPUT_URI.as_str(), Some(&uri));
                    settings.set(gtk::PRINT_SETTINGS_OUTPUT_FILE_FORMAT.as_str(), Some("pdf"));
                    operation.set_print_settings(&settings);
                    operation.print();
                }
                None => {
                    let parent = webview
                        .inner()
                        .toplevel()
                        .and_then(|toplevel| toplevel.downcast::<gtk::Window>().ok());
                    // Blocks on a nested main loop until the dialog closes,
                    // then the job runs and `finished` arrives later.
                    if operation.run_dialog(parent.as_ref()) != PrintOperationResponse::Print {
                        if let Some(send) = reply.borrow_mut().take() {
                            let _ = send.send(Ok(PrintOutcome::Cancelled));
                        }
                        keep_alive.borrow_mut().take();
                    }
                }
            }
        })
        .map_err(|error| error.to_string())?;

    receive
        .await
        .map_err(|_| "The print job ended unexpectedly".to_string())?
}

#[cfg(not(target_os = "linux"))]
#[tauri::command]
pub async fn print_document(
    _window: WebviewWindow,
    _target: PrintTarget,
) -> Result<PrintOutcome, String> {
    Ok(PrintOutcome::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_target_round_trips_from_the_frontend() {
        let target: PrintTarget =
            serde_json::from_str(r#"{"kind":"pdf","path":"/tmp/notes.pdf"}"#).unwrap();
        match target {
            PrintTarget::Pdf { path } => assert_eq!(path, "/tmp/notes.pdf"),
            other => panic!("expected a PDF target, got {other:?}"),
        }
    }

    #[test]
    fn dialog_target_round_trips_from_the_frontend() {
        let target: PrintTarget = serde_json::from_str(r#"{"kind":"dialog"}"#).unwrap();
        assert!(matches!(target, PrintTarget::Dialog));
    }

    #[test]
    fn outcomes_reach_the_frontend_as_plain_names() {
        assert_eq!(
            serde_json::to_string(&PrintOutcome::Cancelled).unwrap(),
            r#""cancelled""#
        );
        assert_eq!(
            serde_json::to_string(&PrintOutcome::Unsupported).unwrap(),
            r#""unsupported""#
        );
    }
}
