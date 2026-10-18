//! Combining a note that was edited on both devices.
//!
//! The note-specific policy behind the generic sync flow in `engine.rs`: which
//! files qualify, when a note is held back because it still carries conflict
//! markers, and how a merge is applied. The text algorithm itself lives in
//! `sync::merge`.
//!
//! Two rules keep the devices from chasing each other with "fix this / fix
//! that" edits:
//!
//! * A note with live markers is never published, never overwritten and never
//!   merged again. The markers stay on the device that found the conflict.
//! * When markers are written, the remote version becomes the base. The
//!   resolved note is then an ordinary edit on top of that version: it
//!   fast-forwards if the remote has not moved and is merged once more if it
//!   has. Every round advances the base, so rounds end.

use super::{SyncEngine, SyncResult};
use crate::error::{CoreError, Result};
use crate::fsutil::SourceMutationFence;
use crate::sync::backend::{compute_hash, FileSnapshot, SyncBackend};
use crate::sync::filesystem::safe_sync_path;
use crate::sync::merge::{
    has_live_markers, scan_marker_bytes, scan_markers, three_way_merge, two_way_merge, MarkerScan,
    MAX_MERGE_BYTES,
};
use crate::sync::state::{PendingMerge, SyncState};
use std::fs;

/// Notes the outliner can open: markdown under `pages/` and `journals/`.
/// Everything else (books, assets, knowledge files, sidecars) keeps the
/// whole-file conflict flow.
pub(super) fn is_mergeable_path(rel_path: &str) -> bool {
    (rel_path.starts_with("pages/") || rel_path.starts_with("journals/"))
        && rel_path.ends_with(".md")
        && !rel_path.contains("/assets/")
}

/// Whether a synced note is worth keeping a merge base for.
pub(super) fn keeps_base(rel_path: &str, len: usize) -> bool {
    is_mergeable_path(rel_path) && len <= MAX_MERGE_BYTES
}

