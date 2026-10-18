//! Notes edited on two devices, driven through the real sync engine.
//!
//! A "device" is a graph folder with its own `SyncEngine`; devices share one
//! in-memory remote the way two machines share a WebDAV folder.

use grafium_core::error::{CoreError, Result};
use grafium_core::sync::backend::{
    compute_hash, revision_changed, FileMetadata, FileSnapshot, SyncBackend,
};
use grafium_core::sync::engine::{ConflictSide, SyncEngine, SyncResult};
use grafium_core::sync::merge::MAX_MERGE_BYTES;
use grafium_core::sync::{scan_markers, three_way_merge};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

type Hook = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Remote {
    files: Mutex<HashMap<String, Vec<u8>>>,
    /// Runs once, just before the next publication of the named file.
    before_publish: Mutex<Option<(String, Hook)>>,
}

impl Remote {
    fn set(&self, path: &str, content: impl AsRef<[u8]>) {
        self.files
            .lock()
            .unwrap()
            .insert(path.into(), content.as_ref().to_vec());
    }

    fn bytes(&self, path: &str) -> Option<Vec<u8>> {
        self.files.lock().unwrap().get(path).cloned()
    }

    fn get(&self, path: &str) -> Option<String> {
        self.bytes(path)
            .map(|bytes| String::from_utf8(bytes).unwrap())
    }

    fn on_publish(&self, path: &str, hook: impl FnOnce() + Send + 'static) {
        *self.before_publish.lock().unwrap() = Some((path.into(), Box::new(hook)));
    }
}

impl SyncBackend for Remote {
    fn name(&self) -> &str {
        "shared"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn list_files(&self) -> Result<Vec<FileMetadata>> {
        Ok(self
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
            .collect())
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        self.bytes(path)
            .ok_or_else(|| CoreError::NotFound(path.into()))
    }

    fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        self.set(path, content);
        Ok(())
    }

    fn delete_file(&self, path: &str) -> Result<()> {
        self.files.lock().unwrap().remove(path);
        Ok(())
    }

    fn publish_if_unchanged(
        &self,
        path: &str,
        expected: &FileSnapshot,
        content: Option<&[u8]>,
    ) -> Result<()> {
        let hook = {
            let mut armed = self.before_publish.lock().unwrap();
            match armed.take() {
                Some((target, hook)) if target == path => Some(hook),
                other => {
                    *armed = other;
                    None
                }
            }
        };
        if let Some(hook) = hook {
            hook();
        }
        let mut files = self.files.lock().unwrap();
        if files.get(path).map(|c| compute_hash(c)) != expected.hash() {
            return Err(revision_changed(path));
        }
        match content {
            Some(content) => files.insert(path.into(), content.to_vec()),
            None => files.remove(path),
        };
        Ok(())
    }
}

struct Device {
    dir: tempfile::TempDir,
    engine: SyncEngine,
}

impl Device {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["pages", "journals"] {
            fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        let engine = SyncEngine::new(dir.path().to_path_buf());
        Self { dir, engine }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, rel: &str, content: impl AsRef<[u8]>) {
        let path = self.root().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        String::from_utf8(self.read_bytes(rel)).unwrap()
    }

    fn read_bytes(&self, rel: &str) -> Vec<u8> {
        fs::read(self.root().join(rel)).unwrap()
    }

    fn sync(&self, remote: &Remote) -> SyncResult {
        let result = self.engine.sync(remote).unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        result
    }

    /// Every version of a note kept under `sync-recovery/<dir>`.
    fn recovered(&self, dir: &str) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.root().join(".grafium/sync-recovery").join(dir)) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| fs::read_to_string(entry.path()).ok())
            .collect()
    }
}

const NOTE: &str = "pages/todo.md";
const TODO: &str =
    "- Groceries\n- Buy milk\n- Call the plumber\n- Book flights\n- Water the plants\n";

