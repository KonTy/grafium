use super::types::{ReaderKind, ReaderResult};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::{File, Metadata};
use std::path::{Component, Path};
use std::sync::{mpsc, Arc, LazyLock, Mutex};
use std::time::{Duration, UNIX_EPOCH};

pub const MAX_EPUB_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fingerprint {
    size: u64,
    modified_ns: u128,
    #[serde(default)]
    identity: Option<FileIdentity>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "platform", rename_all = "camelCase", deny_unknown_fields)]
enum FileIdentity {
    Unix {
        device: u64,
        inode: u64,
        changed_seconds: i64,
        changed_nanos: i64,
    },
    Windows {
        volume: u64,
        id: [u8; 16],
        changed: i64,
    },
}

fn file_identity(file: &File, metadata: &Metadata) -> ReaderResult<FileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = file;
        Ok(FileIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
            changed_seconds: metadata.ctime(),
            changed_nanos: metadata.ctime_nsec(),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        struct BasicInfo {
            creation: i64,
            access: i64,
            write: i64,
            change: i64,
            attributes: u32,
        }
        #[repr(C)]
        struct IdInfo {
            volume: u64,
            id: [u8; 16],
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetFileInformationByHandleEx(
                handle: *mut std::ffi::c_void,
                class: i32,
                info: *mut std::ffi::c_void,
                size: u32,
            ) -> i32;
        }
        let _ = metadata;
        let mut basic = std::mem::MaybeUninit::<BasicInfo>::uninit();
        let mut identity = std::mem::MaybeUninit::<IdInfo>::uninit();
        // SAFETY: both buffers have the exact Windows API layouts and sizes;
        // successful calls initialize them, and the borrowed handle stays open.
        unsafe {
            if GetFileInformationByHandleEx(
                file.as_raw_handle(),
                0,
                basic.as_mut_ptr().cast(),
                std::mem::size_of::<BasicInfo>() as u32,
            ) == 0
                || GetFileInformationByHandleEx(
                    file.as_raw_handle(),
                    18,
                    identity.as_mut_ptr().cast(),
                    std::mem::size_of::<IdInfo>() as u32,
                ) == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let (basic, identity) = (basic.assume_init(), identity.assume_init());
            Ok(FileIdentity::Windows {
                volume: identity.volume,
                id: identity.id,
                changed: basic.change,
            })
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (file, metadata);
        Err("This platform cannot securely identify replacement media".into())
    }
}

impl Fingerprint {
    /// Whether `found` is this registered file. Size and modification time
    /// must always agree. `strict` also requires the same on-disk identity
    /// (device, file ID and change time), which catches a file replaced with
    /// its size and modification time preserved. That identity only means
    /// something while the filesystem stays mounted and keeps file IDs
    /// stable, so remounted drives and shares are compared without it.
    pub fn matches(&self, found: &Fingerprint, strict: bool) -> bool {
        self.size == found.size
            && self.modified_ns == found.modified_ns
            && (!strict || self.identity == found.identity)
    }

    pub fn of(file: &File) -> ReaderResult<Self> {
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            return Err("Source is not a regular file".into());
        }
        Ok(Self {
            size: metadata.len(),
            identity: Some(file_identity(file, &metadata)?),
            modified_ns: metadata
                .modified()
                .map_err(|e| e.to_string())?
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos(),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RootIdentity {
    device: u64,
    inode: u64,
    created_ns: Option<u128>,
    #[serde(default)]
    file_id: Option<[u8; 16]>,
}

impl RootIdentity {
    fn of(file: &File) -> ReaderResult<Self> {
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                created_ns: None,
                file_id: None,
            })
        }
        #[cfg(windows)]
        {
            if let FileIdentity::Windows { volume, id, .. } = file_identity(file, &metadata)? {
                Ok(Self {
                    device: volume,
                    inode: 0,
                    created_ns: None,
                    file_id: Some(id),
                })
            } else {
                Err("Cannot identify library folder".into())
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = metadata;
            Err("This platform cannot securely identify library folders".into())
        }
    }
}

pub fn relative(path: &str) -> ReaderResult<()> {
    if path.is_empty()
        || path.len() > 16_384
        || path.contains('\\')
        || path.contains('\0')
        || path.contains(':')
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err("Source path must be a normalized library-relative path".into());
    }
    Ok(())
}

/// Walk every component with no-follow semantics, including the selected root.
#[cfg(test)]
pub fn root(path: &str) -> ReaderResult<(Dir, RootIdentity)> {
    open_root(path).map_err(|failure| failure.message)
}

struct RootFailure {
    missing: bool,
    message: String,
}

impl RootFailure {
    fn io(error: std::io::Error) -> Self {
        Self {
            missing: error.kind() == std::io::ErrorKind::NotFound,
            message: error.to_string(),
        }
    }

    fn invalid(message: &str) -> Self {
        Self {
            missing: false,
            message: message.into(),
        }
    }
}

