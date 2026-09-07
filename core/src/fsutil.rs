//! Filesystem helpers shared by the graph and sync layers.

use crate::error::Result;
use std::fs;
use std::io::Write;
use std::path::Path;
use uuid::Uuid;

/// Write `content` to `path` so that readers only ever observe the complete
/// old or the complete new file.
///
/// The data is written to a temporary file in the same directory, flushed and
/// fsynced, then renamed over the target. This matters most for removable
/// drives: a stick pulled mid-write can otherwise leave a truncated note or a
/// corrupt sync state file behind.
pub fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if !dir.exists() {
        fs::create_dir_all(dir)?;
    }

    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp_path = dir.join(format!(".{}.{}.tmp", file_name, Uuid::new_v4().as_simple()));

    // Scope the handle so it is closed before the rename (required on
    // Windows, harmless elsewhere).
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(content)?;
        file.flush()?;
        // Durability: without this the rename can land before the data.
        file.sync_all()?;
    }

    if let Err(e) = fs::rename(&tmp_path, path) {
        // Never leave scratch files behind in the user's graph.
        let _ = fs::remove_file(&tmp_path);
        return Err(e.into());
    }

    Ok(())
}