/// The note as text, unless it is too big to diff quickly, is not UTF-8, or
/// holds structured reading-note data that has its own merge rules.
fn mergeable_text(bytes: &[u8]) -> Option<&str> {
    if bytes.len() > MAX_MERGE_BYTES {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let structured = text.contains("grafium-reading-note") || text.contains("reading-note::");
    (!structured).then_some(text)
}

fn changed_while_syncing(rel_path: &str) -> CoreError {
    CoreError::Other(format!(
        "{rel_path} changed while syncing; it will be merged on the next sync"
    ))
}

impl SyncEngine {
    /// Notes that currently hold merge markers and wait for a person. Each
    /// recorded entry is checked against the live file, so a note that has
    /// already been cleaned up disappears before the next sync runs.
    pub fn pending_merges(&self) -> Vec<PendingMerge> {
        let state = SyncState::load(&self.state_path);
        let mut pending: Vec<PendingMerge> = state
            .list_pending_merges()
            .into_iter()
            .filter_map(|entry| {
                let scan = self.scan_note(&entry.rel_path).ok().flatten()?;
                Some(PendingMerge {
                    token: scan.token,
                    conflicts: scan.hunks,
                    ..entry.clone()
                })
            })
            .collect();
        pending.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        pending
    }

    /// Merge a note that changed on both sides. Returns false when the note
    /// does not qualify, so the caller falls back to the whole-file flow.
    pub(super) fn merge_diverged_note(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        fence: &SourceMutationFence,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) -> bool {
        if !is_mergeable_path(rel_path) {
            return false;
        }
        match self.try_merge(backend, rel_path, fence, state, result) {
            Ok(handled) => handled,
            // Nothing local was changed before an error, so the next sync
            // simply tries again.
            Err(error) => {
                result.errors.push(format!("Merge {rel_path}: {error}"));
                true
            }
        }
    }

    /// True when `content` is a note that still carries merge markers. Such a
    /// note is recorded as pending and must not be published or overwritten.
    pub(super) fn hold_if_marked(
        &self,
        rel_path: &str,
        content: &[u8],
        state: &mut SyncState,
    ) -> bool {
        if !is_mergeable_path(rel_path) {
            return false;
        }
        let Some(scan) = scan_marker_bytes(content) else {
            return false;
        };
        let known = state
            .pending_merge(rel_path)
            .is_some_and(|entry| entry.token == scan.token && entry.conflicts == scan.hunks);
        if !known {
            state.record_pending_merge(rel_path, &scan.token, scan.hunks);
        }
        true
    }

    /// Drop pending entries whose markers were edited out (or whose note is
    /// gone) and refresh the spot count of the rest.
    pub(super) fn refresh_pending_merges(&self, state: &mut SyncState) {
        let paths: Vec<String> = state.pending_merges.keys().cloned().collect();
        for rel_path in paths {
            match self.scan_note(&rel_path) {
                Ok(Some(scan)) => state.record_pending_merge(&rel_path, &scan.token, scan.hunks),
                Ok(None) => {
                    state.clear_pending_merge(&rel_path);
                }
                Err(_) => {}
            }
        }
    }

    /// Markers in the note on disk; `None` when it has none or is gone.
    fn scan_note(&self, rel_path: &str) -> Result<Option<MarkerScan>> {
        let path = safe_sync_path(&self.local_root, rel_path)?;
        match fs::read(path) {
            Ok(bytes) => Ok(scan_marker_bytes(&bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn try_merge(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        fence: &SourceMutationFence,
        state: &mut SyncState,
        result: &mut SyncResult,
    ) -> Result<bool> {
        let local = {
            let _source = fence.lock();
            self.local_snapshot(rel_path)?
        };
        let Some(local_bytes) = local.content.as_deref() else {
            return Ok(false);
        };
        if self.hold_if_marked(rel_path, local_bytes, state) {
            return Ok(true);
        }
        let Some(local_text) = mergeable_text(local_bytes) else {
            return Ok(false);
        };
        let remote = backend.read_snapshot(rel_path)?;
        let Some(remote_text) = remote.content.as_deref().and_then(mergeable_text) else {
            return Ok(false);
        };
        // Markers published by another client would nest with ours; a person
        // picks a side instead.
        if has_live_markers(remote_text) {
            return Ok(false);
        }
        let merged = match self.load_base(rel_path, state) {
            Some(base) => three_way_merge(&base, local_text, remote_text),
            None => two_way_merge(local_text, remote_text),
        };
        let Some(merged) = merged else {
            return Ok(false);
        };
        if merged.has_conflicts() {
            self.apply_marked_merge(rel_path, fence, &local, &remote, &merged.content, state)?;
            result.merge_conflicts.push(rel_path.to_string());
        } else {
            let merged = merged.content.as_bytes();
            let outcome =
                self.apply_clean_merge(backend, rel_path, fence, &local, &remote, merged, state)?;
            outcome.report(rel_path, result);
        }
        Ok(true)
    }

    /// The version both devices last agreed on, if it is still trustworthy.
    fn load_base(&self, rel_path: &str, state: &SyncState) -> Option<String> {
        let record = state.files.get(rel_path)?;
        let bytes = fs::read(self.base_path(rel_path)).ok()?;
        if compute_hash(&bytes) != record.hash_at_sync {
            return None;
        }
        let text = String::from_utf8(bytes).ok()?;
        (!has_live_markers(&text)).then_some(text)
    }

    /// Caller holds the fence lock.
    fn ensure_unchanged(
        &self,
        rel_path: &str,
        fence: &SourceMutationFence,
        local: &FileSnapshot,
    ) -> Result<()> {
        if fence.is_current() && self.local_snapshot(rel_path)?.hash() == local.hash() {
            Ok(())
        } else {
            Err(changed_while_syncing(rel_path))
        }
    }

    /// Overlapping edits: write the merged text with markers into the local
    /// note only. Nothing is published.
    fn apply_marked_merge(
        &self,
        rel_path: &str,
        fence: &SourceMutationFence,
        local: &FileSnapshot,
        remote: &FileSnapshot,
        marked: &str,
        state: &mut SyncState,
    ) -> Result<()> {
        let scan = scan_markers(marked)
            .ok_or_else(|| CoreError::Other(format!("Merging {rel_path} produced no markers")))?;
        {
            let _source = fence.lock();
            self.ensure_unchanged(rel_path, fence, local)?;
            self.preserve_copy(rel_path, local)?;
            self.preserve_copy(rel_path, remote)?;
            let path = safe_sync_path(&self.local_root, rel_path)?;
            crate::fsutil::atomic_write(&path, marked.as_bytes())?;
        }
        // Whatever the person decides is an edit on top of the remote version.
        let remote_bytes = remote.content.as_deref().unwrap_or_default();
        self.save_base(rel_path, remote_bytes);
        state.record_sync(rel_path, &compute_hash(remote_bytes), None, None);
        state.record_pending_merge(rel_path, &scan.token, scan.hunks);
        // Persist now: if the app stops before the end of the sync, a base
        // that does not match the markers would make the resolution conflict
        // again.
        state.save(&self.state_path)
    }

    /// Edits that do not overlap: publish the combined note and make it the
    /// local note too.
    fn apply_clean_merge(
        &self,
        backend: &dyn SyncBackend,
        rel_path: &str,
        fence: &SourceMutationFence,
        local: &FileSnapshot,
        remote: &FileSnapshot,
        merged: &[u8],
        state: &mut SyncState,
    ) -> Result<MergeOutcome> {
        let local_bytes = local.content.as_deref().unwrap_or_default();
        let remote_bytes = remote.content.as_deref().unwrap_or_default();
        {
            let _source = fence.lock();
            self.ensure_unchanged(rel_path, fence, local)?;
            if merged != local_bytes {
                self.preserve_copy(rel_path, local)?;
            }
            if merged != remote_bytes {
                self.preserve_copy(rel_path, remote)?;
            }
        }
        if merged != remote_bytes {
            backend.publish_if_unchanged(rel_path, remote, Some(merged))?;
        }
        let _source = fence.lock();
        // The remote already holds the merge here. A note edited while it
        // uploaded keeps that edit; the next sync folds it in.
        self.ensure_unchanged(rel_path, fence, local)?;
        if merged != local_bytes {
            let path = safe_sync_path(&self.local_root, rel_path)?;
            crate::fsutil::atomic_write(&path, merged)?;
        }
        let local_meta = self.current_local_metadata(rel_path)?;
        self.save_base(rel_path, merged);
        state.record_sync(rel_path, &compute_hash(merged), Some(&local_meta), None);
        Ok(if merged == local_bytes {
            MergeOutcome::Pushed
        } else if merged == remote_bytes {
            MergeOutcome::Pulled
        } else {
            MergeOutcome::Combined
        })
    }
}

/// How a clean merge changed the two sides, for the sync report.
enum MergeOutcome {
    /// The local note already had everything; only the remote changed.
    Pushed,
    /// The remote already had everything; only the local note changed.
    Pulled,
    /// Both sides changed.
    Combined,
}

impl MergeOutcome {
    fn report(self, rel_path: &str, result: &mut SyncResult) {
        let list = match self {
            MergeOutcome::Pushed => &mut result.pushed,
            MergeOutcome::Pulled => &mut result.pulled,
            MergeOutcome::Combined => &mut result.merged,
        };
        list.push(rel_path.to_string());
    }
}