fn open_root(path: &str) -> Result<(Dir, RootIdentity), RootFailure> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(RootFailure::invalid("Choose an absolute local library folder"));
    }
    let mut components = path.components();
    let mut base = std::path::PathBuf::new();
    for component in components.by_ref() {
        match component {
            Component::Prefix(_) | Component::RootDir => base.push(component),
            Component::Normal(name) => {
                let mut dir = Dir::open_ambient_dir(&base, cap_std::ambient_authority())
                    .map_err(RootFailure::io)?;
                dir = dir.open_dir_nofollow(name).map_err(RootFailure::io)?;
                for component in components {
                    match component {
                        Component::Normal(name) => {
                            dir = dir.open_dir_nofollow(name).map_err(RootFailure::io)?
                        }
                        _ => {
                            return Err(RootFailure::invalid(
                                "Library folder must not contain traversal components",
                            ))
                        }
                    }
                }
                let file = dir
                    .try_clone()
                    .map_err(RootFailure::io)?
                    .into_std_file();
                let identity = RootIdentity::of(&file).map_err(|message| RootFailure {
                    missing: false,
                    message,
                })?;
                return Ok((dir, identity));
            }
            _ => {
                return Err(RootFailure::invalid(
                    "Library folder must not contain traversal components",
                ))
            }
        }
    }
    Err(RootFailure::invalid(
        "Choose a library folder, not a filesystem root",
    ))
}

/// Long enough for a sleeping USB disk to spin up; short enough that a
/// stalled network share cannot hold the Library indefinitely.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

pub const NOT_CONNECTED: &str = "Not connected";
pub const NOT_RESPONDING: &str = "Not responding";
pub const EMPTY_LOCATION: &str = "Empty; the drive or share may not be mounted";

/// Open a Library location, reporting why it is unreachable instead of
/// waiting forever on a share that stopped answering. While one probe of a
/// location is still stuck in the operating system, further probes of it
/// fail at once rather than piling up more stuck threads.
pub fn probe(path: &str, timeout: Duration) -> Result<(Dir, RootIdentity), String> {
    enum Slot {
        Waiting,
        Done(Result<(Dir, RootIdentity), RootFailure>),
        Abandoned,
    }
    if STALLED
        .lock()
        .map_err(|_| "Library location lock failed")?
        .contains_key(path)
    {
        return Err(NOT_RESPONDING.into());
    }
    let slot = Arc::new(Mutex::new(Slot::Waiting));
    let (sender, receiver) = mpsc::channel();
    let (owned, worker) = (path.to_owned(), slot.clone());
    std::thread::Builder::new()
        .name("library-location-probe".into())
        .spawn(move || {
            let result = open_root(&owned);
            let Ok(mut slot) = worker.lock() else { return };
            if matches!(*slot, Slot::Abandoned) {
                drop(slot);
                count_stalled(&owned, -1);
            } else {
                *slot = Slot::Done(result);
                drop(slot);
                let _ = sender.send(());
            }
        })
        .map_err(|error| error.to_string())?;
    let waited = receiver.recv_timeout(timeout);
    let mut slot = slot.lock().map_err(|_| "Library location lock failed")?;
    match std::mem::replace(&mut *slot, Slot::Abandoned) {
        Slot::Done(Ok(root)) => Ok(root),
        Slot::Done(Err(failure)) if failure.missing => Err(NOT_CONNECTED.into()),
        Slot::Done(Err(failure)) => Err(failure.message),
        Slot::Waiting | Slot::Abandoned
            if waited == Err(mpsc::RecvTimeoutError::Disconnected) =>
        {
            Err("Library location check failed".into())
        }
        Slot::Waiting | Slot::Abandoned => {
            // Counted while the slot is still locked, so the worker cannot
            // finish and uncount it first.
            count_stalled(path, 1);
            Err(NOT_RESPONDING.into())
        }
    }
}

static STALLED: LazyLock<Mutex<HashMap<String, usize>>> = LazyLock::new(Default::default);

fn count_stalled(path: &str, change: isize) {
    if let Ok(mut stalled) = STALLED.lock() {
        let count = stalled.entry(path.to_owned()).or_default();
        *count = count.saturating_add_signed(change);
        if *count == 0 {
            stalled.remove(path);
        }
    }
}

pub fn is_empty(dir: &Dir) -> bool {
    dir.entries().is_ok_and(|mut entries| entries.next().is_none())
}

