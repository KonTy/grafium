use super::backend::{compute_hash, revision_changed, FileMetadata, FileSnapshot, SyncBackend};
use super::engine::{ConflictSide, SyncEngine};
use crate::error::{CoreError, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};

#[derive(Default)]
struct Remote {
    files: Mutex<HashMap<String, Vec<u8>>>,
    listing: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    download: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    publication_race: Mutex<Option<Vec<u8>>>,
    inventory_error: Mutex<bool>,
}

fn wait_for_gate(gate: &Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>) {
    if let Some((ready, proceed)) = gate.lock().unwrap().take() {
        ready.send(()).unwrap();
        proceed
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
    }
}

fn arm_gate(
    gate: &Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (proceed_tx, proceed_rx) = mpsc::channel();
    *gate.lock().unwrap() = Some((ready_tx, proceed_rx));
    (ready_rx, proceed_tx)
}

impl Remote {
    fn set(&self, path: &str, content: &[u8]) {
        self.files
            .lock()
            .unwrap()
            .insert(path.into(), content.to_vec());
    }
    fn bytes(&self, path: &str) -> Option<Vec<u8>> {
        self.files.lock().unwrap().get(path).cloned()
    }
    fn delete(&self, path: &str) {
        self.files.lock().unwrap().remove(path);
    }
}

impl SyncBackend for Remote {
    fn name(&self) -> &str {
        "synthetic"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn list_files(&self) -> Result<Vec<FileMetadata>> {
        if *self.inventory_error.lock().unwrap() {
            return Err(CoreError::Other("Incomplete remote inventory".into()));
        }
        let files = self
            .files
            .lock()
            .unwrap()
            .iter()
            .map(|(path, content)| FileMetadata {
                rel_path: path.clone(),
                size: content.len() as u64,
                modified_at: 0,
                hash: Some(compute_hash(content)),
            })
            .collect();
        wait_for_gate(&self.listing);
        Ok(files)
    }
    fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        self.bytes(path)
            .ok_or_else(|| CoreError::NotFound(path.into()))
    }
    fn read_snapshot(&self, path: &str) -> Result<FileSnapshot> {
        let content = self.bytes(path);
        if path != ".grafium-sync-id" {
            wait_for_gate(&self.download);
        }
        Ok(FileSnapshot {
            content,
            etag: None,
            mutation_fence: None,
        })
    }
    fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        self.set(path, content);
        Ok(())
    }
    fn delete_file(&self, path: &str) -> Result<()> {
        self.delete(path);
        Ok(())
    }
    fn publish_if_unchanged(
        &self,
        path: &str,
        expected: &FileSnapshot,
        content: Option<&[u8]>,
    ) -> Result<()> {
        let mut files = self.files.lock().unwrap();
        if let Some(newer) = self.publication_race.lock().unwrap().take() {
            files.insert(path.into(), newer);
        }
        if files.get(path).map(|c| compute_hash(c)) != expected.hash() {
            return Err(revision_changed(path));
        }
        match content {
            Some(content) => {
                files.insert(path.into(), content.to_vec());
            }
            None => {
                files.remove(path);
            }
        }
        Ok(())
    }
}

fn write(root: &Path, path: &str, content: &[u8]) {
    fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
    fs::write(root.join(path), content).unwrap();
}

fn setup_conflict(
    path: &str,
    local: &[u8],
    remote: &[u8],
) -> (tempfile::TempDir, Remote, SyncEngine) {
    let root = tempfile::tempdir_in(".").unwrap();
    write(root.path(), path, local);
    let backend = Remote::default();
    backend.set(path, remote);
    let engine = SyncEngine::new(root.path().to_path_buf());
    let result = engine.sync(&backend).unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.conflicts, [path]);
    (root, backend, engine)
}

fn recovery_contains(root: &Path, path: &str, content: &[u8]) -> bool {
    fs::read_dir(
        root.join(".grafium/sync-recovery")
            .join(path)
            .parent()
            .unwrap(),
    )
    .unwrap()
    .flatten()
    .any(|entry| {
        entry.file_name().to_string_lossy().contains(".conflict_")
            && fs::read(entry.path()).unwrap() == content
    })
}

