use super::{source, store::ReaderStore, stream, types::*};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        // Never use system temporary directories: these fixtures stay in the
        // repository and TempDir removes only the test's own generated files.
        let directory = tempfile::Builder::new()
            .prefix(".reader-test-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        let root = directory.path().join("library");
        fs::create_dir(&root).unwrap();
        let state = directory.path().join("private");
        Self {
            _directory: directory,
            root,
            state,
        }
    }
    fn put(&self, path: &str, bytes: &[u8]) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn store(&self) -> ReaderStore {
        let mut store = ReaderStore::load(self.state.clone()).unwrap();
        store
            .set_library(self.root.to_str().unwrap().to_owned())
            .unwrap();
        store
    }
}

fn position(track_id: &str, offset_ms: u64) -> ReaderPosition {
    ReaderPosition {
        track_id: Some(track_id.into()),
        offset_ms,
        locator: None,
        voice_id: None,
    }
}

#[test]
fn discovery_groups_top_folders_and_naturally_sorts_discs_and_chapters() {
    let f = Fixture::new();
    for path in [
        "Novel/Disc 2/10.mp3",
        "Novel/Disc 1/10.mp3",
        "Novel/Disc 1/2.mp3",
        "Novel/Disc 1/1.MP3",
        "single.mp3",
        "nested/a/book.epub",
        "nested/b/book.EPUB",
    ] {
        f.put(path, b"media");
    }
    f.put("ignored.txt", b"not media");
    let store = f.store();
    let snapshot = store.snapshot();
    assert_eq!(snapshot.books.len(), 4);
    let novel = snapshot.books.iter().find(|b| b.title == "Novel").unwrap();
    assert_eq!(
        novel
            .tracks
            .iter()
            .map(|t| t.relative_path.as_str())
            .collect::<Vec<_>>(),
        [
            "Novel/Disc 1/1.MP3",
            "Novel/Disc 1/2.mp3",
            "Novel/Disc 1/10.mp3",
            "Novel/Disc 2/10.mp3"
        ]
    );
    assert_eq!(
        snapshot
            .books
            .iter()
            .filter(|b| b.kind == ReaderKind::Epub)
            .count(),
        2
    );
    assert_eq!(
        source::natural_cmp("track999999999999999999999.mp3", "track10.mp3"),
        std::cmp::Ordering::Greater
    );
}

