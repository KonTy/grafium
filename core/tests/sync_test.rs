use grafium_core::error::CoreError;
use grafium_core::sync::backend::{compute_hash, FileMetadata, SyncBackend};
use grafium_core::sync::engine::SyncEngine;
use grafium_core::sync::merge::{three_way_merge, two_way_merge};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// In-memory mock backend for testing (no real filesystem needed for remote)
// ---------------------------------------------------------------------------

struct MockBackend {
    name: String,
    available: bool,
    files: std::sync::Mutex<HashMap<String, Vec<u8>>>,
}

impl MockBackend {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            available: true,
            files: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn set_file(&self, rel_path: &str, content: &[u8]) {
        self.files
            .lock()
            .unwrap()
            .insert(rel_path.to_string(), content.to_vec());
    }

    fn get_file(&self, rel_path: &str) -> Option<Vec<u8>> {
        self.files.lock().unwrap().get(rel_path).cloned()
    }

    fn has_file(&self, rel_path: &str) -> bool {
        self.files.lock().unwrap().contains_key(rel_path)
    }

    fn file_count(&self) -> usize {
        self.files.lock().unwrap().len()
    }

    fn remove_file_directly(&self, rel_path: &str) {
        self.files.lock().unwrap().remove(rel_path);
    }
}

impl SyncBackend for MockBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn list_files(&self) -> grafium_core::error::Result<Vec<FileMetadata>> {
        let files = self.files.lock().unwrap();
        Ok(files
            .iter()
            .map(|(path, content)| FileMetadata {
                rel_path: path.clone(),
                size: content.len() as u64,
                modified_at: 0,
                hash: Some(compute_hash(content)),
            })
            .collect())
    }

    fn read_file(&self, rel_path: &str) -> grafium_core::error::Result<Vec<u8>> {
        let files = self.files.lock().unwrap();
        files
            .get(rel_path)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(format!("File not found: {}", rel_path)))
    }

    fn write_file(&self, rel_path: &str, content: &[u8]) -> grafium_core::error::Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(rel_path.to_string(), content.to_vec());
        Ok(())
    }

    fn delete_file(&self, rel_path: &str) -> grafium_core::error::Result<()> {
        self.files.lock().unwrap().remove(rel_path);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Create a temp directory with pages/ and journals/ subdirs, return the path.
fn setup_local_graph() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("pages")).unwrap();
    fs::create_dir_all(dir.path().join("journals")).unwrap();
    dir
}

fn write_local(dir: &Path, rel_path: &str, content: &str) {
    let full = dir.join(rel_path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&full, content).unwrap();
}

fn read_local(dir: &Path, rel_path: &str) -> String {
    fs::read_to_string(dir.join(rel_path)).unwrap()
}

fn local_exists(dir: &Path, rel_path: &str) -> bool {
    dir.join(rel_path).exists()
}

// ===========================================================================
// Sync integration tests
// ===========================================================================

#[test]
fn test_fresh_sync_pushes_local_to_empty_remote() {
    let local = setup_local_graph();
    write_local(local.path(), "pages/hello.md", "# Hello\n- world\n");
    write_local(local.path(), "journals/2026-05-04.md", "- journal entry\n");

    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    let result = engine.sync(&backend).unwrap();
    assert_eq!(result.pushed.len(), 2);
    assert!(result.pulled.is_empty());
    assert!(result.conflicts.is_empty());
    assert!(result.errors.is_empty());

    // Remote should now have both files
    assert!(backend.has_file("pages/hello.md"));
    assert!(backend.has_file("journals/2026-05-04.md"));
    assert_eq!(
        String::from_utf8(backend.get_file("pages/hello.md").unwrap()).unwrap(),
        "# Hello\n- world\n"
    );
}

#[test]
fn test_fresh_sync_pulls_remote_to_empty_local() {
    let local = setup_local_graph();
    let backend = MockBackend::new("test");
    backend.set_file("pages/remote-page.md", b"# From Remote\n");
    backend.set_file("journals/2026-05-01.md", b"- remote journal\n");

    let engine = SyncEngine::new(local.path().to_path_buf());
    let result = engine.sync(&backend).unwrap();

    assert_eq!(result.pulled.len(), 2);
    assert!(result.pushed.is_empty());
    assert!(result.conflicts.is_empty());
    assert_eq!(
        read_local(local.path(), "pages/remote-page.md"),
        "# From Remote\n"
    );
}