#[test]
fn unchanged_binary_and_marker_containing_bytes_can_choose_either_side() {
    let path = "assets/image.png";
    let local = b"\x89PNG\r\n\xff=======\0local";
    let remote = b"\x89PNG\r\n\xfe<<<<<<<\0remote";
    for chosen in [ConflictSide::Local, ConflictSide::Remote] {
        let (root, backend, engine) = setup_conflict(path, local, remote);
        let view = engine.conflict_state(&backend, path).unwrap();
        let result = engine
            .resolve_conflict(
                &backend,
                path,
                view.local_hash.as_deref(),
                view.remote_hash.as_deref(),
                chosen,
            )
            .unwrap();
        assert!(result.errors.is_empty());
        let expected: &[u8] = if matches!(chosen, ConflictSide::Local) {
            local
        } else {
            remote
        };
        assert_eq!(fs::read(root.path().join(path)).unwrap(), expected);
        assert_eq!(backend.bytes(path).unwrap(), expected);
        assert!(engine.unresolved_conflicts().is_empty());
        assert!(recovery_contains(root.path(), path, local));
        assert!(recovery_contains(root.path(), path, remote));
    }
}

#[test]
fn editing_conflict_is_not_permission_to_overwrite_remote() {
    let path = "pages/doc.md";
    let (root, backend, engine) = setup_conflict(path, b"local", b"remote");
    write(root.path(), path, b"local resolution");
    backend.set(path, b"newer remote");
    let result = engine.sync(&backend).unwrap();
    assert!(result.pushed.is_empty());
    assert_eq!(backend.bytes(path).unwrap(), b"newer remote");
    assert_eq!(engine.unresolved_conflicts().len(), 1);
    assert!(recovery_contains(root.path(), path, b"newer remote"));
}

#[test]
fn stale_resolution_reopens_and_preserves_new_remote_revision() {
    let path = "pages/doc.md";
    for during_publication in [false, true] {
        let (root, backend, engine) = setup_conflict(path, b"local", b"remote");
        let view = engine.conflict_state(&backend, path).unwrap();
        if during_publication {
            *backend.publication_race.lock().unwrap() = Some(b"new remote".to_vec());
        } else {
            backend.set(path, b"new remote");
        }
        assert!(engine
            .resolve_conflict(
                &backend,
                path,
                view.local_hash.as_deref(),
                view.remote_hash.as_deref(),
                ConflictSide::Local
            )
            .is_err());
        assert_eq!(backend.bytes(path).unwrap(), b"new remote");
        assert_eq!(fs::read(root.path().join(path)).unwrap(), b"local");
        assert!(recovery_contains(root.path(), path, b"new remote"));
        assert_eq!(
            engine.unresolved_conflicts()[0].remote_hash,
            Some(compute_hash(b"new remote"))
        );
    }
}

#[test]
fn missing_side_resolution_can_confirm_deletion_or_restore_explicitly() {
    for deleted_side in [ConflictSide::Local, ConflictSide::Remote] {
        for chosen in [ConflictSide::Local, ConflictSide::Remote] {
            let path = "pages/doc.md";
            let (root, backend, engine) = setup_conflict(path, b"local", b"remote");
            match deleted_side {
                ConflictSide::Local => fs::remove_file(root.path().join(path)).unwrap(),
                ConflictSide::Remote => backend.delete(path),
            }
            engine.sync(&backend).unwrap();
            assert_eq!(
                root.path().join(path).exists(),
                !matches!(deleted_side, ConflictSide::Local)
            );
            assert_eq!(
                backend.bytes(path).is_some(),
                !matches!(deleted_side, ConflictSide::Remote)
            );
            let view = engine.conflict_state(&backend, path).unwrap();
            engine
                .resolve_conflict(
                    &backend,
                    path,
                    view.local_hash.as_deref(),
                    view.remote_hash.as_deref(),
                    chosen,
                )
                .unwrap();
            let deletion_chosen = matches!(
                (deleted_side, chosen),
                (ConflictSide::Local, ConflictSide::Local)
                    | (ConflictSide::Remote, ConflictSide::Remote)
            );
            assert_eq!(root.path().join(path).exists(), !deletion_chosen);
            assert_eq!(backend.bytes(path).is_some(), !deletion_chosen);
            assert!(engine.unresolved_conflicts().is_empty());
        }
    }
}

