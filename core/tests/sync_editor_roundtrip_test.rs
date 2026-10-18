//! The outliner and the sync engine working on the same note, the way the app
//! runs them: every device has a real graph, files written by sync are read
//! back into it, and people edit with the same calls the editor uses.

use grafium_core::graph::Graph;
use grafium_core::sync::engine::{SyncEngine, SyncResult};
use grafium_core::sync::filesystem::FilesystemBackend;
use grafium_core::sync::scan_markers;
use std::fs;

const NOTE: &str = "pages/todo.md";
const TODO: &str = "- Buy milk\n- Walk the dog\n- Pay rent\n- Call mom\n- Water plants\n";

struct Device {
    dir: tempfile::TempDir,
    graph: Graph,
    engine: SyncEngine,
}

impl Device {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("pages")).unwrap();
        let graph = Graph::open(dir.path()).unwrap();
        let engine = SyncEngine::new(dir.path().to_path_buf());
        Self { dir, graph, engine }
    }

    fn note(&self) -> String {
        fs::read_to_string(self.dir.path().join(NOTE)).unwrap_or_default()
    }

    /// `(id, text)` of every block the outliner shows for the note.
    fn blocks(&self) -> Vec<(String, String)> {
        let page = self.graph.db.get_page_by_title("todo").unwrap();
        let blocks = self.graph.db.list_blocks_for_page(&page.id).unwrap();
        blocks.into_iter().map(|b| (b.id, b.content)).collect()
    }

    /// Reword the block containing `needle`; the app saves the whole note.
    fn edit(&self, needle: &str, replacement: &str) {
        let (id, _) = self
            .blocks()
            .into_iter()
            .find(|(_, text)| text.contains(needle))
            .unwrap_or_else(|| panic!("no block contains {needle:?}"));
        self.graph.update_block(&id, replacement, None).unwrap();
    }

    fn delete_blocks_where(&self, unwanted: impl Fn(&str) -> bool) {
        for (id, text) in self.blocks() {
            if unwanted(&text) {
                self.graph.delete_block(&id).unwrap();
            }
        }
    }

    /// Sync, then read what sync wrote back into the graph.
    fn sync(&self, backend: &FilesystemBackend) -> SyncResult {
        let result = self.engine.sync(backend).unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        self.graph.reconcile_files_from_disk().unwrap();
        result
    }
}

struct World {
    _remote: tempfile::TempDir,
    backend: FilesystemBackend,
    a: Device,
    b: Device,
}

impl World {
    /// Two devices that have both synced the same note once. `in_app` creates
    /// the note in the outliner; otherwise it arrives as a plain file.
    fn new(in_app: bool) -> Self {
        let remote = tempfile::tempdir().unwrap();
        let backend = FilesystemBackend::new(remote.path().to_path_buf(), "remote".into());
        let (a, b) = (Device::new(), Device::new());
        if in_app {
            a.graph
                .create_page_with_content("todo", false, TODO)
                .unwrap();
        } else {
            fs::write(a.dir.path().join(NOTE), TODO).unwrap();
            a.graph.reconcile_files_from_disk().unwrap();
        }
        a.sync(&backend);
        b.sync(&backend);
        assert_eq!(b.note(), a.note());
        Self {
            _remote: remote,
            backend,
            a,
            b,
        }
    }
}

fn is_marker(text: &str) -> bool {
    [
        "<<<<<<< YOURS (merge ",
        "======= (merge ",
        ">>>>>>> THEIRS (merge ",
    ]
    .iter()
    .any(|marker| text.starts_with(marker))
}

#[test]
fn edits_in_different_blocks_merge_although_each_device_invented_its_own_block_ids() {
    for in_app in [true, false] {
        let world = World::new(in_app);
        world.a.edit("Walk the dog", "Walk the dog (done)");
        world.b.edit("Call mom", "Call mom at 5");
        // The first save in the app wrote an id on every block of the note, and
        // each device made up its own.
        assert!(world.a.note().contains("id:: ") && world.b.note().contains("id:: "));

        world.a.sync(&world.backend);
        let merged = world.b.sync(&world.backend);
        assert_eq!(merged.merged, [NOTE], "in_app {in_app}");
        assert!(merged.merge_conflicts.is_empty() && merged.conflicts.is_empty());

        world.a.sync(&world.backend);
        let note = world.b.note();
        assert!(note.contains("Walk the dog (done)") && note.contains("Call mom at 5"));
        assert!(scan_markers(&note).is_none());
        assert_eq!(world.a.note(), note, "in_app {in_app}");
        assert!(world.a.sync(&world.backend).is_clean());
        assert!(world.b.sync(&world.backend).is_clean());
    }
}

#[test]
fn a_marked_note_is_resolved_in_the_outliner_and_then_syncs_everywhere() {
    let world = World::new(true);
    world.a.edit("Walk the dog", "Walk the dog (done)");
    world.b.edit("Walk the dog", "Walk the cat");
    world.a.sync(&world.backend);

    let marked = world.b.sync(&world.backend);
    assert_eq!(marked.merge_conflicts, [NOTE]);
    let held = scan_markers(&world.b.note()).expect("markers are written into the note");
    assert_eq!(held.hunks, 1);

    // Each marker line is a block of its own that can be selected and deleted.
    let texts: Vec<String> = world.b.blocks().into_iter().map(|(_, text)| text).collect();
    for marker in [
        "<<<<<<< YOURS (merge ",
        "======= (merge ",
        ">>>>>>> THEIRS (merge ",
    ] {
        assert!(
            texts.iter().any(|text| text.starts_with(marker)),
            "{marker}: {texts:?}"
        );
    }

    // Editing around the markers re-saves the note but keeps it on hold, so
    // nothing half-resolved reaches the other device.
    world.b.edit("Pay rent", "Pay rent today");
    assert_eq!(scan_markers(&world.b.note()), Some(held));
    assert!(world.b.sync(&world.backend).pushed.is_empty());
    assert!(world.a.sync(&world.backend).is_clean());
    assert_eq!(world.b.engine.pending_merges().len(), 1);

    // Resolving is deleting the marker lines and the side that is not wanted.
    world
        .b
        .delete_blocks_where(|text| is_marker(text) || text.contains("Walk the cat"));
    let resolved = world.b.note();
    assert!(scan_markers(&resolved).is_none());
    assert!(resolved.contains("Walk the dog (done)") && resolved.contains("Pay rent today"));
    assert!(!resolved.contains("Walk the cat"));

    assert_eq!(world.b.sync(&world.backend).pushed, [NOTE]);
    assert_eq!(world.a.sync(&world.backend).pulled, [NOTE]);
    assert_eq!(world.a.note(), resolved);
    assert!(world.a.engine.pending_merges().is_empty());
    assert!(world.b.engine.pending_merges().is_empty());
    assert!(world.a.sync(&world.backend).is_clean());
    assert!(world.b.sync(&world.backend).is_clean());
}
