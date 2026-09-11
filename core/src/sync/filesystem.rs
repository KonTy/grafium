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

    /// Walk a synced directory. `markdown_only` distinguishes note folders,
    /// where only `.md` participates, from portable support folders such as
    /// `knowledge/` and `assets/`, which may hold non-Markdown files too.
    fn collect_files(
        &self,
        dir: &Path,
        base: &Path,
        markdown_only: bool,
        out: &mut Vec<FileMetadata>,
    ) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                self.collect_files(&path, base, markdown_only, out)?;
            } else if Self::is_syncable_entry(&path, base, markdown_only) {
                let rel = path
                    .strip_prefix(base)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                let meta = entry.metadata()?;
                let modified_at = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as i64)
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

    fn is_syncable_entry(path: &Path, base: &Path, markdown_only: bool) -> bool {
        // Conflict copies are written explicitly by the engine; never pick
        // them up as ordinary files to sync.
        if path.to_string_lossy().contains(".conflict_") {
            return false;
        }
        let rel = path.strip_prefix(base).unwrap_or(path).to_string_lossy();
        if rel.starts_with("pages/") && rel.contains("/assets/") {
            true
        } else if markdown_only {
            path.extension().and_then(|e| e.to_str()) == Some("md")
        } else {
            // Skip the scratch files atomic_write leaves mid-rename.
            !path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.') && n.ends_with(".tmp"))
        }
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
        self.collect_files(&self.root.join("pages"), &self.root, true, &mut files)?;
        self.collect_files(&self.root.join("journals"), &self.root, true, &mut files)?;
        self.collect_files(&self.root.join("knowledge"), &self.root, false, &mut files)?;
        // Media referenced by notes lives here; without it a synced note
        // arrives on the other machine with broken image and audio links.
        self.collect_files(&self.root.join("assets"), &self.root, false, &mut files)?;
        Ok(files)
    }

    fn stat_file(&self, rel_path: &str) -> Result<FileMetadata> {
        let path = self.abs_path(rel_path);
        let meta = fs::metadata(&path)?;
        let modified_at = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);

        Ok(FileMetadata {
            rel_path: rel_path.to_string(),
            size: meta.len(),
            modified_at,
            hash: None,
        })
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
        crate::fsutil::atomic_write(&path, content)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::SyncBackend;
    use tempfile::tempdir;

    #[test]
    fn lists_portable_knowledge_files() -> Result<()> {
        let temp = tempdir()?;
        let path = temp.path().join("knowledge/prompts");
        fs::create_dir_all(&path)?;
        fs::write(path.join("web-research.md"), "- prompt:: cite everything\n")?;
        fs::write(path.join("web-research.json"), "{}\n")?;

        let backend = FilesystemBackend::new(temp.path().to_path_buf(), "local".to_string());
        let mut paths = backend
            .list_files()?
            .into_iter()
            .map(|file| file.rel_path.replace('\\', "/"))
            .collect::<Vec<_>>();
        paths.sort();

        assert_eq!(
            paths,
            vec![
                "knowledge/prompts/web-research.json".to_string(),
                "knowledge/prompts/web-research.md".to_string(),
            ]
        );
        Ok(())
    }
}