#[test]
fn both_missing_conflict_can_be_cleared_without_resurrection() {
    let path = "pages/doc.md";
    let (root, backend, engine) = setup_conflict(path, b"local", b"remote");
    fs::remove_file(root.path().join(path)).unwrap();
    backend.delete(path);
    let view = engine.conflict_state(&backend, path).unwrap();
    assert_eq!(view.local_hash, None);
    assert_eq!(view.remote_hash, None);
    engine
        .resolve_conflict(&backend, path, None, None, ConflictSide::Local)
        .unwrap();
    assert!(engine.unresolved_conflicts().is_empty());
    assert!(!root.path().join(path).exists());
}

#[test]
fn delayed_download_never_loses_an_edit_or_resurrects_deleted_source() {
    for edit in [None, Some(b"edited during download".as_slice())] {
        let root = tempfile::tempdir_in(".").unwrap();
        let path = "pages/doc.md";
        write(root.path(), path, b"base");
        let backend = Arc::new(Remote::default());
        let engine = SyncEngine::new(root.path().to_path_buf());
        engine.sync(backend.as_ref()).unwrap();
        backend.set(path, b"remote edit");
        let (ready_tx, ready_rx) = mpsc::channel();
        let (proceed_tx, proceed_rx) = mpsc::channel();
        *backend.download.lock().unwrap() = Some((ready_tx, proceed_rx));
        let local_root = root.path().to_path_buf();
        let worker_backend = backend.clone();
        let worker = std::thread::spawn(move || {
            SyncEngine::new(local_root)
                .sync(worker_backend.as_ref())
                .unwrap()
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        // Acquiring this during the blocked download also proves no source
        // publication lock is held over network I/O.
        {
            let source_lock = crate::fsutil::graph_operation_lock(root.path()).unwrap();
            let _source = source_lock.lock();
            match edit {
                Some(content) => write(root.path(), path, content),
                None => fs::remove_file(root.path().join(path)).unwrap(),
            }
        }
        proceed_tx.send(()).unwrap();
        let result = worker.join().unwrap();
        assert!(result.pulled.is_empty());
        assert_eq!(result.conflicts, [path]);
        assert_eq!(fs::read(root.path().join(path)).ok().as_deref(), edit);
        assert!(recovery_contains(root.path(), path, b"remote edit"));
    }
}

#[test]
fn deletion_does_not_remove_newer_opposing_data() {
    for deleted_side in [ConflictSide::Local, ConflictSide::Remote] {
        let root = tempfile::tempdir_in(".").unwrap();
        let path = "pages/doc.md";
        write(root.path(), path, b"base");
        let backend = Remote::default();
        let engine = SyncEngine::new(root.path().to_path_buf());
        engine.sync(&backend).unwrap();
        match deleted_side {
            ConflictSide::Local => {
                fs::remove_file(root.path().join(path)).unwrap();
                backend.set(path, b"remote newer");
            }
            ConflictSide::Remote => {
                backend.delete(path);
                write(root.path(), path, b"local newer");
            }
        }
        let result = engine.sync(&backend).unwrap();
        assert_eq!(result.conflicts, [path]);
        assert!(result.deleted_local.is_empty());
        assert!(result.deleted_remote.is_empty());
    }
}

#[test]
fn failed_inventory_leaves_local_sources_untouched() {
    let root = tempfile::tempdir_in(".").unwrap();
    let path = "pages/doc.md";
    write(root.path(), path, b"base");
    let backend = Remote::default();
    let engine = SyncEngine::new(root.path().to_path_buf());
    engine.sync(&backend).unwrap();
    backend.delete(path);
    *backend.inventory_error.lock().unwrap() = true;
    assert!(engine.sync(&backend).is_err());
    assert_eq!(fs::read(root.path().join(path)).unwrap(), b"base");
}

fn create_then_delete_graph_page(root: &Path, title: &str) {
    // Separate Graph handles must share the in-flight sync epoch even after
    // both handles are dropped before publication resumes.
    let page = {
        let graph = crate::graph::Graph::open(root).unwrap();
        graph.create_page(title, false).unwrap()
    };
    crate::graph::Graph::open(root)
        .unwrap()
        .delete_page(&page.id)
        .unwrap();
}

#[test]
fn initially_absent_source_aba_is_rejected_even_during_inventory() {
    for during_listing in [true, false] {
        let root = tempfile::tempdir_in(".").unwrap();
        let path = "pages/Ephemeral.md";
        let backend = Arc::new(Remote::default());
        backend.set(path, b"- remote source\n");
        let (ready, proceed) = arm_gate(if during_listing {
            &backend.listing
        } else {
            &backend.download
        });
        let local_root = root.path().to_path_buf();
        let remote = backend.clone();
        let worker =
            std::thread::spawn(move || SyncEngine::new(local_root).sync(remote.as_ref()).unwrap());
        ready
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        create_then_delete_graph_page(root.path(), "Ephemeral");
        assert!(!root.path().join(path).exists());
        proceed.send(()).unwrap();
        let result = worker.join().unwrap();
        assert!(result.pulled.is_empty(), "{result:?}");
        assert_eq!(result.conflicts, [path]);
        assert!(
            !root.path().join(path).exists(),
            "download resurrected an ABA-deleted source"
        );
        assert!(recovery_contains(root.path(), path, b"- remote source\n"));
    }
}

#[test]
fn same_bytes_and_book_directory_aba_reject_pull_but_sibling_edits_do_not() {
    for mutation in ["same-bytes", "book-directory", "sibling"] {
        let root = tempfile::tempdir_in(".").unwrap();
        let book_id = uuid::Uuid::new_v4();
        let path = format!("books/{book_id}/original.epub");
        write(root.path(), &path, b"base");
        let backend = Arc::new(Remote::default());
        SyncEngine::new(root.path().to_path_buf())
            .sync(backend.as_ref())
            .unwrap();
        backend.set(&path, b"remote version");
        let (ready, proceed) = arm_gate(&backend.download);
        let local_root = root.path().to_path_buf();
        let remote = backend.clone();
        let worker =
            std::thread::spawn(move || SyncEngine::new(local_root).sync(remote.as_ref()).unwrap());
        ready
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let operations = crate::fsutil::graph_operation_lock(root.path()).unwrap();
        {
            let _publication = operations.lock();
            match mutation {
                "same-bytes" => {
                    crate::fsutil::atomic_write(&root.path().join(&path), b"base").unwrap()
                }
                "book-directory" => {
                    let directory = format!("books/{book_id}");
                    let staged = root.path().join("staged-book");
                    fs::rename(root.path().join(&directory), &staged).unwrap();
                    fs::rename(staged, root.path().join(&directory)).unwrap();
                    crate::fsutil::record_source_mutation(root.path(), Path::new(&directory))
                        .unwrap();
                }
                "sibling" => crate::fsutil::atomic_write(
                    &root.path().join(format!("books/{book_id}/position.json")),
                    b"{}",
                )
                .unwrap(),
                _ => unreachable!(),
            }
        }
        proceed.send(()).unwrap();
        let result = worker.join().unwrap();
        let expected = if mutation == "sibling" {
            assert_eq!(result.pulled, [path.clone()]);
            assert!(result.conflicts.is_empty());
            b"remote version".as_slice()
        } else {
            assert!(result.pulled.is_empty());
            assert_eq!(result.conflicts, [path.clone()]);
            b"base".as_slice()
        };
        assert_eq!(fs::read(root.path().join(path)).unwrap(), expected);
    }
}

#[test]
fn explicit_resolution_does_not_restore_an_aba_deleted_source() {
    let path = "pages/Ephemeral.md";
    let (root, backend, engine) = setup_conflict(path, b"- local\n", b"- remote\n");
    fs::remove_file(root.path().join(path)).unwrap();
    let view = engine.conflict_state(&backend, path).unwrap();
    assert_eq!(view.local_hash, None);
    let backend = Arc::new(backend);
    let (ready, proceed) = arm_gate(&backend.download);
    let local_root = root.path().to_path_buf();
    let remote = backend.clone();
    let worker = std::thread::spawn(move || {
        SyncEngine::new(local_root).resolve_conflict(
            remote.as_ref(),
            path,
            view.local_hash.as_deref(),
            view.remote_hash.as_deref(),
            ConflictSide::Remote,
        )
    });
    ready
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    create_then_delete_graph_page(root.path(), "Ephemeral");
    proceed.send(()).unwrap();
    assert!(worker.join().unwrap().is_err());
    assert!(!root.path().join(path).exists());
    assert_eq!(engine.unresolved_conflicts().len(), 1);
}

#[test]
fn sync_local_deletion_invalidates_other_inflight_fences() {
    let root = tempfile::tempdir_in(".").unwrap();
    let path = "pages/doc.md";
    write(root.path(), path, b"base");
    let backend = Remote::default();
    let engine = SyncEngine::new(root.path().to_path_buf());
    engine.sync(&backend).unwrap();
    let fence = crate::fsutil::source_mutation_fence(root.path(), Path::new(path)).unwrap();
    backend.delete(path);
    assert_eq!(engine.sync(&backend).unwrap().deleted_local, [path]);
    let _publication = fence.lock();
    assert!(!fence.is_current());
}

fn annotation_fixture() -> (
    tempfile::TempDir,
    crate::Graph,
    crate::graph::books::BookInfo,
    String,
) {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let file = source.path().join("1.fb2");
    fs::write(
        &file,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
<description><title-info><book-title>Sync fixture</book-title></title-info></description>
<body><section><p>Source words stay unchanged.</p></section></body></FictionBook>"#,
    )
    .unwrap();
    let graph = crate::Graph::open(root.path()).unwrap();
    let book = graph.import_original_book(&file).unwrap();
    let path = Path::new(&book.file_path)
        .with_extension("jsonld")
        .to_string_lossy()
        .replace('\\', "/");
    (root, graph, book, path)
}

fn create_annotation(
    graph: &crate::Graph,
    book: &crate::graph::books::BookInfo,
    body: &str,
) -> crate::graph::books::BookNote {
    graph
        .book_note_save(
            &book.id,
            &uuid::Uuid::new_v4().to_string(),
            None,
            body,
            "",
            None,
            &book.source_sha256,
        )
        .unwrap()
}

#[test]
fn annotation_sync_unions_independent_notes_without_a_common_sync_base() {
    let (root, graph, book, path) = annotation_fixture();
    let first = create_annotation(&graph, &book, "Local annotation");
    let local = fs::read(root.path().join(&path)).unwrap();
    fs::remove_file(root.path().join(&path)).unwrap();
    let second = create_annotation(&graph, &book, "Remote annotation");
    let remote = fs::read(root.path().join(&path)).unwrap();
    write(root.path(), &path, &local);
    let backend = Remote::default();
    backend.set(&path, &remote);
    let engine = SyncEngine::new(root.path().to_path_buf());
    let result = engine.sync(&backend).unwrap();
    assert!(result.errors.is_empty(), "{result:?}");
    assert!(result.conflicts.is_empty(), "{result:?}");
    assert!(result.annotation_conflicts.is_empty());
    assert!(result.merged.contains(&path));
    assert_eq!(
        fs::read(root.path().join(&path)).unwrap(),
        backend.bytes(&path).unwrap()
    );
    let notes = graph.book_notes_list(&book.id).unwrap();
    assert_eq!(notes.len(), 2);
    assert!(notes.iter().any(|note| note.id == first.id));
    assert!(notes.iter().any(|note| note.id == second.id));
    let repeated = engine.sync(&backend).unwrap();
    assert!(repeated.errors.is_empty(), "{repeated:?}");
    assert!(repeated.merged.is_empty(), "{repeated:?}");
}

#[test]
fn annotation_sync_retains_conflicting_edits_until_user_resolves_them() {
    let (root, graph, book, path) = annotation_fixture();
    let first = create_annotation(&graph, &book, "Base annotation");
    let base = fs::read(root.path().join(&path)).unwrap();
    let backend = Remote::default();
    let engine = SyncEngine::new(root.path().to_path_buf());
    assert!(engine.sync(&backend).unwrap().errors.is_empty());
    graph
        .book_note_save(
            &book.id,
            &first.id,
            Some(&first.revision),
            "Local edit",
            "",
            None,
            &book.source_sha256,
        )
        .unwrap();
    let local = fs::read(root.path().join(&path)).unwrap();
    write(root.path(), &path, &base);
    graph
        .book_note_save(
            &book.id,
            &first.id,
            Some(&first.revision),
            "Remote edit",
            "",
            None,
            &book.source_sha256,
        )
        .unwrap();
    backend.set(&path, &fs::read(root.path().join(&path)).unwrap());
    write(root.path(), &path, &local);
    let result = engine.sync(&backend).unwrap();
    assert!(result.errors.is_empty(), "{result:?}");
    assert!(result.conflicts.is_empty(), "{result:?}");
    assert_eq!(result.annotation_conflicts, [path.clone()]);
    assert!(engine.unresolved_conflicts().is_empty());
    let note = graph.book_notes_list(&book.id).unwrap().remove(0);
    assert_eq!(note.conflicts.len(), 2);
    assert!(note
        .conflicts
        .iter()
        .any(|version| version.body == "Local edit"));
    assert!(note
        .conflicts
        .iter()
        .any(|version| version.body == "Remote edit"));
    assert!(graph
        .book_note_save(
            &book.id,
            &note.id,
            Some(&note.revision),
            "Implicit resolution",
            "",
            None,
            &book.source_sha256,
        )
        .is_err());
    let repeated = engine.sync(&backend).unwrap();
    assert_eq!(repeated.annotation_conflicts, [path.clone()]);
    graph
        .book_note_resolve(
            &book.id,
            &note.id,
            &note.revision,
            "Both ideas, merged by user",
            "",
            None,
            &book.source_sha256,
            false,
        )
        .unwrap();
    let result = engine.sync(&backend).unwrap();
    assert!(result.errors.is_empty(), "{result:?}");
    assert!(result.annotation_conflicts.is_empty(), "{result:?}");
    assert_eq!(
        fs::read(root.path().join(&path)).unwrap(),
        backend.bytes(&path).unwrap()
    );
    let merged = graph.book_notes_list(&book.id).unwrap().remove(0);
    assert!(merged.conflicts.is_empty());
    assert_eq!(merged.body, "Both ideas, merged by user");
}

#[test]
fn annotation_tombstone_survives_a_stale_replica_and_edit_delete_is_a_conflict() {
    for edit_remote in [false, true] {
        let (root, graph, book, path) = annotation_fixture();
        let note = create_annotation(&graph, &book, "Base");
        let base = fs::read(root.path().join(&path)).unwrap();
        let backend = Remote::default();
        let engine = SyncEngine::new(root.path().to_path_buf());
        assert!(engine.sync(&backend).unwrap().errors.is_empty());
        if edit_remote {
            graph
                .book_note_save(
                    &book.id,
                    &note.id,
                    Some(&note.revision),
                    "Offline edit",
                    "",
                    None,
                    &book.source_sha256,
                )
                .unwrap();
            backend.set(&path, &fs::read(root.path().join(&path)).unwrap());
            write(root.path(), &path, &base);
        }
        graph
            .book_note_delete(&book.id, &note.id, &note.revision)
            .unwrap();
        assert!(
            root.path().join(&path).is_file(),
            "deletion must keep the tombstone"
        );
        let result = engine.sync(&backend).unwrap();
        assert!(result.errors.is_empty(), "{result:?}");
        let notes = graph.book_notes_list(&book.id).unwrap();
        if edit_remote {
            assert_eq!(result.annotation_conflicts, [path.clone()]);
            assert_eq!(notes.len(), 1);
            assert!(notes[0].conflicts.iter().any(|candidate| candidate.deleted));
            assert!(notes[0]
                .conflicts
                .iter()
                .any(|candidate| candidate.body == "Offline edit"));
        } else {
            assert!(notes.is_empty());
            backend.set(&path, &base);
            let result = engine.sync(&backend).unwrap();
            assert!(result.errors.is_empty(), "{result:?}");
            assert!(graph.book_notes_list(&book.id).unwrap().is_empty());
        }
    }
}

#[test]
fn annotation_sidecar_missing_or_invalid_never_discards_the_surviving_notes() {
    let (root, graph, book, path) = annotation_fixture();
    create_annotation(&graph, &book, "Keep me");
    let local = fs::read(root.path().join(&path)).unwrap();
    let backend = Remote::default();
    let engine = SyncEngine::new(root.path().to_path_buf());
    assert!(engine.sync(&backend).unwrap().errors.is_empty());
    backend.delete(&path);
    let result = engine.sync(&backend).unwrap();
    assert_eq!(result.conflicts, [path.clone()]);
    assert!(result.deleted_local.is_empty());
    assert_eq!(fs::read(root.path().join(&path)).unwrap(), local);
    backend.set(&path, b"malformed annotation sidecar");
    let result = engine.sync(&backend).unwrap();
    assert!(!result.errors.is_empty());
    assert_eq!(result.conflicts, [path.clone()]);
    assert_eq!(fs::read(root.path().join(&path)).unwrap(), local);
    assert_eq!(
        backend.bytes(&path).unwrap(),
        b"malformed annotation sidecar"
    );
}

#[test]
fn annotation_merge_does_not_overwrite_an_edit_during_download() {
    let (root, graph, book, path) = annotation_fixture();
    let note = create_annotation(&graph, &book, "Base");
    let base = fs::read(root.path().join(&path)).unwrap();
    graph
        .book_note_save(
            &book.id,
            &note.id,
            Some(&note.revision),
            "New local edit",
            "",
            None,
            &book.source_sha256,
        )
        .unwrap();
    let newer = fs::read(root.path().join(&path)).unwrap();
    let sync_root = tempfile::tempdir().unwrap();
    write(sync_root.path(), &path, &base);
    let backend = Arc::new(Remote::default());
    backend.set(&path, &base);
    let (ready, proceed) = arm_gate(&backend.download);
    let directory = sync_root.path().to_path_buf();
    let remote = backend.clone();
    let worker =
        std::thread::spawn(move || SyncEngine::new(directory).sync(remote.as_ref()).unwrap());
    ready
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    crate::fsutil::atomic_write(&sync_root.path().join(&path), &newer).unwrap();
    proceed.send(()).unwrap();
    let result = worker.join().unwrap();
    assert!(!result.errors.is_empty(), "{result:?}");
    assert_eq!(fs::read(sync_root.path().join(&path)).unwrap(), newer);
    assert_eq!(backend.bytes(&path).unwrap(), base);
    let result = SyncEngine::new(sync_root.path().to_path_buf())
        .sync(backend.as_ref())
        .unwrap();
    assert!(result.errors.is_empty(), "{result:?}");
    assert!(result.conflicts.is_empty());
    assert_eq!(
        backend.bytes(&path).unwrap(),
        fs::read(sync_root.path().join(&path)).unwrap()
    );
}

#[test]
fn annotation_merge_remote_publication_race_preserves_both_and_retries() {
    let (root, graph, book, path) = annotation_fixture();
    let note = create_annotation(&graph, &book, "Base");
    let base = fs::read(root.path().join(&path)).unwrap();
    graph
        .book_note_save(
            &book.id,
            &note.id,
            Some(&note.revision),
            "Local edit",
            "",
            None,
            &book.source_sha256,
        )
        .unwrap();
    let local = fs::read(root.path().join(&path)).unwrap();
    write(root.path(), &path, &base);
    graph
        .book_note_save(
            &book.id,
            &note.id,
            Some(&note.revision),
            "Racing remote edit",
            "",
            None,
            &book.source_sha256,
        )
        .unwrap();
    let newer = fs::read(root.path().join(&path)).unwrap();
    let sync_root = tempfile::tempdir().unwrap();
    write(sync_root.path(), &path, &local);
    let backend = Remote::default();
    backend.set(&path, &base);
    backend.set(".grafium-sync-id", uuid::Uuid::new_v4().to_string().as_bytes());
    *backend.publication_race.lock().unwrap() = Some(newer.clone());
    let engine = SyncEngine::new(sync_root.path().to_path_buf());
    let result = engine.sync(&backend).unwrap();
    assert!(!result.errors.is_empty(), "{result:?}");
    assert_eq!(fs::read(sync_root.path().join(&path)).unwrap(), local);
    assert_eq!(backend.bytes(&path).unwrap(), newer);
    assert!(recovery_contains(sync_root.path(), &path, &local));
    assert!(recovery_contains(sync_root.path(), &path, &newer));
    let result = engine.sync(&backend).unwrap();
    assert!(result.errors.is_empty(), "{result:?}");
    assert_eq!(result.annotation_conflicts, [path.clone()]);
    assert!(engine.unresolved_conflicts().is_empty());
    assert_eq!(
        backend.bytes(&path).unwrap(),
        fs::read(sync_root.path().join(&path)).unwrap()
    );
}
