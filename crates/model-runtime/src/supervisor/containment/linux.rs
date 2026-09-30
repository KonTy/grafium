use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const CONTROL_LIMIT: u64 = 64 * 1024;
const CGROUP2_MAGIC: libc::c_long = 0x6367_7270;

pub(super) struct Cgroup {
    path: PathBuf,
    procs: File,
    oom_group: bool,
    needs_confirmation: bool,
    removed: bool,
}

impl Cgroup {
    pub(super) fn discover_and_create(limit: u64) -> io::Result<Self> {
        // Never treat root's general privilege as delegated permission.
        let uid = unsafe { libc::geteuid() };
        if uid == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "automatic cgroup containment requires an unprivileged delegated memory controller",
            ));
        }
        let membership = read_bounded(Path::new("/proc/self/cgroup"), CONTROL_LIMIT)?;
        let mounts = read_bounded(Path::new("/proc/self/mountinfo"), 4 * 1024 * 1024)?;
        let (current, boundary) = locate(&membership, &mounts)?;
        let mut candidate = current.as_path();
        loop {
            let error = match delegated(candidate, uid) {
                Ok(()) => match Self::create(candidate, limit, |_| Ok(())) {
                    Ok(group) => return Ok(group),
                    Err(error) => error,
                },
                Err(error) => error,
            };
            if candidate == boundary {
                return Err(error);
            }
            let Some(parent) = candidate.parent() else {
                return Err(error);
            };
            if !parent.starts_with(&boundary) {
                return Err(error);
            }
            candidate = parent;
        }
    }

    fn create(
        parent: &Path,
        limit: u64,
        initialize: impl FnOnce(&Path) -> io::Result<()>,
    ) -> io::Result<Self> {
        let path = parent.join(format!(
            "model-runtime-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        fs::create_dir(&path)?;
        // The real filesystem supplies control files atomically with mkdir.
        // The initializer only exists to emulate them in isolated unit tests.
        let configured = (|| {
            initialize(&path)?;
            let oom_group = configure_limits(&path, limit)?;
            let procs = OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(path.join("cgroup.procs"))?;
            Ok((procs, oom_group))
        })();
        match configured {
            Ok((procs, oom_group)) => Ok(Self {
                path,
                procs,
                oom_group,
                needs_confirmation: false,
                removed: false,
            }),
            Err(error) => {
                if let Err(cleanup) = fs::remove_dir(&path) {
                    tracing::warn!(path = %path.display(), %cleanup, "cannot remove unused worker cgroup");
                }
                Err(error)
            }
        }
    }

    pub(super) fn configure_child(&self, command: &mut Command) {
        let fd = self.procs.as_raw_fd();
        // SAFETY: the pre-opened descriptor stays owned by this cgroup through
        // spawn. Only write/errno inspection run between fork and exec; writing
        // zero moves the forked child itself, before host/native initialization.
        unsafe {
            command.pre_exec(move || loop {
                match libc::write(fd, b"0".as_ptr().cast(), 1) {
                    1 => return Ok(()),
                    -1 => {
                        let error = io::Error::last_os_error();
                        if error.kind() != io::ErrorKind::Interrupted {
                            return Err(error);
                        }
                    }
                    _ => return Err(io::Error::from_raw_os_error(libc::EIO)),
                }
            });
        }
    }

    pub(super) fn oom_group(&self) -> bool {
        self.oom_group
    }

    pub(super) fn spawned(&mut self) {
        self.needs_confirmation = true;
    }

    pub(super) fn confirmed_exit(&mut self) -> io::Result<()> {
        self.needs_confirmation = false;
        self.remove_empty()
    }

    fn remove_empty(&mut self) -> io::Result<()> {
        if self.removed {
            return Ok(());
        }
        let events = read_bounded(&self.path.join("cgroup.events"), CONTROL_LIMIT)?;
        if !events.lines().any(|line| line.trim() == "populated 0")
            || events.lines().any(|line| line.trim() == "populated 1")
        {
            return Err(io::Error::other(
                "worker cgroup is still populated or its state is ambiguous; cleanup is not confirmed",
            ));
        }
        // Never recursively delete or change any parent cgroup.
        fs::remove_dir(&self.path)?;
        self.removed = true;
        Ok(())
    }
}

impl Drop for Cgroup {
    fn drop(&mut self) {
        if !self.needs_confirmation && !self.removed {
            if let Err(error) = self.remove_empty() {
                tracing::warn!(path = %self.path.display(), %error, "worker cgroup cleanup incomplete");
            }
        }
    }
}

fn read_bounded(path: &Path, limit: u64) -> io::Result<String> {
    let mut contents = String::new();
    File::open(path)?
        .take(limit + 1)
        .read_to_string(&mut contents)?;
    if contents.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cgroup metadata exceeds its size limit",
        ));
    }
    Ok(contents)
}

