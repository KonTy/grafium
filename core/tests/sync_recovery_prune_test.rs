//! Housekeeping of the sync metadata folder, driven through the real engine.
//!
//! A recovery copy is the only way back to the version that lost a merge, so it
//! has to outlive the decision about it and only then go. Caches nobody reads
//! must not pile up, and a removed target must not leave its folder behind.

use grafium_core::graph::DEFAULT_METADATA_DIR_NAME;
use grafium_core::sync::backend::compute_hash;
use grafium_core::sync::engine::{ConflictSide, SyncEngine, SyncResult};
use grafium_core::sync::filesystem::FilesystemBackend;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const TARGET: &str = "usb";
const NOTE: &str = "pages/todo.md";
const WHOLE_FILE: &str = "knowledge/doc.md";
const TODO: &str =
    "- Groceries\n- Buy milk\n- Call the plumber\n- Book flights\n- Water the plants\n";
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn target_folder(root: &Path, target: &str) -> PathBuf {
    root.join(DEFAULT_METADATA_DIR_NAME)
        .join("sync-targets")
        .join(compute_hash(target.as_bytes()))
}

/// Every file below `dir` in a stable order; none when `dir` is missing.
fn files_below(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn texts_below(dir: &Path) -> Vec<String> {
    let mut texts: Vec<String> = files_below(dir)
        .iter()
        .map(|file| fs::read_to_string(file).unwrap())
        .collect();
    texts.sort();
    texts
}

fn set_age(path: &Path, age: Duration) {
    let file = OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - age).unwrap();
}

fn age_all(dir: &Path, age: Duration) {
    for file in files_below(dir) {
        set_age(&file, age);
    }
}

fn age_of(path: &Path) -> Duration {
    SystemTime::now()
        .duration_since(fs::metadata(path).unwrap().modified().unwrap())
        .unwrap_or_default()
}

fn plant(path: &Path, text: &str, age: Duration) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    set_age(path, age);
}

struct Device {
    dir: tempfile::TempDir,
    engine: SyncEngine,
}

impl Device {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["pages", "knowledge"] {
            fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        let engine = SyncEngine::new_with_target(dir.path().to_path_buf(), TARGET);
        Self { dir, engine }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn recovery(&self) -> PathBuf {
        target_folder(self.root(), TARGET).join("sync-recovery")
    }

    fn bases(&self) -> PathBuf {
        target_folder(self.root(), TARGET).join("sync-bases")
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.root().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.root().join(rel)).unwrap()
    }

    fn sync(&self, backend: &FilesystemBackend) -> SyncResult {
        let result = self.engine.sync(backend).unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        result
    }
}

/// Two devices that have both synced the same files once.
struct World {
    _remote: tempfile::TempDir,
    backend: FilesystemBackend,
    a: Device,
    b: Device,
}

impl World {
    fn new(files: &[(&str, &str)]) -> Self {
        let remote = tempfile::tempdir().unwrap();
        let backend = FilesystemBackend::new(remote.path().to_path_buf(), "remote".into());
        let (a, b) = (Device::new(), Device::new());
        for (rel, text) in files {
            a.write(rel, text);
        }
        a.sync(&backend);
        b.sync(&backend);
        for (rel, text) in files {
            assert_eq!(b.read(rel), *text);
        }
        Self {
            _remote: remote,
            backend,
            a,
            b,
        }
    }

    /// Each device rewords a different line; B merges, so B keeps both
    /// originals. Afterwards everyone has the merged note again.
    fn merge_cleanly(&self, on_a: (&str, &str), on_b: (&str, &str)) {
        let base = self.a.read(NOTE);
        assert_eq!(self.b.read(NOTE), base);
        self.a.write(NOTE, &base.replace(on_a.0, on_a.1));
        self.b.write(NOTE, &base.replace(on_b.0, on_b.1));
        self.a.sync(&self.backend);
        assert_eq!(self.b.sync(&self.backend).merged, [NOTE]);
        self.a.sync(&self.backend);
        assert_eq!(self.a.read(NOTE), self.b.read(NOTE));
    }
}

// ---------------------------------------------------------------------------
// Recovery copies
// ---------------------------------------------------------------------------

#[test]
fn copies_expire_after_a_month_and_newer_ones_stay() {
    let world = World::new(&[(NOTE, TODO)]);
    world.merge_cleanly(
        ("Buy milk", "Buy oat milk"),
        ("Water the plants", "Water the ferns"),
    );
    let first = texts_below(&world.b.recovery());
    assert_eq!(first.len(), 2, "both originals are kept");

    // A month later another merge happens; the first pair is long forgotten.
    // (Edits on neighbouring lines would collide, so these are apart.)
    age_all(&world.b.recovery(), 31 * DAY);
    world.merge_cleanly(("Groceries", "Shopping"), ("Book flights", "Book trains"));

    let second = texts_below(&world.b.recovery());
    assert_eq!(second.len(), 2);
    assert!(first.iter().all(|text| !second.contains(text)));
}

