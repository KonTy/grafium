//! Sync module: offline-first bidirectional sync with pluggable backends.
//!
//! Supports syncing a local graph folder to a remote target (USB drive,
//! network mount, WebDAV/Nextcloud). Works offline — edits are cached locally
//! and merged when the remote becomes available again.
//!
//! Conflict strategy: notes edited on both sides are merged block by block.
//! Edits that do not overlap combine cleanly; overlapping edits are written
//! into the local note between `<<<<<<< YOURS` / `======= ` / `>>>>>>> THEIRS`
//! marker lines for a person to settle. A marked note never leaves the device
//! and is never overwritten; once the markers are edited out, the resolved
//! note syncs like any other edit. Everything else (binaries, books, deleted
//! versus edited files) keeps the whole-file flow: preserve both byte-exact
//! versions under the target's metadata `sync-recovery/` folder, then require
//! an explicit revision-checked local/remote choice.

pub mod backend;
mod diff;
pub mod engine;
pub mod filesystem;
pub mod merge;
pub mod state;
pub mod webdav;

pub use backend::{FileMetadata, SyncBackend};
pub use engine::SyncEngine;
pub use merge::{
    has_live_markers, scan_markers, three_way_merge, two_way_merge, MarkerScan, MergeResult,
};
pub use state::{PendingMerge, SyncConfig, SyncState, UnresolvedConflict};

#[cfg(test)]
mod safety_tests;
