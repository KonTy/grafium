use super::backend::{compute_hash, FileMetadata, SyncBackend};
use crate::error::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// Filesystem-based sync backend.
/// Works with USB drives, network mounts, or any locally-accessible directory.
pub struct FilesystemBackend {
    /// Root path of the remote graph folder
    root: PathBuf,
    /// Display name
    name: String,
}

impl FilesystemBackend {
    pub fn new(root: PathBuf, name: String) -> Self {
        Self { root, name }
    }

    fn abs_path(&self, rel_path: &str) -> PathBuf {
        self.root.join(rel_path)
    }

    fn collect_md_files(&self, dir: &Path, base: &Path, out: &mut Vec<FileMetadata>) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                self.collect_md_files(&path, base, out)?;
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let rel = path
                    .strip_prefix(base)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                let meta = entry.metadata()?;
                let modified_at = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);

                out.push(FileMetadata {
                    rel_path: rel,
                    size: meta.len(),
                    modified_at,
                    hash: None,
                });
            }
        }
        Ok(())
    }
}

impl SyncBackend for FilesystemBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_available(&self) -> bool {
        self.root.exists() && self.root.is_dir()
    }

    fn list_files(&self) -> Result<Vec<FileMetadata>> {
        let mut files = Vec::new();
        // Scan pages/ and journals/ subdirectories
        let pages_dir = self.root.join("pages");
        let journals_dir = self.root.join("journals");

        self.collect_md_files(&pages_dir, &self.root, &mut files)?;
        self.collect_md_files(&journals_dir, &self.root, &mut files)?;
        Ok(files)
    }

    fn read_file(&self, rel_path: &str) -> Result<Vec<u8>> {
        let path = self.abs_path(rel_path);
        Ok(fs::read(&path)?)
    }

    fn write_file(&self, rel_path: &str, content: &[u8]) -> Result<()> {
        let path = self.abs_path(rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content)?;
        Ok(())
    }

    fn delete_file(&self, rel_path: &str) -> Result<()> {
        let path = self.abs_path(rel_path);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }

    fn file_hash(&self, rel_path: &str) -> Result<String> {
        let path = self.abs_path(rel_path);
        let content = fs::read(&path)?;
        Ok(compute_hash(&content))
    }
}
