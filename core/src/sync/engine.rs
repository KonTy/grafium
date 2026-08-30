use super::backend::{compute_hash, FileMetadata, SyncBackend};
use super::merge;
use super::state::SyncState;
use crate::error::Result;
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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

    /// A result representing a target that could not be synced at all.
    pub fn failed(message: impl Into<String>) -> Self {
        let mut result = Self::new();
        result.errors.push(message.into());
        result
    }

    pub fn is_clean(&self) -> bool {        self.pushed.is_empty()
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
        Self::new_with_metadata_dir(local_root, crate::graph::DEFAULT_METADATA_DIR_NAME)
    }

    pub fn new_with_metadata_dir(local_root: PathBuf, metadata_dir_name: &str) -> Self {
        let state_path = local_root.join(metadata_dir_name).join("sync-state.json");
        let bases_dir = local_root.join(metadata_dir_name).join("sync-bases");
        Self {
            local_root,
            state_path,
            bases_dir,
        }
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

    /// Path of the marker file written at the root of every sync target. It
    /// carries a random id that identifies this particular remote, so we can
    /// tell "the drive is empty / not mounted / not the one we synced with"
    /// apart from "these files were genuinely deleted on the other machine".
    const REMOTE_MARKER_PATH: &'static str = ".grafium-sync-id";

    /// Graph directories that participate in sync. `assets/` carries the media
    /// that notes reference, so omitting it leaves broken links on the other
    /// machine.
    const SYNCED_DIRS: [&'static str; 3] = ["pages/", "journals/", "assets/"];

    /// Only files under the graph's note directories participate in sync. This
    /// also keeps the marker file out of the synced set.
    ///
    /// Paths originate from a remote listing, so a hostile or buggy server can
    /// propose anything here. Reject traversal and absolute components: without
    /// this, an href resolving to `pages/../../../etc/passwd` would be joined
    /// onto the graph root and written outside it.
    fn is_syncable_path(rel_path: &str) -> bool {
        if !Self::SYNCED_DIRS
            .iter()
            .any(|dir| rel_path.starts_with(dir))
        {
            return false;
        }
        if Path::new(rel_path).is_absolute() {
            return false;
        }
        rel_path
            .split(['/', '\\'])
            .all(|component| !component.is_empty() && component != "." && component != "..")
    }

    fn read_remote_marker(backend: &dyn SyncBackend) -> Option<String> {
        let bytes = backend.read_file(Self::REMOTE_MARKER_PATH).ok()?;
        let id = String::from_utf8(bytes).ok()?.trim().to_string();
        if id.is_empty() {
            None
        } else {
            Some(id)
        }
    }

    /// Confirm the backend is the same remote we recorded last time.
    ///
    /// Without this check, a sync target that is reachable but empty — an
    /// auto-mount point whose drive was never actually mounted, a different
    /// stick, or a reformatted one — looks exactly like a remote on which
    /// every file was deleted, and the engine happily deletes the entire
    /// local graph to match it.
    fn verify_remote_identity(
        &self,
        backend: &dyn SyncBackend,
        state: &mut SyncState,
        remote_count: usize,
    ) -> Result<()> {
        let marker = Self::read_remote_marker(backend);
        let never_synced = state.files.is_empty() && state.remote_id.is_none();

        match (marker, never_synced) {
            // First sync against an already-initialised remote: adopt its id.
            (Some(id), true) => {
                state.remote_id = Some(id);
            }
            // First sync against a fresh remote: claim it.
            (None, true) => {
                let id = uuid::Uuid::new_v4().to_string();
                backend.write_file(Self::REMOTE_MARKER_PATH, id.as_bytes())?;
                state.remote_id = Some(id);
            }
            (Some(id), false) => match &state.remote_id {
                Some(known) if known != &id => {
                    return Err(crate::error::CoreError::Other(format!(
                        "Sync target '{}' is a different sync location than the one \
                         this graph was last synced with. Refusing to sync so that \
                         nothing is deleted. If you meant to switch to this location, \
                         reset the sync state for this graph first.",
                        backend.name()
                    )));
                }
                Some(_) => {}
                // State predates the marker; the remote already has one.
                None => state.remote_id = Some(id),
            },
            (None, false) => {
                // Synced before, but the remote has no marker.
                if state.remote_id.is_none() && remote_count > 0 {
                    // State predates the marker and the remote still holds
                    // files: adopt it and write the marker for next time.
                    let id = uuid::Uuid::new_v4().to_string();
                    backend.write_file(Self::REMOTE_MARKER_PATH, id.as_bytes())?;
                    state.remote_id = Some(id);
                } else {
                    return Err(crate::error::CoreError::Other(format!(
                        "Sync target '{}' does not contain this graph's sync marker. \
                         The drive may not be mounted, may be a different device, or \
                         may have been erased. Refusing to sync so that your {} local \
                         note(s) are not deleted.",
                        backend.name(),
                        state.files.len()
                    )));
                }
            }
        }
        Ok(())
    }

    /// Run a full bidirectional sync against the given backend.
    pub fn sync(&self, backend: &dyn SyncBackend) -> Result<SyncResult> {        if !backend.is_available() {
            return Err(crate::error::CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                format!("Sync target '{}' is not available", backend.name()),
            )));
        }

        let mut state = SyncState::load(&self.state_path);
        let mut result = SyncResult::new();

        // Step 0: Collect the remote listing and confirm this really is the
        // sync target we used last time before we act on any deletion.
        let mut remote_files_list = backend.list_files()?;
        remote_files_list.retain(|f| Self::is_syncable_path(&f.rel_path));
        self.verify_remote_identity(backend, &mut state, remote_files_list.len())?;

        // Step 1: Collect local files with cheap metadata. Reuse cached hashes
        // from sync state when size+mtime still match the last sync.
        let mut local_files = self.collect_local_files()?;
        for meta in local_files.values_mut() {
            if meta.hash.is_none() {
                if let Some(hash) = state.cached_local_hash(meta) {
                    meta.hash = Some(hash.to_string());
                }
            }
        }

        // Step 2: Seed cached hashes for the remote listing gathered in step 0.
        for meta in &mut remote_files_list {
            if meta.hash.is_none() {
                if let Some(hash) = state.cached_remote_hash(meta) {
                    meta.hash = Some(hash.to_string());
                }
            }
        }
        let mut remote_files: HashMap<String, _> = remote_files_list
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
                    self.sync_both_exist(
                        backend,
                        &rel_path,
                        &mut local_files,
                        &mut remote_files,
                        &mut state,
                        &mut result,
                    );
                }
                // Both exist but never synced — treat as potential conflict
                (true, true, false) => {
                    self.sync_both_new(
                        backend,
                        &rel_path,
                        &mut local_files,
                        &mut remote_files,
                        &mut state,
                        &mut result,
                    );
                }
                // Only local exists, was previously synced — remote was deleted
                (true, false, true) => {
                    // Remote deletion: delete locally
                    let local_path = self.local_root.join(&rel_path);
                    if let Err(e) = fs::remove_file(&local_path) {
                        result
                            .errors
                            .push(format!("Delete local {}: {}", rel_path, e));
                    } else {
                        state.remove_record(&rel_path);
                        result.deleted_local.push(rel_path);
                    }
                }
                // Only local exists, never synced — new local file, push
                (true, false, false) => {
                    self.push_to_remote(
                        backend,
                        &rel_path,
                        &mut local_files,
                        &mut state,
                        &mut result,
                    );
                }
                // Only remote exists, was previously synced — local was deleted
                (false, true, true) => {
                    // Local deletion: delete on remote
                    if let Err(e) = backend.delete_file(&rel_path) {
                        result
                            .errors
                            .push(format!("Delete remote {}: {}", rel_path, e));
                    } else {
                        state.remove_record(&rel_path);
                        result.deleted_remote.push(rel_path);
                    }
                }
                // Only remote exists, never synced — new remote file, pull
                (false, true, false) => {
                    self.pull_from_remote(
                        backend,
                        &rel_path,
                        remote_files.get(&rel_path).cloned(),
                        &mut state,
                        &mut result,
                    );
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
        local_files: &mut HashMap<String, FileMetadata>,
        remote_files: &mut HashMap<String, FileMetadata>,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let sync_hash = state
            .files
            .get(rel_path)
            .map(|r| r.hash_at_sync.clone())
            .unwrap_or_default();

        let local_meta = match local_files.get(rel_path).cloned() {
            Some(meta) => meta,
            None => return,
        };
        let remote_meta = match remote_files.get(rel_path).cloned() {
            Some(meta) => meta,
            None => return,
        };

        let local_maybe_changed = !state.local_metadata_matches(&local_meta);
        let remote_maybe_changed = !state.remote_metadata_matches(&remote_meta);

        let local_hash = if local_maybe_changed {
            match self.ensure_local_hash(rel_path, local_files) {
                Ok(hash) => Some(hash),
                Err(e) => {
                    result
                        .errors
                        .push(format!("Hash local {}: {}", rel_path, e));
                    return;
                }
            }
        } else {
            local_meta.hash.clone().or_else(|| Some(sync_hash.clone()))
        };
        let remote_hash = if remote_maybe_changed {
            match self.ensure_remote_hash(backend, rel_path, remote_files) {
                Ok(hash) => Some(hash),
                Err(e) => {
                    result
                        .errors
                        .push(format!("Hash remote {}: {}", rel_path, e));
                    return;
                }
            }
        } else {
            remote_meta.hash.clone().or_else(|| Some(sync_hash.clone()))
        };

        let local_changed = local_hash.as_deref() != Some(sync_hash.as_str());
        let remote_changed = remote_hash.as_deref() != Some(sync_hash.as_str());

        match (local_changed, remote_changed) {
            (false, false) => {
                if local_maybe_changed || remote_maybe_changed {
                    state.record_sync(rel_path, &sync_hash, Some(&local_meta), Some(&remote_meta));
                }
            }
            (true, false) => {
                self.push_to_remote(backend, rel_path, local_files, state, result);
            }
            (false, true) => {
                self.pull_from_remote(backend, rel_path, Some(remote_meta.clone()), state, result);
            }
            (true, true) => {
                if local_hash == remote_hash {
                    let local_path = self.local_root.join(rel_path);
                    if let Ok(content) = fs::read(&local_path) {
                        self.save_base(rel_path, &content);
                    }
                    if let Some(ref hash) = local_hash {
                        state.record_sync(rel_path, hash, Some(&local_meta), Some(&remote_meta));
                    }
                } else {
                    self.handle_conflict(
                        backend,
                        rel_path,
                        Some(remote_meta.clone()),
                        state,
                        result,
                    );
                }
            }
        }
    }

    /// Both exist but never synced before — compare content.
    fn sync_both_new(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        local_files: &mut HashMap<String, FileMetadata>,
        remote_files: &mut HashMap<String, FileMetadata>,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let local_meta = match local_files.get(rel_path).cloned() {
            Some(meta) => meta,
            None => return,
        };
        let remote_meta = match remote_files.get(rel_path).cloned() {
            Some(meta) => meta,
            None => return,
        };
        let local_hash = match self.ensure_local_hash(rel_path, local_files) {
            Ok(hash) => hash,
            Err(e) => {
                result
                    .errors
                    .push(format!("Hash local {}: {}", rel_path, e));
                return;
            }
        };
        let remote_hash = match self.ensure_remote_hash(backend, rel_path, remote_files) {
            Ok(hash) => hash,
            Err(e) => {
                result
                    .errors
                    .push(format!("Hash remote {}: {}", rel_path, e));
                return;
            }
        };

        if local_hash == remote_hash {
            // Identical — record in state + save base for future merges
            let local_path = self.local_root.join(rel_path);
            if let Ok(content) = fs::read(&local_path) {
                self.save_base(rel_path, &content);
            }
            state.record_sync(rel_path, &local_hash, Some(&local_meta), Some(&remote_meta));
        } else {
            // Different content — conflict
            self.handle_conflict(backend, rel_path, Some(remote_meta.clone()), state, result);
        }
    }

    /// Push a local file to the remote.
    fn push_to_remote(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        local_files: &mut HashMap<String, FileMetadata>,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let local_path = self.local_root.join(rel_path);
        let content = match fs::read(&local_path) {
            Ok(c) => c,
            Err(e) => {
                result
                    .errors
                    .push(format!("Read local {}: {}", rel_path, e));
                return;
            }
        };
        let hash = compute_hash(&content);
        if let Some(meta) = local_files.get_mut(rel_path) {
            meta.hash = Some(hash.clone());
        }

        if let Err(e) = backend.write_file(rel_path, &content) {
            result.errors.push(format!("Push {}: {}", rel_path, e));
        } else {
            self.save_base(rel_path, &content);
            let local_meta = local_files.get(rel_path).cloned();
            let remote_meta = backend.stat_file(rel_path).ok();
            state.record_sync(rel_path, &hash, local_meta.as_ref(), remote_meta.as_ref());
            result.pushed.push(rel_path.to_string());
        }
    }

    /// Pull a remote file to local.
    fn pull_from_remote(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        remote_meta: Option<FileMetadata>,
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
            result
                .errors
                .push(format!("Write local {}: {}", rel_path, e));
        } else {
            self.save_base(rel_path, &content);
            let local_meta = self.current_local_metadata(rel_path).ok();
            state.record_sync(rel_path, &hash, local_meta.as_ref(), remote_meta.as_ref());
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
    /// Resolve a conflict between two versions of a non-text file.
    ///
    /// Both versions are always preserved. The winner is chosen by content
    /// hash order rather than by which machine synced first, so two machines
    /// resolving the same conflict independently arrive at the same result
    /// instead of ping-ponging. The loser is kept alongside it under a name
    /// derived from its own hash, which is likewise identical on both sides.
    #[allow(clippy::too_many_arguments)]
    fn resolve_binary_conflict(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        local_content: &[u8],
        remote_content: &[u8],
        remote_meta: Option<FileMetadata>,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let local_hash = compute_hash(local_content);
        let remote_hash = compute_hash(remote_content);

        let (winner, winner_hash, loser, loser_hash) = if local_hash <= remote_hash {
            (local_content, local_hash.clone(), remote_content, remote_hash.clone())
        } else {
            (remote_content, remote_hash.clone(), local_content, local_hash.clone())
        };

        let loser_path = make_content_conflict_path(rel_path, &loser_hash);
        let local_loser_path = self.local_root.join(&loser_path);
        if let Some(parent) = local_loser_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = crate::fsutil::atomic_write(&local_loser_path, loser) {
            result
                .errors
                .push(format!("Write conflict copy {}: {}", loser_path, e));
            // Without a preserved copy, overwriting the primary would lose
            // that version for good, so stop here.
            return;
        }
        if let Err(e) = backend.write_file(&loser_path, loser) {
            result
                .errors
                .push(format!("Push conflict copy {}: {}", loser_path, e));
            return;
        }

        let local_path = self.local_root.join(rel_path);
        if winner_hash != local_hash {
            if let Err(e) = crate::fsutil::atomic_write(&local_path, winner) {
                result
                    .errors
                    .push(format!("Write winning {}: {}", rel_path, e));
                return;
            }
        }
        if winner_hash != remote_hash {
            if let Err(e) = backend.write_file(rel_path, winner) {
                result
                    .errors
                    .push(format!("Push winning {}: {}", rel_path, e));
                return;
            }
        }

        self.save_base(rel_path, winner);

        let local_meta = Self::metadata_for_path(&local_path, &self.local_root).ok();
        state.record_sync(
            rel_path,
            &winner_hash,
            local_meta.as_ref(),
            remote_meta.as_ref(),
        );
        result.conflicts.push(rel_path.to_string());
    }

    fn handle_conflict(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        remote_meta: Option<FileMetadata>,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        // Read both versions
        let local_path = self.local_root.join(rel_path);
        let local_content = match fs::read(&local_path) {
            Ok(c) => c,
            Err(e) => {
                result
                    .errors
                    .push(format!("Read local for conflict {}: {}", rel_path, e));
                return;
            }
        };
        let remote_content = match backend.read_file(rel_path) {
            Ok(c) => c,
            Err(e) => {
                result
                    .errors
                    .push(format!("Read remote for conflict {}: {}", rel_path, e));
                return;
            }
        };

        // A line-based merge only makes sense for text. Running it over a PNG
        // or an MP3 would splice the two files together and destroy both.
        if !is_mergeable(rel_path, &local_content, &remote_content) {
            self.resolve_binary_conflict(
                backend,
                rel_path,
                &local_content,
                &remote_content,
                remote_meta,
                state,
                result,
            );
            return;
        }

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
            result
                .errors
                .push(format!("Write conflict backup {}: {}", conflict_path, e));
        }
        // Push conflict backup to remote too so both devices see it
        if let Err(e) = backend.write_file(&conflict_path, &remote_content) {
            result
                .errors
                .push(format!("Push conflict backup {}: {}", conflict_path, e));
        }

        // Write the merged content to local and remote
        let merged_bytes = merge_result.content.as_bytes();
        let merged_hash = compute_hash(merged_bytes);

        if let Err(e) = fs::write(&local_path, merged_bytes) {
            result
                .errors
                .push(format!("Write merged local {}: {}", rel_path, e));
            return;
        }
        if let Err(e) = backend.write_file(rel_path, merged_bytes) {
            result
                .errors
                .push(format!("Push merged {}: {}", rel_path, e));
        }

        self.save_base(rel_path, merged_bytes);
        let local_meta = self.current_local_metadata(rel_path).ok();
        let fresh_remote_meta = backend.stat_file(rel_path).ok().or(remote_meta);
        state.record_sync(
            rel_path,
            &merged_hash,
            local_meta.as_ref(),
            fresh_remote_meta.as_ref(),
        );

        if merge_result.has_conflicts {
            result.conflicts.push(rel_path.to_string());
        } else {
            result.merged.push(rel_path.to_string());
        }
    }

    /// Collect all local .md files under pages/ and journals/ with metadata.
    fn collect_local_files(&self) -> Result<HashMap<String, FileMetadata>> {
        let mut files = HashMap::new();
        let root = self.local_root.clone();
        self.collect_dir_metadata(&root.join("pages"), &root, true, &mut files)?;
        self.collect_dir_metadata(&root.join("journals"), &root, true, &mut files)?;
        self.collect_dir_metadata(&root.join("assets"), &root, false, &mut files)?;
        Ok(files)
    }

    fn collect_dir_metadata(
        &self,
        dir: &Path,
        base: &Path,
        markdown_only: bool,
        out: &mut HashMap<String, FileMetadata>,
    ) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                self.collect_dir_metadata(&path, base, markdown_only, out)?;
            } else if Self::is_collectable(&path, markdown_only) {
                let meta = Self::metadata_for_path(&path, base)?;
                out.insert(meta.rel_path.clone(), meta);
            }
        }
        Ok(())
    }

    /// Note folders sync only `.md`; `assets/` syncs arbitrary media. Conflict
    /// copies and atomic-write scratch files are never collected.
    fn is_collectable(path: &Path, markdown_only: bool) -> bool {
        if path.to_string_lossy().contains(".conflict_") {
            return false;
        }
        if markdown_only {
            return path.extension().and_then(|e| e.to_str()) == Some("md");
        }
        !path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.') && n.ends_with(".tmp"))
    }

    fn metadata_for_path(path: &Path, base: &Path) -> Result<FileMetadata> {
        let meta = fs::metadata(path)?;
        let modified_at = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let rel_path = path
            .strip_prefix(base)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        Ok(FileMetadata {
            rel_path,
            size: meta.len(),
            modified_at,
            hash: None,
        })
    }

    fn current_local_metadata(&self, rel_path: &str) -> Result<FileMetadata> {
        Self::metadata_for_path(&self.local_root.join(rel_path), &self.local_root)
    }

    fn ensure_local_hash(
        &self,
        rel_path: &str,
        local_files: &mut HashMap<String, FileMetadata>,
    ) -> Result<String> {
        if let Some(hash) = local_files.get(rel_path).and_then(|meta| meta.hash.clone()) {
            return Ok(hash);
        }

        let content = fs::read(self.local_root.join(rel_path))?;
        let hash = compute_hash(&content);
        if let Some(meta) = local_files.get_mut(rel_path) {
            meta.hash = Some(hash.clone());
        }
        Ok(hash)
    }

    fn ensure_remote_hash(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        remote_files: &mut HashMap<String, FileMetadata>,
    ) -> Result<String> {
        if let Some(hash) = remote_files
            .get(rel_path)
            .and_then(|meta| meta.hash.clone())
        {
            return Ok(hash);
        }

        let hash = backend.file_hash(rel_path)?;
        if let Some(meta) = remote_files.get_mut(rel_path) {
            meta.hash = Some(hash.clone());
        }
        Ok(hash)
    }
}

