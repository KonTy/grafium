use super::backend::{compute_hash, revision_changed, FileMetadata, FileSnapshot, SyncBackend};
use crate::error::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn safe_sync_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        let std::path::Component::Normal(name) = component else {
            return Err(crate::error::CoreError::Other(
                "Unsafe relative sync path".into(),
            ));
        };
        path.push(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(crate::error::CoreError::Other(
                    "Symlinks are not allowed in sync paths".into(),
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(path)
}

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

    fn abs_path(&self, rel_path: &str) -> Result<PathBuf> {
        safe_sync_path(&self.root, rel_path)
    }

    fn snapshot_unlocked(&self, rel_path: &str) -> Result<FileSnapshot> {
        match fs::read(self.abs_path(rel_path)?) {
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
        match fs::symlink_metadata(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(crate::error::CoreError::Other(
                    "Symlink in remote sync inventory".into(),
                ));
            }
            Ok(_) => {}
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_symlink() {
                return Err(crate::error::CoreError::Other(
                    "Symlink in remote sync inventory".into(),
                ));
            }
            if path.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                {
                    continue;
                }
                self.collect_files(&path, base, markdown_only, out)?;
            } else if Self::is_syncable_entry(&path, base, markdown_only) {
                let rel = path
                    .strip_prefix(base)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
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
        let rel = rel.replace('\\', "/");
        if rel.starts_with("books/") {
            crate::graph::books::is_portable_book_file(&rel)
        } else if rel.starts_with("pages/") && rel.contains("/assets/") {
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
        self.collect_files(&self.root.join("books"), &self.root, false, &mut files)?;
        Ok(files)
    }

    fn stat_file(&self, rel_path: &str) -> Result<FileMetadata> {
        let path = self.abs_path(rel_path)?;
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
        let path = self.abs_path(rel_path)?;
        Ok(fs::read(&path)?)
    }

    fn read_snapshot(&self, rel_path: &str) -> Result<FileSnapshot> {
        let fence = crate::fsutil::source_mutation_fence(&self.root, Path::new(rel_path))?;
        let mut snapshot = {
            let _source = fence.lock();
            self.snapshot_unlocked(rel_path)?
        };
        snapshot.mutation_fence = Some(fence);
        Ok(snapshot)
    }

    fn write_file(&self, rel_path: &str, content: &[u8]) -> Result<()> {
        let source_lock = crate::fsutil::graph_operation_lock(&self.root)?;
        let _source = source_lock.lock();
        let path = self.abs_path(rel_path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        crate::fsutil::atomic_write(&path, content)
    }

    fn delete_file(&self, rel_path: &str) -> Result<()> {
        let source_lock = crate::fsutil::graph_operation_lock(&self.root)?;
        let _source = source_lock.lock();
        let path = self.abs_path(rel_path)?;
        if path.exists() {
            crate::fsutil::record_source_mutation(&self.root, Path::new(rel_path))?;
            fs::remove_file(&path)?;
        }
        Ok(())
    }

    fn publish_if_unchanged(
        &self,
        rel_path: &str,
        expected: &FileSnapshot,
        content: Option<&[u8]>,
    ) -> Result<()> {
        // This excludes cooperating Grafium writers. Filesystems offer no
        // portable atomic compare-and-replace against arbitrary external apps.
        let source_lock = crate::fsutil::graph_operation_lock(&self.root)?;
        let _source = source_lock.lock();
        if expected
            .mutation_fence
            .as_ref()
            .is_some_and(|fence| !fence.is_current())
            || self.snapshot_unlocked(rel_path)?.hash() != expected.hash()
        {
            return Err(revision_changed(rel_path));
        }
        let path = self.abs_path(rel_path)?;
        match content {
            Some(content) => crate::fsutil::atomic_write(&path, content)?,
            None if expected.content.is_some() => {
                crate::fsutil::record_source_mutation(&self.root, Path::new(rel_path))?;
                fs::remove_file(&path)?;
            }
            None => {}
        }
        if self.snapshot_unlocked(rel_path)?.content.as_deref() != content {
            return Err(revision_changed(rel_path));
        }
        Ok(())
    }

    fn file_hash(&self, rel_path: &str) -> Result<String> {
        let path = self.abs_path(rel_path)?;
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

    #[test]
    fn publication_rechecks_existence_and_bytes() -> Result<()> {
        let root = tempfile::tempdir_in(".")?;
        let backend = FilesystemBackend::new(root.path().to_path_buf(), "synthetic".into());
        let missing = backend.read_snapshot("pages/doc.md")?;
        backend.write_file("pages/doc.md", b"new local writer")?;
        assert!(backend
            .publish_if_unchanged("pages/doc.md", &missing, Some(b"stale"))
            .is_err());
        let original = backend.read_snapshot("pages/doc.md")?;
        backend.write_file("pages/doc.md", b"newer writer")?;
        assert!(backend
            .publish_if_unchanged("pages/doc.md", &original, None)
            .is_err());
        assert_eq!(backend.read_file("pages/doc.md")?, b"newer writer");
        let current = backend.read_snapshot("pages/doc.md")?;
        backend.publish_if_unchanged("pages/doc.md", &current, Some(b"\xff=======\0"))?;
        assert_eq!(backend.read_file("pages/doc.md")?, b"\xff=======\0");
        Ok(())
    }

    #[test]
    fn filesystem_snapshot_rejects_absent_and_same_byte_aba() -> Result<()> {
        for initially_present in [false, true] {
            let root = tempfile::tempdir_in(".")?;
            let backend = FilesystemBackend::new(root.path().to_path_buf(), "synthetic".into());
            let path = "pages/doc.md";
            if initially_present {
                backend.write_file(path, b"original")?;
            }
            let snapshot = backend.read_snapshot(path)?;
            backend.write_file(path, b"intervening")?;
            if initially_present {
                backend.write_file(path, b"original")?;
            } else {
                backend.delete_file(path)?;
            }
            assert_eq!(backend.read_snapshot(path)?.hash(), snapshot.hash());
            assert!(backend
                .publish_if_unchanged(path, &snapshot, Some(b"stale"))
                .is_err());
            let current = backend.read_snapshot(path)?;
            backend.publish_if_unchanged(path, &current, Some(b"confirmed"))?;
            assert_eq!(backend.read_file(path)?, b"confirmed");
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inventory_cannot_be_treated_as_a_deletion() -> Result<()> {
        let root = tempfile::tempdir_in(".")?;
        let outside = tempfile::tempdir_in(".")?;
        std::os::unix::fs::symlink(outside.path().canonicalize()?, root.path().join("pages"))?;
        let backend = FilesystemBackend::new(root.path().to_path_buf(), "synthetic".into());
        assert!(backend.list_files().is_err());
        Ok(())
    }
}
