//! Line-based 3-way merge with Git-style conflict markers.
//!
//! Given a **base** (common ancestor), **local** (ours), and **remote** (theirs),
//! produces merged text. Non-overlapping changes are auto-merged; overlapping
//! changes get conflict markers:
//!
//! ```text
//! <<<<<<< local
//! our version of the lines
//! =======
//! their version of the lines
//! >>>>>>> remote
//! ```

/// Conflict markers (Git-style).
const MARKER_LOCAL: &str = "<<<<<<< local";
const MARKER_SEP: &str = "=======";
const MARKER_REMOTE: &str = ">>>>>>> remote";

/// Result of a merge operation.
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// The merged content (may contain conflict markers).
    pub content: String,
    /// Whether any conflicts were found.
    pub has_conflicts: bool,
    /// Number of conflict regions.
    pub conflict_count: usize,
}

/// Perform a 3-way merge of text content.
///
/// * `base`   – content at the last sync point (common ancestor)
/// * `local`  – current local version ("ours")
/// * `remote` – current remote version ("theirs")
///
/// Returns merged content with conflict markers where both sides changed the
/// same region differently.
pub fn three_way_merge(base: &str, local: &str, remote: &str) -> MergeResult {
    // Fast-path: identical
    if local == remote {
        return MergeResult {
            content: local.to_string(),
            has_conflicts: false,
            conflict_count: 0,
        };
    }
    // Fast-path: only one side changed
    if local == base {
        return MergeResult {
            content: remote.to_string(),
            has_conflicts: false,
            conflict_count: 0,
        };
    }
    if remote == base {
        return MergeResult {
            content: local.to_string(),
            has_conflicts: false,
            conflict_count: 0,
        };
    }

    let base_lines: Vec<&str> = base.lines().collect();
    let local_lines: Vec<&str> = local.lines().collect();
    let remote_lines: Vec<&str> = remote.lines().collect();

    // Compute change regions for both sides relative to base
    let local_regions = diff_regions(&base_lines, &local_lines);
    let remote_regions = diff_regions(&base_lines, &remote_lines);

    // Merge the two sets of regions
    merge_change_regions(
        &base_lines,
        &local_lines,
        &remote_lines,
        &local_regions,
        &remote_regions,
    )
}

/// 2-way merge (no base available). Finds common lines via LCS, marks all
/// differing sections as conflicts.
pub fn two_way_merge(local: &str, remote: &str) -> MergeResult {
    if local == remote {
        return MergeResult {
            content: local.to_string(),
            has_conflicts: false,
            conflict_count: 0,
        };
    }
    // Use empty base → everything that differs is a conflict
    three_way_merge("", local, remote)
}

// ---------------------------------------------------------------------------
// LCS (Longest Common Subsequence) – standard DP
// ---------------------------------------------------------------------------

/// Returns indices of matching lines: Vec<(base_idx, other_idx)>.
fn lcs_indices(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    let m = a.len();
    let n = b.len();
    if m == 0 || n == 0 {
        return Vec::new();
    }

    // DP table (O(m*n) — fine for markdown pages of a few hundred lines)
    let mut dp = vec![vec![0u32; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if a[i - 1] == b[j - 1] {
                dp[i - 1][j - 1] + 1
            } else {
                dp[i - 1][j].max(dp[i][j - 1])
            };
        }
    }

    // Back-track to recover the subsequence
    let mut result = Vec::new();
    let (mut i, mut j) = (m, n);
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            result.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] >= dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    result.reverse();
    result
}

// ---------------------------------------------------------------------------
// Diff regions: map base ranges → replacement ranges
// ---------------------------------------------------------------------------

/// A region in the diff. Either "equal" (base kept) or "changed" (base
/// replaced by new content).
#[derive(Debug, Clone)]
struct DiffRegion {
    /// Range in base (start..end, exclusive end)
    base_start: usize,
    base_end: usize,
    /// Range in modified text
    mod_start: usize,
    mod_end: usize,
    /// true  = base[start..end] is the same as modified[mod_start..mod_end]
    /// false = base region was replaced by the modified region
    is_equal: bool,
}

