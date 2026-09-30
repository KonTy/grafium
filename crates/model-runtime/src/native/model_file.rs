//! Shared locks on actual model files, not sidecar names. These coordinate
//! cooperating readers and catalog mutations; they are not a security sandbox.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use fs2::FileExt;

use crate::error::{Result, RuntimeError};
use crate::supervisor::WorkerLease;

/// An unconfirmed cleanup must fail closed. The native host has one resident
/// worker; after retaining its lock we refuse further admissions. This avoids
/// accumulating orphaned locks or guessing that a still-owned process died.
static UNCONFIRMED_EXIT: Mutex<Option<LockedModelFile>> = Mutex::new(None);

pub(super) struct LockedModelFile {
    path: PathBuf,
    file: File,
    locked: bool,
}

pub(super) fn canonical_model_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize().map_err(|error| {
        RuntimeError::Other(format!(
            "Cannot resolve model file {}: {error}",
            path.display()
        ))
    })
}

impl LockedModelFile {
    pub(super) fn acquire(path: &Path) -> Result<Self> {
        let path = canonical_model_path(path)?;
        if !path.is_file() {
            return Err(RuntimeError::Other(format!(
                "Model path is not a regular file: {}",
                path.display(),
            )));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_SHARE_READ: other readers are welcome, but do not permit
            // rename/delete or writing through another handle while loaded.
            options.share_mode(1);
        }
        let file = options.open(&path).map_err(|error| {
            RuntimeError::Other(format!(
                "Cannot open model file {} for inference: {error}",
                path.display()
            ))
        })?;
        FileExt::try_lock_shared(&file).map_err(|error| {
            RuntimeError::Other(format!(
                "Model file is in use for modification or cannot be shared-locked: {}: {error}",
                path.display(),
            ))
        })?;
        let locked = Self {
            path,
            file,
            locked: true,
        };
        locked.verify_path()?;
        Ok(locked)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    fn release(&mut self) -> Result<()> {
        if self.locked {
            FileExt::unlock(&self.file).map_err(|error| {
                RuntimeError::Other(format!(
                    "Cannot release model file lock {}: {error}",
                    self.path.display()
                ))
            })?;
            self.locked = false;
        }
        Ok(())
    }

    fn verify_path(&self) -> Result<()> {
        let canonical = self.path.canonicalize()?;
        let opened = self.file.metadata()?;
        let current = std::fs::metadata(&self.path)?;
        if canonical != self.path || !opened.is_file() || !current.is_file() {
            return Err(RuntimeError::Other(
                "Model file changed while acquiring its inference lock".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (opened.dev(), opened.ino()) != (current.dev(), current.ino()) {
                return Err(RuntimeError::Other(
                    "Model file was replaced while acquiring its inference lock".into(),
                ));
            }
        }
        // On Windows the open handle denies delete/write sharing, preventing a
        // cooperating replacement after open without relying on timestamp IDs.
        Ok(())
    }
}

impl Drop for LockedModelFile {
    fn drop(&mut self) {
        // Explicit unlock clears transient CLOEXEC copies from unrelated
        // concurrent forks. Native loaders always acquire independent locks.
        if let Err(error) = self.release() {
            tracing::error!(%error, "Model file lock cleanup failed");
        }
    }
}

pub(super) fn ensure_exit_confirmed() -> Result<()> {
    if UNCONFIRMED_EXIT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some()
    {
        return Err(RuntimeError::Other(
            "A native worker exit was not confirmed; its model remains locked. Restart this host before loading another model.".into(),
        ));
    }
    Ok(())
}

pub(super) fn lock_for_admission(path: &Path) -> Result<LockedModelFile> {
    ensure_exit_confirmed()?;
    LockedModelFile::acquire(path)
}

/// Transfers an already acquired file lock into the supervised resident lease.
/// CPU workers use this too; recovery bookkeeping is optional.
pub(super) struct ModelFileLease {
    model: Mutex<Option<LockedModelFile>>,
    recovery: Option<Box<dyn WorkerLease>>,
}

impl ModelFileLease {
    pub(super) fn new(model: LockedModelFile, recovery: Option<Box<dyn WorkerLease>>) -> Self {
        Self {
            model: Mutex::new(Some(model)),
            recovery,
        }
    }
}

impl WorkerLease for ModelFileLease {
    fn confirmed_exit(&self, expected: bool) -> Result<()> {
        // The supervisor has proved death (or that spawn never succeeded).
        // Release the model even if subsequent recovery persistence fails.
        {
            let mut model = self.model.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(model) = model.as_mut() {
                model.release()?;
            }
            model.take();
        }
        if let Some(recovery) = &self.recovery {
            recovery.confirmed_exit(expected)?;
        }
        Ok(())
    }
}

impl Drop for ModelFileLease {
    fn drop(&mut self) {
        if let Some(model) = self
            .model
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            tracing::error!(path = %model.path.display(),
                "Native worker exit unconfirmed; retaining model lock until host exit");
            let mut retained = UNCONFIRMED_EXIT
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if retained.is_none() {
                *retained = Some(model);
            } else {
                // Unreachable through the single native supervisor's admission
                // gate. Never unlock a possibly resident model on an invariant
                // failure, including while unwinding a host panic.
                std::mem::forget(model);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exclusive_available(path: &Path) -> bool {
        let file = File::open(path).unwrap();
        let available = FileExt::try_lock_exclusive(&file).is_ok();
        if available {
            FileExt::unlock(&file).unwrap();
        }
        available
    }

    #[test]
    fn multiple_readers_block_mutation_until_the_last_reader_exits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        std::fs::write(&path, b"synthetic model").unwrap();
        let first = LockedModelFile::acquire(&path).unwrap();
        let second = LockedModelFile::acquire(&path).unwrap();
        assert!(!exclusive_available(&path));
        drop(first);
        assert!(!exclusive_available(&path));
        drop(second);
        assert!(exclusive_available(&path));
    }

    #[test]
    fn active_catalog_writer_refuses_a_new_native_reader_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        std::fs::write(&path, b"synthetic model").unwrap();
        let writer = File::open(&path).unwrap();
        FileExt::try_lock_exclusive(&writer).unwrap();
        let error = match LockedModelFile::acquire(&path) {
            Ok(_) => panic!("exclusive writer was ignored"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("shared-locked"), "{error}");
        FileExt::unlock(&writer).unwrap();
        drop(writer);
        assert!(LockedModelFile::acquire(&path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn alias_retargeting_does_not_change_the_locked_canonical_load_path() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.gguf");
        let second = dir.path().join("second.gguf");
        let alias = dir.path().join("selected.gguf");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        let locked = LockedModelFile::acquire(&alias).unwrap();
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&second, &alias).unwrap();
        assert_eq!(locked.path(), first.canonicalize().unwrap());
        assert_eq!(std::fs::read(locked.path()).unwrap(), b"first");
        assert!(!exclusive_available(&first));
        assert!(exclusive_available(&second));
        let hardlink = dir.path().join("hardlink.gguf");
        std::fs::hard_link(&first, &hardlink).unwrap();
        assert!(
            !exclusive_available(&hardlink),
            "locks must protect the inode, not just one name"
        );
    }

    #[cfg(unix)]
    #[test]
    fn replacement_between_open_and_validation_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        std::fs::write(&path, b"first").unwrap();
        let locked = LockedModelFile::acquire(&path).unwrap();
        // Simulate a non-cooperating replacement at the open/lock boundary.
        std::fs::rename(&path, dir.path().join("old.gguf")).unwrap();
        std::fs::write(&path, b"second").unwrap();
        assert!(locked.verify_path().is_err());
    }
}