fn synced_pair(note: &str) -> (Arc<Remote>, Device, Device) {
    let remote = Arc::new(Remote::default());
    let (a, b) = (Device::new(), Device::new());
    a.write(NOTE, note);
    a.sync(&remote);
    b.sync(&remote);
    assert_eq!(b.read(NOTE), note);
    (remote, a, b)
}

/// Both devices reword the same line; A has already published its version.
fn overlapping_edits() -> (Arc<Remote>, Device, Device) {
    let (remote, a, b) = synced_pair(TODO);
    a.write(NOTE, TODO.replace("Buy milk", "Buy oat milk"));
    b.write(NOTE, TODO.replace("Buy milk", "Buy almond milk"));
    a.sync(&remote);
    (remote, a, b)
}

fn marked_by_b() -> (Arc<Remote>, Device, Device) {
    let (remote, a, b) = overlapping_edits();
    assert_eq!(b.sync(&remote).merge_conflicts, [NOTE]);
    (remote, a, b)
}

// ---------------------------------------------------------------------------
// Edits that do not collide
// ---------------------------------------------------------------------------

#[test]
fn edits_in_different_places_merge_without_markers() {
    let (remote, a, b) = synced_pair(TODO);
    a.write(NOTE, TODO.replace("Buy milk", "Buy oat milk"));
    b.write(NOTE, TODO.replace("Water the plants", "Water the ferns"));

    assert_eq!(a.sync(&remote).pushed, [NOTE]);
    let merged = b.sync(&remote);
    assert_eq!(merged.merged, [NOTE]);
    assert!(merged.merge_conflicts.is_empty() && merged.conflicts.is_empty());

    let both = TODO
        .replace("Buy milk", "Buy oat milk")
        .replace("Water the plants", "Water the ferns");
    assert_eq!(b.read(NOTE), both);
    assert_eq!(remote.get(NOTE).unwrap(), both);
    assert_eq!(a.sync(&remote).pulled, [NOTE]);
    assert_eq!(a.read(NOTE), both);

    assert!(a.sync(&remote).is_clean() && b.sync(&remote).is_clean());
    assert!(a.engine.pending_merges().is_empty() && b.engine.pending_merges().is_empty());
}

#[test]
fn block_ids_each_device_invented_do_not_turn_a_merge_into_a_conflict() {
    let (remote, a, b) = synced_pair(TODO);
    // Saving in the app gives every block an id, and a note nobody had saved
    // yet gets different ones on each device.
    let saved = |device: &str, text: &str| -> String {
        text.lines()
            .enumerate()
            .map(|(i, line)| format!("{line}\n  id:: {device}{i}\n"))
            .collect()
    };
    a.write(NOTE, saved("a", &TODO.replace("Buy milk", "Buy oat milk")));
    b.write(
        NOTE,
        saved("b", &TODO.replace("Water the plants", "Water the ferns")),
    );

    assert_eq!(a.sync(&remote).pushed, [NOTE]);
    let merged = b.sync(&remote);
    assert_eq!(merged.merged, [NOTE]);
    assert!(merged.merge_conflicts.is_empty() && merged.conflicts.is_empty());

    let text = b.read(NOTE);
    assert!(text.contains("- Buy oat milk\n") && text.contains("- Water the ferns\n"));
    assert!(scan_markers(&text).is_none());
    assert_eq!(remote.get(NOTE).unwrap(), text);
    assert_eq!(a.sync(&remote).pulled, [NOTE]);
    assert_eq!(a.read(NOTE), text);
    assert!(a.sync(&remote).is_clean() && b.sync(&remote).is_clean());
}