/// Compute change regions between base and modified text.
fn diff_regions<'a>(base: &[&'a str], modified: &[&'a str]) -> Vec<DiffRegion> {
    let matches = lcs_indices(base, modified);
    let mut regions = Vec::new();
    let mut b_pos: usize = 0;
    let mut m_pos: usize = 0;

    for &(bi, mi) in &matches {
        // Any non-matching content before this match point
        if bi > b_pos || mi > m_pos {
            regions.push(DiffRegion {
                base_start: b_pos,
                base_end: bi,
                mod_start: m_pos,
                mod_end: mi,
                is_equal: false,
            });
        }
        // The matching line itself
        regions.push(DiffRegion {
            base_start: bi,
            base_end: bi + 1,
            mod_start: mi,
            mod_end: mi + 1,
            is_equal: true,
        });
        b_pos = bi + 1;
        m_pos = mi + 1;
    }

    // Trailing content after last match
    if b_pos < base.len() || m_pos < modified.len() {
        regions.push(DiffRegion {
            base_start: b_pos,
            base_end: base.len(),
            mod_start: m_pos,
            mod_end: modified.len(),
            is_equal: false,
        });
    }

    // Merge consecutive equal regions and consecutive change regions for cleaner output
    coalesce_regions(&regions)
}

/// Merge adjacent regions of the same kind.
fn coalesce_regions(regions: &[DiffRegion]) -> Vec<DiffRegion> {
    if regions.is_empty() {
        return Vec::new();
    }
    let mut out = vec![regions[0].clone()];
    for r in &regions[1..] {
        let last = out.last_mut().unwrap();
        if last.is_equal == r.is_equal
            && last.base_end == r.base_start
            && last.mod_end == r.mod_start
        {
            last.base_end = r.base_end;
            last.mod_end = r.mod_end;
        } else {
            out.push(r.clone());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 3-way merge of two sets of change regions
// ---------------------------------------------------------------------------

/// Walk base positions and merge changes from local and remote.
///
/// Handles three kinds of diff regions:
/// 1. Equal regions (base lines kept unchanged)
/// 2. Change regions (base lines replaced by modified lines)
/// 3. Pure insertions (new lines inserted between base lines, base_start == base_end)
fn merge_change_regions(
    base: &[&str],
    local: &[&str],
    remote: &[&str],
    local_regions: &[DiffRegion],
    remote_regions: &[DiffRegion],
) -> MergeResult {
    let mut output = Vec::<String>::new();
    let mut conflicts = 0usize;

    // Build per-base-line index: which region covers each base line?
    let local_cover = build_line_to_region(base.len(), local_regions);
    let remote_cover = build_line_to_region(base.len(), remote_regions);

    // Build insertion maps: for each base position, any pure insertion regions
    let local_ins = build_insertion_map(local_regions);
    let remote_ins = build_insertion_map(remote_regions);

    let mut b: usize = 0;

    while b <= base.len() {
        // 1. Handle any pure insertions at this base position
        emit_insertions(
            b,
            &local_ins,
            &remote_ins,
            local_regions,
            remote_regions,
            local,
            remote,
            &mut output,
            &mut conflicts,
        );

        if b >= base.len() {
            break;
        }

        // 2. Handle the base line at position b
        let lr = &local_regions[local_cover[b]];
        let rr = &remote_regions[remote_cover[b]];

        match (lr.is_equal, rr.is_equal) {
            (true, true) => {
                // Both sides kept this line
                output.push(base[b].to_string());
                b += 1;
            }
            (false, true) => {
                // Only local changed — emit local's replacement once at region start
                if b == lr.base_start {
                    for i in lr.mod_start..lr.mod_end {
                        output.push(local[i].to_string());
                    }
                }
                b += 1;
            }
            (true, false) => {
                // Only remote changed — emit remote's replacement once at region start
                if b == rr.base_start {
                    for i in rr.mod_start..rr.mod_end {
                        output.push(remote[i].to_string());
                    }
                }
                b += 1;
            }
            (false, false) => {
                // Both changed — determine the full overlapping region
                let overlap_end = lr.base_end.max(rr.base_end);

                // Only emit once, when we first enter the overlap
                if b == lr.base_start || b == rr.base_start {
                    let l_lines = &local[lr.mod_start..lr.mod_end];
                    let r_lines = &remote[rr.mod_start..rr.mod_end];

                    if l_lines == r_lines {
                        for line in l_lines {
                            output.push(line.to_string());
                        }
                    } else {
                        conflicts += 1;
                        output.push(MARKER_LOCAL.to_string());
                        for line in l_lines {
                            output.push(line.to_string());
                        }
                        output.push(MARKER_SEP.to_string());
                        for line in r_lines {
                            output.push(line.to_string());
                        }
                        output.push(MARKER_REMOTE.to_string());
                    }
                }
                // Jump past the entire overlap
                b = overlap_end;
            }
        }
    }

    let content = if output.is_empty() {
        String::new()
    } else {
        output.join("\n") + "\n"
    };

    MergeResult {
        content,
        has_conflicts: conflicts > 0,
        conflict_count: conflicts,
    }
}

/// Build a per-base-line index: `result[b]` is the index of the region in
/// `regions` that covers base line `b`.
fn build_line_to_region(base_len: usize, regions: &[DiffRegion]) -> Vec<usize> {
    let mut idx = vec![0usize; base_len];
    for (ri, r) in regions.iter().enumerate() {
        for b in r.base_start..r.base_end {
            if b < base_len {
                idx[b] = ri;
            }
        }
    }
    idx
}

/// Build a map from base position → index of the pure insertion region at that
/// position (a change region where `base_start == base_end`).
fn build_insertion_map(regions: &[DiffRegion]) -> std::collections::HashMap<usize, usize> {
    let mut map = std::collections::HashMap::new();
    for (ri, r) in regions.iter().enumerate() {
        if !r.is_equal && r.base_start == r.base_end {
            map.insert(r.base_start, ri);
        }
    }
    map
}

/// Emit insertions at base position `b` from both sides.
fn emit_insertions(
    b: usize,
    local_ins: &std::collections::HashMap<usize, usize>,
    remote_ins: &std::collections::HashMap<usize, usize>,
    local_regions: &[DiffRegion],
    remote_regions: &[DiffRegion],
    local: &[&str],
    remote: &[&str],
    output: &mut Vec<String>,
    conflicts: &mut usize,
) {
    let li = local_ins.get(&b).map(|&i| &local_regions[i]);
    let ri = remote_ins.get(&b).map(|&i| &remote_regions[i]);

    match (li, ri) {
        (None, None) => {}
        (Some(l), None) => {
            for i in l.mod_start..l.mod_end {
                output.push(local[i].to_string());
            }
        }
        (None, Some(r)) => {
            for i in r.mod_start..r.mod_end {
                output.push(remote[i].to_string());
            }
        }
        (Some(l), Some(r)) => {
            let l_lines = &local[l.mod_start..l.mod_end];
            let r_lines = &remote[r.mod_start..r.mod_end];
            if l_lines == r_lines {
                for line in l_lines {
                    output.push(line.to_string());
                }
            } else {
                *conflicts += 1;
                output.push(MARKER_LOCAL.to_string());
                for line in l_lines {
                    output.push(line.to_string());
                }
                output.push(MARKER_SEP.to_string());
                for line in r_lines {
                    output.push(line.to_string());
                }
                output.push(MARKER_REMOTE.to_string());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_content() {
        let r = three_way_merge("hello\nworld\n", "hello\nworld\n", "hello\nworld\n");
        assert!(!r.has_conflicts);
        assert_eq!(r.content, "hello\nworld\n");
    }

    #[test]
    fn test_only_local_changed() {
        let base = "line1\nline2\nline3\n";
        let local = "line1\nmodified\nline3\n";
        let remote = "line1\nline2\nline3\n";
        let r = three_way_merge(base, local, remote);
        assert!(!r.has_conflicts);
        assert_eq!(r.content, "line1\nmodified\nline3\n");
    }

    #[test]
    fn test_only_remote_changed() {
        let base = "line1\nline2\nline3\n";
        let local = "line1\nline2\nline3\n";
        let remote = "line1\nremote_edit\nline3\n";
        let r = three_way_merge(base, local, remote);
        assert!(!r.has_conflicts);
        assert_eq!(r.content, "line1\nremote_edit\nline3\n");
    }

    #[test]
    fn test_both_changed_same_way() {
        let base = "line1\nline2\nline3\n";
        let local = "line1\nboth_same\nline3\n";
        let remote = "line1\nboth_same\nline3\n";
        let r = three_way_merge(base, local, remote);
        assert!(!r.has_conflicts);
        assert!(r.content.contains("both_same"));
    }

    #[test]
    fn test_conflict_both_changed_differently() {
        let base = "line1\nline2\nline3\n";
        let local = "line1\nlocal_edit\nline3\n";
        let remote = "line1\nremote_edit\nline3\n";
        let r = three_way_merge(base, local, remote);
        assert!(r.has_conflicts);
        assert_eq!(r.conflict_count, 1);
        assert!(r.content.contains("<<<<<<< local"));
        assert!(r.content.contains("local_edit"));
        assert!(r.content.contains("======="));
        assert!(r.content.contains("remote_edit"));
        assert!(r.content.contains(">>>>>>> remote"));
    }

    #[test]
    fn test_non_overlapping_changes_auto_merge() {
        let base = "line1\nline2\nline3\nline4\nline5\n";
        let local = "line1\nlocal_change\nline3\nline4\nline5\n";
        let remote = "line1\nline2\nline3\nline4\nremote_change\n";
        let r = three_way_merge(base, local, remote);
        assert!(!r.has_conflicts);
        assert!(r.content.contains("local_change"));
        assert!(r.content.contains("remote_change"));
        assert!(!r.content.contains("line2"));
        assert!(!r.content.contains("line5"));
    }

    #[test]
    fn test_local_adds_lines() {
        let base = "line1\nline2\n";
        let local = "line1\nline2\nnew_local_line\n";
        let remote = "line1\nline2\n";
        let r = three_way_merge(base, local, remote);
        assert!(!r.has_conflicts);
        assert!(r.content.contains("new_local_line"));
    }

    #[test]
    fn test_remote_adds_lines() {
        let base = "line1\nline2\n";
        let local = "line1\nline2\n";
        let remote = "line1\nline2\nnew_remote_line\n";
        let r = three_way_merge(base, local, remote);
        assert!(!r.has_conflicts);
        assert!(r.content.contains("new_remote_line"));
    }

    #[test]
    fn test_local_deletes_lines() {
        let base = "line1\nline2\nline3\n";
        let local = "line1\nline3\n";
        let remote = "line1\nline2\nline3\n";
        let r = three_way_merge(base, local, remote);
        assert!(!r.has_conflicts);
        assert!(!r.content.contains("line2"));
    }

    #[test]
    fn test_two_way_merge_conflict() {
        let r = two_way_merge("local only\n", "remote only\n");
        assert!(r.has_conflicts);
        assert!(r.content.contains("<<<<<<< local"));
        assert!(r.content.contains(">>>>>>> remote"));
    }

    #[test]
    fn test_two_way_merge_identical() {
        let r = two_way_merge("same\n", "same\n");
        assert!(!r.has_conflicts);
    }

    #[test]
    fn test_markdown_properties_conflict() {
        let base = "title:: My Page\ntags:: #rust\n\n- block 1\n- block 2\n";
        let local =
            "title:: My Page\ntags:: #rust, #code\n\n- block 1\n- block 2\n- block 3 local\n";
        let remote =
            "title:: My Page\ntags:: #rust, #docs\n\n- block 1\n- block 2\n- block 3 remote\n";
        let r = three_way_merge(base, local, remote);
        // The tags line changed differently → conflict
        assert!(r.has_conflicts);
        assert!(r.content.contains("title:: My Page")); // unchanged line kept
        assert!(r.content.contains("- block 1")); // unchanged line kept
    }

    #[test]
    fn test_empty_base_both_new() {
        let base = "";
        let local = "new local content\n";
        let remote = "new remote content\n";
        let r = three_way_merge(base, local, remote);
        assert!(r.has_conflicts);
    }

    #[test]
    fn test_lcs_basic() {
        let a = vec!["a", "b", "c", "d"];
        let b = vec!["a", "x", "c", "d"];
        let matches = lcs_indices(&a, &b);
        // Should match a(0,0), c(2,2), d(3,3)
        assert!(matches.contains(&(0, 0)));
        assert!(matches.contains(&(2, 2)));
        assert!(matches.contains(&(3, 3)));
    }

    #[test]
    fn test_lcs_empty() {
        let matches = lcs_indices(&[], &["a", "b"]);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_multiple_conflicts() {
        let base = "a\nb\nc\nd\ne\n";
        let local = "a\nLB\nc\nLD\ne\n";
        let remote = "a\nRB\nc\nRD\ne\n";
        let r = three_way_merge(base, local, remote);
        assert!(r.has_conflicts);
        assert_eq!(r.conflict_count, 2);
    }
}
