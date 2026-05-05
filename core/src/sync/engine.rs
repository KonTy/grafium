use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use chrono::Utc;
use crate::error::Result;
use super::backend::{SyncBackend, compute_hash, hash_file};
use super::merge;
use super::state::SyncState;

/// Result of a sync operation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncResult {
    /// Files pushed from local to remote
    pub pushed: Vec<String>,
    /// Files pulled from remote to local
    pub pulled: Vec<String>,
    /// Files where conflicts were detected (both sides changed)
    pub conflicts: Vec<String>,
    /// Files that were auto-merged (both sides changed, no overlapping edits)
    pub merged: Vec<String>,
    /// Files deleted from remote (local deletion propagated)
    pub deleted_remote: Vec<String>,
    /// Files deleted locally (remote deletion propagated)
    pub deleted_local: Vec<String>,
    /// Errors encountered (non-fatal, per-file)
    pub errors: Vec<String>,
}

impl SyncResult {
    fn new() -> Self {
        Self {
            pushed: Vec::new(),
            pulled: Vec::new(),
            conflicts: Vec::new(),
            merged: Vec::new(),
            deleted_remote: Vec::new(),
            deleted_local: Vec::new(),
            errors: Vec::new(),
        }
    }

    pub fn is_clean(&self) -> bool {
        self.pushed.is_empty()
            && self.pulled.is_empty()
            && self.conflicts.is_empty()
            && self.merged.is_empty()
            && self.deleted_remote.is_empty()
            && self.deleted_local.is_empty()
            && self.errors.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "↑{} ↓{} 🔀{} ⚡{} 🗑{}+{} ❌{}",
            self.pushed.len(),
            self.pulled.len(),
            self.merged.len(),
            self.conflicts.len(),
            self.deleted_remote.len(),
            self.deleted_local.len(),
            self.errors.len(),
        )
    }
}

/// The sync engine orchestrates bidirectional sync between a local graph
/// folder and a remote backend.
pub struct SyncEngine {
    /// Path to the local graph root
    local_root: PathBuf,
    /// Path to the sync state file
    state_path: PathBuf,
    /// Directory for storing base content (common ancestor for 3-way merge)
    bases_dir: PathBuf,
}

impl SyncEngine {
    pub fn new(local_root: PathBuf) -> Self {
        let state_path = local_root.join(".logseq").join("sync-state.json");
        let bases_dir = local_root.join(".logseq").join("sync-bases");
        Self { local_root, state_path, bases_dir }
    }

    /// Path to the cached base content for a given relative path.
    fn base_path(&self, rel_path: &str) -> PathBuf {
        self.bases_dir.join(rel_path)
    }

