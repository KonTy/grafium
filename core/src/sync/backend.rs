use crate::error::{CoreError, Result};
use std::path::Path;

/// Metadata about a file on a sync backend.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileMetadata {
    /// Relative path within the graph (e.g. "pages/foo.md")
    pub rel_path: String,
    /// Size in bytes
    pub size: u64,
    /// Last modified timestamp since the Unix epoch. Backends may provide
    /// second or finer precision; comparisons are only made within the same
    /// side (local-vs-local, remote-vs-remote).
    pub modified_at: i64,
    /// Content hash (SHA-256 hex). None if not yet computed.
    pub hash: Option<String>,
}

/// Trait for sync backends. Each backend represents a remote storage location
/// (USB filesystem, WebDAV server, network share, etc.)
pub trait SyncBackend: Send + Sync {
    /// Human-readable name of the backend (e.g. "USB: /media/usb/notes")
    fn name(&self) -> &str;

    /// Check if the remote is currently reachable/mounted.
    fn is_available(&self) -> bool;

    /// List all .md files on the remote with their metadata.
    fn list_files(&self) -> Result<Vec<FileMetadata>>;

    /// Fetch metadata for one remote file.
    fn stat_file(&self, rel_path: &str) -> Result<FileMetadata> {
        self.list_files()?
            .into_iter()
            .find(|file| file.rel_path == rel_path)
            .ok_or_else(|| CoreError::NotFound(format!("Remote file not found: {rel_path}")))
    }

    /// Read a file's content by relative path.
    fn read_file(&self, rel_path: &str) -> Result<Vec<u8>>;

    /// Write a file to the remote. Creates parent directories as needed.
    fn write_file(&self, rel_path: &str, content: &[u8]) -> Result<()>;

    /// Delete a file on the remote.
    fn delete_file(&self, rel_path: &str) -> Result<()>;

    /// Compute content hash for a remote file.
    fn file_hash(&self, rel_path: &str) -> Result<String> {
        let content = self.read_file(rel_path)?;
        Ok(compute_hash(&content))
    }
}

/// Compute SHA-256 hash of content, returned as hex string.
pub fn compute_hash(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Compute hash for a local file on disk.
pub fn hash_file(path: &Path) -> Result<String> {
    let data = std::fs::read(path)?;
    Ok(compute_hash(&data))
}
