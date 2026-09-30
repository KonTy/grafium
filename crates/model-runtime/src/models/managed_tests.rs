use super::*;
use std::{
    net::TcpListener,
    sync::{atomic::Ordering, Barrier, Mutex},
    thread,
};

fn test_dir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap()
}

fn library(dir: &tempfile::TempDir) -> ModelLibrary {
    ModelLibrary::new(dir.path().join("models"))
        .unwrap()
        .with_disk_space_probe(Arc::new(|_: &Path| Ok(u64::MAX)))
}

fn source(dir: &tempfile::TempDir, name: &str, body: &[u8]) -> PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, body).unwrap();
    path
}

fn digest(body: &[u8]) -> String {
    format!("{:x}", Sha256::digest(body))
}

fn no_stages(library: &ModelLibrary) {
    assert!(fs::read_dir(library.root()).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".part")));
}

#[test]
fn imports_are_durable_owned_copies_and_repeat_import_is_idempotent() {
    let dir = test_dir();
    let library = library(&dir);
    let original = source(&dir, "ggml-small.bin", b"original bytes");
    let imported = library.import(&original).unwrap();
    assert!(!imported.already_present);
    assert!(imported.entry.owned_id.is_some());
    assert_eq!(
        imported.entry.sha256.as_deref(),
        Some(digest(b"original bytes").as_str())
    );
    assert_eq!(fs::read(&original).unwrap(), b"original bytes");
    assert_eq!(
        fs::read(&imported.entry.info.path).unwrap(),
        b"original bytes"
    );
    assert_eq!(library.import(&original).unwrap().entry, imported.entry);
    assert!(library.import(&original).unwrap().already_present);
    assert_eq!(
        ModelLibrary::new(library.root()).unwrap().list().unwrap(),
        vec![imported.entry]
    );
    no_stages(&library);
}

#[test]
fn preexisting_files_stay_unmanaged_and_cannot_be_removed_or_overwritten() {
    let dir = test_dir();
    let library = library(&dir);
    let existing = library.root().join("user.gguf");
    fs::write(&existing, b"users model").unwrap();
    let imported = library.import(&existing).unwrap();
    assert!(imported.already_present);
    assert!(imported.entry.owned_id.is_none());
    assert!(library
        .quarantine_owned("user.gguf", RemovalApproval::ExplicitUserRequest)
        .is_err());
    let different = source(&dir, "user.gguf", b"different bytes");
    assert!(library.import(&different).is_err());
    assert_eq!(fs::read(existing).unwrap(), b"users model");
    assert_eq!(fs::read(different).unwrap(), b"different bytes");
    no_stages(&library);
}

#[test]
fn independent_stages_publish_concurrently_without_losing_manifest_records() {
    let dir = test_dir();
    let first = Arc::new(library(&dir));
    let second = Arc::new(ModelLibrary::new(first.root()).unwrap());
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = [
        (first.clone(), "a.gguf", b"first".as_slice()),
        (second, "b.gguf", b"second".as_slice()),
    ]
    .into_iter()
    .map(|(library, name, bytes)| {
        let barrier = barrier.clone();
        thread::spawn(move || {
            let mut stage = library.stage().unwrap();
            stage.file.write_all(bytes).unwrap();
            stage.file.sync_all().unwrap();
            barrier.wait();
            library
                .publish(&stage, name, bytes.len() as u64, digest(bytes))
                .unwrap()
        })
    })
    .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_ne!(results[0].entry.owned_id, results[1].entry.owned_id);
    assert_eq!(first.list().unwrap().len(), 2);
    assert_eq!(fs::read(first.root().join("a.gguf")).unwrap(), b"first");
    assert_eq!(fs::read(first.root().join("b.gguf")).unwrap(), b"second");
    no_stages(&first);
}

#[test]
fn concurrent_same_name_imports_do_not_truncate_the_winner() {
    let dir = test_dir();
    let library = Arc::new(library(&dir));
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = [b"first".as_slice(), b"second".as_slice()]
        .into_iter()
        .map(|bytes| {
            let library = library.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut stage = library.stage().unwrap();
                stage.file.write_all(bytes).unwrap();
                stage.file.sync_all().unwrap();
                barrier.wait();
                library.publish(&stage, "same.gguf", bytes.len() as u64, digest(bytes))
            })
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let entries = library.list().unwrap();
    assert_eq!(entries.len(), 1);
    let bytes = fs::read(&entries[0].info.path).unwrap();
    assert!(bytes == b"first" || bytes == b"second");
    assert_eq!(entries[0].sha256.as_deref(), Some(digest(&bytes).as_str()));
    no_stages(&library);
}

#[test]
fn manifest_corruption_fails_closed_without_deleting_or_adopting_files() {
    let dir = test_dir();
    let library = library(&dir);
    let original = source(&dir, "owned.gguf", b"owned");
    let imported = library.import(&original).unwrap();
    let manifest_path = library.root().join(MANIFEST_NAME);
    fs::write(&manifest_path, b"{ broken").unwrap();
    assert!(ModelLibrary::new(library.root()).is_err());
    assert!(library.list().is_err());
    assert!(library.import(&source(&dir, "new.gguf", b"new")).is_err());
    assert!(library
        .quarantine_owned(
            imported.entry.owned_id.as_deref().unwrap(),
            RemovalApproval::ExplicitUserRequest
        )
        .is_err());
    assert_eq!(fs::read(&imported.entry.info.path).unwrap(), b"owned");
    assert!(!library.root().join("new.gguf").exists());
    assert_eq!(fs::read(manifest_path).unwrap(), b"{ broken");
    no_stages(&library);
}

