//! Persistent, fail-closed GPU crash recovery, independent of model execution.
//!
//! The host supplies a dedicated, private directory in application state, never
//! graph data or a synchronized folder. Hold an allowed lease for the entire
//! native worker lifetime. Only confirm after observing that process exit;
//! dropping a lease deliberately leaves its pending record behind.
//!
//! All instances must use the same directory on a filesystem supporting advisory
//! locks and atomic replacement. Interrupted writes or damaged state require
//! operator attention: neither opening the store nor authorizing a retry repairs
//! them. Records and lock files are retained, with room for 256 model identities.

use std::collections::BTreeSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fs2::FileExt;
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{Result, RuntimeError};

const VERSION: u32 = 1;
const MAX_RECORDS: usize = 256;
const MAX_KEY_BYTES: usize = 1024;
const MAX_LABEL_BYTES: usize = 256;
const MAX_JOURNAL_BYTES: u64 = 1024 * 1024;
const MAX_FILES: usize = MAX_RECORDS + 3;
const MAX_STORE_BYTES: u64 = MAX_JOURNAL_BYTES * 2 + (MAX_RECORDS as u64 + 1) * 64;
const JOURNAL: &str = "journal.json";
const NEXT: &str = "journal.next";
const STORE_LOCK: &str = "store.lock";
const LOCK_MAGIC: &[u8] = b"model-runtime-gpu-recovery-v1\n";
const ACTIVE_REASON: &str = "A GPU worker for this model/backend is still active in this or another application instance; concurrent GPU attempts are disabled.";

