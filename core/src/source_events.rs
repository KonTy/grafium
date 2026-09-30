use crate::error::{CoreError, Result};
use crate::Graph;
use notify::{Event, EventKind};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub fn visible_source_event_path(relative: &Path) -> bool {
    crate::fsutil::is_authoritative_source(relative)
}

pub fn is_book_source_event_path(path: &Path, books_dir: &Path) -> bool {
    path.strip_prefix(books_dir).is_ok_and(|relative| {
        visible_source_event_path(relative)
            && !path.file_name().is_some_and(|name| name == "position.json")
    })
}

pub fn is_note_source_event_path(path: &Path, roots: &[&Path]) -> bool {
    roots
        .iter()
        .any(|root| path.strip_prefix(root).is_ok_and(visible_source_event_path))
}

pub fn should_process_event(
    event: &Event,
    pages: &Path,
    journals: &Path,
    knowledge: &Path,
) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    let books = pages.parent().unwrap_or(pages).join("books");
    event.paths.iter().any(|path| {
        is_book_source_event_path(path, &books)
            || is_note_source_event_path(path, &[pages, journals, knowledge])
    })
}

pub fn reconcile_watched_paths(
    graph: &Graph,
    paths: &HashSet<PathBuf>,
    rescan: bool,
) -> Result<()> {
    for path in paths {
        let relative = path.strip_prefix(&graph.root_dir)
            .map_err(|_| CoreError::Other("Watcher path escaped its graph".into()))?;
        if relative.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
            return Err(CoreError::Other("Watcher path contains traversal".into()));
        }
    }
    let books = graph.root_dir.join("books");
    let mut reconcile_all = rescan;
    for path in paths {
        if path.is_dir() {
            reconcile_all = true;
        }
        if !path.exists() && path.extension().and_then(|e| e.to_str()) != Some("md") {
            let relative = path
                .strip_prefix(&graph.root_dir)
                .map_err(|_| CoreError::Other("Watcher path escaped its graph".into()))?
                .to_string_lossy()
                .replace('\\', "/");
            if !graph
                .db
                .list_pages_by_file_path_prefix(&format!("{relative}/"))?
                .is_empty()
            {
                reconcile_all = true;
            }
        }
    }
    if reconcile_all {
        return graph.reconcile_files_from_disk();
    }
    let mut errors = Vec::new();
    if paths.iter().any(|path| path.starts_with(&books)) {
        if let Err(error) = graph.reconcile_original_books() {
            errors.push(error.to_string());
        }
    }
    // Deindex rename sources before accepting their destinations.
    for path in paths
        .iter()
        .filter(|path| !path.exists() && !path.starts_with(&books))
    {
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Err(error) = graph.deindex_file(path) {
                errors.push(format!("{}: {error}", path.display()));
            }
        }
    }
    for path in paths
        .iter()
        .filter(|path| path.is_file() && !path.starts_with(&books))
    {
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Err(error) = graph.index_file(path) {
                errors.push(format!("{}: {error}", path.display()));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(CoreError::Other(errors.join("\n")))
    }
}