#[test]
fn progress_bookmarks_and_manual_order_survive_restart_and_rescan() {
    let f = Fixture::new();
    f.put("Novel/1.mp3", b"one");
    f.put("Novel/2.mp3", b"two");
    let mut store = f.store();
    let book = store.snapshot().books.remove(0);
    let saved = position(&book.tracks[1].id, 456_789);
    let bookmark = store
        .add_bookmark(&book.id, saved.clone(), "Remember".into())
        .unwrap();
    let order = book
        .tracks
        .iter()
        .rev()
        .map(|t| t.id.clone())
        .collect::<Vec<_>>();
    store.reorder(&book.id, order.clone()).unwrap();
    drop(store);
    f.put("Novel/0.mp3", b"new");
    let mut store = ReaderStore::load(f.state.clone()).unwrap();
    let reloaded = store.rescan().unwrap().books.remove(0);
    assert_eq!(reloaded.id, book.id);
    assert_eq!(reloaded.position, Some(saved));
    assert_eq!(reloaded.bookmarks[0].id, bookmark.id);
    assert_eq!(
        reloaded.tracks[..2]
            .iter()
            .map(|t| &t.id)
            .collect::<Vec<_>>(),
        order.iter().collect::<Vec<_>>()
    );
    store.delete_bookmark(&book.id, &bookmark.id).unwrap();
    assert!(
        ReaderStore::load(f.state.clone()).unwrap().snapshot().books[0]
            .bookmarks
            .is_empty()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&f.state).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(f.state.join("reader.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn editing_bookmark_note_keeps_position_and_identity_and_persists() {
    let f = Fixture::new();
    f.put("one.mp3", b"one");
    let mut store = f.store();
    let book = store.snapshot().books.remove(0);
    let bookmark = store
        .add_bookmark(&book.id, position(&book.tracks[0].id, 500), "before".into())
        .unwrap();
    store
        .save_position(&book.id, position(&book.tracks[0].id, 900))
        .unwrap();
    let updated = store
        .update_bookmark(&book.id, &bookmark.id, "after".into())
        .unwrap();
    assert_eq!(updated.id, bookmark.id);
    assert_eq!(updated.position, bookmark.position);
    assert_eq!(updated.created_at, bookmark.created_at);
    assert_eq!(updated.note, "after");
    assert!(store
        .update_bookmark(&book.id, &bookmark.id, "x".repeat(16_385))
        .is_err());
    assert!(store
        .update_bookmark(&book.id, "unknown", "no".into())
        .is_err());
    let reloaded = ReaderStore::load(f.state.clone()).unwrap().snapshot();
    assert_eq!(reloaded.books[0].position.as_ref().unwrap().offset_ms, 900);
    assert_eq!(reloaded.books[0].bookmarks[0].note, "after");
}

#[test]
fn missing_and_replaced_media_never_reset_or_silently_resume() {
    let f = Fixture::new();
    f.put("one.mp3", b"one");
    let mut store = f.store();
    let book = store.snapshot().books.remove(0);
    let saved = position(&book.tracks[0].id, 5000);
    store.save_position(&book.id, saved.clone()).unwrap();
    fs::rename(f.root.join("one.mp3"), f.root.join("moved.mp3")).unwrap();
    let snapshot = store.rescan().unwrap();
    assert!(
        !snapshot
            .books
            .iter()
            .find(|b| b.id == book.id)
            .unwrap()
            .available
    );
    assert!(store
        .open_media(&book.id, Some(&book.tracks[0].id))
        .is_err());
    store.relink(&book.id, "moved.mp3".into(), true).unwrap();
    assert_eq!(store.snapshot().books.len(), 1);
    f.put("moved.mp3", b"different replacement");
    assert!(!store.rescan().unwrap().books[0].available);
    assert!(store.relink(&book.id, "moved.mp3".into(), false).is_err());
    store.relink(&book.id, "moved.mp3".into(), true).unwrap();
    assert_eq!(store.snapshot().books[0].position, Some(saved));
    assert!(store.open_media(&book.id, Some(&book.tracks[0].id)).is_ok());
}

#[test]
fn folder_rename_keeps_track_identity_and_replaced_root_needs_confirmation() {
    let f = Fixture::new();
    f.put("old/Disc 1/1.mp3", b"one");
    let mut store = f.store();
    let book = store.snapshot().books.remove(0);
    fs::rename(f.root.join("old"), f.root.join("new")).unwrap();
    store.relink(&book.id, "new".into(), false).unwrap();
    assert_eq!(store.snapshot().books[0].tracks[0].id, book.tracks[0].id);
    fs::rename(&f.root, f.root.with_file_name("original-library")).unwrap();
    fs::create_dir(&f.root).unwrap();
    f.put("new/Disc 1/1.mp3", b"one");
    assert!(store
        .open_media(&book.id, Some(&book.tracks[0].id))
        .is_err());
    assert!(!store.rescan().unwrap().books[0].available);
    store.set_library(f.root.to_str().unwrap().into()).unwrap();
    assert!(!store.snapshot().books[0].available);
    assert!(store.relink(&book.id, "new".into(), false).is_err());
    store.relink(&book.id, "new".into(), true).unwrap();
    assert!(store.snapshot().books[0].available);
}

#[cfg(unix)]
#[test]
fn symlinks_traversals_and_changed_parent_directories_cannot_escape_registration() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = f._directory.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.mp3"), b"outside").unwrap();
    symlink(&outside, f.root.join("link")).unwrap();
    symlink(outside.join("secret.mp3"), f.root.join("secret.mp3")).unwrap();
    f.put("safe/secret.mp3", b"inside");
    let store = f.store();
    assert_eq!(store.snapshot().books.len(), 1);
    let book = store.snapshot().books.remove(0);
    fs::rename(f.root.join("safe"), f.root.join("retained")).unwrap();
    symlink(&outside, f.root.join("safe")).unwrap();
    assert!(store
        .open_media(&book.id, Some(&book.tracks[0].id))
        .is_err());
    let (dir, _) = source::root(f.root.to_str().unwrap()).unwrap();
    for path in [
        "../outside/secret.mp3",
        "/etc/passwd",
        "a/../secret.mp3",
        "a//secret.mp3",
        "a\\secret.mp3",
        "safe/secret.mp3",
        "secret.mp3",
    ] {
        assert!(source::open(&dir, path).is_err(), "{path}");
    }
    symlink(&f.root, f._directory.path().join("linked-library")).unwrap();
    assert!(source::root(f._directory.path().join("linked-library").to_str().unwrap()).is_err());
}

#[test]
fn epub_size_bound_and_positions_are_independent_from_audio() {
    let f = Fixture::new();
    f.put("book.epub", b"PK");
    f.put("audio.mp3", b"mp3");
    let mut store = f.store();
    let books = store.snapshot().books;
    let epub = books.iter().find(|b| b.kind == ReaderKind::Epub).unwrap();
    let audio = books.iter().find(|b| b.kind == ReaderKind::Audio).unwrap();
    let locator = ReaderPosition {
        track_id: None,
        offset_ms: 0,
        voice_id: None,
        locator: Some(EpubLocation {
            kind: "epub".into(),
            cfi: "epubcfi(/6/2!/4/2:0)".into(),
            renderer_version: "foliate".into(),
        }),
    };
    store.save_position(&epub.id, locator.clone()).unwrap();
    assert!(store.save_position(&audio.id, locator.clone()).is_err());
    assert!(store
        .save_position(&epub.id, position(&audio.tracks[0].id, 0))
        .is_err());
    assert_eq!(store.read_epub(&epub.id).unwrap(), b"PK");
    assert_eq!(
        store
            .snapshot()
            .books
            .iter()
            .find(|b| b.id == audio.id)
            .unwrap()
            .position,
        None
    );
    fs::OpenOptions::new()
        .write(true)
        .open(f.root.join("book.epub"))
        .unwrap()
        .set_len(source::MAX_EPUB_BYTES + 1)
        .unwrap();
    store.relink(&epub.id, "book.epub".into(), true).unwrap();
    assert!(store.read_epub(&epub.id).unwrap_err().contains("128 MiB"));
    assert_eq!(
        store
            .snapshot()
            .books
            .iter()
            .find(|b| b.id == epub.id)
            .unwrap()
            .position,
        Some(locator)
    );
}

#[test]
fn backup_merges_history_without_granting_filesystem_access() {
    let f = Fixture::new();
    f.put("one.mp3", b"one");
    let mut store = f.store();
    let book = store.snapshot().books.remove(0);
    let saved = position(&book.tracks[0].id, 1234);
    store
        .add_bookmark(&book.id, saved.clone(), "".into())
        .unwrap();
    let backup = store.export().unwrap();
    let destination = f._directory.path().join("restored");
    let mut restored = ReaderStore::load(destination).unwrap();
    let snapshot = restored.restore(&backup).unwrap();
    assert_eq!(snapshot.library_path, None);
    assert!(!snapshot.books[0].available);
    assert!(restored
        .open_media(&book.id, Some(&book.tracks[0].id))
        .is_err());
    restored
        .set_library(f.root.to_str().unwrap().into())
        .unwrap();
    restored.relink(&book.id, "one.mp3".into(), false).unwrap();
    assert_eq!(restored.snapshot().books.len(), 1);
    assert_eq!(restored.snapshot().books[0].position, Some(saved));
    assert_eq!(restored.snapshot().books[0].bookmarks.len(), 1);
    restored.restore(&backup).unwrap();
    assert_eq!(restored.snapshot().books[0].bookmarks.len(), 1);
    let mut invalid: serde_json::Value = serde_json::from_str(&backup).unwrap();
    invalid["books"][0]["files"][0]["path"] = "../secret.mp3".into();
    assert!(restored.restore(&invalid.to_string()).is_err());
    assert_eq!(restored.snapshot().books[0].bookmarks.len(), 1);
}

#[test]
fn corrupted_database_is_not_replaced_and_invalid_mutations_are_atomic() {
    let f = Fixture::new();
    f.put("one.mp3", b"one");
    let mut store = f.store();
    let book = store.snapshot().books.remove(0);
    let before = store.export().unwrap();
    assert!(store
        .save_position(&book.id, position("unregistered", 10))
        .is_err());
    assert!(store
        .reorder(
            &book.id,
            vec![book.tracks[0].id.clone(), book.tracks[0].id.clone()]
        )
        .is_err());
    assert_eq!(store.export().unwrap(), before);
    fs::write(f.state.join("reader.json"), b"broken").unwrap();
    assert!(ReaderStore::load(f.state.clone()).is_err());
    assert_eq!(fs::read(f.state.join("reader.json")).unwrap(), b"broken");
}

#[test]
fn checkpoints_saved_during_scan_survive_and_old_library_scan_is_rejected() {
    let f = Fixture::new();
    f.put("one.mp3", b"one");
    let mut store = f.store();
    let book = store.snapshot().books.remove(0);
    let scan = store.prepare_scan().unwrap().run().unwrap();
    let saved = position(&book.tracks[0].id, 12_345);
    store.save_position(&book.id, saved.clone()).unwrap();
    store.finish_scan(Ok(scan)).unwrap();
    assert_eq!(store.snapshot().books[0].position, Some(saved));
    let stale = store.prepare_scan().unwrap().run().unwrap();
    let other = f._directory.path().join("other-library");
    fs::create_dir(&other).unwrap();
    store.set_library(other.to_str().unwrap().into()).unwrap();
    assert!(store.finish_scan(Ok(stale)).is_err());
}

#[cfg(unix)]
#[test]
fn fifo_replacement_is_rejected_without_waiting_for_a_writer() {
    use std::ffi::CString;
    let f = Fixture::new();
    f.put("one.mp3", b"one");
    let store = f.store();
    let book = store.snapshot().books.remove(0);
    fs::rename(f.root.join("one.mp3"), f.root.join("retained.mp3")).unwrap();
    let path = CString::new(f.root.join("one.mp3").as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: CString is terminated and alive for the syscall.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(store
        .open_media(&book.id, Some(&book.tracks[0].id))
        .is_err());
}

#[test]
fn ranges_include_suffix_open_ended_and_unsatisfiable_cases() {
    assert_eq!(stream::byte_range(None, 10).unwrap(), None);
    for (value, expected) in [
        ("bytes=1-3", (1, 3)),
        ("bytes=5-", (5, 9)),
        ("bytes=-3", (7, 9)),
        ("bytes=0-999", (0, 9)),
        ("bytes=-999", (0, 9)),
    ] {
        assert_eq!(stream::byte_range(Some(value), 10).unwrap(), Some(expected));
    }
    for value in [
        "bytes=10-",
        "bytes=4-3",
        "bytes=-0",
        "bytes=0-1,3-4",
        "bytes=+1-2",
        "items=1-2",
        "bytes=-",
        "bytes=18446744073709551616-",
    ] {
        assert!(stream::byte_range(Some(value), 10).is_err(), "{value}");
    }
    assert!(stream::byte_range(Some("bytes=0-"), 0).is_err());
}

#[test]
fn epub_locator_uses_existing_reader_kind_discriminator() {
    let wire = serde_json::json!({
        "offsetMs": 0,
        "locator": {
            "kind": "epub",
            "cfi": "epubcfi(/6/2!/4/2:0)",
            "rendererVersion": "foliate"
        }
    });
    let position: ReaderPosition = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(position).unwrap(), wire);
    assert!(serde_json::from_value::<ReaderPosition>(serde_json::json!({
        "offsetMs": 0,
        "locator": {
            "type": "epub",
            "cfi": "epubcfi(/6/2!/4/2:0)",
            "rendererVersion": "foliate"
        }
    }))
    .is_err());
}

#[test]
fn shared_epub_position_survives_restart_bookmark_and_backup_without_conversion() {
    assert_epub_position_roundtrip(None);
}

#[test]
fn voice_aware_epub_position_survives_restart_bookmark_and_backup() {
    assert_epub_position_roundtrip(Some("offline-voice_1"));
}

fn assert_epub_position_roundtrip(voice_id: Option<&str>) {
    let mut fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/private-reader-position.json"
    ))
    .unwrap();
    if let Some(voice_id) = voice_id {
        fixture["voiceId"] = voice_id.into();
    }
    let position: ReaderPosition = serde_json::from_value(fixture.clone()).unwrap();
    let f = Fixture::new();
    f.put("book.epub", b"PK");
    let mut store = f.store();
    let book = store.snapshot().books.remove(0);
    store.save_position(&book.id, position.clone()).unwrap();
    store
        .add_bookmark(&book.id, position, "Synthetic note".into())
        .unwrap();
    drop(store);

    let reopened = ReaderStore::load(f.state.clone()).unwrap();
    let backup = reopened.export().unwrap();
    let exported: serde_json::Value = serde_json::from_str(&backup).unwrap();
    assert_eq!(exported["books"][0]["book"]["position"], fixture);
    assert_eq!(
        exported["books"][0]["book"]["bookmarks"][0]["position"],
        fixture
    );

    let mut restored = ReaderStore::load(f._directory.path().join("restored-epub")).unwrap();
    let snapshot = restored.restore(&backup).unwrap();
    assert_eq!(
        serde_json::to_value(&snapshot.books[0].position).unwrap(),
        fixture
    );
    assert_eq!(
        serde_json::to_value(&snapshot.books[0].bookmarks[0].position).unwrap(),
        fixture
    );
    assert!(!snapshot.books[0].available);
    assert!(restored.read_epub(&book.id).is_err());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires native WebKitGTK, GStreamer audio, ffmpeg, xvfb-run and Python GI"]