#[test]
fn copies_younger_than_a_month_stay() {
    let world = World::new(&[(NOTE, TODO)]);
    world.merge_cleanly(
        ("Buy milk", "Buy oat milk"),
        ("Water the plants", "Water the ferns"),
    );
    let copies = files_below(&world.b.recovery());

    age_all(&world.b.recovery(), 29 * DAY);
    world.b.sync(&world.backend);

    assert_eq!(files_below(&world.b.recovery()), copies);
}

#[test]
fn copies_of_a_note_with_markers_stay_until_it_is_settled_and_then_expire() {
    let world = World::new(&[(NOTE, TODO)]);
    world
        .a
        .write(NOTE, &TODO.replace("Buy milk", "Buy oat milk"));
    world
        .b
        .write(NOTE, &TODO.replace("Buy milk", "Buy almond milk"));
    world.a.sync(&world.backend);
    assert_eq!(world.b.sync(&world.backend).merge_conflicts, [NOTE]);
    let copies = files_below(&world.b.recovery());
    assert_eq!(copies.len(), 2);

    // Markers wait for a person for as long as it takes.
    age_all(&world.b.recovery(), 90 * DAY);
    world.b.sync(&world.backend);
    assert_eq!(files_below(&world.b.recovery()), copies);
    assert_eq!(world.b.engine.pending_merges().len(), 1);

    // The copies are what undoes the decision, so their month starts with it.
    world
        .b
        .write(NOTE, &TODO.replace("Buy milk", "Buy oat and almond milk"));
    assert_eq!(world.b.sync(&world.backend).pushed, [NOTE]);
    assert!(world.b.engine.pending_merges().is_empty());
    assert_eq!(files_below(&world.b.recovery()), copies);
    assert!(copies.iter().all(|copy| age_of(copy) < DAY));

    age_all(&world.b.recovery(), 29 * DAY);
    world.b.sync(&world.backend);
    assert_eq!(files_below(&world.b.recovery()), copies);

    age_all(&world.b.recovery(), 31 * DAY);
    world.b.sync(&world.backend);
    assert!(files_below(&world.b.recovery()).is_empty());
    assert!(
        !world.b.recovery().exists(),
        "an empty recovery folder is removed"
    );
}

#[test]
fn copies_of_an_open_whole_file_conflict_stay_until_it_is_resolved_and_then_expire() {
    let world = World::new(&[(WHOLE_FILE, "line1\noriginal\nline3\n")]);
    world.a.write(WHOLE_FILE, "line1\nremote edit\nline3\n");
    world.b.write(WHOLE_FILE, "line1\nlocal edit\nline3\n");
    world.a.sync(&world.backend);
    assert_eq!(world.b.sync(&world.backend).conflicts, [WHOLE_FILE]);
    let copies = files_below(&world.b.recovery());
    assert_eq!(copies.len(), 2);

    age_all(&world.b.recovery(), 90 * DAY);
    world.b.sync(&world.backend);
    assert_eq!(files_below(&world.b.recovery()), copies);
    assert_eq!(world.b.engine.unresolved_conflicts().len(), 1);

    let view = world
        .b
        .engine
        .conflict_state(&world.backend, WHOLE_FILE)
        .unwrap();
    world
        .b
        .engine
        .resolve_conflict(
            &world.backend,
            WHOLE_FILE,
            view.local_hash.as_deref(),
            view.remote_hash.as_deref(),
            ConflictSide::Local,
        )
        .unwrap();
    assert!(world.b.engine.unresolved_conflicts().is_empty());
    assert_eq!(files_below(&world.b.recovery()), copies);
    assert!(copies.iter().all(|copy| age_of(copy) < DAY));

    age_all(&world.b.recovery(), 31 * DAY);
    world.b.sync(&world.backend);
    assert!(files_below(&world.b.recovery()).is_empty());
}

// ---------------------------------------------------------------------------
// Merge bases
// ---------------------------------------------------------------------------

#[test]
fn bases_nobody_reads_are_swept_and_the_live_one_still_serves_a_merge() {
    let world = World::new(&[(NOTE, TODO)]);
    let live = world.b.bases().join(NOTE);
    assert_eq!(fs::read_to_string(&live).unwrap(), TODO);

    // What older versions left: a base for a file that is never merged, and
    // one for a note that no longer has a sync record.
    let binary = world.b.bases().join("assets/pic.png");
    let orphan = world.b.bases().join("pages/gone.md");
    plant(&binary, "stale", Duration::ZERO);
    plant(&orphan, "stale", Duration::ZERO);

    world.b.sync(&world.backend);

    assert!(live.exists());
    assert!(!binary.exists() && !orphan.exists());
    assert!(!world.b.bases().join("assets").exists());
    world.merge_cleanly(
        ("Buy milk", "Buy oat milk"),
        ("Water the plants", "Water the ferns"),
    );
}