/// Opening storage is never authorization to discard a failed or locked database.
pub fn open_preserving_graph(
    root: &Path,
    db: &Path,
    metadata: &str,
) -> std::result::Result<Graph, String> {
    Graph::open_with_db_path_and_metadata_dir(root, db, metadata).map_err(|error| {
        format!(
            "Could not open graph '{}': {error}. The database and source files were not removed. Check filesystem permissions and storage, or restore a verified backup before attempting index repair.",
            root.display(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watcher_rejects_paths_from_another_graph() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let graph = Graph::open(root.path()).unwrap();
        let path = other.path().join("outside.md");
        std::fs::write(&path, "- Must not be indexed here").unwrap();
        assert!(reconcile_watched_paths(&graph, &HashSet::from([path]), false).is_err());
        assert!(graph.db.search_fts("indexed", 10).unwrap().is_empty());
    }

    #[test]
    fn cloned_watcher_handle_deduplicates_an_exact_app_write() {
        let root = tempfile::tempdir().unwrap();
        let graph = Graph::open(root.path()).unwrap();
        let page = graph.create_page_with_content("Written", false, "- This was just saved in the app.").unwrap();
        let path = graph.page_filesystem_path(&page.id).unwrap();
        graph.db.conn().unwrap().execute("DELETE FROM pending_reindex", []).unwrap();
        let watcher = graph.clone();
        reconcile_watched_paths(&watcher, &HashSet::from([path]), false).unwrap();
        assert_eq!(graph.db.count_pending_reindex().unwrap(), 0);
    }

    #[test]
    fn restored_bytes_after_another_handle_edit_are_not_a_stale_cache_hit() {
        let root = tempfile::tempdir().unwrap();
        let first = Graph::open(root.path()).unwrap();
        let page = first.create_page_with_content("Restored", false, "- Original cobalt source text.").unwrap();
        let path = first.page_filesystem_path(&page.id).unwrap();
        let original = std::fs::read(&path).unwrap();
        let second = Graph::open(root.path()).unwrap();
        second.index_file(&path).unwrap();
        let block = second.db.list_blocks_for_page(&page.id).unwrap().remove(0);
        second.update_block(&block.id, "Replacement zirconium source text.", None).unwrap();
        std::fs::write(&path, original).unwrap();
        first.index_file(&path).unwrap();
        assert!(first.db.search_fts("zirconium", 10).unwrap().is_empty());
        assert!(!first.db.search_fts("cobalt", 10).unwrap().is_empty());
    }

    #[test]
    fn watcher_accepts_directory_changes_but_not_recovery_copies_or_read_events() {
        let root = Path::new("synthetic-graph");
        let changed = |path: &str| {
            should_process_event(
                &Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
                    .add_path(root.join(path)),
                &root.join("pages"),
                &root.join("journals"),
                &root.join("knowledge"),
            )
        };
        for path in [
            "books/id/original.epub",
            "books/id/book.json",
            "pages/Research.v2",
            "pages/Reading Notes",
        ] {
            assert!(changed(path), "{path}");
        }
        for path in [
            "books/id/position.json",
            "books/id/.extract-cache/converted.epub",
            "pages/Reading Notes/Books/id/note.conflict_1234.md",
            "pages/.recovery/saved.md",
            ".grafium/index.db",
        ] {
            assert!(!changed(path), "{path}");
        }
        assert!(!should_process_event(
            &Event::new(EventKind::Access(notify::event::AccessKind::Read))
                .add_path(root.join("pages/A.md")),
            &root.join("pages"),
            &root.join("journals"),
            &root.join("knowledge"),
        ));
    }

    #[test]
    fn recent_save_does_not_hide_a_real_file_or_directory_removal() {
        let root = tempfile::tempdir().unwrap();
        let graph = Graph::open(root.path()).unwrap();
        let page = graph
            .create_page_with_content(
                "Research.v2/Source",
                false,
                "- Recently saved cobalt source #watcher_unique",
            )
            .unwrap();
        let source = graph.page_filesystem_path(&page.id).unwrap();
        std::fs::remove_file(&source).unwrap();
        reconcile_watched_paths(&graph, &HashSet::from([source]), false).unwrap();
        assert!(graph.db.search_fts("cobalt", 10).unwrap().is_empty());

        let page = graph
            .create_page_with_content(
                "Move/Source",
                false,
                "- Directory-only disappearance removes neodymium text.",
            )
            .unwrap();
        let directory = graph.pages_dir.join("Move");
        let outside = tempfile::tempdir().unwrap();
        std::fs::rename(&directory, outside.path().join("Move")).unwrap();
        reconcile_watched_paths(&graph, &HashSet::from([directory]), false).unwrap();
        assert!(graph.db.search_fts("neodymium", 10).unwrap().is_empty());
        assert!(graph.db.get_page_by_id(&page.id).is_err());
    }

    #[test]
    fn unrelated_open_failure_preserves_a_healthy_database_and_local_only_state() {
        let root = tempfile::tempdir().unwrap();
        let graph = Graph::open(root.path()).unwrap();
        let page = graph
            .create_page_with_content("Retained", false, "- Irreplaceable source text.")
            .unwrap();
        graph.db.add_favorite(&page.id).unwrap();
        let db = root.path().join(".grafium/index.db");
        std::fs::remove_dir(root.path().join("books")).unwrap();
        std::fs::write(root.path().join("books"), b"An unrelated legacy file").unwrap();
        let error = open_preserving_graph(root.path(), &db, ".grafium")
            .err()
            .unwrap();
        assert!(error.contains("were not removed"));
        assert!(db.is_file());
        assert_eq!(graph.db.list_favorites().unwrap()[0].id, page.id);
        assert!(!graph.db.search_fts("Irreplaceable", 10).unwrap().is_empty());
    }
}