fn actual_webkit_seeks_and_resumes_two_hour_external_audio() {
    let f = Fixture::new();
    let encoded = std::process::Command::new("timeout")
        .args([
            "90",
            "ffmpeg",
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=24000:cl=mono",
            "-t",
            "7200",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "32k",
            "-threads",
            "1",
        ])
        .arg(f.root.join("silence.mp3"))
        .output()
        .unwrap();
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    let store = Arc::new(Mutex::new(f.store()));
    let book = store.lock().unwrap().snapshot().books.remove(0);
    let server = stream::MediaServer::start(store).unwrap();
    let url = server.url(&book.id, &book.tracks[0].id).unwrap();
    let output = std::process::Command::new("timeout")
        .args(["60", "xvfb-run", "-a", "/usr/bin/python3"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/privateReader.audio.webkit.py"))
        .arg(url)
        .env("GDK_BACKEND", "x11")
        .env("WEBKIT_DISABLE_COMPOSITING_MODE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn http(url: &str, method: &str, extra: &str, route: Option<&str>, host: Option<&str>) -> Vec<u8> {
    let rest = url.strip_prefix("http://").unwrap();
    let (address, path) = rest.split_once('/').unwrap();
    let mut connection = TcpStream::connect(address).unwrap();
    connection
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    write!(
        connection,
        "{method} /{} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n{extra}\r\n",
        route.unwrap_or(path),
        host.unwrap_or(address)
    )
    .unwrap();
    let mut response = Vec::new();
    match connection.read_to_end(&mut response) {
        Ok(_) => {}
        Err(error)
            if error.kind() == std::io::ErrorKind::ConnectionReset
                && response.starts_with(b"HTTP/1.1 ") => {}
        Err(error) => panic!("HTTP response failed: {error}"),
    }
    response
}

#[test]
fn loopback_stream_enforces_capabilities_ranges_head_and_source_revalidation() {
    let f = Fixture::new();
    f.put("one.mp3", b"0123456789");
    let store = Arc::new(Mutex::new(f.store()));
    let book = store.lock().unwrap().snapshot().books.remove(0);
    let server = stream::MediaServer::start(store.clone()).unwrap();
    let url = server.url(&book.id, &book.tracks[0].id).unwrap();
    let response =
        String::from_utf8(http(&url, "GET", "Range: bytes=2-5\r\n", None, None)).unwrap();
    assert!(response.starts_with("HTTP/1.1 206"));
    assert!(response
        .to_lowercase()
        .contains("content-range: bytes 2-5/10"));
    assert!(response.ends_with("\r\n\r\n2345"));
    let head = String::from_utf8(http(&url, "HEAD", "", None, None)).unwrap();
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(head.to_lowercase().contains("content-length: 10"));
    assert!(head.ends_with("\r\n\r\n"));
    for origin in ["tauri://localhost", "http://127.0.0.1:5173"] {
        let response = String::from_utf8(http(
            &url,
            "GET",
            &format!("Origin: {origin}\r\n"),
            None,
            None,
        ))
        .unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response
            .to_lowercase()
            .contains(&format!("access-control-allow-origin: {origin}")));
    }
    for (method, extra, route, host) in [
        ("GET", "", Some("../../one.mp3"), None),
        ("GET", "", None, Some("evil.example")),
        ("GET", "Origin: https://evil.example\r\n", None, None),
        ("POST", "", None, None),
    ] {
        assert!(http(&url, method, extra, route, host).starts_with(b"HTTP/1.1 404"));
    }
    assert!(http(&url, "GET", "Range: bytes=12-\r\n", None, None).starts_with(b"HTTP/1.1 416"));
    f.put("one.mp3", b"replacement");
    assert!(http(&url, "GET", "", None, None).starts_with(b"HTTP/1.1 410"));
    server.revoke();
    assert!(http(&url, "GET", "", None, None).starts_with(b"HTTP/1.1 404"));
}

#[test]
fn large_audio_is_registered_and_seekable_without_full_read() {
    let f = Fixture::new();
    f.put("huge.mp3", b"");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(f.root.join("huge.mp3"))
        .unwrap();
    file.set_len(8 * 1024 * 1024 * 1024).unwrap();
    drop(file);
    let store = Arc::new(Mutex::new(f.store()));
    let book = store.lock().unwrap().snapshot().books.remove(0);
    let server = stream::MediaServer::start(store).unwrap();
    let url = server.url(&book.id, &book.tracks[0].id).unwrap();
    let response = http(&url, "GET", "Range: bytes=-4\r\n", None, None);
    assert!(response.starts_with(b"HTTP/1.1 206"));
    assert!(response.ends_with(&[0, 0, 0, 0]));
    assert!(response.len() < 1024);
    assert!(Path::new(&f.state.join("reader.json")).exists());
}

#[test]
fn equal_size_preserved_mtime_replacement_and_in_place_edits_need_confirmation() {
    let f = Fixture::new();
    f.put("one.mp3", b"original");
    let mut store = f.store();
    let book = store.snapshot().books.remove(0);
    let original_time = fs::metadata(f.root.join("one.mp3"))
        .unwrap()
        .modified()
        .unwrap();
    f.put("replacement.mp3", b"replaced");
    fs::OpenOptions::new()
        .write(true)
        .open(f.root.join("replacement.mp3"))
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(original_time))
        .unwrap();
    fs::rename(f.root.join("replacement.mp3"), f.root.join("one.mp3")).unwrap();
    assert!(store
        .open_media(&book.id, Some(&book.tracks[0].id))
        .is_err());
    assert!(store.relink(&book.id, "one.mp3".into(), false).is_err());
    store.relink(&book.id, "one.mp3".into(), true).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    f.put("one.mp3", b"modified");
    fs::OpenOptions::new()
        .write(true)
        .open(f.root.join("one.mp3"))
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(original_time))
        .unwrap();
    assert!(store
        .open_media(&book.id, Some(&book.tracks[0].id))
        .is_err());
    assert!(store.relink(&book.id, "one.mp3".into(), false).is_err());
    store.relink(&book.id, "one.mp3".into(), true).unwrap();
}