#[test]
fn test_no_changes_second_sync_is_clean() {
    let local = setup_local_graph();
    write_local(local.path(), "pages/stable.md", "stable content\n");

    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    // First sync — push
    let r1 = engine.sync(&backend).unwrap();
    assert_eq!(r1.pushed.len(), 1);

    // Second sync — nothing changed
    let r2 = engine.sync(&backend).unwrap();
    assert!(r2.is_clean(), "Second sync should be clean, got: {:?}", r2);
}

#[test]
fn test_local_edit_pushes_to_remote() {
    let local = setup_local_graph();
    write_local(local.path(), "pages/doc.md", "original\n");

    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    // Initial sync
    engine.sync(&backend).unwrap();

    // Edit locally
    write_local(local.path(), "pages/doc.md", "edited locally\n");

    let r = engine.sync(&backend).unwrap();
    assert_eq!(r.pushed, vec!["pages/doc.md"]);
    assert_eq!(
        String::from_utf8(backend.get_file("pages/doc.md").unwrap()).unwrap(),
        "edited locally\n"
    );
}

#[test]
fn test_remote_edit_pulls_to_local() {
    let local = setup_local_graph();
    write_local(local.path(), "pages/doc.md", "original\n");

    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    // Initial sync
    engine.sync(&backend).unwrap();

    // Edit on remote
    backend.set_file("pages/doc.md", b"edited remotely\n");

    let r = engine.sync(&backend).unwrap();
    assert_eq!(r.pulled, vec!["pages/doc.md"]);
    assert_eq!(
        read_local(local.path(), "pages/doc.md"),
        "edited remotely\n"
    );
}

#[test]
fn test_both_edited_same_content_no_conflict() {
    let local = setup_local_graph();
    write_local(local.path(), "pages/doc.md", "original\n");

    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    // Initial sync
    engine.sync(&backend).unwrap();

    // Both sides change to the same content
    write_local(local.path(), "pages/doc.md", "same edit\n");
    backend.set_file("pages/doc.md", b"same edit\n");

    let r = engine.sync(&backend).unwrap();
    assert!(r.conflicts.is_empty());
    assert!(r.errors.is_empty());
}

#[test]
fn test_conflict_creates_markers_and_backup() {
    let local = setup_local_graph();
    write_local(local.path(), "pages/doc.md", "line1\noriginal\nline3\n");

    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    // Initial sync (this saves the base)
    engine.sync(&backend).unwrap();

    // Both sides change differently
    write_local(local.path(), "pages/doc.md", "line1\nlocal edit\nline3\n");
    backend.set_file("pages/doc.md", b"line1\nremote edit\nline3\n");

    let r = engine.sync(&backend).unwrap();

    // Should be a conflict
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0], "pages/doc.md");

    // The merged file should contain conflict markers
    let merged = read_local(local.path(), "pages/doc.md");
    assert!(
        merged.contains("<<<<<<< local"),
        "Missing local marker in:\n{}",
        merged
    );
    assert!(
        merged.contains("local edit"),
        "Missing local content in:\n{}",
        merged
    );
    assert!(
        merged.contains("======="),
        "Missing separator in:\n{}",
        merged
    );
    assert!(
        merged.contains("remote edit"),
        "Missing remote content in:\n{}",
        merged
    );
    assert!(
        merged.contains(">>>>>>> remote"),
        "Missing remote marker in:\n{}",
        merged
    );

    // Unchanged lines should be preserved
    assert!(
        merged.contains("line1"),
        "Missing unchanged line1 in:\n{}",
        merged
    );
    assert!(
        merged.contains("line3"),
        "Missing unchanged line3 in:\n{}",
        merged
    );

    // A .conflict backup file should exist
    let conflict_files: Vec<_> = fs::read_dir(local.path().join("pages"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".conflict"))
        .collect();
    assert_eq!(conflict_files.len(), 1, "Expected one conflict backup file");

    // The backup should contain the remote version
    let backup = fs::read_to_string(conflict_files[0].path()).unwrap();
    assert_eq!(backup, "line1\nremote edit\nline3\n");

    // Remote should also have the merged version
    let remote_merged = String::from_utf8(backend.get_file("pages/doc.md").unwrap()).unwrap();
    assert!(remote_merged.contains("<<<<<<< local"));
}

