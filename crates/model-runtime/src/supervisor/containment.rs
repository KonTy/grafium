//! OS memory containment, deliberately separate from working-set estimates.
//! No address-space rlimit is used: GPU mappings are not resident RAM.

use std::io;
use std::process::Command;

#[cfg(target_os = "linux")]
mod linux;

pub(super) struct Containment {
    #[cfg(target_os = "linux")]
    group: Option<linux::Cgroup>,
    diagnostic: String,
    enforced: bool,
}

impl Containment {
    pub(super) fn prepare(command: &mut Command, limit: Option<u64>) -> io::Result<Self> {
        if limit == Some(0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native worker memory limit must be greater than zero",
            ));
        }

        let Some(limit) = limit else {
            return Ok(Self::unavailable(
                "no per-worker memory limit was requested".into(),
            ));
        };
        #[cfg(target_os = "linux")]
        {
            if limit > i64::MAX as u64 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "native worker memory limit exceeds the Linux cgroup range",
                ));
            }
            match linux::Cgroup::discover_and_create(limit) {
                Ok(group) => {
                    group.configure_child(command);
                    Ok(Self {
                        diagnostic: format!(
                            "Native worker RAM limit enforced by cgroup v2: {limit} bytes; swap disabled{}.",
                            if group.oom_group() { "; group OOM enabled" } else { "; group OOM unavailable" },
                        ),
                        group: Some(group),
                        enforced: true,
                    })
                }
                Err(error) => Ok(Self::unavailable(error.to_string())),
            }
        }
        #[cfg(windows)]
        {
            let _ = command;
            usize::try_from(limit).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "native worker memory limit is not representable",
                )
            })?;
            Ok(Self {
                diagnostic: format!(
                    "Native worker commit limit enforced by Windows Job Object: {limit} bytes."
                ),
                enforced: true,
            })
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            let _ = (command, limit);
            Ok(Self::unavailable(
                "this platform has no supported hard RAM limiter".into(),
            ))
        }
    }

    pub(super) fn unavailable(reason: String) -> Self {
        Self {
            #[cfg(target_os = "linux")]
            group: None,
            diagnostic: format!(
                "Native worker hard RAM containment unavailable: {reason}; host pressure monitoring is required for monitored fallback and is not an OS-enforced limit."
            ),
            enforced: false,
        }
    }

    pub(super) fn diagnostic(&self) -> &str {
        &self.diagnostic
    }

    pub(super) fn enforced(&self) -> bool {
        self.enforced
    }

    pub(super) fn spawned(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(group) = &mut self.group {
            group.spawned();
        }
    }

    pub(super) fn confirmed_exit(&mut self) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        if let Some(group) = &mut self.group {
            group.confirmed_exit()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_limits_are_rejected_before_accessing_any_os_controller() {
        let mut command = Command::new("not-spawned");
        assert!(Containment::prepare(&mut command, Some(0)).is_err());
        #[cfg(target_os = "linux")]
        assert!(Containment::prepare(&mut command, Some(u64::MAX)).is_err());
        let disabled = Containment::prepare(&mut command, None).unwrap();
        assert!(!disabled.enforced());
        assert!(disabled.diagnostic().contains("no per-worker memory limit"));
    }
}