/// Generate a conflict filename by inserting .conflict before the extension.
/// e.g. "pages/foo.md" -> "pages/foo.conflict.md"
/// True when a line-based merge is meaningful for this file: it must be a
/// note, and both sides must actually be text.
fn is_mergeable(rel_path: &str, local: &[u8], remote: &[u8]) -> bool {
    rel_path.ends_with(".md")
        && std::str::from_utf8(local).is_ok()
        && std::str::from_utf8(remote).is_ok()
}

/// Conflict copy name derived from content rather than wall-clock time, so
/// both machines produce the same filename and converge instead of each
/// creating its own timestamped duplicate.
fn make_content_conflict_path(rel_path: &str, hash: &str) -> String {
    let short = &hash[..hash.len().min(8)];
    match rel_path.rfind('.') {
        Some(dot) => format!(
            "{}.conflict_{}{}",
            &rel_path[..dot],
            short,
            &rel_path[dot..]
        ),
        None => format!("{}.conflict_{}", rel_path, short),
    }
}

fn make_conflict_path(rel_path: &str) -> String {
    if let Some(dot_idx) = rel_path.rfind('.') {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        format!(
            "{}.conflict_{}{}",
            &rel_path[..dot_idx],
            timestamp,
            &rel_path[dot_idx..]
        )
    } else {
        format!("{}.conflict", rel_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[derive(Clone, Default)]
    struct MockBackend {
        files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        metadata: Arc<Mutex<HashMap<String, FileMetadata>>>,
        file_hash_calls: Arc<AtomicUsize>,
        read_file_calls: Arc<AtomicUsize>,
    }

    impl MockBackend {
        fn with_files(files: Vec<(&str, &[u8], i64)>) -> Self {
            let backend = Self::default();
            for (rel_path, content, modified_at) in files {
                backend.set_file(rel_path, content, modified_at);
            }
            backend
        }

        fn set_file(&self, rel_path: &str, content: &[u8], modified_at: i64) {
            self.files
                .lock()
                .unwrap()
                .insert(rel_path.to_string(), content.to_vec());
            self.metadata.lock().unwrap().insert(
                rel_path.to_string(),
                FileMetadata {
                    rel_path: rel_path.to_string(),
                    size: content.len() as u64,
                    modified_at,
                    hash: None,
                },
            );
        }

        fn reset_counters(&self) {
            self.file_hash_calls.store(0, Ordering::SeqCst);
            self.read_file_calls.store(0, Ordering::SeqCst);
        }

        fn file_hash_calls(&self) -> usize {
            self.file_hash_calls.load(Ordering::SeqCst)
        }

        fn read_file_calls(&self) -> usize {
            self.read_file_calls.load(Ordering::SeqCst)
        }

        fn file_bytes(&self, rel_path: &str) -> Vec<u8> {
            self.files
                .lock()
                .unwrap()
                .get(rel_path)
                .cloned()
                .unwrap_or_default()
        }
    }

    impl SyncBackend for MockBackend {
        fn name(&self) -> &str {
            "mock"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn list_files(&self) -> Result<Vec<FileMetadata>> {
            Ok(self.metadata.lock().unwrap().values().cloned().collect())
        }

        fn read_file(&self, rel_path: &str) -> Result<Vec<u8>> {
            // The counters measure note-content transfers. The sync-id marker
            // is a small fixed metadata read on every sync, so exclude it.
            if rel_path != SyncEngine::REMOTE_MARKER_PATH {
                self.read_file_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(self
                .files
                .lock()
                .unwrap()
                .get(rel_path)
                .cloned()
                .unwrap_or_default())
        }

        fn write_file(&self, rel_path: &str, content: &[u8]) -> Result<()> {
            let modified_at = chrono::Utc::now().timestamp();
            self.set_file(rel_path, content, modified_at);
            Ok(())
        }

        fn delete_file(&self, rel_path: &str) -> Result<()> {
            self.files.lock().unwrap().remove(rel_path);
            self.metadata.lock().unwrap().remove(rel_path);
            Ok(())
        }

        fn file_hash(&self, rel_path: &str) -> Result<String> {
            self.file_hash_calls.fetch_add(1, Ordering::SeqCst);
            Ok(compute_hash(&self.file_bytes(rel_path)))
        }
    }

    fn write_local_markdown(root: &Path, rel_path: &str, content: &str) -> Result<()> {
        let path = root.join(rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn sync_uses_cached_hashes_when_metadata_unchanged() -> Result<()> {
        let temp = tempdir()?;
        let rel_path = "pages/foo.md";
        write_local_markdown(temp.path(), rel_path, "same content")?;

        let backend = MockBackend::with_files(vec![(rel_path, b"same content", 42)]);
        let engine = SyncEngine::new(temp.path().to_path_buf());

        let first = engine.sync(&backend)?;
        assert!(first.is_clean());

        backend.reset_counters();

        let local_path = temp.path().join(rel_path);
        let original_permissions = fs::metadata(&local_path)?.permissions();
        let mut no_read_permissions = original_permissions.clone();
        no_read_permissions.set_mode(0o000);
        fs::set_permissions(&local_path, no_read_permissions)?;

        let second = engine.sync(&backend)?;

        fs::set_permissions(&local_path, original_permissions)?;

        assert!(second.is_clean());
        assert_eq!(backend.file_hash_calls(), 0);
        assert_eq!(backend.read_file_calls(), 0);
        Ok(())
    }

    #[test]
    fn sync_detects_real_local_change_and_pushes_without_remote_hash() -> Result<()> {
        let temp = tempdir()?;
        let rel_path = "pages/foo.md";
        write_local_markdown(temp.path(), rel_path, "base content")?;

        let backend = MockBackend::with_files(vec![(rel_path, b"base content", 42)]);
        let engine = SyncEngine::new(temp.path().to_path_buf());

        let first = engine.sync(&backend)?;
        assert!(first.is_clean());

        backend.reset_counters();
        write_local_markdown(temp.path(), rel_path, "base content updated")?;

        let second = engine.sync(&backend)?;

        assert_eq!(second.pushed, vec![rel_path.to_string()]);
        assert_eq!(backend.file_hash_calls(), 0);
        assert_eq!(backend.read_file_calls(), 0);
        assert_eq!(backend.file_bytes(rel_path), b"base content updated");

        backend.reset_counters();
        let third = engine.sync(&backend)?;
        assert!(third.is_clean());
        assert_eq!(backend.file_hash_calls(), 0);
        assert_eq!(backend.read_file_calls(), 0);
        Ok(())
    }
}
