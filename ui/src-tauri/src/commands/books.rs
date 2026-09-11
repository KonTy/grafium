use crate::{current_graph_snapshot, open_graph_snapshot, AppState};
use grafium_core::import::books::{
    import_book_files, import_books_directory, BookImportProgress, BookImportQueueStatus,
    BookImportReport, BookImportStatus,
};
use grafium_core::CoreError;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};

use super::jobs::{JobHandle, JobLink, JobsState};

#[tauri::command(rename_all = "camelCase")]
pub async fn books_import_directory(
    app: AppHandle,
    state: State<'_, AppState>,
    jobs: State<'_, JobsState>,
    source_dir: String,
) -> Result<String, String> {
    let source_path = PathBuf::from(&source_dir);
    let handle = jobs
        .registry
        .start(app.clone(), "book_import", "Import books", true)?;
    let job_id = handle.id().to_string();

    if !source_path.is_dir() && !source_path.is_file() {
        let details = format!(
            "Book import source is not a file or directory: {}\n\nChoose the external folder that contains the original .pdf/.epub files.",
            source_path.display()
        );
        handle.failed_with_details("Could not start book import", Some(details));
        return Ok(job_id);
    }

    let snapshot = match current_graph_snapshot(&app, &state.graph) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            handle.failed_with_details("Could not inspect current graph", Some(err));
            return Ok(job_id);
        }
    };
    if source_inside_graph(&source_path, &snapshot.root_dir) {
        handle.failed_with_details(
            "Choose the original book source",
            Some(source_inside_graph_message(&source_path)),
        );
        return Ok(job_id);
    }

    tauri::async_runtime::spawn_blocking(move || {
        handle.progress(0, 0, "Scanning books...");

        let result = match open_graph_snapshot(&snapshot) {
            Ok(graph) => {
                let job = &handle;
                if source_path.is_file() {
                    import_book_files(
                        &graph,
                        std::slice::from_ref(&source_path),
                        |progress| report_progress(job, &source_path, progress),
                        || job.is_cancelled(),
                    )
                } else {
                    import_books_directory(
                        &graph,
                        &source_path,
                        |progress| report_progress(job, &source_path, progress),
                        || job.is_cancelled(),
                    )
                }
            }
            Err(err) => Err(CoreError::Other(err)),
        };

        match result {
            Ok(report) => {
                let message = describe_report(&report);
                let link = first_imported_link(&report);
                let details = describe_report_details(&report, &source_path);
                if report.failed > 0 {
                    tracing::warn!(target: "grafium::books", "{details}");
                } else {
                    tracing::info!(target: "grafium::books", "{details}");
                }
                if report.failed > 0 && report.imported == 0 && report.skipped == 0 {
                    handle.failed_with_details(message, Some(details));
                } else {
                    handle.succeeded_with_details(message, link, Some(details));
                }
            }
            Err(CoreError::Cancelled) => handle.cancelled(),
            Err(err) => handle.failed(err.to_string()),
        }

        fn describe_report_details(report: &BookImportReport, source_path: &Path) -> String {
            let selected_label = if source_path.is_file() {
                "Selected file"
            } else {
                "Selected folder"
            };
            let mut out = format!(
                "{selected_label}: {}\nDiscovered: {}\nImported: {}\nSkipped: {}\nFailed: {}",
                source_path.display(),
                report.discovered,
                report.imported,
                report.skipped,
                report.failed
            );
            if report.items.is_empty() {
                out.push_str("\n\nNo supported book files were found in the selected folder.");
                out.push_str(&no_supported_book_hint(source_path));
                return out;
            }

            out.push_str("\n\nFiles:");
            for item in &report.items {
                out.push_str("\n- ");
                out.push_str(&item.source_file);
                out.push_str(" [");
                out.push_str(item.format.label());
                out.push_str("] ");
                out.push_str(match item.status {
                    BookImportStatus::Imported => "imported",
                    BookImportStatus::Skipped => "skipped",
                    BookImportStatus::Failed => "failed",
                });
                if let Some(title) = &item.title {
                    out.push_str(" as ");
                    out.push_str(title);
                }
                if let Some(message) = &item.message {
                    out.push_str(" - ");
                    out.push_str(message);
                }
            }
            out
        }
    });

    Ok(job_id)
}

fn report_progress(job: &JobHandle, source_path: &Path, progress: BookImportProgress) {
    let details = book_queue_details(source_path, &progress);
    job.progress_with_details(
        progress.done,
        progress.total,
        progress.message,
        Some(details),
    );
}