#[test]
fn test_3way_merge_auto_resolves_non_overlapping_changes() {
    let local = setup_local_graph();
    let base = "header\n\nparagraph 1\n\nparagraph 2\n\nfooter\n";
    write_local(local.path(), "pages/doc.md", base);

    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    // Initial sync (saves base)
    engine.sync(&backend).unwrap();

    // Local edits paragraph 1, remote edits paragraph 2
    let local_ver = "header\n\nlocal paragraph 1\n\nparagraph 2\n\nfooter\n";
    let remote_ver = "header\n\nparagraph 1\n\nremote paragraph 2\n\nfooter\n";
    write_local(local.path(), "pages/doc.md", local_ver);
    backend.set_file("pages/doc.md", remote_ver.as_bytes());

    let r = engine.sync(&backend).unwrap();

    // Should auto-merge without conflicts
    assert!(
        r.conflicts.is_empty(),
        "Expected auto-merge but got conflicts: {:?}",
        r.conflicts
    );
    assert_eq!(r.merged.len(), 1, "Expected 1 merged file, got: {:?}", r);

    // Merged content should contain both changes
    let merged = read_local(local.path(), "pages/doc.md");
    assert!(
        merged.contains("local paragraph 1"),
        "Missing local change in:\n{}",
        merged
    );
    assert!(
        merged.contains("remote paragraph 2"),
        "Missing remote change in:\n{}",
        merged
    );
    assert!(
        !merged.contains("<<<<<<< local"),
        "Should not have conflict markers in:\n{}",
        merged
    );
}

#[test]
fn test_local_delete_propagates_to_remote() {
    let local = setup_local_graph();
    write_local(local.path(), "pages/to-delete.md", "doomed content\n");

    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    // Initial sync
    engine.sync(&backend).unwrap();
    assert!(backend.has_file("pages/to-delete.md"));

    // Delete locally
    fs::remove_file(local.path().join("pages/to-delete.md")).unwrap();

    let r = engine.sync(&backend).unwrap();
    assert_eq!(r.deleted_remote, vec!["pages/to-delete.md"]);
    assert!(!backend.has_file("pages/to-delete.md"));
}

#[test]
fn test_remote_delete_propagates_to_local() {
    let local = setup_local_graph();
    write_local(local.path(), "pages/to-delete.md", "doomed content\n");

    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    // Initial sync
    engine.sync(&backend).unwrap();
    assert!(local_exists(local.path(), "pages/to-delete.md"));

    // Delete on remote
    backend.remove_file_directly("pages/to-delete.md");

    let r = engine.sync(&backend).unwrap();
    assert_eq!(r.deleted_local, vec!["pages/to-delete.md"]);
    assert!(!local_exists(local.path(), "pages/to-delete.md"));
}

#[test]
fn test_new_files_on_both_sides_no_conflict() {
    let local = setup_local_graph();
    write_local(local.path(), "pages/local-only.md", "local new\n");

    let backend = MockBackend::new("test");
    backend.set_file("pages/remote-only.md", b"remote new\n");

    let engine = SyncEngine::new(local.path().to_path_buf());
    let r = engine.sync(&backend).unwrap();

    // local-only should be pushed, remote-only should be pulled
    assert!(r.pushed.contains(&"pages/local-only.md".to_string()));
    assert!(r.pulled.contains(&"pages/remote-only.md".to_string()));
    assert!(r.conflicts.is_empty());

    // Both sides should have both files
    assert!(backend.has_file("pages/local-only.md"));
    assert!(local_exists(local.path(), "pages/remote-only.md"));
}

