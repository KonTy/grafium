//! Replacements retain the bytes actually displaced, not a pre-race snapshot.

use super::*;

/// On success `recovery` owns the exact old destination. Never delete it until
/// the displaced bytes have been checked and the database transaction commits.
pub(super) fn replace_preserving_displaced(path: &Path, pending: &Path) -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let target = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| CoreError::Other("Invalid file path".into()))?;
        let stage = std::ffi::CString::new(pending.as_os_str().as_bytes())
            .map_err(|_| CoreError::Other("Invalid file path".into()))?;
        // Exchange is one atomic operation: an external writer that wins the
        // race is moved into pending, rather than unlinked by ordinary rename.
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                target.as_ptr(),
                libc::AT_FDCWD,
                stage.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        return Ok(pending.to_path_buf());
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let target = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| CoreError::Other("Invalid file path".into()))?;
        let stage = std::ffi::CString::new(pending.as_os_str().as_bytes())
            .map_err(|_| CoreError::Other("Invalid file path".into()))?;
        let result =
            unsafe { libc::renamex_np(target.as_ptr(), stage.as_ptr(), libc::RENAME_SWAP) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        return Ok(pending.to_path_buf());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn ReplaceFileW(
                replaced: *const u16,
                replacement: *const u16,
                backup: *const u16,
                flags: u32,
                exclude: *mut std::ffi::c_void,
                reserved: *mut std::ffi::c_void,
            ) -> i32;
        }
        let recovery = pending.with_extension("displaced");
        let wide = |path: &Path| {
            path.as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        let target = wide(path);
        let stage = wide(pending);
        let backup = wide(&recovery);
        let result = unsafe {
            ReplaceFileW(
                target.as_ptr(),
                stage.as_ptr(),
                backup.as_ptr(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if result == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        return Ok(recovery);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    Err(CoreError::Other(format!(
        "Safe annotation replacement is unsupported on this platform; original at {}, draft at {}",
        path.display(),
        pending.display()
    )))
}
