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
pub mod state;
pub mod webdav;

pub use backend::{SyncBackend, FileMetadata};
pub use engine::SyncEngine;
pub use state::{SyncState, SyncConfig};