#[test]
fn stale_scan_cannot_resurrect_pre_relink_registration() {
    let f = Fixture::new();
    f.put("old/1.mp3", b"one");
    let mut store = f.store();
    let book = store.snapshot().books.remove(0);
    let stale = store.prepare_scan().unwrap().run().unwrap();
    fs::rename(f.root.join("old"), f.root.join("new")).unwrap();
    store.relink(&book.id, "new".into(), true).unwrap();
    assert!(store.finish_scan(Ok(stale)).is_err());
    let snapshot = store.snapshot();
    assert_eq!(snapshot.books.len(), 1);
    assert!(snapshot.books[0].available);
    assert_eq!(snapshot.books[0].tracks[0].relative_path, "new/1.mp3");
}

#[test]
fn repeated_and_newer_backups_merge_offline_without_source_authority() {
    let f = Fixture::new();
    f.put("one.mp3", b"one");
    let mut original = f.store();
    let book = original.snapshot().books.remove(0);
    original
        .add_bookmark(&book.id, position(&book.tracks[0].id, 100), "first".into())
        .unwrap();
    let first = original.export().unwrap();
    let mut restored = ReaderStore::load(f._directory.path().join("offline")).unwrap();
    restored.restore(&first).unwrap();
    restored.restore(&first).unwrap();
    original
        .add_bookmark(&book.id, position(&book.tracks[0].id, 200), "second".into())
        .unwrap();
    restored.restore(&original.export().unwrap()).unwrap();
    let snapshot = restored.snapshot();
    assert_eq!(snapshot.library_path, None);
    assert_eq!(snapshot.books.len(), 1);
    assert_eq!(snapshot.books[0].bookmarks.len(), 2);
    assert!(!snapshot.books[0].available);
    assert!(restored
        .open_media(&book.id, Some(&book.tracks[0].id))
        .is_err());
    restored
        .set_library(f.root.to_str().unwrap().into())
        .unwrap();
    assert!(restored
        .open_media(&book.id, Some(&book.tracks[0].id))
        .is_err());
    restored.relink(&book.id, "one.mp3".into(), false).unwrap();
    assert!(restored
        .open_media(&book.id, Some(&book.tracks[0].id))
        .is_ok());
}