#[test]
fn oversized_and_duplicate_manifests_are_rejected() {
    let dir = test_dir();
    let library = library(&dir);
    let imported = library
        .import(&source(&dir, "owned.gguf", b"model"))
        .unwrap();
    let mut manifest = library.read_manifest().unwrap();
    manifest.models.push(manifest.models[0].clone());
    fs::write(
        library.root().join(MANIFEST_NAME),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert!(library.list().is_err());
    let file = OpenOptions::new()
        .write(true)
        .open(library.root().join(MANIFEST_NAME))
        .unwrap();
    file.set_len(MAX_MANIFEST_BYTES + 1).unwrap();
    assert!(library.list().is_err());
    assert_eq!(fs::read(imported.entry.info.path).unwrap(), b"model");
}

#[test]
fn owned_removal_is_reversible_and_preserves_unrelated_files_and_sources() {
    let dir = test_dir();
    let library = library(&dir);
    let original = source(&dir, "owned.gguf", b"model");
    let imported = library.import(&original).unwrap();
    fs::write(library.root().join("unmanaged.gguf"), b"user").unwrap();
    fs::write(library.root().join("backup.bak"), b"backup").unwrap();
    fs::create_dir(library.root().join("user-folder")).unwrap();
    fs::write(library.root().join("user-folder/notes"), b"notes").unwrap();
    let result = library
        .quarantine_owned(
            imported.entry.owned_id.as_deref().unwrap(),
            RemovalApproval::ExplicitUserRequest,
        )
        .unwrap();
    assert!(!result.original_path.exists());
    assert_eq!(fs::read(&result.quarantine_path).unwrap(), b"model");
    assert_eq!(fs::read(original).unwrap(), b"model");
    assert_eq!(
        fs::read(library.root().join("unmanaged.gguf")).unwrap(),
        b"user"
    );
    assert_eq!(
        fs::read(library.root().join("backup.bak")).unwrap(),
        b"backup"
    );
    assert_eq!(
        fs::read(library.root().join("user-folder/notes")).unwrap(),
        b"notes"
    );
    assert!(library
        .list()
        .unwrap()
        .iter()
        .all(|entry| entry.owned_id.is_none()));
    assert!(library
        .quarantine_owned(&result.id, RemovalApproval::ExplicitUserRequest)
        .is_err());
    assert!(library
        .resolve_with_vram(&LocalModelRef::named("owned.gguf"), ModelKind::Llm, None)
        .is_err());
    assert!(library
        .resolve_with_vram(
            &LocalModelRef::named(result.quarantine_path.to_string_lossy()),
            ModelKind::Llm,
            None,
        )
        .is_err());
    no_stages(&library);
}

#[test]
fn resident_shared_inode_leases_refuse_removal_until_every_reader_releases() {
    let dir = test_dir();
    let library = library(&dir);
    let original = source(&dir, "owned.gguf", b"model");
    let imported = library.import(&original).unwrap();
    let id = imported.entry.owned_id.as_deref().unwrap();
    let alias = dir.path().join("another-app-model.gguf");
    fs::hard_link(&imported.entry.info.path, &alias).unwrap();
    let first_reader = File::open(&imported.entry.info.path).unwrap();
    let alias_reader = File::open(&alias).unwrap();
    FileExt::try_lock_shared(&first_reader).unwrap();
    FileExt::try_lock_shared(&alias_reader).unwrap();
    let manifest_before = fs::read(library.root().join(MANIFEST_NAME)).unwrap();

    let error = library
        .quarantine_owned(id, RemovalApproval::ExplicitUserRequest)
        .unwrap_err();
    assert!(error.to_string().contains("in use"));
    assert_eq!(fs::read(&imported.entry.info.path).unwrap(), b"model");
    assert_eq!(
        fs::read(library.root().join(MANIFEST_NAME)).unwrap(),
        manifest_before
    );
    drop(first_reader);
    assert!(library
        .quarantine_owned(id, RemovalApproval::ExplicitUserRequest)
        .unwrap_err()
        .to_string()
        .contains("in use"));
    drop(alias_reader);

    let removed = library
        .quarantine_owned(id, RemovalApproval::ExplicitUserRequest)
        .unwrap();
    assert!(!imported.entry.info.path.exists());
    assert_eq!(fs::read(&removed.quarantine_path).unwrap(), b"model");
    assert_eq!(fs::read(original).unwrap(), b"model");
    assert_eq!(fs::read(alias).unwrap(), b"model");
    no_stages(&library);
}

#[test]
fn another_exclusive_inode_lease_refuses_removal_without_waiting() {
    let dir = test_dir();
    let library = library(&dir);
    let imported = library
        .import(&source(&dir, "owned.gguf", b"model"))
        .unwrap();
    let lease = File::open(&imported.entry.info.path).unwrap();
    FileExt::try_lock_exclusive(&lease).unwrap();
    let started = Instant::now();
    assert!(library
        .quarantine_owned(
            imported.entry.owned_id.as_deref().unwrap(),
            RemovalApproval::ExplicitUserRequest,
        )
        .unwrap_err()
        .to_string()
        .contains("in use"));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(fs::read(&imported.entry.info.path).unwrap(), b"model");
}

#[test]
fn publishing_retains_an_exclusive_inode_lease_until_stage_cleanup() {
    let dir = test_dir();
    let library = library(&dir);
    let mut stage = library.stage().unwrap();
    stage.file.write_all(b"model").unwrap();
    stage.file.sync_all().unwrap();
    let imported = library
        .publish(&stage, "owned.gguf", 5, digest(b"model"))
        .unwrap();
    let reader = File::open(&imported.entry.info.path).unwrap();
    assert!(FileExt::try_lock_shared(&reader).is_err());
    drop(stage);
    FileExt::try_lock_shared(&reader).unwrap();
    assert_eq!(fs::read(&imported.entry.info.path).unwrap(), b"model");
    no_stages(&library);
}

#[test]
fn configured_and_auto_resolved_paths_are_canonical() {
    let dir = test_dir();
    let library = library(&dir);
    let imported = library
        .import(&source(&dir, "owned.gguf", b"model"))
        .unwrap();
    let canonical = imported.entry.info.path.canonicalize().unwrap();
    for configured in [None, Some("owned.gguf")] {
        let resolved =
            resolve_model_with_vram(configured, &library.root().join("."), ModelKind::Llm, None)
                .unwrap();
        assert_eq!(resolved.as_os_str(), canonical.as_os_str());
    }
    let external = source(&dir, "external.gguf", b"external");
    let noncanonical = dir.path().join("./external.gguf");
    let resolved =
        resolve_model_with_vram(noncanonical.to_str(), library.root(), ModelKind::Llm, None)
            .unwrap();
    assert_eq!(
        resolved.as_os_str(),
        external.canonicalize().unwrap().as_os_str()
    );
}

#[test]
fn modified_or_replaced_owned_files_cannot_be_removed() {
    let dir = test_dir();
    let library = library(&dir);
    let imported = library
        .import(&source(&dir, "owned.gguf", b"model"))
        .unwrap();
    fs::write(&imported.entry.info.path, b"other").unwrap();
    assert!(library
        .quarantine_owned(
            imported.entry.owned_id.as_deref().unwrap(),
            RemovalApproval::ExplicitUserRequest
        )
        .is_err());
    assert_eq!(fs::read(&imported.entry.info.path).unwrap(), b"other");
    let replacement = source(&dir, "replacement", b"model");
    fs::rename(replacement, &imported.entry.info.path).unwrap();
    assert!(library
        .quarantine_owned(
            imported.entry.owned_id.as_deref().unwrap(),
            RemovalApproval::ExplicitUserRequest
        )
        .is_err());
    assert_eq!(fs::read(&imported.entry.info.path).unwrap(), b"model");
}

#[test]
fn missing_owned_file_does_not_allow_a_duplicate_manifest_path() {
    let dir = test_dir();
    let library = library(&dir);
    let original = source(&dir, "owned.gguf", b"owned");
    let imported = library.import(&original).unwrap();
    fs::rename(
        &imported.entry.info.path,
        library.root().join("renamed.gguf"),
    )
    .unwrap();
    assert!(library.import(&original).is_err());
    assert!(library.list().is_ok());
}

#[test]
fn traversal_partial_and_reserved_names_are_rejected() {
    for name in [
        "../bad.gguf",
        "dir/model.gguf",
        r"dir\model.gguf",
        "C:bad.gguf",
        ".",
        "..",
        "",
        "foo.part",
        "foo.tmp",
        ".model-library-manifest.json",
        "bad\0.gguf",
    ] {
        assert!(validate_basename(name).is_err(), "{name:?}");
    }
    let dir = test_dir();
    let library = library(&dir);
    source(&dir, "outside.gguf", b"original");
    assert!(library
        .resolve_with_vram(
            &LocalModelRef::named("../outside.gguf"),
            ModelKind::Llm,
            None
        )
        .is_err());
    assert!(ModelLibrary::new(dir.path().join("models/../other")).is_err());
    assert!(library
        .import(&source(&dir, "unfinished.gguf.part", b"partial"))
        .is_err());
}

#[cfg(unix)]
#[test]
fn symlinks_are_readable_but_cannot_be_used_for_owned_mutations() {
    use std::os::unix::fs::symlink;
    let dir = test_dir();
    let library = library(&dir);
    let original = source(&dir, "original.gguf", b"original");
    let linked_source = dir.path().join("linked.gguf");
    symlink(&original, &linked_source).unwrap();
    assert!(library.import(&linked_source).is_err());
    let destination = library.root().join("original.gguf");
    symlink(&original, &destination).unwrap();
    assert!(library.import(&original).is_err());
    let entries = library.scan().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].owned_id.is_none());
    assert_eq!(
        library
            .resolve_with_vram(&LocalModelRef::named("original.gguf"), ModelKind::Llm, None)
            .unwrap(),
        original.canonicalize().unwrap()
    );
    let linked_root = dir.path().join("linked-root");
    symlink(library.root(), &linked_root).unwrap();
    let linked_library = ModelLibrary::new(linked_root).unwrap();
    assert_eq!(linked_library.list().unwrap().len(), 1);
    assert!(linked_library.import(&original).is_err());
    symlink(&original, library.root().join(MANIFEST_NAME)).unwrap();
    assert!(library.import(&source(&dir, "new.gguf", b"new")).is_err());
    assert_eq!(fs::read(original).unwrap(), b"original");
}

