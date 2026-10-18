//! Block-aware, whitespace-tolerant text merge for notes.
//!
//! Pure functions, no I/O. Given the version both devices last agreed on
//! (the *base*), this device's note (*local*, shown as `YOURS`) and the other
//! device's (*remote*, shown as `THEIRS`), edits that touch different parts of
//! the note are combined and only genuinely overlapping edits are written into
//! the note as conflict markers a person resolves by editing the text:
//!
//! ```text
//! <<<<<<< YOURS (merge 3fa9c2d1)
//! - the version from this device
//! ======= (merge 3fa9c2d1)
//! - the version from the other device
//! >>>>>>> THEIRS (merge 3fa9c2d1)
//! ```
//!
//! Design rules, each of which exists to keep a human out of busywork:
//!
//! * **Blocks, not lines.** A bullet and its continuation, property
//!   (`id:: …`) and code-fence lines form one unit, so a hunk never separates
//!   a block from its id.
//! * **Noise is not a conflict.** Units are compared ignoring trailing
//!   spaces, tabs, line-ending style and generated block ids (`id:: …`, which
//!   the outliner hides and each device invents for itself until a note has
//!   synced once). A side that only changed such noise never overrides the
//!   other side's real edit. When both sides differ only in noise the
//!   byte-wise smaller text wins, so the result does not depend on which
//!   device ran the merge.
//! * **No base, no guessing.** Without a common ancestor every difference is
//!   a conflict, even a pure insertion, because a missing line may be a
//!   deletion that must not be resurrected.
//! * **Markers carry a signature** (`(merge xxxxxxxx)`), so a note that merely
//!   *discusses* conflict markers is never mistaken for an unresolved merge.

use super::backend::compute_hash;
use super::diff::{align, NONE};

/// Largest note (per side) that is merged; bigger files keep the whole-file
/// conflict flow rather than risking a slow diff.
pub const MAX_MERGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_UNITS: usize = 200_000;
/// Insertions plus deletions a side may differ from its base by.
const MAX_EDITS: usize = 2_000;

const OPEN: &str = "<<<<<<< YOURS";
const SEPARATOR: &str = "=======";
const CLOSE: &str = ">>>>>>> THEIRS";

/// Outcome of a merge. `content` contains conflict markers iff
/// `conflict_count > 0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeResult {
    pub content: String,
    pub conflict_count: usize,
}

impl MergeResult {
    pub fn has_conflicts(&self) -> bool {
        self.conflict_count > 0
    }
}

/// Merge `local` and `remote` against their common ancestor.
///
/// Returns `None` when the inputs are too large or too different to merge
/// quickly; the caller should fall back to asking the user to pick a whole
/// file.
pub fn three_way_merge(base: &str, local: &str, remote: &str) -> Option<MergeResult> {
    if let Some(result) = trivial(local, remote, Some(base)) {
        return Some(result);
    }
    if [base, local, remote]
        .iter()
        .any(|t| t.len() > MAX_MERGE_BYTES)
    {
        return None;
    }
    let (base_units, local_units, remote_units) = (blocks(base), blocks(local), blocks(remote));
    if [&base_units, &local_units, &remote_units]
        .iter()
        .any(|units| units.len() > MAX_UNITS)
    {
        return None;
    }
    let base_to_local = align(&keys(&base_units), &keys(&local_units), MAX_EDITS)?;
    let base_to_remote = align(&keys(&base_units), &keys(&remote_units), MAX_EDITS)?;
    // A base unit both sides still have is a safe fixed point between
    // independent stretches.
    let anchors = (0..base_units.len()).filter_map(|b| {
        let (l, r) = (base_to_local[b], base_to_remote[b]);
        (l != NONE && r != NONE).then_some((Some(b), l, r))
    });
    Some(assemble(
        Some(&base_units),
        &local_units,
        &remote_units,
        anchors,
        &token(Some(base), local, remote),
        eol_of(local),
    ))
}

/// Merge two versions with no known common ancestor. Every difference is
/// reported as a conflict; only identical (or whitespace-only different)
/// stretches are combined.
pub fn two_way_merge(local: &str, remote: &str) -> Option<MergeResult> {
    if let Some(result) = trivial(local, remote, None) {
        return Some(result);
    }
    if [local, remote].iter().any(|t| t.len() > MAX_MERGE_BYTES) {
        return None;
    }
    let (local_units, remote_units) = (blocks(local), blocks(remote));
    if local_units.len() > MAX_UNITS || remote_units.len() > MAX_UNITS {
        return None;
    }
    let local_to_remote = align(&keys(&local_units), &keys(&remote_units), MAX_EDITS)?;
    let anchors = local_to_remote
        .iter()
        .enumerate()
        .filter(|&(_, &r)| r != NONE)
        .map(|(l, &r)| (None, l, r));
    Some(assemble(
        None,
        &local_units,
        &remote_units,
        anchors,
        &token(None, local, remote),
        eol_of(local),
    ))
}