/// Whether file IDs on this filesystem identify the same file across
/// lookups. FAT and exFAT number files as they are read, and FUSE and SMB
/// mounts may do the same, so only size and time can be compared there.
pub fn stable_file_ids(dir: &Dir) -> bool {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::io::AsRawFd;
        let Ok(file) = dir.try_clone().map(|dir| dir.into_std_file()) else {
            return false;
        };
        let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: the descriptor stays open for the call, and the buffer has
        // the layout fstatfs fills in; it is read only after success.
        if unsafe { libc::fstatfs(file.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
            return false;
        }
        #[allow(clippy::unnecessary_cast)]
        let kind = unsafe { stats.assume_init() }.f_type as u64 as u32;
        !matches!(
            kind,
            0x4d44 | 0x2011_bab0 | 0x6573_5546 | 0xff53_4d42 | 0xfe53_4d42 | 0x517b
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = dir;
        true
    }
}

pub fn open(dir: &Dir, path: &str) -> ReaderResult<File> {
    relative(path)?;
    let mut dir = dir.try_clone().map_err(|e| e.to_string())?;
    let parts: Vec<_> = path.split('/').collect();
    for part in &parts[..parts.len() - 1] {
        dir = dir.open_dir_nofollow(part).map_err(|e| e.to_string())?;
    }
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        // A file replaced by a FIFO must fail its metadata check, not block an
        // IO worker (and the checkpoint mutex) waiting for a writer.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = dir
        .open_with(parts[parts.len() - 1], &options)
        .map_err(|e| e.to_string())?
        .into_std();
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Source is not a regular file".into());
    }
    Ok(file)
}

#[derive(Clone, Debug)]
pub struct Discovered {
    pub key: String,
    pub title: String,
    pub epub: bool,
    pub kind: ReaderKind,
    pub files: Vec<(String, Fingerprint)>,
}

pub fn media_kind(path: &str) -> Option<ReaderKind> {
    match Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "epub" => Some(ReaderKind::Epub),
        "mp3" | "m4a" | "m4b" | "aac" | "ogg" | "opus" | "flac" | "wav" => Some(ReaderKind::Audio),
        "mp4" | "m4v" | "webm" | "ogv" | "mov" | "mkv" => Some(ReaderKind::Video),
        _ => None,
    }
}

pub fn media_mime(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mp3" => "audio/mpeg",
        "m4a" | "m4b" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        _ => "application/octet-stream",
    }
}

fn title(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

pub fn discover(dir: &Dir) -> ReaderResult<Vec<Discovered>> {
    let mut paths = Vec::new();
    let mut count = 0;
    walk(dir, "", 0, &mut count, &mut paths)?;
    paths.sort_by(|a, b| natural_cmp(&a.0, &b.0));
    let mut books = Vec::<Discovered>::new();
    let mut audio_groups = HashMap::<String, usize>::new();
    for (path, fingerprint) in paths {
        let kind = media_kind(&path).ok_or("Unsupported discovered media")?;
        let epub = kind == ReaderKind::Epub;
        let key = if kind != ReaderKind::Audio || !path.contains('/') {
            path.clone()
        } else {
            path.split('/').next().unwrap().to_owned()
        };
        if kind == ReaderKind::Audio {
            if let Some(index) = audio_groups.get(&key) {
                books[*index].files.push((path, fingerprint));
                continue;
            }
            audio_groups.insert(key.clone(), books.len());
        }
        books.push(Discovered {
            title: title(&key),
            key,
            epub,
            kind,
            files: vec![(path, fingerprint)],
        });
    }
    books.sort_by(|a, b| natural_cmp(&a.key, &b.key));
    Ok(books)
}

fn walk(
    dir: &Dir,
    prefix: &str,
    depth: usize,
    count: &mut usize,
    out: &mut Vec<(String, Fingerprint)>,
) -> ReaderResult<()> {
    if depth > MAX_DEPTH {
        return Err("Library exceeds the 32-folder nesting limit".into());
    }
    for entry in dir.entries().map_err(|e| e.to_string())? {
        *count += 1;
        if *count > MAX_ENTRIES {
            return Err("Library exceeds the 100,000-entry scan limit".into());
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "Library filenames must be valid Unicode")?;
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        relative(&path)?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            let child = dir.open_dir_nofollow(&name).map_err(|e| e.to_string())?;
            walk(&child, &path, depth + 1, count, out)?;
        } else if kind.is_file() && media_kind(&name).is_some() {
            let file = open(dir, &name)?;
            out.push((path, Fingerprint::of(&file)?));
        }
    }
    Ok(())
}

pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (start_i, start_j) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let mut ai = start_i;
            let mut bj = start_j;
            while ai < i && a[ai] == b'0' {
                ai += 1;
            }
            while bj < j && b[bj] == b'0' {
                bj += 1;
            }
            let order = (i - ai)
                .cmp(&(j - bj))
                .then_with(|| a[ai..i].cmp(&b[bj..j]))
                .then_with(|| (i - start_i).cmp(&(j - start_j)));
            if order != Ordering::Equal {
                return order;
            }
        } else {
            let order = a[i].cmp(&b[j]);
            if order != Ordering::Equal {
                return order;
            }
            i += 1;
            j += 1;
        }
    }
    a.len().cmp(&b.len())
}