#[test]
fn spaces_alone_never_start_a_conflict() {
    let (remote, a, b) = synced_pair(TODO);
    a.write(NOTE, TODO.replace("Buy milk", "Buy milk "));
    b.write(NOTE, TODO.replace("Buy milk", "Buy milk  "));
    a.sync(&remote);

    let result = b.sync(&remote);
    assert!(result.merge_conflicts.is_empty() && result.conflicts.is_empty());
    assert_eq!(b.read(NOTE), a.read(NOTE));
    assert_eq!(remote.get(NOTE).unwrap(), a.read(NOTE));
    assert!(a.sync(&remote).is_clean() && b.sync(&remote).is_clean());
}

// ---------------------------------------------------------------------------
// Edits that collide: markers stay on the device that found them
// ---------------------------------------------------------------------------

#[test]
fn overlapping_edits_get_markers_where_they_are_found_and_nothing_is_published() {
    let (remote, a, b) = overlapping_edits();
    let published = remote.get(NOTE).unwrap();

    let found = b.sync(&remote);
    assert_eq!(found.merge_conflicts, [NOTE]);
    assert!(found.pushed.is_empty() && found.pulled.is_empty() && found.conflicts.is_empty());

    let marked = b.read(NOTE);
    let scan = scan_markers(&marked).expect("markers were written");
    assert_eq!(scan.hunks, 1);
    assert_eq!(
        marked,
        format!(
            "- Groceries\n<<<<<<< YOURS (merge {t})\n- Buy almond milk\n======= (merge {t})\n\
             - Buy oat milk\n>>>>>>> THEIRS (merge {t})\n- Call the plumber\n- Book flights\n\
             - Water the plants\n",
            t = scan.token
        )
    );

    assert_eq!(remote.get(NOTE).unwrap(), published);
    assert!(scan_markers(&a.read(NOTE)).is_none());

    let pending = b.engine.pending_merges();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].rel_path, NOTE);
    assert_eq!(pending[0].conflicts, 1);
    assert_eq!(pending[0].token, scan.token);
    assert!(a.engine.pending_merges().is_empty());

    // Both whole versions stay recoverable.
    let kept = b.recovered("pages");
    assert!(kept.contains(&TODO.replace("Buy milk", "Buy almond milk")));
    assert!(kept.contains(&published));
}

#[test]
fn journals_are_merged_the_same_way() {
    let remote = Remote::default();
    let (a, b) = (Device::new(), Device::new());
    let day = "journals/2026_01_02.md";
    a.write(day, "- morning\n- evening\n");
    a.sync(&remote);
    b.sync(&remote);
    a.write(day, "- morning run\n- evening\n");
    b.write(day, "- morning walk\n- evening\n");
    a.sync(&remote);
    assert_eq!(b.sync(&remote).merge_conflicts, [day]);
    assert_eq!(b.engine.pending_merges()[0].rel_path, day);
}

#[test]
fn a_marked_note_stays_frozen_while_other_notes_keep_syncing() {
    let (remote, a, b) = marked_by_b();
    let marked = b.read(NOTE);

    // The other device moves on: the same note again, and a new one.
    a.write(NOTE, a.read(NOTE).replace("Book flights", "Book trains"));
    a.write("pages/other.md", "- elsewhere\n");
    a.sync(&remote);

    for round in 0..3 {
        let result = b.sync(&remote);
        if round == 0 {
            assert_eq!(result.pulled, ["pages/other.md"]);
        } else {
            assert!(result.is_clean(), "round {round}: {}", result.summary());
        }
        assert_eq!(b.read(NOTE), marked, "marked note was touched");
        assert_eq!(remote.get(NOTE).unwrap(), a.read(NOTE));
        assert_eq!(b.engine.pending_merges().len(), 1);
    }

    // A half-edited hunk still counts: removing only one marker line is not
    // a resolution.
    let partly: String = marked
        .lines()
        .filter(|line| !line.starts_with("<<<<<<<"))
        .map(|line| format!("{line}\n"))
        .collect();
    b.write(NOTE, &partly);
    b.sync(&remote);
    assert_eq!(remote.get(NOTE).unwrap(), a.read(NOTE));
    assert_eq!(b.engine.pending_merges().len(), 1);
    assert_eq!(b.read(NOTE), partly);
}

