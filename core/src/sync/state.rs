use super::backend::FileMetadata;
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Per-file sync record: what we knew about a file at the last sync point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSyncRecord {
    /// Relative path (e.g. "pages/foo.md")
    pub rel_path: String,
    /// Hash of the file content at last sync
    pub hash_at_sync: String,
    /// Timestamp of last sync (Unix epoch seconds)
    pub synced_at: i64,
    /// Local size at last sync, when known.
    #[serde(default)]
    pub local_size: Option<u64>,
    /// Local modified timestamp at last sync, when known.
    #[serde(default)]
    pub local_modified_at: Option<i64>,
    /// Remote size at last sync, when known.
    #[serde(default)]
    pub remote_size: Option<u64>,
    /// Remote modified timestamp at last sync, when known.
    #[serde(default)]
    pub remote_modified_at: Option<i64>,
}

/// Persistent sync state — stored in the local graph folder.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncState {
    /// Map of relative path -> sync record
    pub files: HashMap<String, FileSyncRecord>,
    /// Timestamp of the last completed sync
    pub last_sync: Option<i64>,
}

impl SyncState {
    /// Load sync state from a JSON file, or create empty if not found.
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Save sync state to a JSON file.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }

    /// Record that a file was synced with the given content hash.
    pub fn record_sync(
        &mut self,
        rel_path: &str,
        hash: &str,
        local: Option<&FileMetadata>,
        remote: Option<&FileMetadata>,
    ) {
        let now = chrono::Utc::now().timestamp();
        self.files.insert(
            rel_path.to_string(),
            FileSyncRecord {
                rel_path: rel_path.to_string(),
                hash_at_sync: hash.to_string(),
                synced_at: now,
                local_size: local.map(|meta| meta.size),
                local_modified_at: local.map(|meta| meta.modified_at),
                remote_size: remote.map(|meta| meta.size),
                remote_modified_at: remote.map(|meta| meta.modified_at),
            },
        );
    }

    /// Check if a file has changed since last sync by comparing hashes.
    pub fn has_local_changes(&self, rel_path: &str, current_hash: &str) -> bool {
        match self.files.get(rel_path) {
            Some(record) => record.hash_at_sync != current_hash,
            None => true, // New file, never synced
        }
    }

    pub fn local_metadata_matches(&self, meta: &FileMetadata) -> bool {
        self.files.get(&meta.rel_path).map_or(false, |record| {
            record.local_size == Some(meta.size)
                && record.local_modified_at == Some(meta.modified_at)
        })
    }

    pub fn remote_metadata_matches(&self, meta: &FileMetadata) -> bool {
        self.files.get(&meta.rel_path).map_or(false, |record| {
            record.remote_size == Some(meta.size)
                && record.remote_modified_at == Some(meta.modified_at)
        })
    }

    pub fn cached_local_hash<'a>(&'a self, meta: &FileMetadata) -> Option<&'a str> {
        self.files
            .get(&meta.rel_path)
            .filter(|_| self.local_metadata_matches(meta))
            .map(|record| record.hash_at_sync.as_str())
    }

    pub fn cached_remote_hash<'a>(&'a self, meta: &FileMetadata) -> Option<&'a str> {
        self.files
            .get(&meta.rel_path)
            .filter(|_| self.remote_metadata_matches(meta))
            .map(|record| record.hash_at_sync.as_str())
    }

    /// Remove a file record (e.g. after deletion sync).
    pub fn remove_record(&mut self, rel_path: &str) {
        self.files.remove(rel_path);
    }
}

/// Configuration for a sync target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Unique ID for this sync target
    pub id: String,
    /// Human-readable name (e.g. "USB Stick", "Nextcloud")
    pub name: String,
    /// Backend type
    pub backend_type: BackendType,
    /// Backend-specific config
    pub config: BackendConfig,
    /// Auto-sync when available
    pub auto_sync: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendType {
    Filesystem,
    WebDav,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BackendConfig {
    Filesystem {
        path: PathBuf,
    },
    WebDav {
        url: String,
        username: String,
        password: String,
    },
}

/// All sync configurations for a graph.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncConfigs {
    pub targets: Vec<SyncConfig>,
}

impl SyncConfigs {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }
}