#[cfg(unix)]
#[test]
fn linked_model_files_are_discoverable_unowned_and_resolve_to_stable_targets() {
    use std::os::unix::fs::symlink;
    let dir = test_dir();
    let library = library(&dir);
    let first = source(&dir, "first.gguf", b"first model");
    let second = source(&dir, "second.gguf", b"second model");
    let alias = library.root().join("linked.gguf");
    symlink(&first, &alias).unwrap();
    symlink(
        dir.path().join("missing"),
        library.root().join("broken.gguf"),
    )
    .unwrap();
    symlink(dir.path(), library.root().join("folder.gguf")).unwrap();
    let entries = library.list().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].info.file_name, "linked.gguf");
    assert_eq!(entries[0].info.size_bytes, b"first model".len() as u64);
    assert!(entries[0].owned_id.is_none());
    assert!(entries[0].sha256.is_none());
    let resolved = LocalModelRef::named(alias.to_string_lossy())
        .resolve(library.root(), ModelKind::Llm)
        .unwrap();
    assert_eq!(resolved, first.canonicalize().unwrap());
    assert_eq!(
        library
            .resolve_with_vram(&LocalModelRef::default(), ModelKind::Llm, None)
            .unwrap(),
        resolved
    );
    let replacement = dir.path().join("replacement-link");
    symlink(&second, &replacement).unwrap();
    fs::rename(replacement, alias).unwrap();
    assert_eq!(fs::read(&resolved).unwrap(), b"first model");
    assert_eq!(
        LocalModelRef::named("linked.gguf")
            .resolve(library.root(), ModelKind::Llm)
            .unwrap(),
        second.canonicalize().unwrap()
    );
    assert!(library
        .quarantine_owned("linked.gguf", RemovalApproval::ExplicitUserRequest)
        .is_err());
    assert_eq!(fs::read(first).unwrap(), b"first model");
    assert_eq!(fs::read(second).unwrap(), b"second model");
}