#[test]
fn test_hierarchical_folders_sync() {
    let local = setup_local_graph();
    write_local(
        local.path(),
        "pages/Books/Rust/Chapter1.md",
        "# Chapter 1\n",
    );
    write_local(
        local.path(),
        "pages/Books/Rust/Chapter2.md",
        "# Chapter 2\n",
    );

    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    let r = engine.sync(&backend).unwrap();
    assert_eq!(r.pushed.len(), 2);
    assert!(backend.has_file("pages/Books/Rust/Chapter1.md"));
    assert!(backend.has_file("pages/Books/Rust/Chapter2.md"));
}

#[test]
fn test_remote_hierarchical_pull_creates_dirs() {
    let local = setup_local_graph();
    let backend = MockBackend::new("test");
    backend.set_file("pages/Projects/Alpha/README.md", b"# Alpha\n");

    let engine = SyncEngine::new(local.path().to_path_buf());
    let r = engine.sync(&backend).unwrap();

    assert_eq!(r.pulled.len(), 1);
    assert!(local_exists(local.path(), "pages/Projects/Alpha/README.md"));
    assert_eq!(
        read_local(local.path(), "pages/Projects/Alpha/README.md"),
        "# Alpha\n"
    );
}

#[test]
fn test_conflict_file_skipped_during_sync() {
    let local = setup_local_graph();
    write_local(local.path(), "pages/doc.md", "content\n");
    // Manually create a conflict file — should be ignored by the sync engine
    write_local(
        local.path(),
        "pages/doc.conflict_20260504_120000.md",
        "old conflict\n",
    );

    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    let r = engine.sync(&backend).unwrap();
    // Only the real doc should be pushed, not the conflict file
    assert_eq!(r.pushed, vec!["pages/doc.md"]);
    assert!(!backend.has_file("pages/doc.conflict_20260504_120000.md"));
}

#[test]
fn test_multiple_syncs_with_incremental_changes() {
    let local = setup_local_graph();
    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    // Sync 1: create page
    write_local(local.path(), "pages/evolving.md", "v1\n");
    let r1 = engine.sync(&backend).unwrap();
    assert_eq!(r1.pushed.len(), 1);

    // Sync 2: edit page locally
    write_local(local.path(), "pages/evolving.md", "v2\n");
    let r2 = engine.sync(&backend).unwrap();
    assert_eq!(r2.pushed, vec!["pages/evolving.md"]);

    // Sync 3: edit page on remote
    backend.set_file("pages/evolving.md", b"v3\n");
    let r3 = engine.sync(&backend).unwrap();
    assert_eq!(r3.pulled, vec!["pages/evolving.md"]);
    assert_eq!(read_local(local.path(), "pages/evolving.md"), "v3\n");

    // Sync 4: no changes
    let r4 = engine.sync(&backend).unwrap();
    assert!(r4.is_clean());
}

#[test]
fn test_both_deleted_cleans_state() {
    let local = setup_local_graph();
    write_local(local.path(), "pages/ephemeral.md", "temporary\n");

    let backend = MockBackend::new("test");
    let engine = SyncEngine::new(local.path().to_path_buf());

    // Sync to establish state
    engine.sync(&backend).unwrap();

    // Both sides delete
    fs::remove_file(local.path().join("pages/ephemeral.md")).unwrap();
    backend.remove_file_directly("pages/ephemeral.md");

    let r = engine.sync(&backend).unwrap();
    // Should be clean — state just cleaned up, no actions
    assert!(r.is_clean());
}

#[test]
fn test_journals_sync() {
    let local = setup_local_graph();
    write_local(local.path(), "journals/2026-05-01.md", "- morning\n");
    write_local(local.path(), "journals/2026-05-02.md", "- afternoon\n");

    let backend = MockBackend::new("test");
    backend.set_file("journals/2026-05-03.md", b"- evening\n");

    let engine = SyncEngine::new(local.path().to_path_buf());
    let r = engine.sync(&backend).unwrap();

    assert_eq!(r.pushed.len(), 2);
    assert_eq!(r.pulled.len(), 1);
    assert!(backend.has_file("journals/2026-05-01.md"));
    assert!(local_exists(local.path(), "journals/2026-05-03.md"));
}