/// Unresolved merge markers found in a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerScan {
    /// Signature of the first marker found.
    pub token: String,
    /// Number of conflict hunks still present (at least 1).
    pub hunks: usize,
}

/// Find unresolved merge markers.
///
/// Only lines carrying this module's signature count; indentation and a
/// leading list bullet are tolerated because the outliner may re-serialise a
/// marker line as a block. A partially resolved hunk (one marker line left)
/// still counts: it must not be published.
pub fn scan_markers(text: &str) -> Option<MarkerScan> {
    if !text.contains("(merge ") {
        return None;
    }
    let mut token: Option<&str> = None;
    let mut hunks = 0;
    for line in text.lines() {
        if let Some((is_open, line_token)) = marker_line(line) {
            token.get_or_insert(line_token);
            hunks += usize::from(is_open);
        }
    }
    token.map(|token| MarkerScan {
        token: token.to_string(),
        hunks: hunks.max(1),
    })
}

/// [`scan_markers`] for raw file bytes; invalid UTF-8 is scanned lossily so
/// a damaged note cannot slip past the check.
pub fn scan_marker_bytes(bytes: &[u8]) -> Option<MarkerScan> {
    scan_markers(&String::from_utf8_lossy(bytes))
}

pub fn has_live_markers(text: &str) -> bool {
    scan_markers(text).is_some()
}

/// `Some((is_open, token))` for a signed marker line.
fn marker_line(line: &str) -> Option<(bool, &str)> {
    let mut rest = line.trim_start_matches([' ', '\t']);
    if let Some(after_bullet) = rest.strip_prefix("- ") {
        rest = after_bullet.trim_start_matches([' ', '\t']);
    }
    let rest = rest.trim_end();
    for (label, is_open) in [(OPEN, true), (SEPARATOR, false), (CLOSE, false)] {
        let Some(tail) = rest.strip_prefix(label) else {
            continue;
        };
        let Some(token) = tail
            .strip_prefix(" (merge ")
            .and_then(|t| t.strip_suffix(')'))
        else {
            continue;
        };
        if token.len() == 8
            && token
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Some((is_open, token));
        }
    }
    None
}

fn trivial(local: &str, remote: &str, base: Option<&str>) -> Option<MergeResult> {
    let content = if local == remote || base == Some(remote) {
        local
    } else if base == Some(local) {
        remote
    } else {
        return None;
    };
    Some(MergeResult {
        content: content.to_string(),
        conflict_count: 0,
    })
}

/// Identifies one merge: same inputs, same signature, on every device.
fn token(base: Option<&str>, local: &str, remote: &str) -> String {
    let base = base.map_or_else(|| "-".to_string(), |b| compute_hash(b.as_bytes()));
    let joined = format!(
        "{base}:{}:{}",
        compute_hash(local.as_bytes()),
        compute_hash(remote.as_bytes())
    );
    compute_hash(joined.as_bytes())[..8].to_string()
}

fn eol_of(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

// ---------------------------------------------------------------------------
// Block units
// ---------------------------------------------------------------------------

/// A bullet with everything that belongs to it, or a single other line.
/// Concatenating all units reproduces the text byte for byte.
struct Unit<'a> {
    text: &'a str,
    key: u64,
}

fn keys(units: &[Unit<'_>]) -> Vec<u64> {
    units.iter().map(|unit| unit.key).collect()
}

struct OpenBlock {
    start: usize,
    indent: usize,
    fence: Option<(u8, usize)>,
}

fn blocks(text: &str) -> Vec<Unit<'_>> {
    let mut units = Vec::new();
    let mut open: Option<OpenBlock> = None;
    let mut position = 0;
    for line in text.split_inclusive('\n') {
        let start = position;
        position += line.len();
        let (indent, content) = split_indent(line);
        let blank = content.trim().is_empty();

        if let Some(block) = &mut open {
            let continues = if block.fence.is_some() {
                // Inside a code fence everything deeper than the bullet
                // belongs to it, blank lines included.
                blank || indent > block.indent
            } else {
                !blank && indent > block.indent && !is_bullet(content)
            };
            if continues {
                block.fence = match block.fence {
                    Some(fence) if closes_fence(content, fence) => None,
                    Some(fence) => Some(fence),
                    None => opening_fence(content),
                };
                continue;
            }
            units.push(unit(&text[block.start..start]));
            open = None;
        }

        if is_bullet(content) {
            open = Some(OpenBlock {
                start,
                indent,
                fence: opening_fence(content.trim_start_matches('-').trim_start()),
            });
        } else {
            units.push(unit(line));
        }
    }
    if let Some(block) = open {
        units.push(unit(&text[block.start..]));
    }
    units
}

fn unit(text: &str) -> Unit<'_> {
    Unit {
        text,
        key: key_hash(text),
    }
}

fn split_indent(line: &str) -> (usize, &str) {
    let content = line.trim_start_matches([' ', '\t']);
    (line.len() - content.len(), content)
}