    /// Save a copy of file content as the merge base for future syncs.
    fn save_base(&self, rel_path: &str, content: &[u8]) {
        let path = self.base_path(rel_path);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, content);
    }

    /// Load the cached base content, if available.
    fn load_base(&self, rel_path: &str) -> Option<Vec<u8>> {
        fs::read(self.base_path(rel_path)).ok()
    }

    /// Run a full bidirectional sync against the given backend.
    pub fn sync(&self, backend: &dyn SyncBackend) -> Result<SyncResult> {
        if !backend.is_available() {
            return Err(crate::error::CoreError::Io(
                std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    format!("Sync target '{}' is not available", backend.name()),
                )
            ));
        }

        let mut state = SyncState::load(&self.state_path);
        let mut result = SyncResult::new();

        // Step 1: Collect local files with hashes
        let local_files = self.collect_local_files()?;

        // Step 2: Collect remote files
        let remote_files_list = backend.list_files()?;
        let remote_files: HashMap<String, _> = remote_files_list
            .into_iter()
            .map(|f| (f.rel_path.clone(), f))
            .collect();

        // Step 3: Determine actions for each file
        let all_paths: std::collections::HashSet<String> = local_files
            .keys()
            .chain(remote_files.keys())
            .chain(state.files.keys())
            .cloned()
            .collect();

        for rel_path in all_paths {
            let local_exists = local_files.contains_key(&rel_path);
            let remote_exists = remote_files.contains_key(&rel_path);
            let was_synced = state.files.contains_key(&rel_path);

            match (local_exists, remote_exists, was_synced) {
                // Both exist — check for changes
                (true, true, true) => {
                    self.sync_both_exist(backend, &rel_path, &local_files, &mut state, &mut result);
                }
                // Both exist but never synced — treat as potential conflict
                (true, true, false) => {
                    self.sync_both_new(backend, &rel_path, &local_files, &mut state, &mut result);
                }
                // Only local exists, was previously synced — remote was deleted
                (true, false, true) => {
                    // Remote deletion: delete locally
                    let local_path = self.local_root.join(&rel_path);
                    if let Err(e) = fs::remove_file(&local_path) {
                        result.errors.push(format!("Delete local {}: {}", rel_path, e));
                    } else {
                        state.remove_record(&rel_path);
                        result.deleted_local.push(rel_path);
                    }
                }
                // Only local exists, never synced — new local file, push
                (true, false, false) => {
                    self.push_to_remote(backend, &rel_path, &mut state, &mut result);
                }
                // Only remote exists, was previously synced — local was deleted
                (false, true, true) => {
                    // Local deletion: delete on remote
                    if let Err(e) = backend.delete_file(&rel_path) {
                        result.errors.push(format!("Delete remote {}: {}", rel_path, e));
                    } else {
                        state.remove_record(&rel_path);
                        result.deleted_remote.push(rel_path);
                    }
                }
                // Only remote exists, never synced — new remote file, pull
                (false, true, false) => {
                    self.pull_from_remote(backend, &rel_path, &mut state, &mut result);
                }
                // Neither exists but was synced — both deleted, just clean up state
                (false, false, true) => {
                    state.remove_record(&rel_path);
                }
                // Neither exists, never synced — impossible, skip
                (false, false, false) => {}
            }
        }

        // Save updated state
        state.last_sync = Some(Utc::now().timestamp());
        state.save(&self.state_path)?;

        Ok(result)
    }

    /// Both local and remote exist and were previously synced — check for changes.
    fn sync_both_exist(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        local_files: &HashMap<String, String>,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let local_hash = &local_files[rel_path];
        let local_changed = state.has_local_changes(rel_path, local_hash);

        // Get remote hash
        let remote_hash = match backend.file_hash(rel_path) {
            Ok(h) => h,
            Err(e) => {
                result.errors.push(format!("Hash remote {}: {}", rel_path, e));
                return;
            }
        };
        let sync_hash = state.files.get(rel_path).map(|r| r.hash_at_sync.as_str()).unwrap_or("");
        let remote_changed = remote_hash != sync_hash;

        match (local_changed, remote_changed) {
            (false, false) => {} // No changes anywhere
            (true, false) => {
                // Only local changed — push
                self.push_to_remote(backend, rel_path, state, result);
            }
            (false, true) => {
                // Only remote changed — pull
                self.pull_from_remote(backend, rel_path, state, result);
            }
            (true, true) => {
                // Both changed — conflict!
                if local_hash == &remote_hash {
                    // Same content — no real conflict, just update state + base
                    let local_path = self.local_root.join(rel_path);
                    if let Ok(content) = fs::read(&local_path) {
                        self.save_base(rel_path, &content);
                    }
                    state.record_sync(rel_path, local_hash);
                } else {
                    self.handle_conflict(backend, rel_path, state, result);
                }
            }
        }
    }

    /// Both exist but never synced before — compare content.
    fn sync_both_new(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        local_files: &HashMap<String, String>,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let local_hash = &local_files[rel_path];
        let remote_hash = match backend.file_hash(rel_path) {
            Ok(h) => h,
            Err(e) => {
                result.errors.push(format!("Hash remote {}: {}", rel_path, e));
                return;
            }
        };

        if local_hash == &remote_hash {
            // Identical — record in state + save base for future merges
            let local_path = self.local_root.join(rel_path);
            if let Ok(content) = fs::read(&local_path) {
                self.save_base(rel_path, &content);
            }
            state.record_sync(rel_path, local_hash);
        } else {
            // Different content — conflict
            self.handle_conflict(backend, rel_path, state, result);
        }
    }

    /// Push a local file to the remote.
    fn push_to_remote(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let local_path = self.local_root.join(rel_path);
        let content = match fs::read(&local_path) {
            Ok(c) => c,
            Err(e) => {
                result.errors.push(format!("Read local {}: {}", rel_path, e));
                return;
            }
        };
        let hash = compute_hash(&content);

        if let Err(e) = backend.write_file(rel_path, &content) {
            result.errors.push(format!("Push {}: {}", rel_path, e));
        } else {
            self.save_base(rel_path, &content);
            state.record_sync(rel_path, &hash);
            result.pushed.push(rel_path.to_string());
        }
    }

    /// Pull a remote file to local.
    fn pull_from_remote(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let content = match backend.read_file(rel_path) {
            Ok(c) => c,
            Err(e) => {
                result.errors.push(format!("Pull {}: {}", rel_path, e));
                return;
            }
        };
        let hash = compute_hash(&content);

        let local_path = self.local_root.join(rel_path);
        if let Some(parent) = local_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Err(e) = fs::write(&local_path, &content) {
            result.errors.push(format!("Write local {}: {}", rel_path, e));
        } else {
            self.save_base(rel_path, &content);
            state.record_sync(rel_path, &hash);
            result.pulled.push(rel_path.to_string());
        }
    }

    /// Handle a conflict using 3-way merge with conflict markers.
    ///
    /// If a cached base (common ancestor) is available, performs a true 3-way
    /// merge: non-overlapping changes are auto-merged, overlapping edits get
    /// Git-style conflict markers (`<<<<<<< local` / `=======` / `>>>>>>> remote`).
    ///
    /// If no base is cached (first sync), falls back to 2-way merge (all
    /// differing sections get conflict markers).
    ///
    /// The merged file (possibly with markers) is written to both local and
    /// remote so every device sees the same state. A `.conflict_*.md` backup
    /// of the remote version is still created so no data is ever lost.
    fn handle_conflict(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        // Read both versions
        let local_path = self.local_root.join(rel_path);
        let local_content = match fs::read(&local_path) {
            Ok(c) => c,
            Err(e) => {
                result.errors.push(format!("Read local for conflict {}: {}", rel_path, e));
                return;
            }
        };
        let remote_content = match backend.read_file(rel_path) {
            Ok(c) => c,
            Err(e) => {
                result.errors.push(format!("Read remote for conflict {}: {}", rel_path, e));
                return;
            }
        };

        let local_text = String::from_utf8_lossy(&local_content);
        let remote_text = String::from_utf8_lossy(&remote_content);

        // Attempt 3-way merge if we have a cached base
        let merge_result = if let Some(base_content) = self.load_base(rel_path) {
            let base_text = String::from_utf8_lossy(&base_content);
            merge::three_way_merge(&base_text, &local_text, &remote_text)
        } else {
            // No base available — 2-way merge
            merge::two_way_merge(&local_text, &remote_text)
        };

        // Always save a .conflict backup of the remote version (no data loss)
        let conflict_path = make_conflict_path(rel_path);
        let local_conflict_path = self.local_root.join(&conflict_path);
        if let Some(parent) = local_conflict_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = fs::write(&local_conflict_path, &remote_content) {
            result.errors.push(format!("Write conflict backup {}: {}", conflict_path, e));
        }
        // Push conflict backup to remote too so both devices see it
        if let Err(e) = backend.write_file(&conflict_path, &remote_content) {
            result.errors.push(format!("Push conflict backup {}: {}", conflict_path, e));
        }

        // Write the merged content to local and remote
        let merged_bytes = merge_result.content.as_bytes();
        let merged_hash = compute_hash(merged_bytes);

        if let Err(e) = fs::write(&local_path, merged_bytes) {
            result.errors.push(format!("Write merged local {}: {}", rel_path, e));
            return;
        }
        if let Err(e) = backend.write_file(rel_path, merged_bytes) {
            result.errors.push(format!("Push merged {}: {}", rel_path, e));
        }

        self.save_base(rel_path, merged_bytes);
        state.record_sync(rel_path, &merged_hash);

        if merge_result.has_conflicts {
            result.conflicts.push(rel_path.to_string());
        } else {
            result.merged.push(rel_path.to_string());
        }
    }

    /// Collect all local .md files under pages/ and journals/ with their hashes.
    fn collect_local_files(&self) -> Result<HashMap<String, String>> {
        let mut files = HashMap::new();
        let pages_dir = self.local_root.join("pages");
        let journals_dir = self.local_root.join("journals");

        self.collect_dir_hashes(&pages_dir, &self.local_root, &mut files)?;
        self.collect_dir_hashes(&journals_dir, &self.local_root, &mut files)?;
        Ok(files)
    }

    fn collect_dir_hashes(&self, dir: &Path, base: &Path, out: &mut HashMap<String, String>) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                self.collect_dir_hashes(&path, base, out)?;
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                // Skip .conflict backup files (e.g. foo.conflict_20260504_120000.md)
                if path.to_string_lossy().contains(".conflict_") {
                    continue;
                }
                let rel = path.strip_prefix(base)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                let hash = hash_file(&path)?;
                out.insert(rel, hash);
            }
        }
        Ok(())
    }
}

/// Generate a conflict filename by inserting .conflict before the extension.
/// e.g. "pages/foo.md" -> "pages/foo.conflict.md"
fn make_conflict_path(rel_path: &str) -> String {
    if let Some(dot_idx) = rel_path.rfind('.') {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        format!("{}.conflict_{}{}", &rel_path[..dot_idx], timestamp, &rel_path[dot_idx..])
    } else {
        format!("{}.conflict", rel_path)
    }
}