#[test]
fn test_sync_result_summary() {
    let local = setup_local_graph();
    write_local(local.path(), "pages/a.md", "a\n");

    let backend = MockBackend::new("test");
    backend.set_file("pages/b.md", b"b\n");

    let engine = SyncEngine::new(local.path().to_path_buf());
    let r = engine.sync(&backend).unwrap();

    let summary = r.summary();
    assert!(
        summary.contains("↑1"),
        "Summary should show 1 push: {}",
        summary
    );
    assert!(
        summary.contains("↓1"),
        "Summary should show 1 pull: {}",
        summary
    );
}

// ===========================================================================
// 3-way merge unit tests (beyond what's in merge.rs)
// ===========================================================================

#[test]
fn test_merge_real_markdown_page() {
    let base = "\
title:: Weekly Review
tags:: #review, #planning

- Review last week
  - Completed tasks
  - Blocked items
- Plan next week
  - Set priorities
  - Schedule meetings
";
    let local = "\
title:: Weekly Review
tags:: #review, #planning, #work

- Review last week
  - Completed tasks
  - Blocked items
  - Carried-over items
- Plan next week
  - Set priorities
  - Schedule meetings
";
    let remote = "\
title:: Weekly Review
tags:: #review, #planning

- Review last week
  - Completed tasks
  - Blocked items
- Plan next week
  - Set priorities
  - Schedule meetings
  - Book room for standup
";

    let r = three_way_merge(base, local, remote);

    // Local changed tags and added a line to review section
    // Remote added a line to plan section
    // These are non-overlapping → should auto-merge
    assert!(
        !r.has_conflicts,
        "Expected clean merge but got conflicts:\n{}",
        r.content
    );
    assert!(r.content.contains("#work"), "Missing local tag change");
    assert!(
        r.content.contains("Carried-over items"),
        "Missing local addition:\n{}",
        r.content
    );
    assert!(
        r.content.contains("Book room for standup"),
        "Missing remote addition:\n{}",
        r.content
    );
}

#[test]
fn test_merge_preserves_properties() {
    let base = "title:: My Page\nauthor:: Alice\nstatus:: draft\n\n- content here\n";
    let local = "title:: My Page\nauthor:: Alice\nstatus:: in-review\n\n- content here\n";
    let remote = "title:: My Page\nauthor:: Bob\nstatus:: draft\n\n- content here\n";

    let r = three_way_merge(base, local, remote);

    // status changed by local, author changed by remote → both non-overlapping
    assert!(!r.has_conflicts, "Expected clean merge:\n{}", r.content);
    assert!(
        r.content.contains("status:: in-review"),
        "Missing local status"
    );
    assert!(r.content.contains("author:: Bob"), "Missing remote author");
}

#[test]
fn test_merge_conflict_markers_format() {
    let base = "line1\nline2\n";
    let local = "line1\nlocal change\n";
    let remote = "line1\nremote change\n";

    let r = three_way_merge(base, local, remote);
    assert!(r.has_conflicts);

    // Verify marker format
    let lines: Vec<&str> = r.content.lines().collect();
    assert!(lines.contains(&"<<<<<<< local"));
    assert!(lines.contains(&"======="));
    assert!(lines.contains(&">>>>>>> remote"));

    // local content should be between <<<<<<< and =======
    let local_start = lines.iter().position(|l| *l == "<<<<<<< local").unwrap();
    let sep = lines.iter().position(|l| *l == "=======").unwrap();
    let remote_end = lines.iter().position(|l| *l == ">>>>>>> remote").unwrap();

    assert!(local_start < sep);
    assert!(sep < remote_end);

    // Check local content is between markers
    let local_content: Vec<&&str> = lines[local_start + 1..sep].iter().collect();
    assert!(local_content.iter().any(|l| l.contains("local change")));

    // Check remote content is between markers
    let remote_content: Vec<&&str> = lines[sep + 1..remote_end].iter().collect();
    assert!(remote_content.iter().any(|l| l.contains("remote change")));
}

#[test]
fn test_two_way_merge_no_base() {
    let local = "local only line 1\nshared line\nlocal only line 2\n";
    let remote = "remote only line 1\nshared line\nremote only line 2\n";

    let r = two_way_merge(local, remote);
    // With no base, differing sections should be conflicts
    assert!(r.has_conflicts);
    // But the shared line should appear
    assert!(r.content.contains("shared line"));
}
