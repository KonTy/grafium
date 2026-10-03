//! Attachment bytes outlive text deletion until an explicit, reviewed purge.

use super::asset_cleanup::{checked_path, create_directory, fingerprint, protected_source};
use super::*;

const TRASH: &str = ".grafium/asset-trash";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetTrashEntry {
    pub filename: String,
    pub trash_filename: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Serialize)]
pub struct AssetTrashScan {
    pub graph_path: String,
    pub assets: Vec<AssetTrashEntry>,
}

#[derive(Debug, Default, Serialize)]
pub struct AssetTrashResult {
    pub restored: Vec<String>,
    pub purged: Vec<String>,
    pub errors: Vec<String>,
}

fn error(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

fn managed_asset(relative: &Path) -> bool {
    let parts = relative.components().collect::<Vec<_>>();
    parts.iter().all(|part| {
        matches!(part, std::path::Component::Normal(name)
        if !name.to_string_lossy().starts_with('.'))
    }) && parts
        .iter()
        .take(parts.len().saturating_sub(1))
        .any(|part| part.as_os_str() == "assets")
        && !protected_source(relative)
}

fn flush_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(
        path.parent()
            .ok_or_else(|| error("Missing parent directory"))?,
    )?
    .sync_all()?;
    Ok(())
}

// Unlike std::fs::rename, this must never replace an external file that appeared
// after the preview or preflight. Unsupported filesystems fail with bytes intact.
fn rename_no_replace(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(source.as_os_str().as_bytes())
            .map_err(|_| error("Invalid attachment path"))?;
        let to = std::ffi::CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| error("Invalid attachment path"))?;
        // SAFETY: both C strings remain alive and NUL-terminated for the call.
        // Android's libc declares the flag as c_int; renameat2 takes c_uint.
        #[cfg(target_os = "android")]
        let flags = libc::RENAME_NOREPLACE as libc::c_uint;
        #[cfg(target_os = "linux")]
        let flags = libc::RENAME_NOREPLACE;
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                flags,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(source.as_os_str().as_bytes())
            .map_err(|_| error("Invalid attachment path"))?;
        let to = std::ffi::CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| error("Invalid attachment path"))?;
        // SAFETY: both C strings remain alive and NUL-terminated for the call.
        if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        return Ok(());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn MoveFileW(from: *const u16, to: *const u16) -> i32;
        }
        let from: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<_> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        // SAFETY: both UTF-16 buffers are NUL-terminated and live through the call.
        if unsafe { MoveFileW(from.as_ptr(), to.as_ptr()) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        return Ok(());
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        windows
    )))]
    Err(error(format!(
        "No safe attachment restore on this platform: {} → {}",
        source.display(),
        destination.display()
    )))
}

