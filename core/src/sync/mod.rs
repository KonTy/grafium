//! Sync module: offline-first bidirectional sync with pluggable backends.
//!
//! Supports syncing a local graph folder to a remote target (USB drive,
//! network mount, WebDAV/Nextcloud). Works offline — edits are cached locally
//! and merged when the remote becomes available again.
//!
//! Conflict strategy: keep both versions (creates `.conflict.md` files).

pub mod backend;
pub mod engine;
pub mod filesystem;
pub mod merge;
pub mod state;
pub mod webdav;

pub use backend::{FileMetadata, SyncBackend};
pub use engine::SyncEngine;
pub use merge::{three_way_merge, two_way_merge, MergeResult};
pub use state::{SyncConfig, SyncState};