fn delegated(path: &Path, uid: u32) -> io::Result<()> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fstatfs initializes the supplied output on success.
    if unsafe { libc::fstatfs(directory.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { stats.assume_init() }.f_type != CGROUP2_MAGIC {
        return Err(io::Error::other(
            "delegation path is not a cgroup v2 filesystem",
        ));
    }
    if directory.metadata()?.uid() != uid
        || fs::symlink_metadata(path.join("cgroup.procs"))?.uid() != uid
        || fs::symlink_metadata(path.join("cgroup.subtree_control"))?.uid() != uid
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cgroup is not delegated to this user",
        ));
    }
    for control in ["cgroup.controllers", "cgroup.subtree_control"] {
        if !read_bounded(&path.join(control), CONTROL_LIMIT)?
            .split_whitespace()
            .any(|name| name == "memory")
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "memory controller is not already enabled for delegated children",
            ));
        }
    }
    if read_bounded(&path.join("cgroup.type"), CONTROL_LIMIT)?.trim() != "domain" {
        return Err(io::Error::other(
            "delegated cgroup is not a memory-accounting domain",
        ));
    }
    Ok(())
}

fn configure_limits(path: &Path, limit: u64) -> io::Result<bool> {
    if limit == 0 || limit > i64::MAX as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid cgroup memory limit",
        ));
    }
    let high = ((u128::from(limit) * 9) / 10).max(1) as u64;
    write_control(path, "memory.max", &limit.to_string())?;
    write_control(path, "memory.high", &high.to_string())?;
    write_control(path, "memory.swap.max", "0")?;
    match write_control(path, "memory.oom.group", "1") {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn write_control(path: &Path, name: &str, value: &str) -> io::Result<()> {
    OpenOptions::new()
        .write(true)
        .truncate(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path.join(name))?
        .write_all(value.as_bytes())
}

fn decode_path(value: &str) -> io::Result<PathBuf> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'\\' {
            let escape = bytes
                .get(offset + 1..offset + 4)
                .ok_or_else(|| io::Error::other("invalid cgroup path escape"))?;
            decoded.push(match escape {
                b"040" => b' ',
                b"011" => b'\t',
                b"012" => b'\n',
                b"134" => b'\\',
                _ => return Err(io::Error::other("invalid cgroup path escape")),
            });
            offset += 4;
        } else {
            decoded.push(bytes[offset]);
            offset += 1;
        }
    }
    if decoded.contains(&0)
        || decoded
            .split(|byte| *byte == b'/')
            .any(|part| part == b".." || part == b".")
    {
        return Err(io::Error::other("unsafe cgroup path"));
    }
    let path = PathBuf::from(OsString::from_vec(decoded));
    if !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(io::Error::other("cgroup path is not absolute"));
    }
    Ok(path)
}