#[test]
fn a_look_alike_without_the_signature_is_ordinary_text() {
    let remote = Remote::default();
    let (a, b) = (Device::new(), Device::new());
    let text = "- see\n<<<<<<< HEAD\n- mine\n=======\n- theirs\n>>>>>>> branch\n\
                - also\n<<<<<<< YOURS (merge nothex!!)\n";
    a.write(NOTE, text);
    assert_eq!(a.sync(&remote).pushed, [NOTE]);
    assert_eq!(b.sync(&remote).pulled, [NOTE]);
    assert_eq!(b.read(NOTE), text);
    assert!(a.engine.pending_merges().is_empty());
}

#[test]
fn two_unrelated_copies_meeting_for_the_first_time_are_marked_without_a_base() {
    let remote = Remote::default();
    let (a, b) = (Device::new(), Device::new());
    a.write(NOTE, "- one\n- two\n- three\n");
    b.write(NOTE, "- one\n- TWO\n- three\n");
    assert_eq!(a.sync(&remote).pushed, [NOTE]);

    let found = b.sync(&remote);
    assert_eq!(found.merge_conflicts, [NOTE]);
    let marked = b.read(NOTE);
    assert_eq!(scan_markers(&marked).unwrap().hunks, 1);
    // The shared lines are written once, outside the markers.
    assert_eq!(marked.matches("- one\n").count(), 1);
    assert_eq!(marked.matches("- three\n").count(), 1);
    assert_eq!(remote.get(NOTE).unwrap(), "- one\n- two\n- three\n");
}

// ---------------------------------------------------------------------------
// Resolving: the answer to "does it ping-pong?"
// ---------------------------------------------------------------------------

#[test]
fn resolving_publishes_one_version_and_the_other_device_just_adopts_it() {
    let (remote, a, b) = marked_by_b();

    // The person keeps both lines.
    let resolved = TODO.replace("- Buy milk\n", "- Buy almond milk\n- Buy oat milk\n");
    b.write(NOTE, &resolved);
    let result = b.sync(&remote);
    assert_eq!(result.pushed, [NOTE]);
    assert!(result.merged.is_empty() && result.merge_conflicts.is_empty());
    assert!(b.engine.pending_merges().is_empty());

    assert_eq!(a.sync(&remote).pulled, [NOTE]);
    assert_eq!(a.read(NOTE), resolved);
    assert_eq!(remote.get(NOTE).unwrap(), resolved);

    for _ in 0..2 {
        assert!(a.sync(&remote).is_clean());
        assert!(b.sync(&remote).is_clean());
    }
}

#[test]
fn resolving_after_the_remote_moved_merges_once_more_and_then_settles() {
    let (remote, a, b) = marked_by_b();
    a.write(NOTE, a.read(NOTE).replace("Book flights", "Book trains"));
    a.sync(&remote);

    let resolved = TODO.replace("- Buy milk\n", "- Buy almond milk\n- Buy oat milk\n");
    b.write(NOTE, &resolved);
    assert_eq!(b.sync(&remote).merged, [NOTE]);
    let settled = resolved.replace("Book flights", "Book trains");
    assert_eq!(b.read(NOTE), settled);
    assert_eq!(remote.get(NOTE).unwrap(), settled);
    assert_eq!(a.sync(&remote).pulled, [NOTE]);
    assert_eq!(a.read(NOTE), settled);
    assert!(a.sync(&remote).is_clean() && b.sync(&remote).is_clean());
}

#[test]
fn resolving_with_a_stray_space_is_adopted_without_a_follow_up() {
    let (remote, a, b) = marked_by_b();

    // "Fixed it", but it is a space off from what the other device holds.
    b.write(NOTE, TODO.replace("Buy milk", "Buy oat milk "));
    assert_eq!(b.sync(&remote).pushed, [NOTE]);
    assert_eq!(a.sync(&remote).pulled, [NOTE]);
    assert_eq!(a.read(NOTE), b.read(NOTE));
    for _ in 0..2 {
        assert!(a.sync(&remote).is_clean());
        assert!(b.sync(&remote).is_clean());
    }
}

