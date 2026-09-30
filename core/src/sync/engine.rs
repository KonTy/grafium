use super::backend::{compute_hash, revision_changed, FileMetadata, FileSnapshot, SyncBackend};
use super::state::{SyncState, UnresolvedConflict};
use crate::error::Result;
use crate::fsutil::SourceMutationFence;
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

// Serialize state-file transactions, not graph editing. Never use this mutex
// in Graph mutators; network I/O must not hold the source-publication lock.
static SYNC_OPERATIONS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConflictState {
    pub graph_path: String,
    pub rel_path: String,
    pub local_hash: Option<String>,
    pub remote_hash: Option<String>,
    pub local_size: Option<u64>,
    pub remote_size: Option<u64>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictSide {
    Local,
    Remote,
}

/// Result of a sync operation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncResult {
    /// Files pushed from local to remote
    pub pushed: Vec<String>,
    /// Files pulled from remote to local
    pub pulled: Vec<String>,
    /// Files where conflicts were detected (both sides changed)
    pub conflicts: Vec<String>,
    /// Files merged without discarding either side's annotation history.
    pub merged: Vec<String>,
    /// Annotation sidecars whose concurrent note revisions need a user merge.
    pub annotation_conflicts: Vec<String>,
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
            annotation_conflicts: Vec::new(),
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

    pub fn is_clean(&self) -> bool {
        self.pushed.is_empty()
            && self.pulled.is_empty()
            && self.conflicts.is_empty()
            && self.merged.is_empty()
            && self.annotation_conflicts.is_empty()
            && self.deleted_remote.is_empty()
            && self.deleted_local.is_empty()
            && self.errors.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "↑{} ↓{} 🔀{} ⚡{} notes:{} 🗑{}+{} ❌{}",
            self.pushed.len(),
            self.pulled.len(),
            self.merged.len(),
            self.conflicts.len(),
            self.annotation_conflicts.len(),
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

    /// Construct an engine isolated to one configured sync target.
    pub fn new_with_target(local_root: PathBuf, target_key: impl AsRef<str>) -> Self {
        Self::new_with_metadata_dir_and_target(
            local_root,
            crate::graph::DEFAULT_METADATA_DIR_NAME,
            target_key,
        )
    }

    pub fn new_with_target_id(local_root: PathBuf, target_id: impl AsRef<str>) -> Self {
        Self::new_with_target(local_root, target_id)
    }

    pub fn new_with_metadata_dir_and_target(
        local_root: PathBuf,
        metadata_dir_name: &str,
        target_key: impl AsRef<str>,
    ) -> Self {
        let target = compute_hash(target_key.as_ref().as_bytes());
        let root = local_root
            .join(metadata_dir_name)
            .join("sync-targets")
            .join(target);
        Self {
            local_root,
            state_path: root.join("sync-state.json"),
            bases_dir: root.join("sync-bases"),
        }
    }

    /// Return conflicts still waiting for an explicit side selection.
    pub fn unresolved_conflicts(&self) -> Vec<UnresolvedConflict> {
        SyncState::load(&self.state_path)
            .list_unresolved_conflicts()
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn last_sync(&self) -> Option<i64> {
        SyncState::load(&self.state_path).last_sync
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

    /// Path of the marker file written at the root of every sync target. It
    /// carries a random id that identifies this particular remote, so we can
    /// tell "the drive is empty / not mounted / not the one we synced with"
    /// apart from "these files were genuinely deleted on the other machine".
    const REMOTE_MARKER_PATH: &'static str = ".grafium-sync-id";

    /// Graph directories that participate in sync. `knowledge/` carries
    /// portable AI/rule/prompt knowledge, and `assets/` carries the media that
    /// notes reference, so omitting either leaves the other machine incomplete.
    const SYNCED_DIRS: [&'static str; 5] =
        ["pages/", "journals/", "knowledge/", "assets/", "books/"];

    /// Only files under the graph's note directories participate in sync. This
    /// also keeps the marker file out of the synced set.
    ///
    /// Paths originate from a remote listing, so a hostile or buggy server can
    /// propose anything here. Reject traversal and absolute components: without
    /// this, an href resolving to `pages/../../../etc/passwd` would be joined
    /// onto the graph root and written outside it.
    pub(super) fn is_syncable_path(rel_path: &str) -> bool {
        if rel_path.contains(".conflict_") {
            return false;
        }
        if !Self::SYNCED_DIRS
            .iter()
            .any(|dir| rel_path.starts_with(dir))
        {
            return false;
        }
        if Path::new(rel_path).is_absolute() {
            return false;
        }
        if rel_path.starts_with("books/") && !crate::graph::books::is_portable_book_file(rel_path) {
            return false;
        }
        if rel_path.starts_with("pages/")
            && !rel_path.ends_with(".md")
            && !rel_path.contains("/assets/")
        {
            return false;
        }
        if rel_path.starts_with("journals/") && !rel_path.ends_with(".md") {
            return false;
        }
        rel_path
            .split(['/', '\\'])
            .all(|component| !component.is_empty() && component != "." && component != "..")
    }

    fn read_remote_marker(backend: &dyn SyncBackend) -> Result<Option<String>> {
        let Some(bytes) = backend.read_snapshot(Self::REMOTE_MARKER_PATH)?.content else {
            return Ok(None);
        };
        let id = String::from_utf8(bytes)
            .map_err(|e| crate::error::CoreError::Other(format!("Invalid sync marker: {e}")))?
            .trim()
            .to_string();
        if id.is_empty() {
            Err(crate::error::CoreError::Other("Empty sync marker".into()))
        } else {
            Ok(Some(id))
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
        let marker = Self::read_remote_marker(backend)?;
        let never_synced = state.files.is_empty() && state.remote_id.is_none();

        match (marker, never_synced) {
            // First sync against an already-initialised remote: adopt its id.
            (Some(id), true) => {
                state.remote_id = Some(id);
            }
            // First sync against a fresh remote: claim it.
            (None, true) => {
                let id = uuid::Uuid::new_v4().to_string();
                backend.publish_if_unchanged(
                    Self::REMOTE_MARKER_PATH,
                    &FileSnapshot {
                        content: None,
                        etag: None,
                        mutation_fence: None,
                    },
                    Some(id.as_bytes()),
                )?;
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
                    backend.publish_if_unchanged(
                        Self::REMOTE_MARKER_PATH,
                        &FileSnapshot {
                            content: None,
                            etag: None,
                            mutation_fence: None,
                        },
                        Some(id.as_bytes()),
                    )?;
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
    pub fn sync(&self, backend: &dyn SyncBackend) -> Result<SyncResult> {
        let _sync = SYNC_OPERATIONS
            .lock()
            .map_err(|e| crate::error::CoreError::Other(e.to_string()))?;
        // Pin generations before any remote request; files discovered during
        // that request still belong to this initial local snapshot.
        let epoch = crate::fsutil::graph_mutation_epoch(&self.local_root)?;
        if !backend.is_available() {
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
            .chain(state.unresolved_conflicts.keys())
            .cloned()
            .collect();

        for rel_path in all_paths {
            if !Self::is_syncable_path(&rel_path) {
                continue;
            }
            let fence = epoch.for_path(Path::new(&rel_path))?;
            if let Err(e) = super::filesystem::safe_sync_path(&self.local_root, &rel_path) {
                result
                    .errors
                    .push(format!("Unsafe sync path {rel_path}: {e}"));
                continue;
            }
            // A removable drive can be pulled out part way through. Once a
            // file operation has failed, confirm the target is still there
            // before working through the rest of the graph against it.
            if !result.errors.is_empty() && !backend.is_available() {
                state.last_sync = Some(Utc::now().timestamp());
                let _ = state.save(&self.state_path);
                return Err(crate::error::CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    format!(
                        "Sync target '{}' disappeared part way through. {} file(s)                          were synced before it went away.",
                        backend.name(),
                        result.pushed.len() + result.pulled.len()
                    ),
                )));
            }

            let local_exists = local_files.contains_key(&rel_path);
            let remote_exists = remote_files.contains_key(&rel_path);
            let was_synced = state.files.contains_key(&rel_path);

            if crate::graph::books::is_annotation_sidecar(&rel_path) {
                if local_exists && remote_exists {
                    self.sync_annotation_sidecar(
                        backend,
                        &rel_path,
                        &fence,
                        &mut state,
                        &mut result,
                    );
                    continue;
                }
                // Deleting an entire sidecar is not a per-note tombstone. Keep
                // its surviving history until the user explicitly chooses.
                if was_synced && local_exists != remote_exists {
                    self.handle_conflict(backend, &rel_path, None, &mut state, &mut result);
                    continue;
                }
            }

            if state.unresolved_conflict(&rel_path).is_some()
                && self.handle_pending_resolution(
                    backend,
                    &rel_path,
                    &mut local_files,
                    &mut state,
                    &mut result,
                )
            {
                continue;
            }

            match (local_exists, remote_exists, was_synced) {
                // Both exist — check for changes
                (true, true, true) => {
                    self.sync_both_exist(
                        backend,
                        &rel_path,
                        &fence,
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
                    self.propagate_deletion(
                        backend,
                        &rel_path,
                        &fence,
                        ConflictSide::Remote,
                        &mut state,
                        &mut result,
                    );
                }
                // Only local exists, never synced — new local file, push
                (true, false, false) => {
                    self.push_to_remote(
                        backend,
                        &rel_path,
                        &fence,
                        &mut local_files,
                        &mut state,
                        &mut result,
                    );
                }
                // Only remote exists, was previously synced — local was deleted
                (false, true, true) => {
                    self.propagate_deletion(
                        backend,
                        &rel_path,
                        &fence,
                        ConflictSide::Local,
                        &mut state,
                        &mut result,
                    );
                }
                // Only remote exists, never synced — new remote file, pull
                (false, true, false) => {
                    self.pull_from_remote(
                        backend,
                        &rel_path,
                        &fence,
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

    fn sync_annotation_sidecar(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        fence: &SourceMutationFence,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let operation = (|| -> Result<()> {
            let local = {
                let _source = fence.lock();
                self.local_snapshot(rel_path)?
            };
            let remote = backend.read_snapshot(rel_path)?;
            let (Some(local_bytes), Some(remote_bytes)) =
                (local.content.as_deref(), remote.content.as_deref())
            else {
                let _source = fence.lock();
                return self.record_conflict(
                    rel_path,
                    &self.local_snapshot(rel_path)?,
                    &remote,
                    state,
                    result,
                );
            };
            let merged = (|| -> Result<Vec<u8>> {
                crate::graph::books::validate_annotation_sidecar(rel_path, local_bytes)?;
                crate::graph::books::validate_annotation_sidecar(rel_path, remote_bytes)?;
                crate::graph::books::merge_annotation_sidecars(local_bytes, remote_bytes)
            })();
            let merged = match merged {
                Ok(merged) => merged,
                Err(error) => {
                    let _source = fence.lock();
                    self.record_conflict(
                        rel_path,
                        &self.local_snapshot(rel_path)?,
                        &remote,
                        state,
                        result,
                    )?;
                    return Err(error);
                }
            };
            let conflict_count = crate::graph::books::annotation_conflict_count(&merged)?;
            {
                let _source = fence.lock();
                if !fence.is_current() || self.local_snapshot(rel_path)?.hash() != local.hash() {
                    self.record_conflict(
                        rel_path,
                        &self.local_snapshot(rel_path)?,
                        &remote,
                        state,
                        result,
                    )?;
                    return Err(revision_changed(rel_path));
                }
                if local_bytes != merged || remote_bytes != merged {
                    self.preserve_copy(rel_path, &local)?;
                    self.preserve_copy(rel_path, &remote)?;
                }
            }
            if remote_bytes != merged {
                if let Err(error) = backend.publish_if_unchanged(rel_path, &remote, Some(&merged)) {
                    self.reopen_after_remote_change(backend, rel_path, &remote, state, result)?;
                    return Err(error);
                }
            }
            let _source = fence.lock();
            if !fence.is_current() || self.local_snapshot(rel_path)?.hash() != local.hash() {
                self.record_conflict(
                    rel_path,
                    &self.local_snapshot(rel_path)?,
                    &FileSnapshot {
                        content: Some(merged),
                        ..FileSnapshot::missing()
                    },
                    state,
                    result,
                )?;
                return Err(revision_changed(rel_path));
            }
            if local_bytes != merged {
                let path = super::filesystem::safe_sync_path(&self.local_root, rel_path)?;
                crate::fsutil::atomic_write(&path, &merged)?;
            }
            let local_meta = self.current_local_metadata(rel_path)?;
            self.save_base(rel_path, &merged);
            state.record_sync(rel_path, &compute_hash(&merged), Some(&local_meta), None);
            state.resolve_unresolved_conflict(rel_path);
            if local_bytes != merged || remote_bytes != merged {
                result.merged.push(rel_path.into());
            }
            if conflict_count > 0 {
                result.annotation_conflicts.push(rel_path.into());
            }
            Ok(())
        })();
        if let Err(error) = operation {
            result
                .errors
                .push(format!("Merge annotations {rel_path}: {error}"));
        }
    }

    /// Both local and remote exist and were previously synced — check for changes.
    fn sync_both_exist(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        fence: &SourceMutationFence,
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
                self.push_to_remote(backend, rel_path, fence, local_files, state, result);
            }
            (false, true) => {
                self.pull_from_remote(
                    backend,
                    rel_path,
                    fence,
                    Some(remote_meta.clone()),
                    state,
                    result,
                );
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

    /// Editing a file is not consent to overwrite a different remote version.
    /// Conflicts, including deletions and binary files, require an explicit
    /// revision-checked choice.
    fn handle_pending_resolution(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        _local_files: &mut HashMap<String, FileMetadata>,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) -> bool {
        let Some(conflict) = state.unresolved_conflict(rel_path).cloned() else {
            return false;
        };
        let refresh = (|| -> Result<()> {
            let remote = backend.read_snapshot(rel_path)?;
            let source_lock = crate::fsutil::graph_operation_lock(&self.local_root)?;
            let _source = source_lock.lock();
            let local = self.local_snapshot(rel_path)?;
            if local.hash() != conflict.local_hash || remote.hash() != conflict.remote_hash {
                self.record_conflict(rel_path, &local, &remote, state, result)?;
            }
            Ok(())
        })();
        if let Err(e) = refresh {
            result
                .errors
                .push(format!("Refresh conflict {rel_path}: {e}"));
        }
        true
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
        fence: &SourceMutationFence,
        local_files: &mut HashMap<String, FileMetadata>,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let operation = (|| -> Result<()> {
            let local = {
                let _source = fence.lock();
                self.local_snapshot(rel_path)?
            };
            let Some(content) = local.content.as_deref() else {
                return Err(revision_changed(rel_path));
            };
            let annotation_conflicts = if crate::graph::books::is_annotation_sidecar(rel_path) {
                crate::graph::books::validate_annotation_sidecar(rel_path, content)?;
                crate::graph::books::annotation_conflict_count(content)?
            } else {
                0
            };
            let remote = backend.read_snapshot(rel_path)?;
            let baseline = state.files.get(rel_path).map(|r| r.hash_at_sync.clone());
            if remote.hash() != baseline && remote.hash() != local.hash() {
                let source_lock = crate::fsutil::graph_operation_lock(&self.local_root)?;
                let _source = source_lock.lock();
                return self.record_conflict(
                    rel_path,
                    &self.local_snapshot(rel_path)?,
                    &remote,
                    state,
                    result,
                );
            }
            {
                let _source = fence.lock();
                if !fence.is_current() || self.local_snapshot(rel_path)?.hash() != local.hash() {
                    self.record_conflict(
                        rel_path,
                        &self.local_snapshot(rel_path)?,
                        &remote,
                        state,
                        result,
                    )?;
                    return Err(revision_changed(rel_path));
                }
                self.preserve_copy(rel_path, &remote)?;
            }
            if let Err(e) = backend.publish_if_unchanged(rel_path, &remote, Some(content)) {
                self.reopen_after_remote_change(backend, rel_path, &remote, state, result)?;
                return Err(e);
            }
            result.pushed.push(rel_path.to_string());
            let _source = fence.lock();
            if !fence.is_current() || self.local_snapshot(rel_path)?.hash() != local.hash() {
                self.record_conflict(
                    rel_path,
                    &self.local_snapshot(rel_path)?,
                    &local,
                    state,
                    result,
                )?;
                return Err(revision_changed(rel_path));
            }
            self.save_base(rel_path, content);
            // Metadata is from the captured local revision, never from a newer
            // edit that arrived while uploading.
            let local_meta = local_files.get(rel_path).cloned();
            state.record_sync(rel_path, &compute_hash(content), local_meta.as_ref(), None);
            if annotation_conflicts > 0 {
                result.annotation_conflicts.push(rel_path.into());
            }
            Ok(())
        })();
        if let Err(e) = operation {
            result.errors.push(format!("Push {rel_path}: {e}"));
        }
    }

    /// Pull a remote file to local.
    fn pull_from_remote(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        fence: &SourceMutationFence,
        _remote_meta: Option<FileMetadata>,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let operation = (|| -> Result<()> {
            let before = {
                let _source = fence.lock();
                self.local_snapshot(rel_path)?
            };
            let baseline = state.files.get(rel_path).map(|r| r.hash_at_sync.clone());
            let remote = backend.read_snapshot(rel_path)?;
            let _source = fence.lock();
            let current = self.local_snapshot(rel_path)?;
            if !fence.is_current()
                || before.hash() != baseline
                || current.hash() != before.hash()
                || remote.content.is_none()
            {
                self.record_conflict(rel_path, &current, &remote, state, result)?;
                return Ok(());
            }
            self.preserve_copy(rel_path, &current)?;
            let content = remote.content.as_deref().unwrap();
            let annotation_conflicts = if crate::graph::books::is_annotation_sidecar(rel_path) {
                crate::graph::books::validate_annotation_sidecar(rel_path, content)?;
                crate::graph::books::annotation_conflict_count(content)?
            } else {
                0
            };
            let local_path = super::filesystem::safe_sync_path(&self.local_root, rel_path)?;
            crate::fsutil::atomic_write(&local_path, content)?;
            self.save_base(rel_path, content);
            let local_meta = self.current_local_metadata(rel_path)?;
            state.record_sync(rel_path, &compute_hash(content), Some(&local_meta), None);
            result.pulled.push(rel_path.to_string());
            if annotation_conflicts > 0 {
                result.annotation_conflicts.push(rel_path.into());
            }
            Ok(())
        })();
        if let Err(e) = operation {
            result.errors.push(format!("Pull {rel_path}: {e}"));
        }
    }

    fn handle_conflict(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        _remote_meta: Option<FileMetadata>,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let operation = (|| -> Result<()> {
            let remote = backend.read_snapshot(rel_path)?;
            let source_lock = crate::fsutil::graph_operation_lock(&self.local_root)?;
            let _source = source_lock.lock();
            self.record_conflict(
                rel_path,
                &self.local_snapshot(rel_path)?,
                &remote,
                state,
                result,
            )
        })();
        if let Err(e) = operation {
            result
                .errors
                .push(format!("Preserve conflict {rel_path}: {e}"));
        }
    }

    /// Caller holds the graph source lock while reading/publishing locally.
    fn local_snapshot(&self, rel_path: &str) -> Result<FileSnapshot> {
        let path = super::filesystem::safe_sync_path(&self.local_root, rel_path)?;
        match fs::read(path) {
            Ok(content) => Ok(FileSnapshot {
                content: Some(content),
                etag: None,
                mutation_fence: None,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FileSnapshot {
                content: None,
                etag: None,
                mutation_fence: None,
            }),
            Err(e) => Err(e.into()),
        }
    }

    fn preserve_copy(&self, rel_path: &str, snapshot: &FileSnapshot) -> Result<Option<String>> {
        let Some(content) = snapshot.content.as_deref() else {
            return Ok(None);
        };
        // Recovery is deliberately outside pages/assets/books. A reader or
        // index rebuild must never mistake a preserved version for a source.
        let recovery = self
            .state_path
            .parent()
            .unwrap()
            .join("sync-recovery")
            .join(make_conflict_path(rel_path, &compute_hash(content)));
        let backup = recovery
            .strip_prefix(&self.local_root)
            .map_err(|e| crate::error::CoreError::Other(e.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let path = super::filesystem::safe_sync_path(&self.local_root, &backup)?;
        if path.exists() {
            if fs::read(&path)? != content {
                return Err(crate::error::CoreError::Other(format!(
                    "Recovery copy changed: {backup}"
                )));
            }
        } else {
            crate::fsutil::atomic_write(&path, content)?;
        }
        Ok(Some(backup))
    }

    fn record_conflict(
        &self,
        rel_path: &str,
        local: &FileSnapshot,
        remote: &FileSnapshot,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) -> Result<()> {
        let local_backup = self.preserve_copy(rel_path, local)?;
        let remote_backup = self.preserve_copy(rel_path, remote)?;
        state.record_unresolved_conflict(
            rel_path,
            local.hash(),
            remote.hash(),
            &remote_backup.or(local_backup).unwrap_or_default(),
        );
        if !result.conflicts.iter().any(|path| path == rel_path) {
            result.conflicts.push(rel_path.into());
        }
        Ok(())
    }

    fn reopen_after_remote_change(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        original: &FileSnapshot,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) -> Result<()> {
        let remote = backend.read_snapshot(rel_path)?;
        let source_lock = crate::fsutil::graph_operation_lock(&self.local_root)?;
        let _source = source_lock.lock();
        self.preserve_copy(rel_path, original)?;
        self.record_conflict(
            rel_path,
            &self.local_snapshot(rel_path)?,
            &remote,
            state,
            result,
        )
    }

    fn propagate_deletion(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        fence: &SourceMutationFence,
        deleted_side: ConflictSide,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) {
        let operation = (|| -> Result<()> {
            let baseline = state.files.get(rel_path).map(|r| r.hash_at_sync.clone());
            let remote = backend.read_snapshot(rel_path)?;
            {
                let _source = fence.lock();
                let local = self.local_snapshot(rel_path)?;
                let unchanged = match deleted_side {
                    ConflictSide::Local => local.content.is_none() && remote.hash() == baseline,
                    ConflictSide::Remote => remote.content.is_none() && local.hash() == baseline,
                };
                if !fence.is_current() || !unchanged {
                    return self.record_conflict(rel_path, &local, &remote, state, result);
                }
                self.preserve_copy(rel_path, &local)?;
                self.preserve_copy(rel_path, &remote)?;
                if matches!(deleted_side, ConflictSide::Remote) {
                    let path = super::filesystem::safe_sync_path(&self.local_root, rel_path)?;
                    crate::fsutil::record_source_mutation(&self.local_root, Path::new(rel_path))?;
                    fs::remove_file(path)?;
                    state.remove_record(rel_path);
                    result.deleted_local.push(rel_path.into());
                    return Ok(());
                }
            }
            if let Err(e) = backend.publish_if_unchanged(rel_path, &remote, None) {
                self.reopen_after_remote_change(backend, rel_path, &remote, state, result)?;
                return Err(e);
            }
            result.deleted_remote.push(rel_path.into());
            let _source = fence.lock();
            let local = self.local_snapshot(rel_path)?;
            if !fence.is_current() || local.content.is_some() {
                self.record_conflict(rel_path, &local, &FileSnapshot::missing(), state, result)?;
                return Err(revision_changed(rel_path));
            }
            state.remove_record(rel_path);
            Ok(())
        })();
        if let Err(e) = operation {
            result
                .errors
                .push(format!("Propagate deletion {rel_path}: {e}"));
        }
    }

    /// Refresh the explicit choice dialog without changing either primary file.
    pub fn conflict_state(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
    ) -> Result<ConflictState> {
        let _sync = SYNC_OPERATIONS
            .lock()
            .map_err(|e| crate::error::CoreError::Other(e.to_string()))?;
        let mut state = SyncState::load(&self.state_path);
        self.validate_conflict_path(rel_path, &state)?;
        self.verify_remote_identity(backend, &mut state, 0)?;
        let remote = backend.read_snapshot(rel_path)?;
        let source_lock = crate::fsutil::graph_operation_lock(&self.local_root)?;
        let _source = source_lock.lock();
        let local = self.local_snapshot(rel_path)?;
        self.record_conflict(
            rel_path,
            &local,
            &remote,
            &mut state,
            &mut SyncResult::new(),
        )?;
        state.save(&self.state_path)?;
        Ok(ConflictState {
            graph_path: self.local_root.to_string_lossy().into_owned(),
            rel_path: rel_path.into(),
            local_hash: local.hash(),
            remote_hash: remote.hash(),
            local_size: local.content.as_ref().map(|c| c.len() as u64),
            remote_size: remote.content.as_ref().map(|c| c.len() as u64),
        })
    }

    fn validate_conflict_path(&self, rel_path: &str, state: &SyncState) -> Result<()> {
        if !Self::is_syncable_path(rel_path) || state.unresolved_conflict(rel_path).is_none() {
            return Err(crate::error::CoreError::Other(
                "Unresolved sync conflict not found".into(),
            ));
        }
        Ok(())
    }

    /// Resolve exactly the versions presented to the user. None selects a
    /// deletion; bytes are never decoded, merged, or inspected for markers.
    pub fn resolve_conflict(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        expected_local_hash: Option<&str>,
        expected_remote_hash: Option<&str>,
        chosen: ConflictSide,
    ) -> Result<SyncResult> {
        let _sync = SYNC_OPERATIONS
            .lock()
            .map_err(|e| crate::error::CoreError::Other(e.to_string()))?;
        let epoch = crate::fsutil::graph_mutation_epoch(&self.local_root)?;
        let fence = epoch.for_path(Path::new(rel_path))?;
        let mut state = SyncState::load(&self.state_path);
        self.validate_conflict_path(rel_path, &state)?;
        let mut result = SyncResult::new();
        let operation = (|| -> Result<()> {
            // Do not allow a dialog opened against a different remote identity.
            self.verify_remote_identity(backend, &mut state, 0)?;
            let remote = backend.read_snapshot(rel_path)?;
            let local = {
                let _source = fence.lock();
                let local = self.local_snapshot(rel_path)?;
                let recorded = state.unresolved_conflict(rel_path).unwrap();
                if !fence.is_current()
                    || local.hash().as_deref() != expected_local_hash
                    || remote.hash().as_deref() != expected_remote_hash
                    || recorded.local_hash.as_deref() != expected_local_hash
                    || recorded.remote_hash.as_deref() != expected_remote_hash
                {
                    self.record_conflict(rel_path, &local, &remote, &mut state, &mut result)?;
                    return Err(revision_changed(rel_path));
                }
                self.preserve_copy(rel_path, &local)?;
                self.preserve_copy(rel_path, &remote)?;
                local
            };
            let chosen_content = match chosen {
                ConflictSide::Local => local.content.as_deref(),
                ConflictSide::Remote => remote.content.as_deref(),
            };
            if matches!(chosen, ConflictSide::Local) {
                if let Err(e) = backend.publish_if_unchanged(rel_path, &remote, chosen_content) {
                    self.reopen_after_remote_change(
                        backend,
                        rel_path,
                        &remote,
                        &mut state,
                        &mut result,
                    )?;
                    return Err(e);
                }
            }
            let _source = fence.lock();
            let current = self.local_snapshot(rel_path)?;
            if !fence.is_current() || current.hash() != local.hash() {
                // In keep-local, the remote is now the captured local snapshot.
                let opposite = if matches!(chosen, ConflictSide::Local) {
                    &local
                } else {
                    &remote
                };
                self.record_conflict(rel_path, &current, opposite, &mut state, &mut result)?;
                return Err(revision_changed(rel_path));
            }
            if matches!(chosen, ConflictSide::Remote) {
                let path = super::filesystem::safe_sync_path(&self.local_root, rel_path)?;
                match chosen_content {
                    Some(content) => {
                        crate::fsutil::atomic_write(&path, content)?;
                        result.pulled.push(rel_path.into());
                    }
                    None if local.content.is_some() => {
                        crate::fsutil::record_source_mutation(
                            &self.local_root,
                            Path::new(rel_path),
                        )?;
                        fs::remove_file(path)?;
                        result.deleted_local.push(rel_path.into());
                    }
                    None => {}
                }
            } else if chosen_content.is_some() {
                result.pushed.push(rel_path.into());
            } else if remote.content.is_some() {
                result.deleted_remote.push(rel_path.into());
            }
            if let Some(content) = chosen_content {
                let meta = self.current_local_metadata(rel_path)?;
                state.record_sync(rel_path, &compute_hash(content), Some(&meta), None);
                self.save_base(rel_path, content);
            } else {
                state.remove_record(rel_path);
            }
            state.resolve_unresolved_conflict(rel_path);
            Ok(())
        })();
        // A failed revision check still persists the reopened conflict.
        state.save(&self.state_path)?;
        operation?;
        Ok(result)
    }

    /// Collect all local graph files with metadata.
    fn collect_local_files(&self) -> Result<HashMap<String, FileMetadata>> {
        let mut files = HashMap::new();
        let root = self.local_root.clone();
        self.collect_dir_metadata(&root.join("pages"), &root, true, &mut files)?;
        self.collect_dir_metadata(&root.join("journals"), &root, true, &mut files)?;
        self.collect_dir_metadata(&root.join("knowledge"), &root, false, &mut files)?;
        self.collect_dir_metadata(&root.join("assets"), &root, false, &mut files)?;
        self.collect_dir_metadata(&root.join("books"), &root, false, &mut files)?;
        Ok(files)
    }

    fn collect_dir_metadata(
        &self,
        dir: &Path,
        base: &Path,
        markdown_only: bool,
        out: &mut HashMap<String, FileMetadata>,
    ) -> Result<()> {
        match fs::symlink_metadata(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
            Ok(meta) if meta.file_type().is_symlink() => return Ok(()),
            Ok(_) => {}
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_symlink()
                || entry.file_name().to_string_lossy().starts_with('.')
            {
                continue;
            }
            if path.is_dir() {
                self.collect_dir_metadata(&path, base, markdown_only, out)?;
            } else if Self::is_collectable(&path, base, markdown_only) {
                let meta = Self::metadata_for_path(&path, base)?;
                out.insert(meta.rel_path.clone(), meta);
            }
        }
        Ok(())
    }

    /// Note folders sync only `.md`; `knowledge/` and `assets/` sync arbitrary
    /// supporting files. Conflict copies and atomic-write scratch files are
    /// never collected.
    fn is_collectable(path: &Path, base: &Path, markdown_only: bool) -> bool {
        if path.to_string_lossy().contains(".conflict_") {
            return false;
        }
        let rel = path
            .strip_prefix(base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with("books/") {
            return crate::graph::books::is_portable_book_file(&rel.replace('\\', "/"));
        }
        if rel.starts_with("pages/") && rel.contains("/assets/") {
            return true;
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
            .map(|p| p.to_string_lossy().replace('\\', "/"))
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
/// Conflict copy name derived from content rather than wall-clock time, so
/// both machines produce the same filename and converge instead of each
/// creating its own timestamped duplicate.
fn make_content_conflict_path(rel_path: &str, hash: &str) -> String {
    let short = hash;
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

fn make_conflict_path(rel_path: &str, hash: &str) -> String {
    make_content_conflict_path(rel_path, hash)
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
            self.files
                .lock()
                .unwrap()
                .get(rel_path)
                .cloned()
                .ok_or_else(|| crate::error::CoreError::NotFound(rel_path.into()))
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

    #[test]
    fn sync_pushes_portable_knowledge_files() -> Result<()> {
        let temp = tempdir()?;
        let rel_path = "knowledge/prompts/web-research.md";
        write_local_markdown(temp.path(), rel_path, "- prompt:: cite everything\n")?;

        let backend = MockBackend::default();
        let engine = SyncEngine::new(temp.path().to_path_buf());
        let result = engine.sync(&backend)?;

        assert_eq!(result.pushed, vec![rel_path.to_string()]);
        assert_eq!(
            backend.file_bytes(rel_path),
            b"- prompt:: cite everything\n"
        );
        Ok(())
    }

    #[test]
    fn sync_pushes_and_pulls_originals_but_never_extraction_caches() -> Result<()> {
        let local = tempfile::tempdir_in(".")?;
        let id = uuid::Uuid::new_v4().to_string();
        let original = format!("books/{id}/original.epub");
        let metadata = format!("books/{id}/book.json");
        write_local_markdown(local.path(), &original, "original bytes")?;
        write_local_markdown(local.path(), &metadata, "{}")?;
        write_local_markdown(
            local.path(),
            &format!("books/{id}/.extract-cache/converted.epub"),
            "cache",
        )?;
        write_local_markdown(local.path(), &format!("books/{id}/cached.txt"), "cache")?;
        let backend = MockBackend::default();
        let result = SyncEngine::new(local.path().to_path_buf()).sync(&backend)?;
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.pushed.len(), 2);
        assert_eq!(backend.file_bytes(&original), b"original bytes");
        let other = tempfile::tempdir_in(".")?;
        let result = SyncEngine::new(other.path().to_path_buf()).sync(&backend)?;
        assert_eq!(result.pulled.len(), 2);
        assert_eq!(fs::read(other.path().join(&original))?, b"original bytes");
        assert!(!SyncEngine::is_syncable_path(&format!(
            "books/{id}/../escape"
        )));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn sync_does_not_follow_original_book_symlinks() -> Result<()> {
        let local = tempfile::tempdir_in(".")?;
        let outside = tempfile::tempdir_in(".")?;
        let id = uuid::Uuid::new_v4().to_string();
        fs::create_dir_all(local.path().join("books"))?;
        std::os::unix::fs::symlink(
            outside.path().canonicalize()?,
            local.path().join("books").join(&id),
        )?;
        let path = format!("books/{id}/original.pdf");
        let backend = MockBackend::with_files(vec![(&path, b"remote original", 42)]);
        let result = SyncEngine::new(local.path().to_path_buf()).sync(&backend)?;
        assert!(!result.errors.is_empty());
        assert!(!outside.path().join("original.pdf").exists());
        Ok(())
    }

    /// Conversations are deliberately unsyncable. They live in
    /// `.grafium/index.db`, and `collect_local_files` is an allowlist of note
    /// folders, so a chat cannot reach a USB stick, a file server, or another
    /// install. This test is the guarantee: if someone ever widens the
    /// allowlist to "everything under the graph root", it fails here rather
    /// than leaking a transcript to a shared drive.
    #[test]
    fn sync_never_collects_chat_conversations() -> Result<()> {
        let temp = tempdir()?;
        write_local_markdown(temp.path(), "pages/real-note.md", "- a real note\n")?;

        let private = temp.path().join(".grafium");
        fs::create_dir_all(private.join("nested"))?;
        fs::write(private.join("index.db"), b"sqlite chat threads live here")?;
        fs::write(private.join("index.db-wal"), b"and here")?;
        fs::write(private.join("nested/conversations.json"), b"[]")?;

        let backend = MockBackend::default();
        let engine = SyncEngine::new(temp.path().to_path_buf());
        let result = engine.sync(&backend)?;

        assert_eq!(result.pushed, vec!["pages/real-note.md".to_string()]);
        let pushed = backend.files.lock().unwrap();
        assert!(
            pushed.keys().all(|path| !path.starts_with(".grafium/")),
            "sync collected private chat state: {:?}",
            pushed.keys().collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn sync_accepts_remote_knowledge_paths() -> Result<()> {
        let temp = tempdir()?;
        let rel_path = "knowledge/rules/links.yaml";
        let backend = MockBackend::with_files(vec![(rel_path, b"rules: []\n", 42)]);
        let engine = SyncEngine::new(temp.path().to_path_buf());

        let result = engine.sync(&backend)?;

        assert_eq!(result.pulled, vec![rel_path.to_string()]);
        assert_eq!(fs::read(temp.path().join(rel_path))?, b"rules: []\n");
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
    fn sync_revalidates_remote_content_before_pushing_local_change() -> Result<()> {
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
        assert_eq!(backend.read_file_calls(), 2);
        assert_eq!(backend.file_bytes(rel_path), b"base content updated");

        backend.reset_counters();
        let third = engine.sync(&backend)?;
        assert!(third.is_clean());
        assert_eq!(backend.file_hash_calls(), 1);
        assert_eq!(backend.read_file_calls(), 0);
        Ok(())
    }
}
