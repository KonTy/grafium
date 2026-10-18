//! Keeping a sync target's metadata folder from growing without bound.
//!
//! Two kinds of files pile up next to the sync state, and they need opposite
//! treatment:
//!
//! * `sync-recovery/` holds byte-exact copies of notes taken before a merge or
//!   a conflict. They are the only way back to the version that lost, so they
//!   are kept for [`RECOVERY`] and trimmed oldest first when they outgrow its
//!   size cap. Nothing is removed while its note still has an open conflict or
//!   markers, and a copy's clock restarts whenever a sync needs it again or its
//!   note is settled, so the time to undo a decision counts from the decision.
//! * `sync-bases/` holds the merge base of each synced note. It is a cache that
//!   syncing rebuilds, so whatever a sync would never read again goes at once.
//!
//! Cleaning runs at the end of a successful sync. It is best effort: a file
//! that cannot be removed is left for the next sync, and nothing here can fail
//! one.

use super::text_merge::keeps_base;
use super::{make_conflict_path, SyncEngine};
use crate::sync::state::SyncState;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long recovery copies live and how much room they may take.
#[derive(Debug, Clone, Copy)]
pub(super) struct Retention {
    /// A copy older than this is removed.
    pub max_age: Duration,
    /// Copies that are not protected are trimmed to this many bytes, oldest
    /// first.
    pub max_bytes: u64,
    /// A copy younger than this is never removed, so a cleanup cannot take
    /// what another sync has just written.
    pub grace: Duration,
}

/// The one place that decides how long recovery copies are kept.
pub(super) const RECOVERY: Retention = Retention {
    max_age: Duration::from_secs(30 * 24 * 60 * 60),
    max_bytes: 256 * 1024 * 1024,
    grace: Duration::from_secs(60 * 60),
};

/// What a cleanup removed.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Report {
    pub files: usize,
    pub bytes: u64,
}

struct Candidate {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
    protected: bool,
}

/// Indexes of the candidates to remove: everything past `max_age`, then the
/// oldest of the rest until what is left fits `max_bytes`. Protected
/// candidates are never removed and do not count toward the cap; neither are
/// candidates inside the grace period.
fn plan(candidates: &[Candidate], retention: &Retention, now: SystemTime) -> Vec<usize> {
    // A timestamp in the future (a clock that was set back) counts as new.
    let age_of = |candidate: &Candidate| now.duration_since(candidate.modified).unwrap_or_default();
    let mut doomed = Vec::new();
    let mut kept = Vec::new();
    let mut kept_bytes = 0u64;
    for (index, candidate) in candidates.iter().enumerate() {
        if candidate.protected {
            continue;
        }
        let age = age_of(candidate);
        if age > retention.max_age && age >= retention.grace {
            doomed.push(index);
        } else {
            kept_bytes = kept_bytes.saturating_add(candidate.size);
            kept.push(index);
        }
    }
    if kept_bytes > retention.max_bytes {
        kept.sort_by(|&a, &b| {
            let (a, b) = (&candidates[a], &candidates[b]);
            a.modified
                .cmp(&b.modified)
                .then_with(|| a.path.cmp(&b.path))
        });
        for index in kept {
            if kept_bytes <= retention.max_bytes {
                break;
            }
            let candidate = &candidates[index];
            if age_of(candidate) >= retention.grace {
                kept_bytes = kept_bytes.saturating_sub(candidate.size);
                doomed.push(index);
            }
        }
    }
    doomed
}

struct Tree {
    files: Vec<(PathBuf, u64, SystemTime)>,
    dirs: Vec<PathBuf>,
}