fn is_bullet(content: &str) -> bool {
    content.starts_with("- ") || content.trim_end() == "-"
}

fn opening_fence(content: &str) -> Option<(u8, usize)> {
    let marker = *content.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let length = content.bytes().take_while(|&b| b == marker).count();
    (length >= 3).then_some((marker, length))
}

fn closes_fence(content: &str, (marker, length): (u8, usize)) -> bool {
    let run = content.bytes().take_while(|&b| b == marker).count();
    run >= length && content[run..].trim().is_empty()
}

fn trim_trailing_space(line: &str) -> &str {
    line.trim_end_matches([' ', '\t', '\r', '\n'])
}

/// A generated block id (`  id:: 3f2a…`). The outliner hides these and gives
/// every device its own random ids until a note has been saved and synced once,
/// so two copies of the same block can differ only here.
fn is_block_id_line(line: &str) -> bool {
    let line = line.trim_start_matches([' ', '\t']);
    line.strip_prefix("id:: ").is_some_and(|value| {
        let value = value.trim();
        !value.is_empty() && !value.contains([' ', '\t'])
    })
}

/// The lines of a unit that count when comparing: trailing whitespace and
/// generated block ids do not. The first line is the bullet itself, so it is
/// always kept.
fn comparable_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive('\n')
        .enumerate()
        .filter(|&(index, line)| index == 0 || !is_block_id_line(line))
        .map(|(_, line)| trim_trailing_space(line))
}

/// FNV-1a over a unit's comparable lines; deterministic across runs and
/// platforms.
fn key_hash(text: &str) -> u64 {
    const PRIME: u64 = 0x100_0000_01b3;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (index, line) in comparable_lines(text).enumerate() {
        if index > 0 {
            hash = (hash ^ u64::from(b'\n')).wrapping_mul(PRIME);
        }
        for byte in line.bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
        }
    }
    hash
}

fn same_text(a: &[Unit<'_>], b: &[Unit<'_>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.text == y.text)
}

/// Equal ignoring trailing whitespace and generated block ids. Hashes only
/// prefilter; the lines are compared for real so a collision can only cause an
/// extra conflict.
fn same_key(a: &[Unit<'_>], b: &[Unit<'_>]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.key == y.key && comparable_lines(x.text).eq(comparable_lines(y.text)))
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Side {
    Local,
    Remote,
}

/// A symmetric tie-break between texts that differ only in whitespace, so
/// both devices end up with the same bytes.
fn byte_smaller(local: &[Unit<'_>], remote: &[Unit<'_>]) -> Side {
    let bytes = |units: &[Unit<'_>]| {
        units
            .iter()
            .flat_map(|u| u.text.bytes())
            .collect::<Vec<_>>()
    };
    if bytes(local) <= bytes(remote) {
        Side::Local
    } else {
        Side::Remote
    }
}

