//! Identifies the exact build a running Grafium came from.
//!
//! Every value here is baked in at compile time by `build.rs`. The `env!`
//! macros are deliberate: they fail the build outright if the stamping step
//! ever stops emitting them, so this module cannot quietly degrade into
//! reporting placeholder data.

/// Hand-bumped release number from `Cargo.toml`. Identifies the release, not
/// the build: many commits share one value.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Abbreviated commit the binary was compiled from, or `unknown` when built
/// outside a Git checkout (a source tarball, for instance).
pub const GIT_SHA: &str = env!("GRAFIUM_GIT_SHA");

/// Build time as `YYYY-MM-DDTHH:MM:SSZ`, UTC.
pub const BUILD_TIME: &str = env!("GRAFIUM_BUILD_TIME");

/// Whether tracked files had uncommitted edits when this was built.
///
/// Best-effort by nature: Git records no filesystem event for an unstaged
/// edit, so a build can miss one. Treat a `true` as reliable and a `false`
/// as "nothing was detected" — `scripts/deploy-local.sh` re-checks against
/// live Git state at install time, which is the authoritative answer.
pub fn is_dirty() -> bool {
    env!("GRAFIUM_GIT_DIRTY") == "1"
}

/// Compact form for the in-app About section, e.g. `0.0.123 (f8cb86e)`.
pub fn short() -> String {
    format!(
        "{VERSION} ({GIT_SHA}{})",
        if is_dirty() { "-dirty" } else { "" }
    )
}

/// Full single-line form for `grafium --version` and startup logs.
pub fn full() -> String {
    format!(
        "Grafium {VERSION} (commit {GIT_SHA}{}, built {BUILD_TIME})",
        if is_dirty() { ", dirty" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_sha_is_a_short_hash_or_the_explicit_unknown_marker() {
        assert!(
            GIT_SHA == "unknown"
                || (GIT_SHA.len() >= 7 && GIT_SHA.chars().all(|c| c.is_ascii_hexdigit())),
            "unexpected GIT_SHA {GIT_SHA:?}: expected a hex abbreviation or \"unknown\""
        );
    }

    /// Guards `build.rs`'s hand-rolled calendar conversion. A wrong era
    /// calculation still produces a plausible-looking string, so check the
    /// shape and that the date is sane rather than merely non-empty.
    #[test]
    fn build_time_is_an_iso_utc_instant_in_a_plausible_range() {
        assert_eq!(
            BUILD_TIME.len(),
            20,
            "expected YYYY-MM-DDTHH:MM:SSZ, got {BUILD_TIME:?}"
        );
        assert!(BUILD_TIME.ends_with('Z'), "{BUILD_TIME:?} is not UTC");

        let (date, time) = BUILD_TIME
            .trim_end_matches('Z')
            .split_once('T')
            .unwrap_or_else(|| panic!("{BUILD_TIME:?} has no date/time separator"));

        let parts: Vec<u32> = date
            .split('-')
            .chain(time.split(':'))
            .map(|part| {
                part.parse()
                    .unwrap_or_else(|_| panic!("non-numeric field in {BUILD_TIME:?}"))
            })
            .collect();
        let [year, month, day, hour, minute, second] = parts[..] else {
            panic!("expected 6 fields in {BUILD_TIME:?}");
        };

        // Catches an epoch-shift bug in `civil_from_days`, which would land
        // the date near 1970 or far in the future rather than near today.
        assert!(
            (2024..=2100).contains(&year),
            "implausible build year in {BUILD_TIME:?}"
        );
        assert!((1..=12).contains(&month), "bad month in {BUILD_TIME:?}");
        assert!((1..=31).contains(&day), "bad day in {BUILD_TIME:?}");
        assert!(hour < 24, "bad hour in {BUILD_TIME:?}");
        assert!(minute < 60, "bad minute in {BUILD_TIME:?}");
        assert!(second < 60, "bad second in {BUILD_TIME:?}");
    }

    /// The About line renders this verbatim, and `deploy-local.sh` greps the
    /// commit out of `full()`. Both break silently if the shape drifts.
    #[test]
    fn rendered_forms_carry_the_commit() {
        assert!(short().contains(GIT_SHA), "short() dropped the commit");
        assert!(short().starts_with(VERSION), "short() dropped the version");

        let full = full();
        assert!(
            full.contains(&format!("commit {GIT_SHA}")),
            "deploy-local.sh parses `commit <sha>` out of {full:?}"
        );
        assert!(full.contains(BUILD_TIME), "full() dropped the build time");
    }
}