fn mutation_lock_bounded(file: &File) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        match FileExt::try_lock_exclusive(file) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock
                || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
            {
                if std::time::Instant::now() >= deadline {
                    return Err(RuntimeError::Other("GPU recovery journal is busy; refusing an unrecorded GPU attempt".into()));
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecoveryStore {
    directory: PathBuf,
}

#[derive(Debug)]
pub enum GpuAttempt {
    Allowed(RecoveryLease),
    CpuOnly { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedModel {
    pub key: String,
    pub label: String,
    pub reason: String,
}

/// Not cloneable: there must be one owner of a worker's recovery lifetime.
#[derive(Debug)]
pub struct RecoveryLease {
    store: RecoveryStore,
    key: String,
    attempt: String,
    lock: Mutex<Option<File>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    #[serde(deserialize_with = "bounded_records")]
    records: Vec<Record>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    key: String,
    label: String,
    state: State,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum State {
    Ready,
    Blocked {
        failure: Failure,
        authorized: bool,
    },
    Pending {
        attempt: String,
        #[serde(deserialize_with = "Deserialize::deserialize")]
        prior_failure: Option<Failure>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Failure {
    UnconfirmedExit,
    UnexpectedExit,
}

impl Failure {
    fn reason(self) -> &'static str {
        match self {
            Self::UnconfirmedExit => "The previous GPU worker has no confirmed exit (the application or worker may have crashed). GPU is disabled until an explicit one-shot retry.",
            Self::UnexpectedExit => "The previous GPU worker exited unexpectedly or was terminated after a timeout or protocol failure. GPU is disabled until an explicit one-shot retry.",
        }
    }
}

impl RecoveryStore {
    /// Opens or initializes a dedicated directory, preserving all existing data.
    /// Missing, unrecognized, partial, or oversized state is an error, not a reset.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let directory = create_directory(path.as_ref())?;
        let store = Self { directory };
        let lock_path = store.directory.join(STORE_LOCK);
        let (mut lock, created) = match fs::symlink_metadata(&lock_path) {
            Ok(_) => (open_regular(&lock_path, true)?, false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !store.scan()?.is_empty() {
                    return Err(invalid("store lock is missing from a nonempty directory"));
                }
                match new_file(&lock_path) {
                    Ok(file) => (file, true),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        (open_regular(&lock_path, true)?, false)
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        };
        mutation_lock_bounded(&lock)?;
        if created {
            let files = store.scan()?;
            if files.len() != 1 || !files.contains(STORE_LOCK) {
                return Err(invalid("unexpected files during initialization"));
            }
            store.save(&Journal {
                version: VERSION,
                records: Vec::new(),
            })?;
            lock.write_all(LOCK_MAGIC)?;
            lock.sync_all()?;
            sync_directory(&store.directory)?;
        } else {
            check_lock(&lock)?;
            store.load()?;
        }
        Ok(store)
    }

    /// Durably records an attempt *before* the caller initializes or spawns GPU
    /// work. Keys are opaque, bounded identities, never filesystem paths.
    /// Labels must be short, non-sensitive basenames or workload descriptions.
    pub fn prepare(&self, key: &str, label: &str) -> Result<GpuAttempt> {
        validate_text(key, MAX_KEY_BYTES, "key")?;
        validate_text(label, MAX_LABEL_BYTES, "label")?;
        let _mutation = self.mutation_lock()?;
        let mut journal = self.load()?;
        let index = match journal.records.iter().position(|record| record.key == key) {
            Some(index) => index,
            None => {
                if journal.records.len() == MAX_RECORDS {
                    return Err(invalid(
                        "model identity limit reached; no records were evicted",
                    ));
                }
                let lock = new_file(&self.directory.join(lock_name(key)))?;
                lock.sync_all()?;
                sync_directory(&self.directory)?;
                journal.records.push(Record {
                    key: key.into(),
                    label: label.into(),
                    state: State::Ready,
                });
                journal.records.len() - 1
            }
        };
        let record = &mut journal.records[index];
        let Some(lock) = self.attempt_lock(key)? else {
            return Ok(GpuAttempt::CpuOnly {
                reason: ACTIVE_REASON.into(),
            });
        };
        if matches!(record.state, State::Pending { .. }) {
            record.state = State::Blocked {
                failure: Failure::UnconfirmedExit,
                authorized: false,
            };
            self.save(&journal)?;
            return Ok(GpuAttempt::CpuOnly {
                reason: Failure::UnconfirmedExit.reason().into(),
            });
        }
        let prior_failure = match record.state {
            State::Blocked {
                failure,
                authorized: false,
            } => {
                return Ok(GpuAttempt::CpuOnly {
                    reason: failure.reason().into(),
                });
            }
            State::Blocked {
                failure,
                authorized: true,
            } => Some(failure),
            State::Ready => None,
            State::Pending { .. } => unreachable!(),
        };
        let attempt = Uuid::new_v4().to_string();
        record.label = label.into();
        record.state = State::Pending {
            attempt: attempt.clone(),
            prior_failure,
        };
        // Authorization consumption and the pending marker are one atomic commit.
        self.save(&journal)?;
        Ok(GpuAttempt::Allowed(RecoveryLease {
            store: self.clone(),
            key: key.into(),
            attempt,
            lock: Mutex::new(Some(lock)),
        }))
    }

    /// Includes quarantines with an unused authorization or an active retry.
    /// A fresh, live worker is not classified as crashed. Stale attempts are
    /// quarantined durably, without aging out any denied record.
    pub fn blocked(&self) -> Result<Vec<BlockedModel>> {
        let _mutation = self.mutation_lock()?;
        let mut journal = self.load()?;
        let mut changed = false;
        let mut blocked = Vec::new();
        for record in &mut journal.records {
            if matches!(record.state, State::Pending { .. })
                && self.attempt_lock(&record.key)?.is_some()
            {
                record.state = State::Blocked {
                    failure: Failure::UnconfirmedExit,
                    authorized: false,
                };
                changed = true;
            }
            let failure = match record.state {
                State::Blocked { failure, .. } => Some(failure),
                State::Pending { prior_failure, .. } => prior_failure,
                State::Ready => None,
            };
            if let Some(failure) = failure {
                blocked.push(BlockedModel {
                    key: record.key.clone(),
                    label: record.label.clone(),
                    reason: failure.reason().into(),
                });
            }
        }
        if changed {
            self.save(&journal)?;
        }
        Ok(blocked)
    }

    /// Authorizes exactly one future preparation for a quarantined key.
    /// Repeated calls do not accumulate credits or clear its diagnostics.
    /// An active worker, an unknown key, or a healthy key is an error.
    pub fn allow_once(&self, key: &str) -> Result<()> {
        validate_text(key, MAX_KEY_BYTES, "key")?;
        let _mutation = self.mutation_lock()?;
        let mut journal = self.load()?;
        let record = journal
            .records
            .iter_mut()
            .find(|record| record.key == key)
            .ok_or_else(|| invalid("cannot authorize an unknown model identity"))?;
        let _attempt = self
            .attempt_lock(key)?
            .ok_or_else(|| invalid(ACTIVE_REASON))?;
        let failure = match record.state {
            State::Pending { .. } => Failure::UnconfirmedExit,
            State::Blocked { failure, .. } => failure,
            State::Ready => return Err(invalid("model identity is not quarantined")),
        };
        record.state = State::Blocked {
            failure,
            authorized: true,
        };
        self.save(&journal)
    }

    fn mutation_lock(&self) -> Result<File> {
        let lock = open_regular(&self.directory.join(STORE_LOCK), true)?;
        mutation_lock_bounded(&lock)?;
        check_lock(&lock)?;
        Ok(lock)
    }

    fn attempt_lock(&self, key: &str) -> Result<Option<File>> {
        let lock = open_regular(&self.directory.join(lock_name(key)), true)?;
        check_attempt_lock(&lock)?;
        match FileExt::try_lock_exclusive(&lock) {
            Ok(()) => Ok(Some(lock)),
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
            {
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn scan(&self) -> Result<BTreeSet<String>> {
        let mut names = BTreeSet::new();
        let mut total_bytes = 0;
        for (index, entry) in fs::read_dir(&self.directory)?.enumerate() {
            if index >= MAX_FILES {
                return Err(invalid("directory entry limit exceeded"));
            }
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("unrecognized non-UTF-8 filename"))?;
            let limit = match name.as_str() {
                JOURNAL => MAX_JOURNAL_BYTES,
                NEXT => {
                    return Err(invalid(
                        "interrupted journal write; journal.next was preserved",
                    ))
                }
                STORE_LOCK => LOCK_MAGIC.len() as u64,
                _ if is_model_lock(&name) => 0,
                _ => return Err(invalid("unrecognized directory entry; nothing was removed")),
            };
            let metadata = fs::symlink_metadata(entry.path())?;
            check_regular(&metadata)?;
            if metadata.len() > limit {
                return Err(invalid("file size limit exceeded"));
            }
            total_bytes += metadata.len();
            if total_bytes > MAX_STORE_BYTES {
                return Err(invalid("total state size limit exceeded"));
            }
            names.insert(name);
        }
        Ok(names)
    }

    fn load(&self) -> Result<Journal> {
        let files = self.scan()?;
        if !files.contains(JOURNAL) || !files.contains(STORE_LOCK) {
            return Err(invalid(
                "incomplete store; required journal or lock is missing",
            ));
        }
        let bytes = read_bounded(&self.directory.join(JOURNAL), MAX_JOURNAL_BYTES)?;
        let journal: Journal = serde_json::from_slice(&bytes)
            .map_err(|error| invalid(&format!("malformed journal: {error}")))?;
        if journal.version != VERSION {
            return Err(invalid("unsupported journal version"));
        }
        let mut expected = BTreeSet::from([STORE_LOCK.to_owned(), JOURNAL.to_owned()]);
        for record in &journal.records {
            validate_text(&record.key, MAX_KEY_BYTES, "stored key")?;
            validate_text(&record.label, MAX_LABEL_BYTES, "stored label")?;
            if let State::Pending { attempt, .. } = &record.state {
                if Uuid::parse_str(attempt)
                    .map(|id| id.to_string() != *attempt || id.get_version_num() != 4)
                    .unwrap_or(true)
                {
                    return Err(invalid("invalid pending attempt identity"));
                }
            }
            let name = lock_name(&record.key);
            if !expected.insert(name.clone()) {
                return Err(invalid("duplicate model identity or filename hash"));
            }
            check_attempt_lock(&open_regular(&self.directory.join(name), false)?)?;
        }
        if files != expected {
            return Err(invalid(
                "journal and lock files disagree; diagnostics were preserved",
            ));
        }
        Ok(journal)
    }

    fn save(&self, journal: &Journal) -> Result<()> {
        let bytes = serde_json::to_vec(journal)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(invalid("serialized journal exceeds the size limit"));
        }
        self.publish(|file| {
            file.write_all(&bytes)?;
            file.sync_all()
        })
    }

    fn publish(&self, write_and_sync: impl FnOnce(&mut File) -> io::Result<()>) -> Result<()> {
        let next = self.directory.join(NEXT);
        let mut file = new_file(&next)?;
        // Never truncate the old journal or delete a failed write's diagnostics.
        write_and_sync(&mut file)?;
        drop(file);
        fs::rename(next, self.directory.join(JOURNAL))?;
        sync_directory(&self.directory)
    }
}

impl RecoveryLease {
    /// Call only after the native process has actually exited. `expected` is true
    /// for normal shutdown, idle eviction, or intentional cancellation; false
    /// for native crashes, timeouts, or protocol failures. The first successful
    /// confirmation wins; later calls cannot clear a newer attempt.
    pub fn confirm_exit(&self, expected: bool) -> Result<()> {
        let mut lock = self
            .lock
            .lock()
            .map_err(|_| invalid("lease confirmation mutex was poisoned"))?;
        if lock.is_none() {
            return Ok(());
        }
        let _mutation = self.store.mutation_lock()?;
        let mut journal = self.store.load()?;
        let record = journal
            .records
            .iter_mut()
            .find(|record| record.key == self.key)
            .ok_or_else(|| invalid("pending model identity is missing"))?;
        if !matches!(&record.state, State::Pending { attempt, .. } if attempt == &self.attempt) {
            return Err(invalid(
                "pending attempt identity changed; refusing to overwrite it",
            ));
        }
        record.state = if expected {
            State::Ready
        } else {
            State::Blocked {
                failure: Failure::UnexpectedExit,
                authorized: false,
            }
        };
        self.store.save(&journal)?;
        // Release while still holding the store lock, after the durable commit.
        *lock = None;
        Ok(())
    }
}

fn invalid(reason: &str) -> RuntimeError {
    RuntimeError::Other(format!("GPU recovery state is unavailable: {reason}"))
}

fn validate_text(text: &str, max_bytes: usize, name: &str) -> Result<()> {
    if text.is_empty() || text.len() > max_bytes || text.chars().any(char::is_control) {
        return Err(invalid(&format!(
            "{name} must contain 1..={max_bytes} UTF-8 bytes without control characters"
        )));
    }
    Ok(())
}

fn lock_name(key: &str) -> String {
    format!("{:x}.lock", Sha256::digest(key.as_bytes()))
}

fn is_model_lock(name: &str) -> bool {
    name.len() == 69
        && name.ends_with(".lock")
        && name.as_bytes()[..64]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn bounded_records<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<Record>, D::Error> {
    struct Records;
    impl<'de> Visitor<'de> for Records {
        type Value = Vec<Record>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "at most {MAX_RECORDS} model records")
        }

        fn visit_seq<A: SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut records = Vec::new();
            while let Some(record) = sequence.next_element()? {
                if records.len() == MAX_RECORDS {
                    return Err(de::Error::custom("model record limit exceeded"));
                }
                records.push(record);
            }
            Ok(records)
        }
    }
    deserializer.deserialize_seq(Records)
}

fn check_regular(metadata: &Metadata) -> Result<()> {
    if !metadata.file_type().is_file() {
        return Err(invalid(
            "state entry is not a regular file (links are not allowed)",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(invalid("hard-linked state file is not allowed"));
        }
    }
    Ok(())
}

fn open_regular(path: &Path, writable: bool) -> Result<File> {
    check_regular(&fs::symlink_metadata(path)?)?;
    let mut options = OpenOptions::new();
    options.read(true).write(writable);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    check_regular(&file.metadata()?)?;
    Ok(file)
}

fn new_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = open_regular(path, false)?;
    if file.metadata()?.len() > limit {
        return Err(invalid("file size limit exceeded"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("file grew beyond the size limit"));
    }
    Ok(bytes)
}

fn check_lock(mut file: &File) -> Result<()> {
    if file.metadata()?.len() != LOCK_MAGIC.len() as u64 {
        return Err(invalid("incomplete or damaged lock file"));
    }
    let mut bytes = [0; LOCK_MAGIC.len()];
    file.read_exact(&mut bytes)?;
    if bytes != LOCK_MAGIC {
        return Err(invalid("invalid lock file header"));
    }
    Ok(())
}

fn check_attempt_lock(file: &File) -> Result<()> {
    // Empty files let us validate live locks without reading a byte range locked
    // by another Windows process. Their inodes must never be replaced or removed.
    if file.metadata()?.len() != 0 {
        return Err(invalid("damaged attempt lock file"));
    }
    Ok(())
}

fn create_directory(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if path.components().count() > 128 {
        return Err(invalid("application-state directory path is too deep"));
    }
    let mut missing = Vec::new();
    let mut ancestor = path.as_path();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if !metadata.file_type().is_dir() {
                    return Err(invalid(
                        "application-state path is not a directory or is a link",
                    ));
                }
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(ancestor);
                ancestor = ancestor.parent().ok_or_else(|| {
                    invalid("application-state directory has no existing ancestor")
                })?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    for directory in missing.into_iter().rev() {
        match fs::create_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if !fs::symlink_metadata(directory)?.file_type().is_dir() {
                    return Err(invalid(
                        "application-state directory was replaced by a link or file",
                    ));
                }
            }
            Err(error) => return Err(error.into()),
        }
        if let Some(parent) = directory.parent() {
            sync_directory(parent)?;
        }
    }
    Ok(fs::canonicalize(path)?)
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        match File::open(path)?.sync_all() {
            Ok(()) => {}
            // Some filesystems do not implement directory fsync.
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::EINVAL) | Some(libc::ENOTSUP)
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    // std does not support opening directories for fsync on Windows.
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contended_journal_lock_returns_instead_of_hanging_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        let owner = File::create(&path).unwrap();
        FileExt::lock_exclusive(&owner).unwrap();
        let contender = OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let start = std::time::Instant::now();
        assert!(mutation_lock_bounded(&contender).unwrap_err().to_string().contains("busy"));
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
    }
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    fn fixture() -> (TempDir, RecoveryStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = RecoveryStore::open(directory.path().join("recovery")).unwrap();
        (directory, store)
    }

    fn allowed(store: &RecoveryStore, key: &str) -> RecoveryLease {
        match store.prepare(key, "model.gguf").unwrap() {
            GpuAttempt::Allowed(lease) => lease,
            other => panic!("expected GPU permission, got {other:?}"),
        }
    }

    fn denied(store: &RecoveryStore, key: &str) -> String {
        match store.prepare(key, "model.gguf").unwrap() {
            GpuAttempt::CpuOnly { reason } => reason,
            other => panic!("expected CPU fallback, got {other:?}"),
        }
    }

    fn journal_bytes(store: &RecoveryStore) -> Vec<u8> {
        fs::read(store.directory.join(JOURNAL)).unwrap()
    }

    fn write_fixture(store: &RecoveryStore, journal: &Journal) {
        fs::write(
            store.directory.join(JOURNAL),
            serde_json::to_vec(journal).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn fresh_attempt_is_durable_and_normal_exit_allows_another() {
        let (_directory, store) = fixture();
        let lease = allowed(&store, "model/backend");
        let snapshot: Journal = serde_json::from_slice(&journal_bytes(&store)).unwrap();
        assert!(
            matches!(&snapshot.records[0].state, State::Pending { attempt, .. } if attempt == &lease.attempt)
        );
        assert!(store.blocked().unwrap().is_empty());
        lease.confirm_exit(true).unwrap();
        let reopened = RecoveryStore::open(&store.directory).unwrap();
        assert!(reopened.blocked().unwrap().is_empty());
        allowed(&reopened, "model/backend")
            .confirm_exit(true)
            .unwrap();
    }

    #[test]
    fn drop_leaves_marker_and_reopen_quarantines_without_aging_it_out() {
        let (_directory, store) = fixture();
        let lease = allowed(&store, "model/backend");
        let pending = journal_bytes(&store);
        drop(lease);
        assert_eq!(journal_bytes(&store), pending);
        for _ in 0..3 {
            let reopened = RecoveryStore::open(&store.directory).unwrap();
            assert!(denied(&reopened, "model/backend").contains("no confirmed exit"));
            let blocked = reopened.blocked().unwrap();
            assert_eq!(blocked.len(), 1);
            let value = serde_json::to_value(&blocked[0]).unwrap();
            assert_eq!(value["key"], "model/backend");
            assert_eq!(value["label"], "model.gguf");
            assert!(value["reason"].as_str().unwrap().contains("one-shot retry"));
        }
    }

    #[test]
    fn blocked_discovers_stale_attempts_but_not_active_attempts() {
        let (_directory, store) = fixture();
        let active = allowed(&store, "active");
        drop(allowed(&store, "stale"));
        let reopened = RecoveryStore::open(&store.directory).unwrap();
        let blocked = reopened.blocked().unwrap();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].key, "stale");
        assert!(denied(&reopened, "active").contains("still active"));
        active.confirm_exit(true).unwrap();
    }

    #[test]
    fn authorization_is_one_shot_and_only_success_clears_quarantine() {
        let (_directory, store) = fixture();
        allowed(&store, "key").confirm_exit(false).unwrap();
        assert!(denied(&store, "key").contains("exited unexpectedly"));
        store.allow_once("key").unwrap();
        store.allow_once("key").unwrap();
        assert_eq!(store.blocked().unwrap().len(), 1);
        let reopened = RecoveryStore::open(&store.directory).unwrap();
        let trial = allowed(&reopened, "key");
        assert_eq!(store.blocked().unwrap().len(), 1);
        assert!(denied(&store, "key").contains("still active"));
        assert!(store.allow_once("key").is_err());
        trial.confirm_exit(false).unwrap();
        assert!(denied(&store, "key").contains("exited unexpectedly"));
        store.allow_once("key").unwrap();
        drop(allowed(&store, "key"));
        assert!(denied(&reopened, "key").contains("no confirmed exit"));
        store.allow_once("key").unwrap();
        allowed(&store, "key").confirm_exit(true).unwrap();
        assert!(reopened.blocked().unwrap().is_empty());
        allowed(&reopened, "key").confirm_exit(true).unwrap();
    }

    #[test]
    fn authorization_can_observe_a_stale_attempt_directly() {
        let (_directory, store) = fixture();
        drop(allowed(&store, "key"));
        store.allow_once("key").unwrap();
        allowed(&store, "key").confirm_exit(true).unwrap();
        assert!(store.allow_once("key").is_err());
        assert!(store.allow_once("unknown").is_err());
    }

    #[test]
    fn two_instances_serialize_fresh_and_authorized_preparations() {
        for retry in [false, true] {
            let (_directory, store) = fixture();
            if retry {
                allowed(&store, "same").confirm_exit(false).unwrap();
                store.allow_once("same").unwrap();
            }
            let other = RecoveryStore::open(&store.directory).unwrap();
            let barrier = Arc::new(Barrier::new(2));
            let results = std::thread::scope(|scope| {
                let first = scope.spawn(|| {
                    barrier.wait();
                    store.prepare("same", "first")
                });
                let second = scope.spawn(|| {
                    barrier.wait();
                    other.prepare("same", "second")
                });
                [
                    first.join().unwrap().unwrap(),
                    second.join().unwrap().unwrap(),
                ]
            });
            assert_eq!(
                results
                    .iter()
                    .filter(|result| matches!(result, GpuAttempt::Allowed(_)))
                    .count(),
                1
            );
            assert_eq!(
                results
                    .iter()
                    .filter(|result| matches!(result, GpuAttempt::CpuOnly { .. }))
                    .count(),
                1
            );
            assert_eq!(store.blocked().unwrap().len(), usize::from(retry));
            allowed(&other, "different").confirm_exit(true).unwrap();
            for result in results {
                if let GpuAttempt::Allowed(lease) = result {
                    lease.confirm_exit(true).unwrap();
                }
            }
            assert!(store.blocked().unwrap().is_empty());
        }
    }

    #[test]
    fn concurrent_instances_cannot_consume_the_same_retry_twice() {
        let (_directory, store) = fixture();
        allowed(&store, "key").confirm_exit(false).unwrap();
        store.allow_once("key").unwrap();
        let other = RecoveryStore::open(&store.directory).unwrap();
        let trial = allowed(&store, "key");
        assert!(denied(&other, "key").contains("still active"));
        drop(trial);
        assert!(denied(&other, "key").contains("no confirmed exit"));
        assert!(denied(&store, "key").contains("one-shot retry"));
    }

    #[test]
    fn repeated_confirmation_cannot_change_a_result_or_a_newer_attempt() {
        let (_directory, store) = fixture();
        let old = allowed(&store, "key");
        old.confirm_exit(false).unwrap();
        old.confirm_exit(true).unwrap();
        assert_eq!(store.blocked().unwrap().len(), 1);
        store.allow_once("key").unwrap();
        let current = allowed(&store, "key");
        let pending = journal_bytes(&store);
        old.confirm_exit(true).unwrap();
        old.confirm_exit(false).unwrap();
        assert_eq!(journal_bytes(&store), pending);
        current.confirm_exit(true).unwrap();
        current.confirm_exit(true).unwrap();
        drop(old);
        assert!(store.blocked().unwrap().is_empty());
    }

    #[test]
    fn mismatched_confirmation_preserves_the_newer_marker() {
        let (_directory, store) = fixture();
        let lease = allowed(&store, "key");
        let mut snapshot = store.load().unwrap();
        if let State::Pending { attempt, .. } = &mut snapshot.records[0].state {
            *attempt = Uuid::new_v4().to_string();
        }
        write_fixture(&store, &snapshot);
        let pending = journal_bytes(&store);
        assert!(lease.confirm_exit(true).is_err());
        assert_eq!(journal_bytes(&store), pending);
    }

    #[test]
    fn malformed_truncated_and_unknown_journal_fields_fail_closed() {
        for broken in [
            b"".as_slice(),
            b"{",
            br#"{"version":1,"records":[]"#,
            br#"{"version":99,"records":[]}"#,
            br#"{"version":1,"records":[],"extra":true}"#,
            br#"{"version":1,"version":1,"records":[]}"#,
        ] {
            let (_directory, store) = fixture();
            drop(allowed(&store, "key"));
            fs::write(store.directory.join(JOURNAL), broken).unwrap();
            assert!(RecoveryStore::open(&store.directory).is_err());
            assert!(store.prepare("key", "label").is_err());
            assert!(store.blocked().is_err());
            assert!(store.allow_once("key").is_err());
            assert_eq!(journal_bytes(&store), broken);
        }
    }

    #[test]
    fn missing_journal_or_lock_does_not_initialize_a_fresh_store() {
        for name in [JOURNAL, STORE_LOCK] {
            let (_directory, store) = fixture();
            drop(allowed(&store, "key"));
            fs::remove_file(store.directory.join(name)).unwrap();
            assert!(RecoveryStore::open(&store.directory).is_err());
            assert!(RecoveryStore::open(&store.directory).is_err());
            assert!(!store.directory.join(name).exists());
        }
    }

    #[test]
    fn duplicate_records_and_invalid_attempt_identifiers_fail_closed() {
        let (_directory, store) = fixture();
        let lease = allowed(&store, "key");
        let snapshot = journal_bytes(&store);
        let mut value: serde_json::Value = serde_json::from_slice(&snapshot).unwrap();
        let duplicate = value["records"][0].clone();
        value["records"].as_array_mut().unwrap().push(duplicate);
        fs::write(
            store.directory.join(JOURNAL),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert!(store.blocked().is_err());
        let mut value: serde_json::Value = serde_json::from_slice(&snapshot).unwrap();
        value["records"][0]["state"]["attempt"] = "../unsafe".into();
        fs::write(
            store.directory.join(JOURNAL),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert!(lease.confirm_exit(true).is_err());
        assert!(store.prepare("key", "label").is_err());
    }

    #[test]
    fn incomplete_pending_fields_cannot_hide_an_existing_quarantine() {
        let (_directory, store) = fixture();
        allowed(&store, "key").confirm_exit(false).unwrap();
        store.allow_once("key").unwrap();
        let trial = allowed(&store, "key");
        let snapshot = journal_bytes(&store);
        for field in ["attempt", "prior_failure"] {
            let mut value: serde_json::Value = serde_json::from_slice(&snapshot).unwrap();
            value["records"][0]["state"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            let damaged = serde_json::to_vec(&value).unwrap();
            fs::write(store.directory.join(JOURNAL), &damaged).unwrap();
            assert!(RecoveryStore::open(&store.directory).is_err());
            assert!(store.blocked().is_err());
            assert!(trial.confirm_exit(true).is_err());
            assert_eq!(journal_bytes(&store), damaged);
        }
    }

    #[test]
    fn partial_initialization_and_missing_model_locks_are_not_repaired() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(STORE_LOCK), []).unwrap();
        assert!(RecoveryStore::open(directory.path()).is_err());
        assert!(!directory.path().join(JOURNAL).exists());

        let (_directory, store) = fixture();
        drop(allowed(&store, "key"));
        let pending = journal_bytes(&store);
        let lock = store.directory.join(lock_name("key"));
        fs::remove_file(&lock).unwrap();
        assert!(RecoveryStore::open(&store.directory).is_err());
        assert!(store.prepare("key", "model").is_err());
        assert!(store.prepare("unrelated", "model").is_err());
        assert!(!lock.exists());
        assert_eq!(journal_bytes(&store), pending);
    }

    #[test]
    fn oversized_input_is_rejected_before_state_changes_and_keys_are_hashed() {
        let (directory, store) = fixture();
        let before = journal_bytes(&store);
        for (key, label) in [
            ("".to_owned(), "label".to_owned()),
            ("key".to_owned(), "".to_owned()),
            ("k".repeat(MAX_KEY_BYTES + 1), "label".to_owned()),
            ("key".to_owned(), "l".repeat(MAX_LABEL_BYTES + 1)),
            ("key\n".to_owned(), "label".to_owned()),
            ("key".to_owned(), "label\0".to_owned()),
        ] {
            assert!(store.prepare(&key, &label).is_err());
            assert_eq!(journal_bytes(&store), before);
        }
        let unsafe_key = "../../outside\\model/backend:gpu";
        let lease = allowed(&store, unsafe_key);
        assert!(store.directory.join(lock_name(unsafe_key)).is_file());
        assert!(!directory.path().join("outside").exists());
        assert!(store
            .scan()
            .unwrap()
            .iter()
            .all(|name| { name == STORE_LOCK || name == JOURNAL || is_model_lock(name) }));
        lease.confirm_exit(true).unwrap();
        let key = "k".repeat(MAX_KEY_BYTES);
        let label = "l".repeat(MAX_LABEL_BYTES);
        match store.prepare(&key, &label).unwrap() {
            GpuAttempt::Allowed(lease) => lease.confirm_exit(true).unwrap(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn stored_size_and_record_count_bounds_are_enforced() {
        let (_directory, store) = fixture();
        let snapshot = journal_bytes(&store);
        OpenOptions::new()
            .write(true)
            .open(store.directory.join(JOURNAL))
            .unwrap()
            .set_len(MAX_JOURNAL_BYTES + 1)
            .unwrap();
        assert!(store
            .prepare("key", "label")
            .unwrap_err()
            .to_string()
            .contains("size limit"));
        fs::write(store.directory.join(JOURNAL), &snapshot).unwrap();
        let record = serde_json::json!({"key":"key","label":"label","state":{"status":"ready"}});
        let oversized =
            serde_json::json!({"version":VERSION,"records":vec![record; MAX_RECORDS + 1]});
        fs::write(
            store.directory.join(JOURNAL),
            serde_json::to_vec(&oversized).unwrap(),
        )
        .unwrap();
        assert!(store
            .blocked()
            .unwrap_err()
            .to_string()
            .contains("record limit"));
        fs::write(store.directory.join(JOURNAL), &snapshot).unwrap();
        let lease = allowed(&store, "key");
        let mut journal = store.load().unwrap();
        journal.records[0].label = "x".repeat(MAX_LABEL_BYTES + 1);
        write_fixture(&store, &journal);
        assert!(lease.confirm_exit(true).is_err());
    }

    #[test]
    fn directory_scan_is_bounded_and_unknown_files_are_preserved() {
        let (_directory, store) = fixture();
        for index in 0..MAX_FILES {
            fs::write(store.directory.join(lock_name(&index.to_string())), []).unwrap();
        }
        assert!(store
            .scan()
            .unwrap_err()
            .to_string()
            .contains("entry limit"));
        let (_directory, store) = fixture();
        let unknown = store.directory.join("real-notes.md");
        fs::write(&unknown, b"untouched").unwrap();
        assert!(RecoveryStore::open(&store.directory).is_err());
        assert_eq!(fs::read(unknown).unwrap(), b"untouched");
    }

    #[test]
    fn identity_capacity_never_evicts_existing_quarantines() {
        let (_directory, store) = fixture();
        let mut journal = store.load().unwrap();
        for index in 0..MAX_RECORDS {
            let key = index.to_string();
            fs::write(store.directory.join(lock_name(&key)), []).unwrap();
            journal.records.push(Record {
                key,
                label: "model".into(),
                state: State::Blocked {
                    failure: Failure::UnexpectedExit,
                    authorized: false,
                },
            });
        }
        write_fixture(&store, &journal);
        let before = journal_bytes(&store);
        assert!(store
            .prepare("one-too-many", "model")
            .unwrap_err()
            .to_string()
            .contains("identity limit"));
        assert_eq!(store.blocked().unwrap().len(), MAX_RECORDS);
        assert_eq!(journal_bytes(&store), before);
        store.allow_once("0").unwrap();
        allowed(&store, "0").confirm_exit(true).unwrap();
        assert_eq!(store.blocked().unwrap().len(), MAX_RECORDS - 1);
    }

    #[test]
    fn failed_durable_write_preserves_pending_and_partial_diagnostics() {
        let (_directory, store) = fixture();
        let lease = allowed(&store, "key");
        let pending = journal_bytes(&store);
        let result = store.publish(|file| {
            file.write_all(b"{\"version\":")?;
            Err(io::Error::other("injected durable-write failure"))
        });
        assert!(result.is_err());
        assert_eq!(journal_bytes(&store), pending);
        assert_eq!(
            fs::read(store.directory.join(NEXT)).unwrap(),
            b"{\"version\":"
        );
        assert!(lease.confirm_exit(true).is_err());
        drop(lease);
        assert!(RecoveryStore::open(&store.directory).is_err());
        assert_eq!(journal_bytes(&store), pending);
        assert!(store.allow_once("key").is_err());
    }

    #[test]
    fn orphaned_or_damaged_lock_files_fail_closed() {
        let (_directory, store) = fixture();
        fs::write(store.directory.join(lock_name("orphan")), []).unwrap();
        assert!(store.prepare("orphan", "label").is_err());
        let (_directory, store) = fixture();
        drop(allowed(&store, "key"));
        fs::write(store.directory.join(lock_name("key")), b"damaged").unwrap();
        assert!(store.prepare("key", "label").is_err());
        fs::write(store.directory.join(STORE_LOCK), b"").unwrap();
        assert!(RecoveryStore::open(&store.directory).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_hard_links_never_modify_their_targets() {
        use std::os::unix::fs::symlink;
        for hard_link in [false, true] {
            let (directory, store) = fixture();
            drop(allowed(&store, "key"));
            let target = directory.path().join("real-data");
            fs::write(&target, LOCK_MAGIC).unwrap();
            let lock = store.directory.join(lock_name("key"));
            fs::remove_file(&lock).unwrap();
            if hard_link {
                fs::hard_link(&target, &lock).unwrap();
            } else {
                symlink(&target, &lock).unwrap();
            }
            assert!(store.prepare("key", "model").is_err());
            assert!(RecoveryStore::open(&store.directory).is_err());
            assert_eq!(fs::read(target).unwrap(), LOCK_MAGIC);
        }
        let (directory, store) = fixture();
        let link = directory.path().join("linked-directory");
        symlink(&store.directory, &link).unwrap();
        assert!(RecoveryStore::open(link).is_err());
    }

    struct FixtureProcess(Child);

    impl Drop for FixtureProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn start_fixture(directory: &Path) -> FixtureProcess {
        let mut child = FixtureProcess(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "recovery::tests::subprocess_lease_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("MODEL_RUNTIME_RECOVERY_FIXTURE", directory)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !directory.join("ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "fixture exited before acquiring a lease"
            );
            assert!(Instant::now() < deadline, "fixture did not become ready");
            std::thread::sleep(Duration::from_millis(10));
        }
        child
    }

    #[test]
    fn live_process_is_not_a_crash_but_killed_parent_is_quarantined() {
        let (directory, store) = fixture();
        let mut child = start_fixture(directory.path());
        assert!(denied(&store, "subprocess").contains("still active"));
        assert!(store.blocked().unwrap().is_empty());
        assert!(store.allow_once("subprocess").is_err());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let reopened = RecoveryStore::open(&store.directory).unwrap();
        assert!(denied(&reopened, "subprocess").contains("no confirmed exit"));
        reopened.allow_once("subprocess").unwrap();
        allowed(&reopened, "subprocess").confirm_exit(true).unwrap();
    }

    #[test]
    fn normally_exited_process_releases_its_lease_without_quarantine() {
        let (directory, store) = fixture();
        let mut child = start_fixture(directory.path());
        fs::write(directory.path().join("finish"), []).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "fixture did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(store.blocked().unwrap().is_empty());
        allowed(&store, "subprocess").confirm_exit(true).unwrap();
    }

    #[test]
    #[ignore = "subprocess fixture invoked only by recovery tests"]
    fn subprocess_lease_fixture() {
        let directory = PathBuf::from(std::env::var_os("MODEL_RUNTIME_RECOVERY_FIXTURE").unwrap());
        let store = RecoveryStore::open(directory.join("recovery")).unwrap();
        let lease = allowed(&store, "subprocess");
        fs::write(directory.join("ready"), []).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !directory.join("finish").exists() {
            assert!(Instant::now() < deadline, "fixture timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        // This process is a lock-owning host fixture, not a native GPU worker.
        lease.confirm_exit(true).unwrap();
    }
}