#[cfg(unix)]
#[test]
fn symlinked_ancestors_and_library_roots_allow_read_only_use_without_creating_files() {
    use std::os::unix::fs::symlink;
    let dir = test_dir();
    let actual_parent = dir.path().join("private");
    let actual_models = actual_parent.join("models");
    fs::create_dir_all(&actual_models).unwrap();
    let actual_file = actual_models.join("user.gguf");
    fs::write(&actual_file, b"user model").unwrap();
    let linked_parent = dir.path().join("var");
    symlink(&actual_parent, &linked_parent).unwrap();
    let linked_models = linked_parent.join("models");
    let library = ModelLibrary::new(&linked_models).unwrap();
    assert_eq!(scan_models_dir(&linked_models).unwrap().len(), 1);
    assert_eq!(library.list().unwrap().len(), 1);
    assert!(library.list().unwrap()[0].owned_id.is_none());
    assert_eq!(
        LocalModelRef::named(linked_models.join("user.gguf").to_string_lossy())
            .resolve(dir.path(), ModelKind::Llm)
            .unwrap(),
        actual_file.canonicalize().unwrap()
    );
    assert_eq!(
        LocalModelRef::named("user.gguf")
            .resolve(&linked_models, ModelKind::Llm)
            .unwrap(),
        actual_file.canonicalize().unwrap()
    );
    assert!(library
        .import(&source(&dir, "new.gguf", b"new"))
        .unwrap_err()
        .to_string()
        .contains("read-only"));
    assert!(library
        .quarantine_owned("user.gguf", RemovalApproval::ExplicitUserRequest)
        .is_err());
    assert_eq!(fs::read_dir(&actual_models).unwrap().count(), 1);
    assert!(!actual_models.join(LOCK_NAME).exists());
    assert!(!actual_models.join(MANIFEST_NAME).exists());
    assert_eq!(fs::read(actual_file).unwrap(), b"user model");
}

#[cfg(unix)]
#[test]
fn replacing_an_owned_path_with_a_symlink_does_not_transfer_ownership() {
    use std::os::unix::fs::symlink;
    let dir = test_dir();
    let library = library(&dir);
    let imported = library
        .import(&source(&dir, "owned.gguf", b"owned"))
        .unwrap();
    let backup = library.root().join("user-preserved-backup.bak");
    fs::rename(&imported.entry.info.path, &backup).unwrap();
    let external = source(&dir, "external.gguf", b"external");
    symlink(&external, &imported.entry.info.path).unwrap();
    let entry = library
        .list()
        .unwrap()
        .into_iter()
        .find(|entry| entry.info.file_name == "owned.gguf")
        .unwrap();
    assert!(entry.owned_id.is_none());
    assert!(entry.sha256.is_none());
    assert_eq!(
        library
            .resolve_with_vram(&LocalModelRef::named("owned.gguf"), ModelKind::Llm, None)
            .unwrap(),
        external.canonicalize().unwrap()
    );
    assert!(library
        .quarantine_owned(
            imported.entry.owned_id.as_deref().unwrap(),
            RemovalApproval::ExplicitUserRequest
        )
        .is_err());
    assert_eq!(fs::read(external).unwrap(), b"external");
    assert_eq!(fs::read(backup).unwrap(), b"owned");
}