fn walk_trash(
    root: &Path,
    dir: &Path,
    candidates: Option<&HashSet<String>>,
    out: &mut Vec<AssetTrashEntry>,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(error(format!(
                "Symbolic link in attachment trash: {}",
                path.display()
            )));
        }
        if kind.is_dir() {
            walk_trash(root, &path, candidates, out)?;
        } else if kind.is_file() {
            let relative = path.strip_prefix(root).map_err(|e| error(e.to_string()))?;
            let mut parts = relative
                .strip_prefix(TRASH)
                .map_err(|e| error(e.to_string()))?
                .components();
            let batch = parts
                .next()
                .ok_or_else(|| error("Invalid trash directory"))?;
            Uuid::parse_str(&batch.as_os_str().to_string_lossy())
                .map_err(|_| error("Invalid trash batch"))?;
            let original = parts.as_path();
            if !managed_asset(original) {
                return Err(error(format!(
                    "Unexpected file in attachment trash: {}",
                    relative.display()
                )));
            }
            let filename = original.to_string_lossy().replace('\\', "/");
            if candidates.is_some_and(|candidates| !candidates.contains(&filename)) {
                continue;
            }
            let (size, sha256) = fingerprint(&path)?;
            out.push(AssetTrashEntry {
                filename,
                trash_filename: relative.to_string_lossy().replace('\\', "/"),
                size,
                sha256,
            });
        } else {
            return Err(error(format!(
                "Special file in attachment trash: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

impl Graph {
    pub fn take_asset_cleanup_warnings(&self) -> Vec<String> {
        std::mem::take(&mut *self.asset_cleanup_warnings.lock())
    }

    pub(super) fn asset_cleanup_warning(&self, message: String) {
        tracing::warn!("{message}");
        self.asset_cleanup_warnings.lock().push(message);
    }

    pub fn list_asset_trash(&self) -> Result<AssetTrashScan> {
        self.list_asset_trash_matching(None)
    }

    fn list_asset_trash_matching(
        &self,
        candidates: Option<&HashSet<String>>,
    ) -> Result<AssetTrashScan> {
        let _operation = self.source_operations.lock();
        let root = self.root_dir.canonicalize()?;
        let mut assets = Vec::new();
        match fs::symlink_metadata(root.join(TRASH)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
            Ok(_) => {
                let trash = checked_path(&root, Path::new(TRASH))?;
                for entry in fs::read_dir(trash)? {
                    let entry = entry?;
                    if entry.file_name() == ".purged" || entry.file_name() == ".selected" {
                        continue;
                    }
                    if !entry.file_type()?.is_dir() || entry.file_type()?.is_symlink() {
                        return Err(error("Invalid attachment trash directory"));
                    }
                    walk_trash(&root, &entry.path(), candidates, &mut assets)?;
                }
            }
        }
        assets.sort_by(|a, b| {
            (&a.filename, &a.trash_filename).cmp(&(&b.filename, &b.trash_filename))
        });
        Ok(AssetTrashScan {
            graph_path: root.to_string_lossy().into_owned(),
            assets,
        })
    }

    pub fn asset_trash_containing_folder(
        &self,
        graph_path: &str,
        entry: &AssetTrashEntry,
    ) -> Result<PathBuf> {
        let _operation = self.source_operations.lock();
        let root = self.root_dir.canonicalize()?;
        if root.to_string_lossy() != graph_path {
            return Err(error("The active graph changed. Refresh trash before continuing."));
        }
        let path = Self::checked_trash_entry_path(&root, entry)?;
        Ok(path
            .parent()
            .ok_or_else(|| error("Trashed file has no containing folder"))?
            .to_path_buf())
    }

    fn checked_trash_entry_path(root: &Path, entry: &AssetTrashEntry) -> Result<PathBuf> {
        let relative = Path::new(&entry.trash_filename);
        let mut parts = relative
            .strip_prefix(TRASH)
            .map_err(|_| error("Invalid trash path"))?
            .components();
        Uuid::parse_str(
            &parts
                .next()
                .ok_or_else(|| error("Missing trash batch"))?
                .as_os_str()
                .to_string_lossy(),
        )
        .map_err(|_| error("Invalid trash batch"))?;
        if parts.as_path() != Path::new(&entry.filename) || !managed_asset(parts.as_path()) {
            return Err(error("Invalid original attachment path"));
        }
        let path = checked_path(root, relative)?;
        if !fs::symlink_metadata(&path)?.is_file() {
            return Err(error("Trashed file changed since preview; refresh before continuing"));
        }
        Ok(path)
    }

    fn validated_trash_entry(&self, root: &Path, entry: &AssetTrashEntry) -> Result<PathBuf> {
        let path = Self::checked_trash_entry_path(root, entry)?;
        if fingerprint(&path)? != (entry.size, entry.sha256.clone()) {
            return Err(error(
                "Trashed file changed since preview; refresh before continuing",
            ));
        }
        Ok(path)
    }

    fn version_record_dir(kind: &str, filename: &str) -> PathBuf {
        PathBuf::from(TRASH)
            .join(kind)
            .join(format!("{:x}", Sha256::digest(filename.as_bytes())))
    }

    fn record_asset_version(&self, root: &Path, kind: &str, entry: &AssetTrashEntry) -> Result<()> {
        let directory = Self::version_record_dir(kind, &entry.filename);
        create_directory(root, &directory)?;
        let record = root.join(directory).join(format!("{}.json", entry.sha256));
        Self::atomic_write(&record, &serde_json::to_string(entry)?)
    }

    fn asset_versions(
        &self,
        root: &Path,
        kind: &str,
        filename: &str,
    ) -> Result<HashSet<(u64, String)>> {
        let relative = Self::version_record_dir(kind, filename);
        match fs::symlink_metadata(root.join(&relative)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }
        let directory = checked_path(root, &relative)?;
        let mut versions = HashSet::new();
        for record in fs::read_dir(directory)? {
            let record = record?;
            let path = checked_path(root, &relative.join(record.file_name()))?;
            let entry: AssetTrashEntry = serde_json::from_str(&fs::read_to_string(path)?)?;
            if entry.filename != filename {
                return Err(error(
                    "Attachment version history does not match its original path",
                ));
            }
            versions.insert((entry.size, entry.sha256));
        }
        Ok(versions)
    }

    fn restore_trash_entry(&self, root: &Path, entry: &AssetTrashEntry) -> Result<Vec<String>> {
        let source = self.validated_trash_entry(root, entry)?;
        let original = Path::new(&entry.filename);
        let parent = original
            .parent()
            .ok_or_else(|| error("Missing attachment directory"))?;
        create_directory(root, parent)?;
        let destination = root.join(original);
        rename_no_replace(&source, &destination).map_err(|e| error(format!(
            "Cannot restore {} without overwriting an existing file: {e}. The recovery copy remains in trash.",
            entry.filename
        )))?;
        let mut warnings = Vec::new();
        for result in [
            crate::fsutil::record_source_mutation(root, original),
            flush_parent(&destination),
            flush_parent(&source),
        ] {
            if let Err(e) = result {
                warnings.push(format!(
                    "{} was restored, but its durability/sync bookkeeping failed: {e}",
                    entry.filename
                ));
            }
        }
        Ok(warnings)
    }

    pub fn restore_trashed_assets(
        &self,
        graph_path: &str,
        requested: &[AssetTrashEntry],
    ) -> Result<AssetTrashResult> {
        let _operation = self.source_operations.lock();
        let root = self.root_dir.canonicalize()?;
        if root.to_string_lossy() != graph_path {
            return Err(error("The active graph changed. Refresh its trash."));
        }
        let mut result = AssetTrashResult::default();
        let mut seen = HashSet::new();
        for entry in requested {
            if !seen.insert(&entry.trash_filename) {
                continue;
            }
            let restore = || -> Result<Vec<String>> {
                self.validated_trash_entry(&root, entry)?;
                // Preserve the user's explicit version choice even across restart.
                // A failed no-overwrite move does not authorize different bytes.
                self.record_asset_version(&root, ".selected", entry)?;
                self.restore_trash_entry(&root, entry)
            };
            match restore() {
                Ok(warnings) => {
                    result.restored.push(entry.trash_filename.clone());
                    result.errors.extend(warnings);
                }
                Err(e) => result.errors.push(format!("{}: {e}", entry.filename)),
            }
        }
        Ok(result)
    }

    pub fn purge_trashed_assets(
        &self,
        graph_path: &str,
        requested: &[AssetTrashEntry],
    ) -> Result<AssetTrashResult> {
        let _operation = self.source_operations.lock();
        let root = self.root_dir.canonicalize()?;
        if root.to_string_lossy() != graph_path {
            return Err(error("The active graph changed. Refresh its trash."));
        }
        let mut result = AssetTrashResult::default();
        let mut seen = HashSet::new();
        for entry in requested {
            if !seen.insert(&entry.trash_filename) {
                continue;
            }
            let purge = || -> Result<Vec<String>> {
                let path = self.validated_trash_entry(&root, entry)?;
                if self.asset_is_referenced(Path::new(&entry.filename))? {
                    return Err(error("Attachment is referenced again; restore it instead of permanently deleting it"));
                }
                self.record_asset_version(&root, ".purged", entry)?;
                // Revalidate after writing the durable tombstone.
                self.validated_trash_entry(&root, entry)?;
                if self.asset_is_referenced(Path::new(&entry.filename))? {
                    return Err(error(
                        "Attachment became referenced during purge; its bytes were preserved",
                    ));
                }
                fs::remove_file(&path)?;
                Ok(flush_parent(&path)
                    .err()
                    .map(|e| {
                        format!(
                            "{} was permanently deleted, but its directory flush failed: {e}",
                            entry.filename
                        )
                    })
                    .into_iter()
                    .collect())
            };
            match purge() {
                Ok(warnings) => {
                    result.purged.push(entry.trash_filename.clone());
                    result.errors.extend(warnings);
                }
                Err(e) => result.errors.push(format!("{}: {e}", entry.filename)),
            }
        }
        Ok(result)
    }

    fn content_asset_paths(&self, source: &Path, content: &str) -> HashSet<String> {
        // Every managed attachment lives under an `assets` folder, and only a
        // percent-encoded reference could spell that folder differently. Most
        // edits touch neither, so skip re-parsing the whole page on every save.
        if !content.contains("assets") && !content.contains('%') {
            return HashSet::new();
        }
        let mut refs = extract_media_refs(content);
        let parsed = parser::parse_page(content, "page.md");
        let mut properties = vec![&parsed.properties];
        let mut blocks = parsed.blocks.iter().collect::<Vec<_>>();
        while let Some(block) = blocks.pop() {
            properties.push(&block.properties);
            blocks.extend(&block.children);
        }
        while let Some(value) = properties.pop() {
            match value {
                serde_json::Value::String(raw) => refs.push(raw.clone()),
                serde_json::Value::Array(values) => properties.extend(values),
                serde_json::Value::Object(values) => properties.extend(values.values()),
                _ => {}
            }
        }
        refs.into_iter()
            .filter_map(|raw| self.asset_reference_path(source, &raw))
            .collect()
    }

    fn asset_reference_path(&self, source: &Path, raw: &str) -> Option<String> {
        let root = self.root_dir.canonicalize().ok()?;
        let source = root.join(source.strip_prefix(&self.root_dir).ok()?);
        let raw = raw.split(['?', '#']).next()?.trim();
        let lower = raw.to_ascii_lowercase();
        if [
            "http:",
            "https:",
            "//",
            "data:",
            "mailto:",
            "grafium-asset:",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        {
            return None;
        }
        let raw = raw
            .rsplit_once('|')
            .filter(|(_, size)| {
                !size.is_empty() && size.chars().all(|c| c.is_ascii_digit() || c == 'x')
            })
            .map_or(raw, |(path, _)| path);
        let decoded = crate::import::books::percent_decode_lossy(raw);
        let joined = if raw.starts_with("file:") {
            url::Url::parse(raw).ok()?.to_file_path().ok()?
        } else if ["/assets/", "/pages/", "/journals/", "/books/"]
            .iter()
            .any(|p| decoded.starts_with(p))
        {
            root.join(decoded.trim_start_matches('/'))
        } else if Path::new(&decoded).is_absolute() {
            PathBuf::from(decoded)
        } else {
            source.parent()?.join(decoded)
        };
        let mut normalized = PathBuf::new();
        for component in joined.components() {
            match component {
                std::path::Component::ParentDir => {
                    if !normalized.pop() {
                        return None;
                    }
                }
                std::path::Component::CurDir => {}
                component => normalized.push(component),
            }
        }
        let relative = normalized.strip_prefix(root).ok()?;
        managed_asset(relative).then(|| relative.to_string_lossy().replace('\\', "/"))
    }

    pub(super) fn restore_changed_block_assets(
        &self,
        source: &Path,
        before: &str,
        before_properties: &serde_json::Value,
        after: &str,
        after_properties: &serde_json::Value,
    ) -> Result<()> {
        let paths = |content: &str, properties: &serde_json::Value| {
            let mut paths = self.content_asset_paths(source, content);
            let mut pending = vec![properties];
            while let Some(value) = pending.pop() {
                match value {
                    serde_json::Value::String(raw) => {
                        paths.extend(self.content_asset_paths(source, raw));
                        if let Some(path) = self.asset_reference_path(source, raw) {
                            paths.insert(path);
                        }
                    }
                    serde_json::Value::Array(values) => pending.extend(values),
                    serde_json::Value::Object(values) => pending.extend(values.values()),
                    _ => {}
                }
            }
            paths
        };
        let before = paths(before, before_properties);
        let after = paths(after, after_properties);
        self.restore_asset_paths(after.difference(&before).cloned().collect())
    }

    pub(super) fn restore_changed_content_assets(
        &self,
        source: &Path,
        before: &str,
        after: &str,
    ) -> Result<()> {
        if before == after {
            return Ok(());
        }
        let before = self.content_asset_paths(source, before);
        let after = self.content_asset_paths(source, after);
        let paths = after.difference(&before).cloned().collect::<HashSet<_>>();
        self.restore_asset_paths(paths)
    }

    fn restore_asset_paths(&self, paths: HashSet<String>) -> Result<()> {
        if paths.is_empty() || !self.root_dir.join(TRASH).exists() {
            return Ok(());
        }
        let root = self.root_dir.canonicalize()?;
        let scan = self.list_asset_trash_matching(Some(&paths))?;
        for filename in paths {
            let purged = self.asset_versions(&root, ".purged", &filename)?;
            let selected = self.asset_versions(&root, ".selected", &filename)?;
            let entries: Vec<_> = scan
                .assets
                .iter()
                .filter(|a| a.filename == filename)
                .collect();
            match fs::symlink_metadata(root.join(&filename)) {
                Ok(_) => {
                    if !entries.is_empty() || !purged.is_empty() {
                        let original = checked_path(&root, Path::new(&filename))?;
                        let current = fingerprint(&original)?;
                        if !selected.contains(&current)
                            && (purged.iter().any(|version| version != &current)
                                || entries
                                    .iter()
                                    .any(|entry| current != (entry.size, entry.sha256.clone())))
                        {
                            return Err(error(format!("Cannot restore {filename}: a different file already occupies its path. Existing and trashed bytes were preserved; resolve the conflict before retrying.")));
                        }
                    }
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            if !entries.is_empty() {
                let chosen = entries
                    .iter()
                    .filter(|entry| selected.contains(&(entry.size, entry.sha256.clone())))
                    .copied()
                    .collect::<Vec<_>>();
                let choices = if chosen.is_empty() { &entries } else { &chosen };
                let entry = choices[0];
                let identity = (entry.size, entry.sha256.clone());
                if !selected.contains(&identity)
                    && purged.iter().any(|version| version != &identity)
                {
                    return Err(error(format!("Cannot restore {filename}: a different version was permanently deleted. Choose the intended surviving version with Restore in Settings before undoing.")));
                }
                if choices
                    .iter()
                    .any(|other| other.sha256 != entry.sha256 || other.size != entry.size)
                {
                    return Err(error(format!("Multiple versions of {filename} are in trash. Restore the intended file in Settings before undoing.")));
                }
                for warning in self.restore_trash_entry(&root, entry)? {
                    self.asset_cleanup_warning(warning);
                }
            } else if !purged.is_empty() {
                return Err(error(format!("Cannot restore {filename}: its attachment bytes were permanently deleted. Undo cannot recover this file.")));
            }
        }
        Ok(())
    }

    pub(super) fn trash_removed_content_assets(&self, source: &Path, before: &str, after: &str) {
        if before == after {
            return;
        }
        let before = self.content_asset_paths(source, before);
        if before.is_empty() {
            return;
        }
        let after = self.content_asset_paths(source, after);
        let removed: HashSet<_> = before.difference(&after).cloned().collect();
        if removed.is_empty() {
            return;
        }
        if let Err(e) = self.trash_candidate_assets(&removed) {
            self.asset_cleanup_warning(format!("Your edit was saved, but attachment cleanup needs attention: {e}. Review Settings > Asset Cleanup."));
        }
    }

    pub(super) fn trash_candidate_assets(&self, candidates: &HashSet<String>) -> Result<usize> {
        if candidates.is_empty() {
            return Ok(0);
        }
        let scan = self.scan_unused_assets_matching(Some(candidates))?;
        let requested = scan
            .assets
            .into_iter()
            .filter(|asset| candidates.contains(&asset.filename))
            .collect::<Vec<_>>();
        let result = self.trash_unused_assets(&scan.graph_path, &requested)?;
        if !result.errors.is_empty() {
            self.asset_cleanup_warning(format!("Some attachments could not be moved to trash: {}. Review Settings > Asset Cleanup.", result.errors.join("; ")));
        }
        Ok(result.moved.len())
    }
}

#[cfg(test)]
mod tests;