#[test]
fn the_base_of_a_deleted_note_goes_with_it() {
    let world = World::new(&[(NOTE, TODO)]);
    assert!(world.b.bases().join(NOTE).exists());

    fs::remove_file(world.a.root().join(NOTE)).unwrap();
    world.a.sync(&world.backend);
    let result = world.b.sync(&world.backend);

    assert_eq!(result.deleted_local, [NOTE]);
    assert!(!world.b.root().join(NOTE).exists());
    assert!(files_below(&world.b.bases()).is_empty());
}

// ---------------------------------------------------------------------------
// Removed sync targets
// ---------------------------------------------------------------------------

#[test]
fn a_removed_target_leaves_no_state_and_its_copies_expire_with_a_later_sync() {
    let (usb_remote, cloud_remote) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let usb = FilesystemBackend::new(usb_remote.path().to_path_buf(), "usb".into());
    let cloud = FilesystemBackend::new(cloud_remote.path().to_path_buf(), "cloud".into());
    let graph = tempfile::tempdir().unwrap();
    fs::create_dir_all(graph.path().join("pages")).unwrap();
    fs::write(graph.path().join(NOTE), TODO).unwrap();
    let usb_engine = SyncEngine::new_with_target(graph.path().to_path_buf(), "usb");
    let cloud_engine = SyncEngine::new_with_target(graph.path().to_path_buf(), "cloud");
    usb_engine.sync(&usb).unwrap();
    cloud_engine.sync(&cloud).unwrap();

    // A clean merge on the usb target keeps both originals there.
    fs::write(
        graph.path().join(NOTE),
        TODO.replace("Buy milk", "Buy oat milk"),
    )
    .unwrap();
    fs::write(
        usb_remote.path().join(NOTE),
        TODO.replace("Water the plants", "Water the ferns"),
    )
    .unwrap();
    assert_eq!(usb_engine.sync(&usb).unwrap().merged, [NOTE]);

    let usb_folder = target_folder(graph.path(), "usb");
    let cloud_folder = target_folder(graph.path(), "cloud");
    let copies = files_below(&usb_folder.join("sync-recovery"));
    assert_eq!(copies.len(), 2);
    assert!(usb_folder.join("sync-state.json").exists());
    assert!(!files_below(&usb_folder.join("sync-bases")).is_empty());
    set_age(&copies[0], 40 * DAY);
    // A folder that is not a target's is none of this cleanup's business.
    let stranger = graph
        .path()
        .join(DEFAULT_METADATA_DIR_NAME)
        .join("sync-targets/notes/sync-bases/mine.txt");
    plant(&stranger, "mine", 90 * DAY);

    SyncEngine::forget_target(graph.path().to_path_buf(), DEFAULT_METADATA_DIR_NAME, "usb");

    assert!(!usb_folder.join("sync-state.json").exists());
    assert!(!usb_folder.join("sync-bases").exists());
    assert!(!copies[0].exists(), "an expired copy goes at once");
    assert!(copies[1].exists(), "a young copy is still the only record");

    // The next sync of any target finishes the job once the copy has expired.
    cloud_engine.sync(&cloud).unwrap();
    assert!(copies[1].exists());
    set_age(&copies[1], 31 * DAY);
    let result = cloud_engine.sync(&cloud).unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);

    assert!(!usb_folder.exists(), "nothing of the target is left");
    assert!(cloud_folder.join("sync-state.json").exists());
    assert!(stranger.exists());
    assert!(cloud_engine.sync(&cloud).unwrap().is_clean());
}

#[test]
fn a_graph_called_sync_targets_is_not_mistaken_for_removed_targets() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("sync-targets");
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join(NOTE), TODO).unwrap();
    // Folders in the graph that look like what a removed target leaves.
    let lookalike = root.join(compute_hash(b"lookalike"));
    let cache = lookalike.join("sync-bases/mine.txt");
    let copy = lookalike.join("sync-recovery/mine.txt");
    plant(&cache, "mine", Duration::ZERO);
    plant(&copy, "mine", 90 * DAY);

    let remote = tempfile::tempdir().unwrap();
    let backend = FilesystemBackend::new(remote.path().to_path_buf(), "remote".into());
    // The legacy layout keeps its state straight in the metadata folder.
    let engine = SyncEngine::new(root.clone());
    assert_eq!(engine.sync(&backend).unwrap().pushed, [NOTE]);

    assert!(cache.exists() && copy.exists());
}