#[cfg(unix)]
#[test]
fn model_aliases_cannot_expose_partial_downloads_or_quarantine_backups() {
    use std::os::unix::fs::symlink;
    let dir = test_dir();
    let library = library(&dir);
    for (name, target) in [
        ("partial.gguf", "download.gguf.part"),
        ("removed.gguf", ".model-library-trash-backup.quarantine"),
    ] {
        let hidden = source(&dir, target, b"not selectable");
        symlink(hidden, library.root().join(name)).unwrap();
        assert!(LocalModelRef::named(name)
            .resolve(library.root(), ModelKind::Llm)
            .is_err());
    }
    assert!(library.list().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn replaced_root_does_not_redirect_mutations_outside_the_original_directory() {
    use std::os::unix::fs::symlink;
    let dir = test_dir();
    let library = library(&dir);
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::rename(library.root(), dir.path().join("old-root")).unwrap();
    symlink(&outside, library.root()).unwrap();
    assert!(library
        .import(&source(&dir, "source.gguf", b"source"))
        .is_err());
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
}

#[test]
fn ownership_lock_wait_has_a_deadline() {
    let dir = test_dir();
    let library = library(&dir);
    let _lock = library.lock().unwrap();
    let started = Instant::now();
    assert!(library.list().unwrap_err().to_string().contains("busy"));
    assert!(started.elapsed() >= LOCK_TIMEOUT);
    assert!(started.elapsed() < Duration::from_secs(10));
}

fn gguf_text(buffer: &mut Vec<u8>, value: &str) {
    buffer.extend((value.len() as u64).to_le_bytes());
    buffer.extend(value.as_bytes());
}

#[test]
fn catalog_metadata_is_pure_rust_and_hostile_headers_are_only_missing_metadata() {
    let dir = test_dir();
    let mut bytes = b"GGUF".to_vec();
    bytes.extend(3u32.to_le_bytes());
    bytes.extend(0u64.to_le_bytes());
    bytes.extend(2u64.to_le_bytes());
    for (key, value) in [
        ("general.architecture", "llama"),
        ("general.name", "Test model"),
    ] {
        gguf_text(&mut bytes, key);
        bytes.extend(8u32.to_le_bytes());
        gguf_text(&mut bytes, value);
    }
    source(&dir, "tiny.gguf", &bytes);
    let info = scan_models_dir(dir.path()).unwrap().remove(0);
    assert_eq!(info.architecture.as_deref(), Some("llama"));
    assert!(info.description.as_deref().unwrap().contains("Test model"));
    let mut hostile = bytes[..24].to_vec();
    hostile.extend(u64::MAX.to_le_bytes());
    source(&dir, "hostile.gguf", &hostile);
    let info = scan_models_dir(dir.path()).unwrap().remove(0);
    assert_eq!(info.file_name, "hostile.gguf");
    assert!(info.architecture.is_none());
    assert!(info.description.is_none());
}

#[test]
fn model_ref_preserves_the_existing_serde_shape() {
    let reference: LocalModelRef = serde_json::from_str(r#"{"model":"file.gguf"}"#).unwrap();
    assert_eq!(reference, LocalModelRef::named("file.gguf"));
    assert_eq!(
        serde_json::to_string(&LocalModelRef::default()).unwrap(),
        r#"{"model":null}"#
    );
}

fn fixture(response: Vec<u8>) -> (String, thread::JoinHandle<()>) {
    fixture_checked(response, Duration::ZERO, |_| {})
}

fn fixture_checked(
    response: Vec<u8>,
    delay: Duration,
    check_request: impl FnOnce(&[u8]) + Send + 'static,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while request.len() < 8192 {
            if stream.read(&mut byte).unwrap_or(0) == 0 {
                break;
            }
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        assert!(request.starts_with(b"GET /model"));
        check_request(&request);
        thread::sleep(delay);
        let _ = stream.write_all(&response);
    });
    (format!("http://{address}/model"), worker)
}

fn request(url: String, body: &[u8], max_bytes: u64) -> DownloadRequest {
    DownloadRequest {
        url,
        file_name: "download.gguf".into(),
        expected_sha256: digest(body),
        max_bytes,
    }
}

fn response(body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend(body);
    response
}

#[tokio::test]
async fn download_verifies_streamed_bytes_before_publishing_and_reports_progress() {
    let dir = test_dir();
    let library = library(&dir);
    let body = b"downloaded model bytes";
    let (url, worker) = fixture(response(body));
    let updates = Mutex::new(Vec::new());
    let notify = |progress| updates.lock().unwrap().push(progress);
    let imported = library
        .download(
            request(url, body, 1024),
            &NetworkConfig::new(Duration::from_secs(3)).unwrap(),
            None,
            Some(&notify),
        )
        .await
        .unwrap();
    worker.join().unwrap();
    assert_eq!(fs::read(imported.entry.info.path).unwrap(), body);
    assert_eq!(imported.entry.sha256, Some(digest(body)));
    assert!(imported.entry.owned_id.is_some());
    let updates = updates.into_inner().unwrap();
    assert_eq!(updates.first().unwrap().downloaded_bytes, 0);
    assert_eq!(updates.last().unwrap().downloaded_bytes, body.len() as u64);
    assert_eq!(updates.last().unwrap().total_bytes, Some(body.len() as u64));
    no_stages(&library);
}

#[tokio::test]
async fn checksum_failure_oversize_and_cancel_leave_nothing_selectable() {
    for scenario in ["checksum", "length", "chunked", "cancel"] {
        let dir = test_dir();
        let library = library(&dir);
        let body = b"model bytes";
        let wire = if scenario == "chunked" {
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nb\r\nmodel bytes\r\n0\r\n\r\n".to_vec()
        } else {
            response(body)
        };
        let (url, worker) = fixture(wire);
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let notify = move |progress: DownloadProgress| {
            if scenario == "cancel" && progress.downloaded_bytes > 0 {
                signal.store(true, Ordering::Release);
            }
        };
        let max_bytes = if scenario == "length" || scenario == "chunked" {
            3
        } else {
            1024
        };
        let mut request = request(url, body, max_bytes);
        if scenario == "checksum" {
            request.expected_sha256 = digest(b"different bytes");
        }
        let result = library
            .download(
                request,
                &NetworkConfig::new(Duration::from_secs(3)).unwrap(),
                Some(cancel),
                Some(&notify),
            )
            .await;
        assert!(result.is_err(), "{scenario}");
        if scenario == "cancel" {
            assert!(matches!(result, Err(RuntimeError::Cancelled)));
        }
        worker.join().unwrap();
        assert!(library.list().unwrap().is_empty(), "{scenario}");
        assert!(!library.root().join("download.gguf").exists(), "{scenario}");
        no_stages(&library);
    }
}

#[tokio::test]
async fn aborting_download_future_cleans_only_its_own_stage() {
    let dir = test_dir();
    let library = library(&dir);
    let unrelated_stage = library.root().join(".model-library-stage-unrelated.part");
    fs::write(&unrelated_stage, b"another operation").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/model", listener.local_addr().unwrap());
    let network = NetworkConfig::new(Duration::from_secs(3)).unwrap();
    let mut future = Box::pin(library.download(request(url, b"body", 100), &network, None, None));
    tokio::select! {
        _ = &mut future => panic!("server has not replied"),
        accepted = listener.accept() => { accepted.unwrap(); }
    }
    drop(future);
    assert_eq!(fs::read(&unrelated_stage).unwrap(), b"another operation");
    assert_eq!(
        fs::read_dir(library.root())
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part"))
            .count(),
        1
    );
}

#[tokio::test]
async fn cancellation_interrupts_a_stalled_http_response() {
    let dir = test_dir();
    let library = library(&dir);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/model", listener.local_addr().unwrap());
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let network = NetworkConfig::new(Duration::from_secs(3)).unwrap();
    let mut future =
        Box::pin(library.download(request(url, b"body", 100), &network, Some(cancel), None));
    let stream = tokio::select! {
        _ = &mut future => panic!("server has not replied"),
        accepted = listener.accept() => accepted.unwrap().0,
    };
    signal.store(true, Ordering::Release);
    assert!(matches!(
        tokio::time::timeout(Duration::from_millis(500), &mut future)
            .await
            .unwrap(),
        Err(RuntimeError::Cancelled)
    ));
    drop(future);
    drop(stream);
    assert!(library.list().unwrap().is_empty());
    no_stages(&library);
}

#[tokio::test]
async fn denied_redirect_hops_and_credential_urls_never_make_a_request() {
    let dir = test_dir();
    let library = library(&dir);
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let redirect = format!(
        "HTTP/1.1 302 Found\r\nLocation: http://{}/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        target.local_addr().unwrap()
    );
    let (url, worker) = fixture(redirect.into_bytes());
    let denied_port = target.local_addr().unwrap().port();
    let network = NetworkConfig::new(Duration::from_secs(3))
        .unwrap()
        .with_policy(Arc::new(move |url: &url::Url| {
            if url.port() == Some(denied_port) {
                Err(failure("Host denied redirect target"))
            } else {
                Ok(())
            }
        }));
    assert!(library
        .download(request(url, b"body", 1024), &network, None, None)
        .await
        .unwrap_err()
        .to_string()
        .contains("Host denied redirect target"));
    worker.join().unwrap();
    assert_eq!(
        target.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let credential_url = format!("http://name:secret@{}/model", target.local_addr().unwrap());
    let error = library
        .download(request(credential_url, b"body", 1024), &network, None, None)
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("secret"));
    assert_eq!(
        target.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(library.list().unwrap().is_empty());
    no_stages(&library);
}

#[tokio::test]
async fn host_policy_invalid_digests_precancellation_and_opaque_clients_fail_before_network() {
    let dir = test_dir();
    let library = library(&dir);
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let url = format!("http://{}/model", target.local_addr().unwrap());
    let network = NetworkConfig::new(Duration::from_secs(3)).unwrap();
    let mut invalid = request(url.clone(), b"body", 1024);
    invalid.expected_sha256.clear();
    assert!(library
        .download(invalid, &network, None, None)
        .await
        .is_err());
    let cancelled = Arc::new(AtomicBool::new(true));
    assert!(matches!(
        library
            .download(
                request(url.clone(), b"body", 1024),
                &network,
                Some(cancelled),
                None
            )
            .await,
        Err(RuntimeError::Cancelled)
    ));
    let denied = network.with_policy(Arc::new(|_: &url::Url| {
        Err(failure("Host denied download"))
    }));
    assert!(library
        .download(request(url.clone(), b"body", 1024), &denied, None, None)
        .await
        .unwrap_err()
        .to_string()
        .contains("Host denied"));
    let opaque =
        NetworkConfig::from_client(reqwest::Client::new(), Arc::new(|_: &url::Url| Ok(())));
    assert!(library
        .download(request(url, b"body", 1024), &opaque, None, None)
        .await
        .unwrap_err()
        .to_string()
        .contains("custom clients"));
    assert_eq!(
        target.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    no_stages(&library);
}

fn redirect_response(location: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nSet-Cookie: private=origin-only\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

#[tokio::test]
async fn allowed_redirects_authorize_every_hop_without_forwarding_credentials() {
    let dir = test_dir();
    let library = library(&dir);
    let body = b"CDN model bytes";
    let (destination, destination_worker) =
        fixture_checked(response(body), Duration::ZERO, |request| {
            let request = String::from_utf8_lossy(request).to_ascii_lowercase();
            assert!(request.starts_with("get /model http/1.1\r\n"));
            for private in [
                "authorization:",
                "proxy-authorization:",
                "cookie:",
                "referer:",
                "origin-secret",
            ] {
                assert!(!request.contains(private), "{request}");
            }
        });
    let (origin, origin_worker) = fixture(redirect_response(&destination));
    let origin = format!("{origin}?token=origin-secret");
    let authorized = Arc::new(Mutex::new(Vec::new()));
    let observations = authorized.clone();
    let network = NetworkConfig::new(Duration::from_secs(3))
        .unwrap()
        .with_policy(Arc::new(move |url: &url::Url| {
            observations.lock().unwrap().push(url.to_string());
            Ok(())
        }));
    let imported = library
        .download(request(origin.clone(), body, 1024), &network, None, None)
        .await
        .unwrap();
    origin_worker.join().unwrap();
    destination_worker.join().unwrap();
    assert_eq!(*authorized.lock().unwrap(), vec![origin, destination]);
    assert_eq!(fs::read(imported.entry.info.path).unwrap(), body);
    no_stages(&library);
}

#[tokio::test]
async fn redirect_loops_hop_limits_and_invalid_locations_clean_stages() {
    for scenario in ["loop", "limit", "missing", "credentials"] {
        let dir = test_dir();
        let library = library(&dir)
            .with_transfer_policy(TransferPolicy {
                max_redirects: if scenario == "limit" { 0 } else { 5 },
                ..TransferPolicy::default()
            })
            .unwrap();
        let response = match scenario {
            "loop" => redirect_response("/model"),
            "limit" => redirect_response("/model-next"),
            "missing" => {
                b"HTTP/1.1 302 Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            }
            _ => redirect_response("http://user:never-forward-me@127.0.0.1/model"),
        };
        let (url, worker) = fixture(response);
        let error = library
            .download(
                request(url, b"body", 1024),
                &NetworkConfig::new(Duration::from_secs(3)).unwrap(),
                None,
                None,
            )
            .await
            .unwrap_err();
        worker.join().unwrap();
        let message = error.to_string();
        assert!(!message.contains("never-forward-me"));
        let expected = match scenario {
            "loop" => "loop",
            "limit" => "redirect limit",
            "missing" => "Location",
            _ => "credentials",
        };
        assert!(message.contains(expected), "{scenario}: {message}");
        assert!(library.list().unwrap().is_empty());
        no_stages(&library);
    }
}

#[test]
fn redirect_validation_rejects_downgrades_credentials_and_non_http_schemes() {
    let https = url::Url::parse("https://models.example/model").unwrap();
    for location in [
        "http://cdn.example/model",
        "https://user:secret@cdn.example/model",
        "file:///outside.gguf",
        "data:text/plain,model",
    ] {
        assert!(redirect_target(&https, location).is_err(), "{location}");
    }
    assert!(redirect_target(&https, "http://cdn.example/model")
        .unwrap_err()
        .to_string()
        .contains("downgrade"));
    assert_eq!(
        redirect_target(&https, "/cdn/model?signature=explicit")
            .unwrap()
            .as_str(),
        "https://models.example/cdn/model?signature=explicit"
    );
    let http = url::Url::parse("http://models.example/model").unwrap();
    assert!(redirect_target(&http, "https://cdn.example/model").is_ok());
}

#[tokio::test]
async fn model_transfer_timeout_overrides_short_inference_timeout() {
    let dir = test_dir();
    let library = library(&dir)
        .with_transfer_policy(TransferPolicy {
            total_timeout: Duration::from_secs(2),
            ..TransferPolicy::default()
        })
        .unwrap();
    let (url, worker) = fixture_checked(response(b"body"), Duration::from_millis(100), |_| {});
    let network = NetworkConfig::new(Duration::from_millis(10)).unwrap();
    let result = library
        .download(request(url, b"body", 1024), &network, None, None)
        .await;
    worker.join().unwrap();
    assert!(result.is_ok(), "{result:?}");
    no_stages(&library);
}

#[tokio::test]
async fn transfer_deadline_terminates_stalled_body_and_cleans_stage() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = test_dir();
    let library = library(&dir)
        .with_transfer_policy(TransferPolicy {
            total_timeout: Duration::from_millis(150),
            ..TransferPolicy::default()
        })
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/model", listener.local_addr().unwrap());
    let network = NetworkConfig::new(Duration::from_secs(3)).unwrap();
    let (finish, finished) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") && header.len() < 8192 {
            header.push(stream.read_u8().await.unwrap());
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n")
            .await
            .unwrap();
        let _ = finished.await;
    });
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        library.download(request(url, b"body", 100), &network, None, None),
    )
    .await
    .unwrap()
    .unwrap_err();
    let _ = finish.send(());
    server.await.unwrap();
    assert!(error.to_string().contains("deadline"), "{error}");
    assert!(library.list().unwrap().is_empty());
    no_stages(&library);
}

#[test]
fn synthetic_disk_budget_refuses_import_and_preserves_the_source() {
    let dir = test_dir();
    let library = library(&dir)
        .with_transfer_policy(TransferPolicy {
            disk_headroom_bytes: 100,
            ..TransferPolicy::default()
        })
        .unwrap()
        .with_disk_space_probe(Arc::new(|_: &Path| Ok(104)));
    let original = source(&dir, "source.gguf", b"model");
    let error = library.import(&original).unwrap_err();
    assert!(error.to_string().contains("free disk space"));
    assert_eq!(fs::read(original).unwrap(), b"model");
    assert!(library.list().unwrap().is_empty());
    no_stages(&library);
}

#[test]
fn falling_synthetic_disk_budget_interrupts_staged_import() {
    use std::sync::atomic::AtomicUsize;
    let dir = test_dir();
    let probes = Arc::new(AtomicUsize::new(0));
    let observed = probes.clone();
    let library = library(&dir)
        .with_transfer_policy(TransferPolicy {
            disk_headroom_bytes: 16,
            ..TransferPolicy::default()
        })
        .unwrap()
        .with_disk_space_probe(Arc::new(move |_: &Path| {
            Ok(if observed.fetch_add(1, Ordering::SeqCst) < 2 {
                1024 * 1024
            } else {
                0
            })
        }));
    let bytes = vec![b'x'; 128 * 1024];
    let original = source(&dir, "source.gguf", &bytes);
    assert!(library
        .import(&original)
        .unwrap_err()
        .to_string()
        .contains("free disk space"));
    assert_eq!(probes.load(Ordering::SeqCst), 3);
    assert_eq!(fs::read(original).unwrap(), bytes);
    assert!(library.list().unwrap().is_empty());
    no_stages(&library);
}

#[tokio::test]
async fn synthetic_download_disk_budgets_check_headers_and_each_streamed_write() {
    use std::sync::atomic::AtomicU64;
    for scenario in ["headers", "stream", "after-write"] {
        let dir = test_dir();
        let available = Arc::new(AtomicU64::new(if scenario == "headers" {
            110
        } else {
            1000
        }));
        let observed = available.clone();
        let library = library(&dir)
            .with_transfer_policy(TransferPolicy {
                disk_headroom_bytes: 100,
                ..TransferPolicy::default()
            })
            .unwrap()
            .with_disk_space_probe(Arc::new(
                move |_: &Path| Ok(observed.load(Ordering::SeqCst)),
            ));
        let body = b"model bytes";
        let wire = if scenario == "stream" {
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nb\r\nmodel bytes\r\n0\r\n\r\n".to_vec()
        } else {
            response(body)
        };
        let (url, worker) = fixture(wire);
        let signal = available.clone();
        let wrote_bytes = Arc::new(AtomicBool::new(false));
        let written = wrote_bytes.clone();
        let notify = move |progress: DownloadProgress| {
            if progress.downloaded_bytes > 0 {
                written.store(true, Ordering::SeqCst);
            }
            if scenario == "stream" || (scenario == "after-write" && progress.downloaded_bytes > 0)
            {
                signal.store(0, Ordering::SeqCst);
            }
        };
        let error = library
            .download(
                request(url, body, 1024),
                &NetworkConfig::new(Duration::from_secs(3)).unwrap(),
                None,
                Some(&notify),
            )
            .await
            .unwrap_err();
        worker.join().unwrap();
        assert!(
            error.to_string().contains("free disk space"),
            "{scenario}: {error}"
        );
        assert_eq!(
            wrote_bytes.load(Ordering::SeqCst),
            scenario == "after-write"
        );
        assert!(library.list().unwrap().is_empty());
        no_stages(&library);
    }
}

#[test]
fn transfer_policy_rejects_unbounded_or_zero_safety_limits() {
    let dir = test_dir();
    for policy in [
        TransferPolicy {
            max_redirects: 11,
            ..TransferPolicy::default()
        },
        TransferPolicy {
            total_timeout: Duration::ZERO,
            ..TransferPolicy::default()
        },
        TransferPolicy {
            total_timeout: Duration::from_secs(25 * 60 * 60),
            ..TransferPolicy::default()
        },
        TransferPolicy {
            disk_headroom_bytes: 0,
            ..TransferPolicy::default()
        },
    ] {
        assert!(library(&dir).with_transfer_policy(policy).is_err());
    }
}