#[test]
fn both_devices_fixing_the_same_spot_by_a_space_agree_without_a_conflict() {
    let (remote, a, b) = marked_by_b();

    // B resolves and publishes. Meanwhile A tidies the same line differently.
    b.write(NOTE, TODO.replace("Buy milk", "Buy oat milk "));
    b.sync(&remote);
    a.write(NOTE, TODO.replace("Buy milk", "Buy oat milk  "));
    let result = a.sync(&remote);
    assert!(result.merge_conflicts.is_empty() && result.conflicts.is_empty());
    b.sync(&remote);
    assert_eq!(a.read(NOTE), b.read(NOTE));
    assert_eq!(a.read(NOTE), remote.get(NOTE).unwrap());
    assert!(a.sync(&remote).is_clean() && b.sync(&remote).is_clean());
}

#[test]
fn a_signed_marker_pasted_into_a_note_is_held_until_it_is_edited_out() {
    let remote = Remote::default();
    let a = Device::new();
    let quoted = three_way_merge("x\n", "y\n", "z\n").unwrap().content;
    a.write(NOTE, &quoted);

    let result = a.sync(&remote);
    assert!(result.pushed.is_empty());
    assert!(remote.get(NOTE).is_none(), "a marked note was published");
    assert_eq!(a.engine.pending_merges().len(), 1);

    a.write(NOTE, "x\n");
    assert_eq!(a.sync(&remote).pushed, [NOTE]);
    assert!(a.engine.pending_merges().is_empty());
}

#[test]
fn a_marked_note_is_not_published_to_another_sync_target() {
    let (_remote, _a, b) = marked_by_b();

    let second = Remote::default();
    let engine = SyncEngine::new_with_target(b.root().to_path_buf(), "second");
    let result = engine.sync(&second).unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(second.get(NOTE).is_none(), "a marked note was published");
    assert_eq!(engine.pending_merges().len(), 1);
}

#[test]
fn keeping_a_text_that_still_has_markers_is_refused() {
    // Reading-note data keeps the whole-file dialog, which is where a person
    // could otherwise "keep mine" with markers in it.
    let path = "pages/book.md";
    let remote = Remote::default();
    let (a, b) = (Device::new(), Device::new());
    a.write(path, "- a\nreading-note:: base\n");
    a.sync(&remote);
    b.sync(&remote);
    a.write(path, "- a\nreading-note:: first\n");
    b.write(path, "- a\nreading-note:: second\n");
    a.sync(&remote);
    assert_eq!(b.sync(&remote).conflicts, [path]);

    b.write(path, three_way_merge("x\n", "y\n", "z\n").unwrap().content);
    let view = b.engine.conflict_state(&remote, path).unwrap();
    let error = b
        .engine
        .resolve_conflict(
            &remote,
            path,
            view.local_hash.as_deref(),
            view.remote_hash.as_deref(),
            ConflictSide::Local,
        )
        .unwrap_err();
    assert!(error.to_string().contains("merge markers"), "{error}");
    assert_eq!(remote.get(path).unwrap(), "- a\nreading-note:: first\n");
    assert_eq!(b.engine.unresolved_conflicts().len(), 1);
}

// ---------------------------------------------------------------------------
// Files that are never text-merged
// ---------------------------------------------------------------------------