/// The regular files and the folders in `root`, `root` itself included.
/// Symbolic links and anything else that is not a plain file or folder are
/// neither followed, listed nor ever removed. `None` when `root` cannot be
/// read, usually because nothing has been written there yet.
fn walk(root: &Path) -> Option<Tree> {
    let mut tree = Tree {
        files: Vec::new(),
        dirs: vec![root.to_path_buf()],
    };
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) if dir == root => return None,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
                tree.dirs.push(entry.path());
            } else if kind.is_file() {
                if let Ok(meta) = entry.metadata() {
                    // Without a timestamp a file is treated as new, never old.
                    let modified = meta.modified().unwrap_or_else(|_| SystemTime::now());
                    tree.files.push((entry.path(), meta.len(), modified));
                }
            }
        }
    }
    Some(tree)
}

/// Remove folders left empty, deepest first. A folder that still holds
/// anything is kept: `remove_dir` refuses it.
fn remove_empty_dirs(mut dirs: Vec<PathBuf>) {
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for dir in dirs {
        let _ = fs::remove_dir(dir);
    }
}

/// Remove one file; true when this call removed it.
fn remove(path: &Path) -> bool {
    match fs::remove_file(path) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "could not remove a sync metadata file");
            false
        }
    }
}

/// Apply `retention` to the files below `dir`. `protected` is asked about the
/// full path of every file and must say true for the ones that have to stay.
pub(super) fn prune_recovery(
    dir: &Path,
    retention: &Retention,
    now: SystemTime,
    protected: impl Fn(&Path) -> bool,
) -> Report {
    let Some(tree) = walk(dir) else {
        return Report::default();
    };
    let candidates: Vec<Candidate> = tree
        .files
        .into_iter()
        .map(|(path, size, modified)| Candidate {
            protected: protected(&path),
            path,
            size,
            modified,
        })
        .collect();
    let mut report = Report::default();
    for index in plan(&candidates, retention, now) {
        let candidate = &candidates[index];
        if remove(&candidate.path) {
            report.files += 1;
            report.bytes += candidate.size;
        }
    }
    remove_empty_dirs(tree.dirs);
    report
}

/// Remove every file below `dir` that `keep` rejects. `keep` is given the
/// path relative to `dir`, written with `/`.
pub(super) fn sweep(dir: &Path, keep: impl Fn(&str) -> bool) -> Report {
    let Some(tree) = walk(dir) else {
        return Report::default();
    };
    let mut report = Report::default();
    for (path, size, _) in tree.files {
        let Ok(relative) = path.strip_prefix(dir) else {
            continue;
        };
        if !keep(&slashed(relative)) && remove(&path) {
            report.files += 1;
            report.bytes += size;
        }
    }
    remove_empty_dirs(tree.dirs);
    report
}