fn book_queue_details(source_path: &Path, progress: &BookImportProgress) -> String {
    let selected_label = if source_path.is_file() {
        "Selected file"
    } else {
        "Selected folder"
    };
    let mut queued = 0usize;
    let mut importing = 0usize;
    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    for item in &progress.queue {
        match item.status {
            BookImportQueueStatus::Queued => queued += 1,
            BookImportQueueStatus::Importing => importing += 1,
            BookImportQueueStatus::Imported => imported += 1,
            BookImportQueueStatus::Skipped => skipped += 1,
            BookImportQueueStatus::Failed => failed += 1,
        }
    }

    let mut out = format!(
        "{selected_label}: {}\nProgress: {} of {}\nQueued: {queued}\nImporting: {importing}\nImported: {imported}\nSkipped: {skipped}\nFailed: {failed}",
        source_path.display(),
        progress.done,
        progress.total
    );

    if progress.queue.is_empty() {
        return out;
    }

    out.push_str("\n\nBook queue:");
    let shown = progress.queue.iter().take(100);
    for item in shown {
        out.push_str("\n- [");
        out.push_str(match item.status {
            BookImportQueueStatus::Queued => "queued",
            BookImportQueueStatus::Importing => "importing",
            BookImportQueueStatus::Imported => "imported",
            BookImportQueueStatus::Skipped => "skipped",
            BookImportQueueStatus::Failed => "failed",
        });
        out.push_str("] ");
        out.push_str(&item.source_file);
        out.push_str(" [");
        out.push_str(item.format.label());
        out.push(']');
        if let Some(title) = &item.title {
            out.push_str(" as ");
            out.push_str(title);
        }
        if let Some(message) = &item.message {
            out.push_str(" - ");
            out.push_str(message);
        }
    }
    if progress.queue.len() > 100 {
        out.push_str(&format!(
            "\n- ... and {} more queued files",
            progress.queue.len() - 100
        ));
    }
    out
}

fn source_inside_graph(source_path: &Path, graph_root: &Path) -> bool {
    let Some(source) = source_path.canonicalize().ok() else {
        return false;
    };
    let Some(root) = graph_root.canonicalize().ok() else {
        return false;
    };
    source.starts_with(&root)
}

fn source_inside_graph_message(source_path: &Path) -> String {
    format!(
        "This path is inside the current Grafium graph: {}\n\nGrafium writes imported books to pages/Books automatically. For import, choose the original book file or the folder that contains the original .pdf/.epub files, wherever that source lives outside the graph.",
        source_path.display()
    )
}

fn no_supported_book_hint(source_path: &Path) -> String {
    let mut dirs_seen = 0usize;
    let mut files_seen = 0usize;
    let mut unsupported = Vec::new();
    scan_no_supported_hint(
        source_path,
        0,
        &mut dirs_seen,
        &mut files_seen,
        &mut unsupported,
    );

    let mut out = format!(
        "\n\nScanned: {files_seen} files in {dirs_seen} folders\nSupported extensions: epub, pdf, html, htm, xhtml, md, markdown, mdown, txt, text, fb2, mobi, azw, azw3, azw4, lit, lrf, pdb, rb, snb, tcr, odt, docx, rtf, cbz, cbr"
    );
    if files_seen == 0 {
        out.push_str("\nThe selected folder appears to be empty or unreadable.");
    } else if !unsupported.is_empty() {
        out.push_str("\nUnsupported files seen:");
        for file in unsupported {
            out.push_str("\n- ");
            out.push_str(&file);
        }
    }
    out
}

fn scan_no_supported_hint(
    dir: &Path,
    depth: usize,
    dirs_seen: &mut usize,
    files_seen: &mut usize,
    unsupported: &mut Vec<String>,
) {
    if depth > 8 || *files_seen >= 500 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    *dirs_seen += 1;

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            scan_no_supported_hint(&path, depth + 1, dirs_seen, files_seen, unsupported);
            continue;
        }

        *files_seen += 1;
        if unsupported.len() < 10 {
            unsupported.push(path.display().to_string());
        }
    }
}

fn describe_report(report: &BookImportReport) -> String {
    if report.discovered == 0 {
        return "No supported book files found".to_string();
    }

    let mut parts = Vec::new();
    if report.imported > 0 {
        parts.push(format!(
            "imported {} {}",
            report.imported,
            plural(report.imported, "book", "books")
        ));
    }
    if report.skipped > 0 {
        parts.push(format!("skipped {}", report.skipped));
    }
    if report.failed > 0 {
        parts.push(format!("failed {}", report.failed));
    }

    format!("Book import finished: {}", parts.join(", "))
}

fn first_imported_link(report: &BookImportReport) -> Option<JobLink> {
    report
        .items
        .iter()
        .find(|item| {
            item.index_page_id.is_some()
                && matches!(
                    item.status,
                    BookImportStatus::Imported | BookImportStatus::Skipped
                )
        })
        .and_then(|item| {
            Some(JobLink {
                page_id: item.index_page_id.clone()?,
                page_title: item.index_page_title.clone(),
                label: item.title.clone().unwrap_or_else(|| "Book".to_string()),
            })
        })
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}