fn assert_stays_whole_file(path: &str, base: &[u8], first: &[u8], second: &[u8]) {
    let remote = Remote::default();
    let (a, b) = (Device::new(), Device::new());
    a.write(path, base);
    a.sync(&remote);
    b.sync(&remote);
    a.write(path, first);
    b.write(path, second);
    a.sync(&remote);

    let result = b.sync(&remote);
    assert_eq!(result.conflicts, [path], "{}", result.summary());
    assert!(result.merge_conflicts.is_empty() && result.merged.is_empty());
    assert!(b.read_bytes(path) == second, "local file was changed");
    assert!(remote.bytes(path).unwrap() == first, "remote was changed");
    assert!(b.engine.pending_merges().is_empty());
}

#[test]
fn reading_note_pages_keep_the_whole_file_dialog() {
    assert_stays_whole_file(
        "pages/book.md",
        b"- a\nreading-note:: base\n",
        b"- a\nreading-note:: first\n",
        b"- a\nreading-note:: second\n",
    );
}

#[test]
fn notes_that_are_not_text_keep_the_whole_file_dialog() {
    assert_stays_whole_file(
        "pages/odd.md",
        b"- a\n\xff\n",
        b"- a\n\xfe\n",
        b"- a\n\xfd\n",
    );
}

#[test]
fn oversized_notes_keep_the_whole_file_dialog() {
    let line = |n: usize| format!("- line {n}\n");
    let big: String = (0..MAX_MERGE_BYTES / 8).map(line).collect();
    assert!(big.len() > MAX_MERGE_BYTES);
    assert_stays_whole_file(
        "pages/huge.md",
        big.as_bytes(),
        big.replacen(&line(10), "- first\n", 1).as_bytes(),
        big.replacen(&line(90_000), "- second\n", 1).as_bytes(),
    );
}

#[test]
fn a_note_deleted_on_one_device_and_edited_on_the_other_is_not_marked() {
    let (remote, a, b) = synced_pair(TODO);
    fs::remove_file(a.root().join(NOTE)).unwrap();
    a.sync(&remote);
    b.write(NOTE, TODO.replace("Buy milk", "Buy oat milk"));
    let result = b.sync(&remote);
    assert!(result.merge_conflicts.is_empty() && result.merged.is_empty());
    assert!(scan_markers(&b.read(NOTE)).is_none());
    assert!(b.engine.pending_merges().is_empty());
}

// ---------------------------------------------------------------------------
// Interruptions
// ---------------------------------------------------------------------------

#[test]
fn a_remote_that_moves_during_a_merge_is_retried_not_lost() {
    let (remote, a, b) = synced_pair(TODO);
    a.write(NOTE, TODO.replace("Buy milk", "Buy oat milk"));
    let mine = TODO.replace("Water the plants", "Water the ferns");
    b.write(NOTE, &mine);
    a.sync(&remote);

    // A third device publishes while B is about to upload its merge.
    let third = TODO
        .replace("Buy milk", "Buy oat milk")
        .replace("Call the plumber", "Call the electrician");
    let racing = remote.clone();
    remote.on_publish(NOTE, move || racing.set(NOTE, &third));
    let attempt = b.engine.sync(&*remote).unwrap();
    assert_eq!(attempt.errors.len(), 1, "{:?}", attempt.errors);
    assert_eq!(b.read(NOTE), mine, "the note changed despite the failure");

    let all = TODO
        .replace("Buy milk", "Buy oat milk")
        .replace("Call the plumber", "Call the electrician")
        .replace("Water the plants", "Water the ferns");
    assert_eq!(b.sync(&remote).merged, [NOTE]);
    assert_eq!(b.read(NOTE), all);
    assert_eq!(remote.get(NOTE).unwrap(), all);
}