/// Which side's version of a stretch to keep, or `None` for a real conflict.
fn choose(base: Option<&[Unit<'_>]>, local: &[Unit<'_>], remote: &[Unit<'_>]) -> Option<Side> {
    if same_text(local, remote) {
        return Some(Side::Local);
    }
    let Some(base) = base else {
        return same_key(local, remote).then(|| byte_smaller(local, remote));
    };
    if same_text(local, base) {
        return Some(Side::Remote);
    }
    if same_text(remote, base) {
        return Some(Side::Local);
    }
    if same_key(local, remote) {
        return Some(byte_smaller(local, remote));
    }
    if same_key(local, base) {
        return Some(Side::Remote);
    }
    if same_key(remote, base) {
        return Some(Side::Local);
    }
    None
}

struct Output {
    text: String,
    conflicts: usize,
    eol: &'static str,
}

impl Output {
    /// Text joined after an unterminated final line must start on a new line.
    fn ensure_line_start(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push_str(self.eol);
        }
    }

    fn push_units(&mut self, units: &[Unit<'_>]) {
        for unit in units {
            self.ensure_line_start();
            self.text.push_str(unit.text);
        }
    }

    fn push_marker(&mut self, label: &str, token: &str) {
        self.ensure_line_start();
        self.text.push_str(label);
        self.text.push_str(" (merge ");
        self.text.push_str(token);
        self.text.push(')');
        self.text.push_str(self.eol);
    }

    fn push_stretch(
        &mut self,
        base: Option<&[Unit<'_>]>,
        local: &[Unit<'_>],
        remote: &[Unit<'_>],
        token: &str,
    ) {
        match choose(base, local, remote) {
            Some(Side::Local) => self.push_units(local),
            Some(Side::Remote) => self.push_units(remote),
            None => {
                // Lines both sides share at the edges stay outside the
                // markers; keeping either side still gives exactly its text.
                let head = local
                    .iter()
                    .zip(remote)
                    .take_while(|(l, r)| l.text == r.text)
                    .count();
                let tail = local[head..]
                    .iter()
                    .rev()
                    .zip(remote[head..].iter().rev())
                    .take_while(|(l, r)| l.text == r.text)
                    .count();
                self.push_units(&local[..head]);
                self.push_marker(OPEN, token);
                self.push_units(&local[head..local.len() - tail]);
                self.push_marker(SEPARATOR, token);
                self.push_units(&remote[head..remote.len() - tail]);
                self.push_marker(CLOSE, token);
                self.push_units(&local[local.len() - tail..]);
                self.conflicts += 1;
            }
        }
    }
}

/// Walk the fixed points in order, resolving the stretch before each one and
/// the fixed point itself, then whatever follows the last. `anchors` yields
/// `(base index, local index, remote index)` in increasing order.
fn assemble<'a>(
    base: Option<&[Unit<'a>]>,
    local: &[Unit<'a>],
    remote: &[Unit<'a>],
    anchors: impl Iterator<Item = (Option<usize>, usize, usize)>,
    token: &str,
    eol: &'static str,
) -> MergeResult {
    let mut out = Output {
        text: String::with_capacity(local.iter().map(|u| u.text.len()).sum()),
        conflicts: 0,
        eol,
    };
    let (mut b_pos, mut l_pos, mut r_pos) = (0, 0, 0);
    for (anchor_b, anchor_l, anchor_r) in anchors {
        out.push_stretch(
            base.zip(anchor_b).map(|(units, at)| &units[b_pos..at]),
            &local[l_pos..anchor_l],
            &remote[r_pos..anchor_r],
            token,
        );
        out.push_stretch(
            base.zip(anchor_b).map(|(units, at)| &units[at..=at]),
            &local[anchor_l..=anchor_l],
            &remote[anchor_r..=anchor_r],
            token,
        );
        b_pos = anchor_b.map_or(0, |at| at + 1);
        l_pos = anchor_l + 1;
        r_pos = anchor_r + 1;
    }
    out.push_stretch(
        base.map(|units| &units[b_pos..]),
        &local[l_pos..],
        &remote[r_pos..],
        token,
    );
    MergeResult {
        content: out.text,
        conflict_count: out.conflicts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merged(base: &str, local: &str, remote: &str) -> MergeResult {
        three_way_merge(base, local, remote).expect("small inputs always merge")
    }

    fn token_of(result: &MergeResult) -> String {
        scan_markers(&result.content)
            .expect("conflicted merge has markers")
            .token
    }

    #[test]
    fn identical_and_one_sided_changes_take_the_fast_path() {
        assert_eq!(merged("a\n", "b\n", "b\n").content, "b\n");
        assert_eq!(merged("a\n", "a\n", "b\n").content, "b\n");
        assert_eq!(merged("a\n", "b\n", "a\n").content, "b\n");
        assert!(!merged("a\n", "b\n", "a\n").has_conflicts());
    }

    #[test]
    fn edits_to_different_blocks_are_combined() {
        let base = "- one\n  id:: 1\n- two\n  id:: 2\n- three\n  id:: 3\n";
        let local = "- ONE\n  id:: 1\n- two\n  id:: 2\n- three\n  id:: 3\n";
        let remote = "- one\n  id:: 1\n- two\n  id:: 2\n- THREE\n  id:: 3\n";
        let result = merged(base, local, remote);
        assert!(!result.has_conflicts(), "{}", result.content);
        assert_eq!(
            result.content,
            "- ONE\n  id:: 1\n- two\n  id:: 2\n- THREE\n  id:: 3\n"
        );
    }

    #[test]
    fn overlapping_edits_become_one_marked_hunk_with_whole_blocks() {
        let base = "- keep\n- shared\n  id:: 9\n- tail\n";
        let local = "- keep\n- shared mine\n  id:: 9\n- tail\n";
        let remote = "- keep\n- shared theirs\n  id:: 9\n- tail\n";
        let result = merged(base, local, remote);
        let token = token_of(&result);
        assert_eq!(result.conflict_count, 1);
        assert_eq!(
            result.content,
            format!(
                "- keep\n<<<<<<< YOURS (merge {token})\n- shared mine\n  id:: 9\n\
                 ======= (merge {token})\n- shared theirs\n  id:: 9\n\
                 >>>>>>> THEIRS (merge {token})\n- tail\n"
            )
        );
    }

    #[test]
    fn lines_both_sides_share_stay_outside_the_markers() {
        let base = "- a\n- b\n- c\n- d\n";
        // Both made the same edit to `b`; each changed a different neighbour.
        let local = "- a\n- B\n- C mine\n- d\n";
        let remote = "- a\n- B\n- C theirs\n- d\n";
        let result = merged(base, local, remote);
        let token = token_of(&result);
        assert_eq!(result.conflict_count, 1);
        assert_eq!(
            result.content,
            format!(
                "- a\n- B\n<<<<<<< YOURS (merge {token})\n- C mine\n\
                 ======= (merge {token})\n- C theirs\n\
                 >>>>>>> THEIRS (merge {token})\n- d\n"
            )
        );

        // The shared line may also come last.
        let local = "- a\n- B mine\n- C\n- d\n";
        let remote = "- a\n- B theirs\n- C\n- d\n";
        let result = merged(base, local, remote);
        let token = token_of(&result);
        assert_eq!(
            result.content,
            format!(
                "- a\n<<<<<<< YOURS (merge {token})\n- B mine\n\
                 ======= (merge {token})\n- B theirs\n\
                 >>>>>>> THEIRS (merge {token})\n- C\n- d\n"
            )
        );
    }

    #[test]
    fn keeping_either_side_of_a_trimmed_hunk_gives_exactly_that_side() {
        let base = "- a\n- b\n- c\n- d\n- e\n";
        let local = "- a\n- x\n- b mine\n- y\n- e\n";
        let remote = "- a\n- x\n- b theirs\n- y\n- e\n";
        let result = merged(base, local, remote);
        assert_eq!(result.conflict_count, 1);
        assert!(result.content.contains("- x\n<<<<<<< YOURS"));
        let keep = |yours: bool| -> String {
            let mut inside = None;
            let mut out = String::new();
            for line in result.content.split_inclusive('\n') {
                if line.starts_with("<<<<<<< YOURS") {
                    inside = Some(true);
                } else if line.starts_with("=======") {
                    inside = Some(false);
                } else if line.starts_with(">>>>>>> THEIRS") {
                    inside = None;
                } else if inside.is_none_or(|is_yours| is_yours == yours) {
                    out.push_str(line);
                }
            }
            out
        };
        assert_eq!(keep(true), local);
        assert_eq!(keep(false), remote);
    }

    #[test]
    fn a_hunk_never_splits_a_block_from_its_id_line() {
        let base = "- a\n  id:: 1\n";
        let result = merged(base, "- a mine\n  id:: 1\n", "- a theirs\n  id:: 1\n");
        let lines: Vec<&str> = result.content.lines().collect();
        let mut id_lines = 0;
        for (index, line) in lines.iter().enumerate() {
            if line.starts_with("  id::") {
                id_lines += 1;
                assert!(lines[index - 1].starts_with("- a "), "{}", result.content);
            }
        }
        assert_eq!(id_lines, 2, "{}", result.content);
    }

    #[test]
    fn deleting_against_editing_conflicts_but_matching_deletions_do_not() {
        let base = "- a\n- b\n- c\n";
        let edited = merged(base, "- a\n- c\n", "- a\n- B\n- c\n");
        assert_eq!(edited.conflict_count, 1);
        let token = token_of(&edited);
        assert_eq!(
            edited.content,
            format!(
                "- a\n<<<<<<< YOURS (merge {token})\n======= (merge {token})\n- B\n\
                 >>>>>>> THEIRS (merge {token})\n- c\n"
            )
        );

        let both = merged(base, "- a\n- c\n", "- a\n- c\n");
        assert_eq!(both.content, "- a\n- c\n");
        let one = merged(base, "- a\n- c\n", "- a\n- b\n- c\n- d\n");
        assert!(!one.has_conflicts(), "{}", one.content);
        assert_eq!(one.content, "- a\n- c\n- d\n");
    }

    #[test]
    fn adjacent_edits_conflict_like_git_but_separated_edits_do_not() {
        let base = "- a\n- b\n- c\n- d\n";
        let adjacent = merged(base, "- A\n- b\n- c\n- d\n", "- a\n- B\n- c\n- d\n");
        assert_eq!(adjacent.conflict_count, 1);
        let apart = merged(base, "- A\n- b\n- c\n- d\n", "- a\n- b\n- C\n- d\n");
        assert!(!apart.has_conflicts());
        assert_eq!(apart.content, "- A\n- b\n- C\n- d\n");
    }

    #[test]
    fn insertions_in_different_places_combine_and_in_the_same_place_conflict() {
        let base = "- a\n- b\n";
        let apart = merged(base, "- a\n- x\n- b\n", "- a\n- b\n- y\n");
        assert_eq!(apart.content, "- a\n- x\n- b\n- y\n");
        let same_place = merged(base, "- a\n- x\n- b\n", "- a\n- y\n- b\n");
        assert_eq!(same_place.conflict_count, 1);
        let identical = merged(base, "- a\n- x\n- b\n", "- a\n- x\n- b\n");
        assert_eq!(identical.content, "- a\n- x\n- b\n");
    }

    #[test]
    fn trailing_whitespace_and_line_endings_never_conflict() {
        let base = "- a\n- b\n";
        // One side re-saved with trailing spaces, the other made a real edit.
        let result = merged(base, "- a  \n- b\n", "- a\n- B\n");
        assert!(!result.has_conflicts(), "{}", result.content);
        assert_eq!(
            result.content, "- a  \n- B\n",
            "the whitespace edit survives"
        );
        // The real edit wins over a whitespace-only change to the same block.
        let over = merged(base, "- a\n- b  \n", "- a\n- B\n");
        assert_eq!(over.content, "- a\n- B\n");
        // Both sides differ from the base only in whitespace, differently.
        let both = merged(base, "- a \n- b\n", "- a\t\n- b\n");
        assert!(!both.has_conflicts());
        // The same real edit with different trailing whitespace is one edit.
        let same = merged(base, "- A \n- b\n", "- A\n- b\n");
        assert!(!same.has_conflicts());
        // Whitespace-only differences against a deletion are not conflicts.
        let deleted = merged(base, "- a  \n- b\n", "- b\n");
        assert!(!deleted.has_conflicts());
        assert_eq!(deleted.content, "- b\n");
    }

    #[test]
    fn generated_block_ids_are_not_content() {
        fn saved(items: &[&str], device: &str) -> String {
            items
                .iter()
                .enumerate()
                .map(|(i, item)| format!("- {item}\n  id:: {device}{i}\n"))
                .collect()
        }
        // Neither device had saved the note in the app before, so each one
        // invented its own ids while making a different edit.
        let base = "- Buy milk\n- Walk the dog\n- Pay rent\n- Call mom\n- Water plants\n";
        let local = saved(
            &[
                "Buy milk",
                "Walk the dog",
                "Pay rent",
                "Call mom at 5",
                "Water plants",
            ],
            "b",
        );
        let remote = saved(
            &[
                "Buy milk",
                "Walk the dog (done)",
                "Pay rent",
                "Call mom",
                "Water plants",
            ],
            "a",
        );
        let result = merged(base, &local, &remote);
        assert!(!result.has_conflicts(), "{}", result.content);
        // An edit keeps the id of the side that made it; untouched blocks take
        // the byte-smaller id, whichever device runs the merge.
        assert_eq!(
            result.content,
            "- Buy milk\n  id:: a0\n- Walk the dog (done)\n  id:: a1\n- Pay rent\n  id:: a2\n\
             - Call mom at 5\n  id:: b3\n- Water plants\n  id:: a4\n"
        );
        assert_eq!(result, merged(base, &remote, &local));
    }

    #[test]
    fn a_side_that_only_gained_ids_never_overrides_a_real_edit() {
        let base = "- one\n- two\n";
        let saved = "- one\n  id:: 1\n- two\n  id:: 2\n";
        let edited = "- one\n- TWO\n";
        let result = merged(base, saved, edited);
        assert_eq!(result.content, "- one\n  id:: 1\n- TWO\n");
        assert_eq!(result.content, merged(base, edited, saved).content);
    }

    #[test]
    fn real_edits_still_conflict_when_ids_differ() {
        let base = "- shared\n";
        let result = merged(
            base,
            "- shared mine\n  id:: b1\n",
            "- shared theirs\n  id:: a1\n",
        );
        let token = token_of(&result);
        assert_eq!(result.conflict_count, 1);
        assert_eq!(
            result.content,
            format!(
                "<<<<<<< YOURS (merge {token})\n- shared mine\n  id:: b1\n\
                 ======= (merge {token})\n- shared theirs\n  id:: a1\n\
                 >>>>>>> THEIRS (merge {token})\n"
            )
        );
    }

    #[test]
    fn only_a_blocks_own_id_line_is_ignored() {
        // A page property named `id` and an id-looking bullet are content.
        let base = "id:: 1\n\n- a\n";
        let page_property = merged(base, "id:: 2\n\n- a\n", "id:: 3\n\n- a\n");
        assert_eq!(page_property.conflict_count, 1, "{}", page_property.content);
        let first_line = merged("- id:: x\n", "- id:: y\n", "- id:: z\n");
        assert_eq!(first_line.conflict_count, 1, "{}", first_line.content);
        // Other block properties are content too.
        let property = merged(
            "- a\n  id:: 1\n  color:: red\n",
            "- a\n  id:: 1\n  color:: blue\n",
            "- a\n  id:: 1\n  color:: green\n",
        );
        assert_eq!(property.conflict_count, 1, "{}", property.content);
        // Prose that merely starts with `id::` is content: two different
        // rewrites of it must not be settled silently.
        let prose = merged(
            "- a\n  id:: see below\n",
            "- a\n  id:: see above\n",
            "- a\n  id:: see sideways\n",
        );
        assert_eq!(prose.conflict_count, 1, "{}", prose.content);
    }

    #[test]
    fn clean_merges_do_not_depend_on_which_device_runs_them() {
        let cases = [
            ("- a\n- b\n", "- a  \n- B\n", "- a\n- b\t\n"),
            ("- a\n- b\n- c\n", "- A\n- b\n- c \n", "- a\n- b\n- C\n"),
            ("x\ny\n", "x \ny\n", "x\t\ny\n"),
            ("a\nb", "a\nb\n", "a\nb\nc\n"),
            (
                "- a\n- m\n- b\n",
                "- a\n  id:: 1\n- m\n  id:: 2\n- B\n  id:: 3\n",
                "- A\n  id:: 4\n- m\n  id:: 5\n- b\n  id:: 6\n",
            ),
        ];
        for (base, local, remote) in cases {
            let ab = merged(base, local, remote);
            let ba = merged(base, remote, local);
            assert!(!ab.has_conflicts() && !ba.has_conflicts());
            assert_eq!(ab.content, ba.content, "base {base:?}");
        }
    }

    #[test]
    fn conflicts_are_deterministic_and_the_signature_depends_on_the_sides() {
        let base = "- a\n- b\n";
        let ab = merged(base, "- A\n- b\n", "- X\n- b\n");
        let ba = merged(base, "- X\n- b\n", "- A\n- b\n");
        assert_eq!(ab.conflict_count, ba.conflict_count);
        assert_ne!(token_of(&ab), token_of(&ba));
        assert_eq!(ab, merged(base, "- A\n- b\n", "- X\n- b\n"));
    }

    #[test]
    fn keeps_crlf_and_a_missing_final_newline() {
        let result = merged(
            "alpha\r\nbeta\r\ngamma\r\n",
            "alpha EDITED\r\nbeta\r\ngamma\r\n",
            "alpha\r\nbeta\r\ngamma EDITED\r\n",
        );
        assert_eq!(result.content, "alpha EDITED\r\nbeta\r\ngamma EDITED\r\n");

        let result = merged("a\nb\nc", "a EDITED\nb\nc", "a\nb\nc EDITED");
        assert_eq!(result.content, "a EDITED\nb\nc EDITED");

        let conflicted = merged("a\r\nb", "a\r\nmine", "a\r\ntheirs");
        let token = token_of(&conflicted);
        assert_eq!(
            conflicted.content,
            format!(
                "a\r\n<<<<<<< YOURS (merge {token})\r\nmine\r\n======= (merge {token})\r\n\
                 theirs\r\n>>>>>>> THEIRS (merge {token})\r\n"
            )
        );
    }

    #[test]
    fn text_joined_after_a_missing_newline_starts_on_a_new_line() {
        let result = merged("a\nb", "a\nb\n", "a\nb\nc\n");
        assert!(!result.has_conflicts());
        assert_eq!(result.content, "a\nb\nc\n");
        let result = merged("a\nb\n", "a\nb", "a\nb\nc\n");
        assert_eq!(result.content, "a\nb\nc\n");
    }

    #[test]
    fn code_fences_keep_bullet_like_lines_inside_their_block() {
        let base = "- code\n  ```\n  - not a bullet\n\n  more\n  ```\n- middle\n- after\n";
        assert_eq!(blocks(base).len(), 3, "the fence is one block");
        let local = base.replace("more", "more mine");
        let remote = base.replace("- after", "- after theirs");
        let result = merged(base, &local, &remote);
        assert!(!result.has_conflicts(), "{}", result.content);
        assert_eq!(
            result.content,
            "- code\n  ```\n  - not a bullet\n\n  more mine\n  ```\n- middle\n- after theirs\n"
        );
    }

    #[test]
    fn units_reassemble_the_text_exactly() {
        for text in [
            "",
            "\n",
            "title:: x\n\n- a\n  id:: 1\n  - child\n    cont\n- b",
            "plain\n\n  indented\n- x\n\n- y\r\n",
            "- ```\n  - inner\n- outer\n",
            "-\n- \n",
        ] {
            let joined: String = blocks(text).iter().map(|u| u.text).collect();
            assert_eq!(joined, text);
        }
    }

    #[test]
    fn two_way_merge_marks_every_difference_including_insertions() {
        let local = "- shared\n- only here\n- end\n";
        let remote = "- shared\n- end\n";
        let result = two_way_merge(local, remote).unwrap();
        assert_eq!(result.conflict_count, 1);
        assert!(result.content.contains("- only here\n======="));
        assert_eq!(result.content.matches("- shared\n").count(), 1);

        assert!(!two_way_merge("- a\n", "- a \n").unwrap().has_conflicts());
        assert!(!two_way_merge("- a\n- b\n", "- a\n- b\n")
            .unwrap()
            .has_conflicts());
    }

    #[test]
    fn signature_scan_only_recognises_real_markers() {
        let result = merged("- a\n", "- b\n", "- c\n");
        assert!(has_live_markers(&result.content));
        let scan = scan_markers(&result.content).unwrap();
        assert_eq!(scan.hunks, 1);
        assert_eq!(scan.token.len(), 8);
        assert_eq!(
            scan_marker_bytes(result.content.as_bytes()),
            Some(scan.clone())
        );

        // The outliner may re-serialise marker lines as indented bullets.
        let token = &scan.token;
        assert!(has_live_markers(&format!(
            "- <<<<<<< YOURS (merge {token})\n"
        )));
        assert!(has_live_markers(&format!(
            "  - ======= (merge {token})  \n"
        )));
        assert!(has_live_markers(&format!(">>>>>>> THEIRS (merge {token})")));

        // Notes that talk about conflict markers are left alone.
        for text in [
            "<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> branch\n",
            "<<<<<<< YOURS\nx\n=======\ny\n>>>>>>> THEIRS\n",
            "- use <<<<<<< YOURS (merge 12345678) in prose\n",
            "<<<<<<< YOURS (merge NOTAHEX1)\n",
            "<<<<<<< YOURS (merge 1234567)\n",
            "======= (merge 123456789)\n",
        ] {
            assert!(!has_live_markers(text), "{text:?}");
        }
    }

    #[test]
    fn invalid_utf8_is_still_scanned_for_markers() {
        let mut bytes = b"- caf\xe9\n".to_vec();
        bytes.extend_from_slice(b"<<<<<<< YOURS (merge 0123abcd)\n");
        assert!(scan_marker_bytes(&bytes).is_some());
        assert!(scan_marker_bytes(b"- caf\xe9\n").is_none());
    }

    #[test]
    fn a_partly_resolved_hunk_still_counts_as_unresolved() {
        let result = merged("- a\n", "- b\n", "- c\n");
        let without_open: String = result
            .content
            .lines()
            .filter(|line| !line.starts_with("<<<<<<<"))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(scan_markers(&without_open).unwrap().hunks, 1);
        let resolved: String = result
            .content
            .lines()
            .filter(|line| !line.contains("(merge "))
            .map(|line| format!("{line}\n"))
            .collect();
        assert!(!has_live_markers(&resolved));
    }

    #[test]
    fn oversized_or_far_apart_inputs_decline_to_merge() {
        let big = "x\n".repeat(MAX_MERGE_BYTES / 2 + 1);
        assert!(three_way_merge("a\n", &big, "b\n").is_none());
        assert!(two_way_merge(&big, "b\n").is_none());

        let base: String = (0..MAX_EDITS + 10)
            .map(|i| format!("- base {i}\n"))
            .collect();
        let local: String = (0..MAX_EDITS + 10)
            .map(|i| format!("- mine {i}\n"))
            .collect();
        assert!(three_way_merge(&base, &local, "- other\n").is_none());
        // Identical oversized text never needs a merge.
        assert_eq!(three_way_merge("a\n", &big, &big).unwrap().content, big);
    }

    #[test]
    fn a_large_note_with_scattered_edits_merges_quickly() {
        let base: String = (0..30_000)
            .map(|i| format!("- line {i}\n  id:: {i}\n"))
            .collect();
        let mut local = base.clone();
        let mut remote = base.clone();
        for step in 0..40 {
            let mine = 100 + step * 500;
            let theirs = 350 + step * 500;
            local = local.replacen(
                &format!("- line {mine}\n"),
                &format!("- line {mine} mine\n"),
                1,
            );
            remote = remote.replacen(
                &format!("- line {theirs}\n"),
                &format!("- line {theirs} theirs\n"),
                1,
            );
        }
        let started = std::time::Instant::now();
        let result = merged(&base, &local, &remote);
        assert!(!result.has_conflicts());
        assert_eq!(result.content.matches(" mine\n").count(), 40);
        assert_eq!(result.content.matches(" theirs\n").count(), 40);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "merge took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn merging_never_invents_text_and_is_symmetric() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |bound: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % bound) as usize
        };
        let pool = ["- a", "- b", "- c", "- d", "- e", "  id:: 1", "", "plain"];
        let make = |next: &mut dyn FnMut(u64) -> usize, len: usize| -> String {
            (0..len)
                .map(|_| format!("{}\n", pool[next(pool.len() as u64)]))
                .collect()
        };
        let mutate = |next: &mut dyn FnMut(u64) -> usize, text: &str| -> String {
            let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
            for _ in 0..next(3) {
                let at = next(lines.len() as u64 + 1);
                let pick = next(pool.len() as u64);
                match next(3) {
                    0 if at < lines.len() => {
                        lines.remove(at);
                    }
                    1 => lines.insert(at, pool[pick].to_string()),
                    _ if at < lines.len() => lines[at] = pool[pick].to_string(),
                    _ => {}
                }
            }
            lines.iter().map(|l| format!("{l}\n")).collect()
        };
        for _ in 0..2000 {
            let len = 1 + next(8);
            let base = make(&mut next, len);
            let local = mutate(&mut next, &base);
            let remote = mutate(&mut next, &base);
            let result = merged(&base, &local, &remote);

            let allowed: std::collections::HashSet<&str> = local
                .lines()
                .chain(remote.lines())
                .chain(base.lines())
                .collect();
            for line in result.content.lines() {
                assert!(
                    marker_line(line).is_some() || allowed.contains(line),
                    "invented line {line:?}\nbase {base:?}\nlocal {local:?}\nremote {remote:?}"
                );
            }
            let hunks = scan_markers(&result.content).map(|scan| scan.hunks);
            assert_eq!(
                hunks,
                result.has_conflicts().then_some(result.conflict_count)
            );

            let swapped = merged(&base, &remote, &local);
            assert_eq!(result.has_conflicts(), swapped.has_conflicts());
            if !result.has_conflicts() {
                assert_eq!(result.content, swapped.content, "base {base:?}");
            }
        }
    }
}