/// `path` written with `/`, the way note paths are written everywhere else.
fn slashed(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether `text` is a content hash as `compute_hash` writes it.
fn is_hash(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The note a recovery copy was taken from, given the copy's path below
/// `sync-recovery/`: the inverse of `make_content_conflict_path`. `None` for
/// names that do not follow that scheme, such as copies older versions made.
fn conflict_origin(copy: &str) -> Option<String> {
    const TAG: &str = ".conflict_";
    const HASH_LEN: usize = 64;
    let mut end = copy.len();
    while let Some(at) = copy[..end].rfind(TAG) {
        let hash = at + TAG.len();
        let rest = hash + HASH_LEN;
        // What follows the hash is the note's extension, which starts with a
        // dot, or nothing.
        if copy.get(hash..rest).is_some_and(is_hash)
            && (copy[rest..].is_empty() || copy[rest..].starts_with('.'))
        {
            return Some(format!("{}{}", &copy[..at], &copy[rest..]));
        }
        end = at;
    }
    None
}

/// Mark a file as used now, so retention counts from this moment. Best effort:
/// a copy that cannot be touched ages as before.
pub(super) fn touch(path: &Path) {
    if let Ok(file) = fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// Clear out a target folder whose state is gone. Its caches go at once. Its
/// recovery copies are kept for the usual time, because nothing else holds the
/// versions that lost, and the folder goes with its last file.
fn retire(folder: &Path, now: SystemTime) {
    let _ = fs::remove_dir_all(folder.join("sync-bases"));
    prune_recovery(&folder.join("sync-recovery"), &RECOVERY, now, |_| false);
    // `remove_dir` refuses a folder that still holds anything.
    let _ = fs::remove_dir(folder);
}

impl SyncEngine {
    /// The folder holding this target's sync state, bases and recovery copies.
    fn target_folder(&self) -> &Path {
        self.state_path
            .parent()
            .expect("sync state file has a parent directory")
    }

    /// Where this target keeps the copies taken before a merge or a conflict.
    pub(super) fn recovery_dir(&self) -> PathBuf {
        self.target_folder().join("sync-recovery")
    }

    /// Trim this target's metadata folder. Called with the state that was just
    /// saved, after a successful sync.
    pub(super) fn tidy_metadata(&self, state: &SyncState) {
        let now = SystemTime::now();
        let recovery = self.recovery_dir();

        // Everything a person may still have to decide about stays: the copy
        // an open conflict points at, and every copy of a note that has an
        // open conflict or markers.
        let open_notes: HashSet<&str> = state
            .unresolved_conflicts
            .keys()
            .chain(state.pending_merges.keys())
            .map(String::as_str)
            .collect();
        let pinned: HashSet<PathBuf> = state
            .unresolved_conflicts
            .values()
            .map(|conflict| self.local_root.join(&conflict.backup_path))
            .collect();
        let expired = prune_recovery(&recovery, &RECOVERY, now, |path| {
            pinned.contains(path)
                || path
                    .strip_prefix(&recovery)
                    .ok()
                    .and_then(|copy| conflict_origin(&slashed(copy)))
                    .is_some_and(|note| open_notes.contains(note.as_str()))
        });

        // A base is read only for a note that has a sync record.
        let stale = sweep(&self.bases_dir, |note| {
            state.files.get(note).is_some_and(|record| {
                let size = record.local_size.unwrap_or(0);
                keeps_base(note, usize::try_from(size).unwrap_or(usize::MAX))
            })
        });

        self.retire_forgotten_targets(now);

        if expired.files + stale.files > 0 {
            tracing::info!(
                recovery_copies = expired.files,
                recovery_bytes = expired.bytes,
                stale_bases = stale.files,
                "sync metadata cleaned"
            );
        }
    }

    /// Folders of removed targets, whose state is gone, still hold recovery
    /// copies until those expire. Any later sync finishes the job.
    fn retire_forgotten_targets(&self, now: SystemTime) {
        let own = self.target_folder();
        // Target folders live side by side in
        // `<graph>/<metadata>/sync-targets/<target>`. The legacy layout keeps
        // its files in the metadata folder itself, and a graph can be called
        // anything, so the position is checked, not just the name.
        let Some(targets) = own.parent().filter(|dir| {
            dir.file_name().is_some_and(|name| name == "sync-targets")
                && own.ancestors().nth(3) == Some(self.local_root.as_path())
        }) else {
            return;
        };
        let Ok(entries) = fs::read_dir(targets) else {
            return;
        };
        for entry in entries.flatten() {
            let folder = entry.path();
            let is_target = entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry.file_name().to_str().is_some_and(is_hash);
            if is_target && folder != own && !folder.join("sync-state.json").exists() {
                retire(&folder, now);
            }
        }
    }

    /// Start a new retention period for every recovery copy taken from
    /// `rel_path`. Called when its markers are edited out, so the time to undo
    /// the decision counts from the decision, not from the conflict.
    pub(super) fn refresh_recovery_copies(&self, rel_path: &str) {
        let recovery = self.recovery_dir();
        // Copies of a note sit where a copy with any hash would.
        let probe = make_conflict_path(rel_path, &"0".repeat(64));
        let Some(parent) = Path::new(&probe).parent() else {
            return;
        };
        let Ok(entries) = fs::read_dir(recovery.join(parent)) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
            let origin = path
                .strip_prefix(&recovery)
                .ok()
                .and_then(|copy| conflict_origin(&slashed(copy)));
            if is_file && origin.as_deref() == Some(rel_path) {
                touch(&path);
            }
        }
    }

    /// Clear out what a removed sync target left behind. Its state and merge
    /// bases are of no use to anyone now and go at once; its recovery copies
    /// follow the usual retention (see `retire`).
    pub fn forget_target(
        local_root: PathBuf,
        metadata_dir_name: &str,
        target_key: impl AsRef<str>,
    ) {
        let engine =
            Self::new_with_metadata_dir_and_target(local_root, metadata_dir_name, target_key);
        let _ = fs::remove_file(&engine.state_path);
        let _ = fs::remove_file(engine.state_path.with_extension("json.corrupt"));
        retire(engine.target_folder(), SystemTime::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::backend::compute_hash;
    use tempfile::tempdir;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);
    const MINUTE: Duration = Duration::from_secs(60);

    fn candidate(
        name: &str,
        size: u64,
        age: Duration,
        protected: bool,
        now: SystemTime,
    ) -> Candidate {
        Candidate {
            path: PathBuf::from(name),
            size,
            modified: now - age,
            protected,
        }
    }

    fn names(candidates: &[Candidate], picked: Vec<usize>) -> Vec<String> {
        let mut names: Vec<String> = picked
            .into_iter()
            .map(|index| candidates[index].path.display().to_string())
            .collect();
        names.sort();
        names
    }

    fn plan_names(candidates: &[Candidate], retention: &Retention, now: SystemTime) -> Vec<String> {
        names(candidates, plan(candidates, retention, now))
    }

    fn write_aged(path: &Path, bytes: &[u8], age: Duration) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    fn modified_ago(path: &Path) -> Duration {
        SystemTime::now()
            .duration_since(fs::metadata(path).unwrap().modified().unwrap())
            .unwrap_or_default()
    }

    #[test]
    fn copies_past_the_age_limit_are_removed_and_younger_ones_stay() {
        let now = SystemTime::now();
        let candidates = [
            candidate("old", 1, 31 * DAY, false, now),
            candidate("edge", 1, 29 * DAY, false, now),
            candidate("new", 1, 2 * MINUTE, false, now),
        ];
        assert_eq!(plan_names(&candidates, &RECOVERY, now), ["old"]);
    }

    #[test]
    fn the_size_cap_trims_the_oldest_copies_first() {
        let now = SystemTime::now();
        let retention = Retention {
            max_age: 1000 * DAY,
            max_bytes: 100,
            grace: Duration::ZERO,
        };
        let candidates = [
            candidate("a", 40, 5 * DAY, false, now),
            candidate("b", 40, 4 * DAY, false, now),
            candidate("c", 40, 3 * DAY, false, now),
            candidate("d", 40, 2 * DAY, false, now),
            candidate("e", 40, DAY, false, now),
        ];
        // 200 bytes become 80: the three oldest go and the two newest stay.
        assert_eq!(plan_names(&candidates, &retention, now), ["a", "b", "c"]);
    }

    #[test]
    fn protected_copies_stay_whatever_their_age_and_do_not_count_toward_the_cap() {
        let now = SystemTime::now();
        let retention = Retention {
            max_age: 10 * DAY,
            max_bytes: 50,
            grace: Duration::ZERO,
        };
        let candidates = [
            candidate("open-conflict", 10_000, 90 * DAY, true, now),
            candidate("settled-old", 40, 20 * DAY, false, now),
            candidate("settled-new", 40, DAY, false, now),
        ];
        // The big protected copy neither goes nor pushes out `settled-new`.
        assert_eq!(plan_names(&candidates, &retention, now), ["settled-old"]);
    }

    #[test]
    fn a_fresh_copy_survives_even_when_it_alone_exceeds_the_cap() {
        let now = SystemTime::now();
        let retention = Retention {
            max_bytes: 10,
            ..RECOVERY
        };
        let candidates = [candidate("just-written", 5_000, 2 * MINUTE, false, now)];
        assert!(plan(&candidates, &retention, now).is_empty());

        let candidates = [candidate("an-hour-later", 5_000, 61 * MINUTE, false, now)];
        assert_eq!(plan(&candidates, &retention, now), [0]);
    }

    #[test]
    fn a_timestamp_in_the_future_counts_as_new() {
        let now = SystemTime::now();
        let ahead = Candidate {
            path: PathBuf::from("ahead"),
            size: 1,
            modified: now + 400 * DAY,
            protected: false,
        };
        assert!(plan(&[ahead], &RECOVERY, now).is_empty());
    }

    #[test]
    fn equal_timestamps_are_trimmed_in_a_stable_order() {
        let now = SystemTime::now();
        let retention = Retention {
            max_age: 1000 * DAY,
            max_bytes: 40,
            grace: Duration::ZERO,
        };
        let candidates = [
            candidate("b", 40, DAY, false, now),
            candidate("a", 40, DAY, false, now),
        ];
        assert_eq!(plan_names(&candidates, &retention, now), ["a"]);
    }

    #[test]
    fn pruning_removes_expired_files_and_the_folders_they_leave_empty() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("sync-recovery");
        write_aged(&root.join("pages/old.md"), b"old", 40 * DAY);
        write_aged(&root.join("pages/new.md"), b"new", DAY);
        write_aged(&root.join("knowledge/deep/gone.md"), b"gone", 40 * DAY);

        let report = prune_recovery(&root, &RECOVERY, SystemTime::now(), |_| false);

        assert_eq!(report, Report { files: 2, bytes: 7 });
        assert!(!root.join("pages/old.md").exists());
        assert!(root.join("pages/new.md").exists());
        assert!(
            !root.join("knowledge").exists(),
            "empty folders are removed"
        );
        assert!(root.exists(), "a folder that still holds a copy stays");
    }

    #[test]
    fn a_folder_left_empty_goes_with_its_subfolders() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("sync-recovery");
        write_aged(&root.join("pages/deep/old.md"), b"old", 40 * DAY);

        prune_recovery(&root, &RECOVERY, SystemTime::now(), |_| false);

        assert!(!root.exists());
    }

    #[test]
    fn pruning_a_folder_that_does_not_exist_does_nothing() {
        let dir = tempdir().unwrap();
        let report = prune_recovery(
            &dir.path().join("missing"),
            &RECOVERY,
            SystemTime::now(),
            |_| false,
        );
        assert_eq!(report, Report::default());
    }

    #[test]
    fn protected_paths_survive_any_age() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("sync-recovery");
        write_aged(&root.join("pages/kept.md"), b"kept", 400 * DAY);
        write_aged(&root.join("pages/lost.md"), b"lost", 400 * DAY);

        prune_recovery(&root, &RECOVERY, SystemTime::now(), |path| {
            path.ends_with("kept.md")
        });

        assert!(root.join("pages/kept.md").exists());
        assert!(!root.join("pages/lost.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_never_followed_or_removed() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let outside = dir.path().join("outside");
        write_aged(&outside.join("precious.md"), b"mine", 400 * DAY);
        let root = dir.path().join("sync-recovery");
        fs::create_dir_all(&root).unwrap();
        symlink(&outside, root.join("linked-folder")).unwrap();
        symlink(outside.join("precious.md"), root.join("linked-file")).unwrap();

        let report = prune_recovery(&root, &RECOVERY, SystemTime::now(), |_| false);

        assert_eq!(report, Report::default());
        assert!(outside.join("precious.md").exists());
        assert!(fs::symlink_metadata(root.join("linked-folder")).is_ok());
        assert!(fs::symlink_metadata(root.join("linked-file")).is_ok());
    }

    #[test]
    fn sweeping_removes_what_is_not_kept_and_the_folders_it_empties() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("sync-bases");
        write_aged(&root.join("pages/live.md"), b"live", Duration::ZERO);
        write_aged(&root.join("pages/orphan.md"), b"orphan", Duration::ZERO);
        write_aged(&root.join("assets/deep/pic.png"), b"png", Duration::ZERO);

        let report = sweep(&root, |note| note == "pages/live.md");

        assert_eq!(report, Report { files: 2, bytes: 9 });
        assert!(root.join("pages/live.md").exists());
        assert!(!root.join("pages/orphan.md").exists());
        assert!(!root.join("assets").exists());

        sweep(&root, |_| false);
        assert!(!root.exists(), "nothing kept, nothing left");
    }

    #[test]
    fn touching_restarts_the_clock() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("copy.md");
        write_aged(&path, b"x", 40 * DAY);
        assert!(modified_ago(&path) > 39 * DAY);

        touch(&path);

        assert!(modified_ago(&path) < MINUTE);
    }

    #[test]
    fn the_origin_of_a_copy_is_the_note_it_was_taken_from() {
        let hash = compute_hash(b"some version");
        for note in [
            "pages/todo.md",
            "pages/My Note, v2.md",
            "pages/日本語.md",
            "journals/2024_05_01.md",
            "knowledge/rules/format.v1.md",
            "books/no-extension",
            "assets/archive.tar.gz",
            "pages/ns___child.md",
            // A note that is itself named like a recovery copy.
            &format!("pages/x.conflict_{}.md", compute_hash(b"older")),
        ] {
            let copy = make_conflict_path(note, &hash);
            assert_eq!(conflict_origin(&copy).as_deref(), Some(note), "{copy}");
        }
    }

    #[test]
    fn names_that_are_not_recovery_copies_have_no_origin() {
        let hash = compute_hash(b"v");
        for name in [
            "pages/todo.md",
            "pages/a.conflict_xyz.md",
            "pages/a.conflict_.md",
            // Too short, too long, upper case, or followed by something else.
            &format!("pages/a.conflict_{}.md", &hash[..63]),
            &format!("pages/a.conflict_{hash}0.md"),
            &format!("pages/a.conflict_{}.md", hash.to_uppercase()),
            &format!("pages/a.conflict_{hash}-1.md"),
            // Names that older versions made.
            "pages/a.conflict_20240101T101500.md",
            "pages/a (conflict copy).md",
            ".hidden.tmp",
        ] {
            assert_eq!(conflict_origin(name), None, "{name}");
        }
    }

    fn graph_folder_of(engine: &SyncEngine) -> PathBuf {
        engine
            .target_folder()
            .ancestors()
            .nth(3)
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn a_legacy_engine_in_a_graph_named_like_the_targets_folder_leaves_it_alone() {
        let parent = tempdir().unwrap();
        let root = parent.path().join("sync-targets");
        let lookalike = root.join(compute_hash(b"lookalike"));
        write_aged(&lookalike.join("sync-bases/keep.txt"), b"x", Duration::ZERO);
        write_aged(&lookalike.join("sync-recovery/old.txt"), b"y", 90 * DAY);
        let engine = SyncEngine::new(root);

        engine.retire_forgotten_targets(SystemTime::now());

        assert!(lookalike.join("sync-bases/keep.txt").exists());
        assert!(lookalike.join("sync-recovery/old.txt").exists());
    }

    #[test]
    fn only_target_folders_without_state_are_retired() {
        let graph = tempdir().unwrap();
        let own = SyncEngine::new_with_target(graph.path().to_path_buf(), "own");
        assert_eq!(graph_folder_of(&own), graph.path());
        let targets = own.target_folder().parent().unwrap().to_path_buf();

        let gone = targets.join(compute_hash(b"gone"));
        write_aged(&gone.join("sync-bases/x"), b"b", Duration::ZERO);
        write_aged(&gone.join("sync-recovery/old.txt"), b"c", 90 * DAY);
        write_aged(&gone.join("sync-recovery/new.txt"), b"d", Duration::ZERO);
        let kept = targets.join(compute_hash(b"kept"));
        write_aged(&kept.join("sync-state.json"), b"{}", Duration::ZERO);
        write_aged(&kept.join("sync-bases/x"), b"f", Duration::ZERO);
        let not_a_target = targets.join("notes");
        write_aged(&not_a_target.join("sync-bases/x"), b"e", Duration::ZERO);

        own.retire_forgotten_targets(SystemTime::now());

        assert!(!gone.join("sync-bases").exists());
        assert!(!gone.join("sync-recovery/old.txt").exists());
        assert!(gone.join("sync-recovery/new.txt").exists());
        assert!(kept.join("sync-bases/x").exists());
        assert!(not_a_target.join("sync-bases/x").exists());
    }

    #[test]
    fn a_copy_that_is_needed_again_starts_a_new_retention_period() {
        use crate::sync::backend::FileSnapshot;
        let graph = tempdir().unwrap();
        let engine = SyncEngine::new_with_target(graph.path().to_path_buf(), "usb");
        let version = FileSnapshot {
            content: Some(b"version".to_vec()),
            ..Default::default()
        };

        let backup = engine
            .preserve_copy("pages/todo.md", &version)
            .unwrap()
            .unwrap();
        let path = graph.path().join(&backup);
        assert!(path.starts_with(engine.recovery_dir()));
        write_aged(&path, b"version", 20 * DAY);
        assert!(modified_ago(&path) > 19 * DAY);

        let again = engine
            .preserve_copy("pages/todo.md", &version)
            .unwrap()
            .unwrap();

        assert_eq!(again, backup);
        assert_eq!(fs::read(&path).unwrap(), b"version");
        assert!(modified_ago(&path) < MINUTE);
    }

    #[test]
    fn forgetting_a_target_clears_its_state_and_caches_and_lets_its_copies_expire() {
        let graph = tempdir().unwrap();
        let engine = SyncEngine::new_with_target(graph.path().to_path_buf(), "usb");
        let folder = engine.target_folder().to_path_buf();
        write_aged(&folder.join("sync-state.json"), b"{}", Duration::ZERO);
        write_aged(
            &folder.join("sync-state.json.corrupt"),
            b"{",
            Duration::ZERO,
        );
        write_aged(&folder.join("sync-bases/pages/a.md"), b"b", Duration::ZERO);
        write_aged(&folder.join("sync-recovery/pages/old.md"), b"c", 90 * DAY);
        write_aged(
            &folder.join("sync-recovery/pages/new.md"),
            b"d",
            Duration::ZERO,
        );

        SyncEngine::forget_target(
            graph.path().to_path_buf(),
            crate::graph::DEFAULT_METADATA_DIR_NAME,
            "usb",
        );

        assert!(!folder.join("sync-state.json").exists());
        assert!(!folder.join("sync-state.json.corrupt").exists());
        assert!(!folder.join("sync-bases").exists());
        assert!(!folder.join("sync-recovery/pages/old.md").exists());
        assert!(folder.join("sync-recovery/pages/new.md").exists());
    }

    #[test]
    fn a_forgotten_target_with_nothing_left_to_keep_leaves_no_folder() {
        let graph = tempdir().unwrap();
        let engine = SyncEngine::new_with_target(graph.path().to_path_buf(), "usb");
        let folder = engine.target_folder().to_path_buf();
        write_aged(&folder.join("sync-state.json"), b"{}", Duration::ZERO);
        write_aged(&folder.join("sync-bases/pages/a.md"), b"b", Duration::ZERO);
        write_aged(&folder.join("sync-recovery/pages/old.md"), b"c", 90 * DAY);

        SyncEngine::forget_target(
            graph.path().to_path_buf(),
            crate::graph::DEFAULT_METADATA_DIR_NAME,
            "usb",
        );

        assert!(!folder.exists());
        // Forgetting a target that never synced is harmless too.
        SyncEngine::forget_target(
            graph.path().to_path_buf(),
            crate::graph::DEFAULT_METADATA_DIR_NAME,
            "never-synced",
        );
    }
}