#[test]
fn an_edit_made_while_a_merge_uploads_is_never_overwritten() {
    let long = format!("{TODO}- Pay rent\n- Walk the dog\n");
    let (remote, a, b) = synced_pair(&long);
    a.write(NOTE, long.replace("Buy milk", "Buy oat milk"));
    let mine = long.replace("Water the plants", "Water the ferns");
    b.write(NOTE, &mine);
    a.sync(&remote);

    // B keeps typing, away from the other edits, while its merge uploads.
    let typing = mine.replace("Walk the dog", "Walk the cat");
    let path = b.root().join(NOTE);
    let typed = typing.clone();
    remote.on_publish(NOTE, move || fs::write(path, typed).unwrap());
    let attempt = b.engine.sync(&*remote).unwrap();
    assert_eq!(attempt.errors.len(), 1, "{:?}", attempt.errors);
    assert_eq!(b.read(NOTE), typing, "the latest text was overwritten");

    let all = typing.replace("Buy milk", "Buy oat milk");
    assert_eq!(b.sync(&remote).merged, [NOTE]);
    assert_eq!(b.read(NOTE), all);
    assert_eq!(remote.get(NOTE).unwrap(), all);
    assert_eq!(a.sync(&remote).pulled, [NOTE]);
    assert_eq!(a.read(NOTE), all);
}

// ---------------------------------------------------------------------------
// Three devices, random edits, people resolving at random
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Pick {
    Yours,
    Theirs,
    Both,
}

const PICKS: [Pick; 3] = [Pick::Yours, Pick::Theirs, Pick::Both];

/// What a person does with every marked spot.
fn resolve(text: &str, pick: Pick) -> String {
    let (mut out, mut yours, mut theirs) = (String::new(), String::new(), String::new());
    let mut side = 0;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end();
        if bare.starts_with("<<<<<<< YOURS (merge ") {
            side = 1;
        } else if side == 1 && bare.starts_with("======= (merge ") {
            side = 2;
        } else if side == 2 && bare.starts_with(">>>>>>> THEIRS (merge ") {
            if pick != Pick::Theirs {
                out.push_str(&yours);
            }
            if pick != Pick::Yours {
                out.push_str(&theirs);
            }
            (yours.clear(), theirs.clear());
            side = 0;
        } else {
            match side {
                0 => out.push_str(line),
                1 => yours.push_str(line),
                _ => theirs.push_str(line),
            }
        }
    }
    out
}

/// Deepest nesting of marker blocks in a note.
fn nesting(text: &str) -> usize {
    let (mut depth, mut deepest) = (0usize, 0usize);
    for line in text.lines() {
        if line.starts_with("<<<<<<< YOURS (merge ") {
            depth += 1;
            deepest = deepest.max(depth);
        } else if line.starts_with(">>>>>>> THEIRS (merge ") {
            depth = depth.saturating_sub(1);
        }
    }
    deepest
}

fn tokens_in(line: &str) -> impl Iterator<Item = String> + '_ {
    line.split_whitespace()
        .filter(|word| word.starts_with('t') && word[1..].chars().all(|c| c.is_ascii_digit()))
        .filter(|word| word.len() > 1)
        .map(str::to_string)
}

/// A bullet line and the id the app gave it, if the app ever saved the note.
type Block = (String, Option<String>);

fn blocks_of(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    for line in text.lines() {
        match (line.strip_prefix("  id:: "), blocks.last_mut()) {
            (Some(id), Some(last)) => last.1 = Some(id.to_string()),
            _ => blocks.push((line.to_string(), None)),
        }
    }
    blocks
}