#[test]
fn library_selection_and_relink_scans_allow_concurrent_durable_checkpoints() {
    use super::ReaderState;
    use std::sync::{mpsc, Barrier};
    use std::time::Duration;
    let f = Fixture::new();
    f.put("old/1.mp3", b"one");
    let state = Arc::new(ReaderState::default());
    let book = state
        .set_library(f.state.clone(), f.root.to_str().unwrap().into())
        .unwrap()
        .books
        .remove(0);
    for relink in [false, true] {
        if relink {
            fs::rename(f.root.join("old"), f.root.join("new")).unwrap();
        }
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (worker, worker_dir, root, id, worker_entered, worker_release) = (
            state.clone(),
            f.state.clone(),
            f.root.to_str().unwrap().to_owned(),
            book.id.clone(),
            entered.clone(),
            release.clone(),
        );
        let scan_thread = std::thread::spawn(move || {
            let scan = move |job: super::store::ScanJob| {
                worker_entered.wait();
                worker_release.wait();
                job.run()
            };
            if relink {
                worker.relink_with_scan(worker_dir, &id, "new".into(), true, scan)
            } else {
                worker.set_library_with_scan(worker_dir, root, scan)
            }
        });
        entered.wait();
        let (tx, rx) = mpsc::channel();
        let (worker, worker_dir, id, track) = (
            state.clone(),
            f.state.clone(),
            book.id.clone(),
            book.tracks[0].id.clone(),
        );
        let checkpoint = std::thread::spawn(move || {
            tx.send(worker.with_store(worker_dir, |store| {
                store.save_position(&id, position(&track, 42_000))
            }))
            .unwrap();
        });
        let saved = rx.recv_timeout(Duration::from_secs(2));
        // Always release the worker even when asserting a failed concurrency test.
        release.wait();
        checkpoint.join().unwrap();
        assert!(saved.unwrap().is_ok());
        scan_thread.join().unwrap().unwrap();
        assert_eq!(
            ReaderStore::load(f.state.clone()).unwrap().snapshot().books[0]
                .position
                .as_ref()
                .unwrap()
                .offset_ms,
            42_000
        );
    }
}