fn locate(membership: &str, mountinfo: &str) -> io::Result<(PathBuf, PathBuf)> {
    let mut memberships = membership
        .lines()
        .filter_map(|line| line.strip_prefix("0::"));
    let path = decode_path(
        memberships
            .next()
            .ok_or_else(|| io::Error::other("cgroup v2 membership is unavailable"))?,
    )?;
    if memberships.next().is_some() {
        return Err(io::Error::other("ambiguous cgroup v2 membership"));
    }
    let mut mounts = Vec::new();
    for line in mountinfo.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        if after.split_whitespace().next() != Some("cgroup2") {
            continue;
        }
        let fields: Vec<_> = before.split_whitespace().collect();
        if fields.len() < 6 {
            return Err(io::Error::other("invalid cgroup mount information"));
        }
        let root = decode_path(fields[3])?;
        let mount = decode_path(fields[4])?;
        if let Ok(relative) = path.strip_prefix(&root) {
            mounts.push((root.components().count(), mount.join(relative), mount));
        }
    }
    mounts.sort_by_key(|(depth, _, _)| *depth);
    mounts
        .pop()
        .map(|(_, current, boundary)| (current, boundary))
        .ok_or_else(|| io::Error::other("current cgroup v2 mount is unavailable"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_controls(path: &Path) -> io::Result<()> {
        for name in [
            "memory.max",
            "memory.high",
            "memory.swap.max",
            "memory.oom.group",
            "cgroup.procs",
        ] {
            fs::write(path.join(name), "max")?;
        }
        fs::write(path.join("cgroup.events"), "populated 0\n")
    }

    #[test]
    fn mount_roots_escaping_and_path_boundaries_are_validated() {
        let mounts = "1 2 0:1 /user.slice /sys/fs/cgroup rw - cgroup2 cgroup rw\n\
                      2 1 0:2 /user.slice/app /delegate\\040space rw - cgroup2 cgroup rw\n";
        assert_eq!(
            locate("0::/user.slice/app/leaf\n", mounts).unwrap(),
            (
                PathBuf::from("/delegate space/leaf"),
                PathBuf::from("/delegate space")
            )
        );
        for membership in [
            "0::/../escape",
            "0::/user.slice/./app",
            "0::relative",
            "0::/other",
            "0::/user.slice\n0::/user.slice",
        ] {
            assert!(locate(membership, mounts).is_err(), "{membership}");
        }
        assert!(decode_path("/bad\\000name").is_err());
        assert!(decode_path("/bad\\12").is_err());
    }

    #[test]
    fn only_unique_child_limits_change_and_invalid_limits_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        fake_controls(root.path()).unwrap();
        fs::write(root.path().join("cgroup.subtree_control"), "memory").unwrap();
        let group = Cgroup::create(root.path(), 10_000, fake_controls).unwrap();
        assert_eq!(
            fs::read_to_string(group.path.join("memory.max")).unwrap(),
            "10000"
        );
        assert_eq!(
            fs::read_to_string(group.path.join("memory.high")).unwrap(),
            "9000"
        );
        assert_eq!(
            fs::read_to_string(group.path.join("memory.swap.max")).unwrap(),
            "0"
        );
        assert_eq!(
            fs::read_to_string(group.path.join("memory.oom.group")).unwrap(),
            "1"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("memory.max")).unwrap(),
            "max"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("cgroup.subtree_control")).unwrap(),
            "memory"
        );
        assert!(configure_limits(&group.path, 0).is_err());
        assert!(configure_limits(&group.path, u64::MAX).is_err());
        // Ordinary temporary directories are never mistaken for delegated
        // kernel controllers, irrespective of file names/ownership.
        assert!(delegated(root.path(), unsafe { libc::geteuid() }).is_err());
    }

    #[test]
    fn optional_oom_group_and_missing_required_controller_files_are_distinct() {
        let root = tempfile::tempdir().unwrap();
        fake_controls(root.path()).unwrap();
        fs::remove_file(root.path().join("memory.oom.group")).unwrap();
        assert!(!configure_limits(root.path(), 1000).unwrap());
        fs::remove_file(root.path().join("memory.swap.max")).unwrap();
        assert!(configure_limits(root.path(), 1000).is_err());
    }

    #[test]
    fn control_symlinks_cannot_modify_the_parent_controller() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("memory.max"), "parent-limit").unwrap();
        let child = root.path().join("owned-child");
        fs::create_dir(&child).unwrap();
        fake_controls(&child).unwrap();
        fs::remove_file(child.join("memory.max")).unwrap();
        std::os::unix::fs::symlink(root.path().join("memory.max"), child.join("memory.max"))
            .unwrap();
        assert!(configure_limits(&child, 1_000).is_err());
        assert_eq!(
            fs::read_to_string(root.path().join("memory.max")).unwrap(),
            "parent-limit"
        );
    }

    #[test]
    fn pre_exec_attaches_before_executable_runs_without_touching_system_cgroups() {
        let root = tempfile::tempdir().unwrap();
        let mut group = Cgroup::create(root.path(), 1000, fake_controls).unwrap();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 0"]);
        group.configure_child(&mut command);
        let mut child = command.spawn().unwrap();
        group.spawned();
        assert!(child.wait().unwrap().success());
        assert!(fs::read_to_string(group.path.join("cgroup.procs"))
            .unwrap()
            .starts_with('0'));
        fs::write(group.path.join("cgroup.events"), "populated 1\n").unwrap();
        assert!(group.confirmed_exit().is_err());
        assert!(group.path.exists());
        // The kernel has virtual control files; a tempfs emulation instead
        // deliberately fails rmdir rather than recursively removing anything.
        fs::write(group.path.join("cgroup.events"), "populated 0\n").unwrap();
        assert!(group.confirmed_exit().is_err());
        assert!(root.path().exists());
    }
}