/// One random edit; `lost` collects tokens the writer removed on purpose.
/// With `app` (device number, ids invented so far) the note is saved the way
/// the app saves it: every block is written with an id, and a block that never
/// had one gets an id no other device has.
fn edit(
    text: &str,
    rng: &mut Rng,
    made: &mut usize,
    lost: &mut HashSet<String>,
    mut app: Option<(usize, &mut usize)>,
) -> String {
    let mut blocks = blocks_of(text);
    if blocks.is_empty() {
        return "- fresh\n".to_string();
    }
    let at = rng.below(blocks.len());
    let mut token = || {
        *made += 1;
        format!("t{}", *made)
    };
    match rng.below(6) {
        0 | 1 => blocks[at].0.push_str(&format!(" {}", token())),
        2 => blocks.insert(at, (format!("- new {}", token()), None)),
        3 if blocks.len() > 3 => {
            lost.extend(tokens_in(&blocks.remove(at).0));
        }
        4 => blocks[at].0.push(' '),
        _ => {
            lost.extend(tokens_in(&blocks[at].0));
            blocks[at].0 = format!("- reworded {}", token());
        }
    }
    let mut out = String::new();
    for (line, id) in blocks {
        out.push_str(&line);
        out.push('\n');
        let id = id.or_else(|| {
            app.as_mut().map(|(device, invented)| {
                **invented += 1;
                format!("d{device}-{invented}")
            })
        });
        if let Some(id) = id {
            out.push_str(&format!("  id:: {id}\n"));
        }
    }
    out
}

fn fuzz(seed: u64, fixed: Option<Pick>, ids: bool) {
    let mut rng = Rng::new(seed);
    let remote = Arc::new(Remote::default());
    let devices = [Device::new(), Device::new(), Device::new()];
    let start: String = (0..8).map(|i| format!("- item {i}\n")).collect();
    devices[0].write(NOTE, &start);
    for device in &devices {
        device.sync(&remote);
    }

    let (mut made, mut lost, mut invented) = (0, HashSet::new(), 0);
    let what = |rng: &mut Rng| fixed.unwrap_or(PICKS[rng.below(3)]);
    let sync_checked = |device: &Device| {
        let result = device.sync(&remote);
        assert!(
            nesting(&device.read(NOTE)) <= 1,
            "seed {seed} {fixed:?} ids {ids}: markers nested"
        );
        result
    };

    for _ in 0..24 {
        let which = rng.below(3);
        let device = &devices[which];
        let text = device.read(NOTE);
        if scan_markers(&text).is_none() {
            let app = ids.then_some((which, &mut invented));
            device.write(NOTE, edit(&text, &mut rng, &mut made, &mut lost, app));
        } else if rng.below(2) == 0 {
            device.write(NOTE, resolve(&text, what(&mut rng)));
        }
        for _ in 0..rng.below(3) {
            sync_checked(&devices[rng.below(3)]);
        }
    }

    // Everyone settles: resolve what is marked, sync, repeat until quiet.
    let mut passes = 0;
    loop {
        passes += 1;
        assert!(
            passes <= 10,
            "seed {seed} {fixed:?} ids {ids}: still changing"
        );
        let mut quiet = true;
        for device in &devices {
            let text = device.read(NOTE);
            if scan_markers(&text).is_some() {
                device.write(NOTE, resolve(&text, what(&mut rng)));
                quiet = false;
            }
            quiet &= sync_checked(device).is_clean();
        }
        if quiet {
            break;
        }
    }

    let settled = remote.get(NOTE).unwrap();
    for device in &devices {
        assert_eq!(
            device.read(NOTE),
            settled,
            "seed {seed} {fixed:?} ids {ids}"
        );
        assert!(device.engine.pending_merges().is_empty());
    }
    assert!(scan_markers(&settled).is_none());
    if fixed == Some(Pick::Both) {
        for token in (1..=made).map(|n| format!("t{n}")) {
            assert!(
                lost.contains(&token) || tokens_in(&settled).any(|t| t == token),
                "seed {seed} ids {ids}: {token} vanished without anyone removing it:\n{settled}"
            );
        }
    }
}

#[test]
fn three_devices_always_settle_and_never_lose_an_edit() {
    let seeds: u64 = std::env::var("GRAFIUM_SYNC_FUZZ_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(12);
    for seed in 0..seeds {
        for fixed in [
            Some(Pick::Both),
            Some(Pick::Yours),
            Some(Pick::Theirs),
            None,
        ] {
            // Plain text, and the way the app saves: each device invents its own ids.
            for ids in [false, true] {
                fuzz(seed, fixed, ids);
            }
        }
    }
}