fn wait_for_connections(server: &stream::MediaServer, expected: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if server.active_connections() == expected {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(server.active_connections(), expected);
}

#[test]
fn incomplete_headers_have_bounded_connections_and_absolute_deadline() {
    use std::time::Duration;
    let f = Fixture::new();
    f.put("one.mp3", b"one");
    let store = Arc::new(Mutex::new(f.store()));
    let book = store.lock().unwrap().snapshot().books.remove(0);
    let server = stream::MediaServer::start_with_timeouts(
        store,
        Duration::from_millis(150),
        Duration::from_millis(150),
    )
    .unwrap();
    let url = server.url(&book.id, &book.tracks[0].id).unwrap();
    let address = url
        .strip_prefix("http://")
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    let mut clients = Vec::new();
    for _ in 0..12 {
        let mut client = TcpStream::connect(address).unwrap();
        client.write_all(b"GET /").unwrap();
        clients.push(client);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while std::time::Instant::now() < deadline {
        assert!(server.active_connections() <= 4);
        std::thread::sleep(Duration::from_millis(5));
    }
    wait_for_connections(&server, 0);
    assert!(http(&url, "HEAD", "", None, None).starts_with(b"HTTP/1.1 200"));
    // Oversized and duplicate sensitive headers are rejected before routing.
    assert!(http(
        &url,
        "GET",
        &format!("X-Padding: {}\r\n", "x".repeat(17_000)),
        None,
        None
    )
    .starts_with(b"HTTP/1.1 400"));
    assert!(http(&url, "GET", "Host: evil.example\r\n", None, None).starts_with(b"HTTP/1.1 400"));
    drop(clients);
}

#[test]
fn stalled_authorized_responses_release_all_workers_on_write_deadline() {
    use std::time::Duration;
    let f = Fixture::new();
    f.put("large.mp3", b"");
    fs::OpenOptions::new()
        .write(true)
        .open(f.root.join("large.mp3"))
        .unwrap()
        .set_len(128 * 1024 * 1024)
        .unwrap();
    let store = Arc::new(Mutex::new(f.store()));
    let book = store.lock().unwrap().snapshot().books.remove(0);
    let server = stream::MediaServer::start_with_timeouts(
        store,
        Duration::from_secs(1),
        Duration::from_millis(200),
    )
    .unwrap();
    let url = server.url(&book.id, &book.tracks[0].id).unwrap();
    let (address, path) = url
        .strip_prefix("http://")
        .unwrap()
        .split_once('/')
        .unwrap();
    let mut clients = Vec::new();
    for _ in 0..4 {
        let mut client = TcpStream::connect(address).unwrap();
        write!(client, "GET /{path} HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
        clients.push(client);
    }
    wait_for_connections(&server, 4);
    wait_for_connections(&server, 0);
    assert!(http(&url, "GET", "Range: bytes=0-3\r\n", None, None).starts_with(b"HTTP/1.1 206"));
    drop(clients);
}
