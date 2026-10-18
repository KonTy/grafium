//! Sequence alignment for the note merge: Myers' O(ND) shortest-edit-script
//! search over pre-hashed lines, with a hard budget on the edit distance.
//!
//! Pure and allocation-light. A typical note edit is a handful of lines, so
//! the common prefix and suffix are stripped first and the search only runs
//! over the changed middle. The budget bounds both time and memory
//! (`O(budget²)` words); callers treat an exceeded budget as "do not merge".

/// Marks a position of the first sequence that has no partner in the second.
pub(crate) const NONE: usize = usize::MAX;

const INVALID: i32 = -1;

/// For each index of `a`, the index of `b` it is paired with, or [`NONE`].
///
/// Pairs are strictly increasing in both sequences and their count is the
/// longest common subsequence length. Returns `None` when `a` and `b` differ by
/// more than `max_edits` insertions plus deletions. A change that is only
/// insertions or only deletions (after the common prefix and suffix) needs no
/// search, so it is always aligned however large it is.
pub(crate) fn align(a: &[u64], b: &[u64], max_edits: usize) -> Option<Vec<usize>> {
    let mut pairs = vec![NONE; a.len()];
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    for (index, slot) in pairs.iter_mut().enumerate().take(prefix) {
        *slot = index;
    }
    let shorter = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(shorter)
        .take_while(|(x, y)| x == y)
        .count();
    for back in 0..suffix {
        pairs[a.len() - 1 - back] = b.len() - 1 - back;
    }
    let (a_mid, b_mid) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    shortest_edit_script(a_mid, b_mid, max_edits, |i, j| {
        pairs[prefix + i] = prefix + j;
    })?;
    Some(pairs)
}

/// The next furthest-reaching point on diagonal `k = 2i - d` of round `d`,
/// derived from round `d - 1`; `None` when the diagonal is outside the grid.
/// Returns the x coordinate after the edit move, and whether it was a move
/// down (an insertion from `b`) rather than right (a deletion from `a`).
fn step(prev: Option<&[i32]>, i: usize, n: i32, m: i32) -> Option<(i32, bool)> {
    let Some(prev) = prev else {
        return Some((0, true));
    };
    let k = 2 * i as i32 - prev.len() as i32; // d = prev.len()
    let down = prev
        .get(i)
        .copied()
        .filter(|&x| x != INVALID && x - (k + 1) < m);
    let right = i
        .checked_sub(1)
        .and_then(|j| prev.get(j))
        .copied()
        .filter(|&x| x != INVALID && x < n)
        .map(|x| x + 1);
    match (down, right) {
        (Some(d), Some(r)) if d >= r => Some((d, true)),
        (Some(_), Some(r)) => Some((r, false)),
        (Some(d), None) => Some((d, true)),
        (None, Some(r)) => Some((r, false)),
        (None, None) => None,
    }
}

fn shortest_edit_script(
    a: &[u64],
    b: &[u64],
    max_edits: usize,
    mut on_match: impl FnMut(usize, usize),
) -> Option<()> {
    if a.is_empty() || b.is_empty() {
        return Some(());
    }
    let (n, m) = (a.len() as i32, b.len() as i32);
    let budget = max_edits.min(a.len() + b.len());
    // trace[d][i] is the furthest x reached on diagonal k = 2i - d in round d.
    let mut trace: Vec<Vec<i32>> = Vec::new();
    for d in 0..=budget {
        let prev = trace.last().map(Vec::as_slice);
        let mut round = vec![INVALID; d + 1];
        for (i, slot) in round.iter_mut().enumerate() {
            let Some((mut x, _)) = step(prev, i, n, m) else {
                continue;
            };
            let k = 2 * i as i32 - d as i32;
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            *slot = x;
            if x == n && y == m {
                trace.push(round);
                backtrack(&trace, n, m, &mut on_match);
                return Some(());
            }
        }
        trace.push(round);
    }
    None
}

fn backtrack(trace: &[Vec<i32>], n: i32, m: i32, on_match: &mut impl FnMut(usize, usize)) {
    let (mut x, mut y) = (n, m);
    for d in (1..trace.len()).rev() {
        let k = x - y;
        let i = ((k + d as i32) / 2) as usize;
        let (start_x, from_down) = step(Some(&trace[d - 1]), i, n, m)
            .expect("a recorded furthest-reaching point has a predecessor");
        for back in 1..=(x - start_x) {
            on_match((x - back) as usize, (y - back) as usize);
        }
        x = if from_down { start_x } else { start_x - 1 };
        y = x - (if from_down { k + 1 } else { k - 1 });
    }
    // Round 0 is a pure snake from the origin.
    for back in 1..=x {
        on_match((x - back) as usize, (y - back) as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference implementation: textbook O(nm) dynamic programme.
    fn lcs_len(a: &[u64], b: &[u64]) -> usize {
        let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in 1..=a.len() {
            for j in 1..=b.len() {
                table[i][j] = if a[i - 1] == b[j - 1] {
                    table[i - 1][j - 1] + 1
                } else {
                    table[i - 1][j].max(table[i][j - 1])
                };
            }
        }
        table[a.len()][b.len()]
    }

    fn assert_valid_lcs(a: &[u64], b: &[u64], pairs: &[usize]) {
        assert_eq!(pairs.len(), a.len());
        let mut last: Option<usize> = None;
        let mut matched = 0;
        for (i, &j) in pairs.iter().enumerate() {
            if j == NONE {
                continue;
            }
            assert_eq!(a[i], b[j], "paired elements must be equal");
            if let Some(last) = last {
                assert!(j > last, "pairs must be strictly increasing: {pairs:?}");
            }
            last = Some(j);
            matched += 1;
        }
        assert_eq!(matched, lcs_len(a, b), "not a longest common subsequence");
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self, bound: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % bound
        }
    }

    #[test]
    fn matches_the_dynamic_programme_on_random_sequences() {
        let mut rng = Rng(0x9e3779b97f4a7c15);
        for _ in 0..3000 {
            let alphabet = 1 + rng.next(5);
            let a: Vec<u64> = (0..rng.next(14)).map(|_| rng.next(alphabet)).collect();
            let b: Vec<u64> = (0..rng.next(14)).map(|_| rng.next(alphabet)).collect();
            let pairs = align(&a, &b, usize::MAX / 2).expect("unbounded budget never gives up");
            assert_valid_lcs(&a, &b, &pairs);
        }
    }

    #[test]
    fn handles_empty_identical_and_disjoint_inputs() {
        assert_eq!(align(&[], &[], 10), Some(vec![]));
        assert_eq!(align(&[1, 2], &[], 10), Some(vec![NONE, NONE]));
        assert_eq!(align(&[], &[1, 2], 10), Some(vec![]));
        assert_eq!(align(&[1, 2, 3], &[1, 2, 3], 0), Some(vec![0, 1, 2]));
        assert_eq!(align(&[1, 2], &[3, 4], 10), Some(vec![NONE, NONE]));
    }

    #[test]
    fn single_edit_in_the_middle_pairs_everything_else() {
        let a = [1, 2, 3, 4, 5];
        let b = [1, 2, 9, 4, 5];
        assert_eq!(align(&a, &b, 4), Some(vec![0, 1, NONE, 3, 4]));
    }

    #[test]
    fn gives_up_when_the_edit_distance_exceeds_the_budget() {
        let a: Vec<u64> = (0..50).collect();
        let b: Vec<u64> = (100..150).collect();
        assert!(align(&a, &b, 10).is_none());
        assert!(align(&a, &b, 100).is_some());
        // Prefix and suffix are free: they never count against the budget.
        let mut replaced = a.clone();
        replaced[25] = 999;
        assert!(
            align(&a, &replaced, 1).is_none(),
            "a replacement is two edits"
        );
        assert!(align(&a, &replaced, 2).is_some());
        // A pure insertion or deletion needs no search, whatever its size.
        let mut grown = a.clone();
        grown.splice(25..25, 1000..1500);
        assert!(align(&a, &grown, 0).is_some());
        assert_eq!(
            align(&grown, &a, 0)
                .unwrap()
                .iter()
                .filter(|&&j| j == NONE)
                .count(),
            500
        );
    }

    #[test]
    fn large_inputs_with_few_edits_are_fast() {
        let a: Vec<u64> = (0..200_000).collect();
        let mut b = a.clone();
        b.remove(70_000);
        b.insert(150_000, u64::MAX);
        let started = std::time::Instant::now();
        let pairs = align(&a, &b, 64).expect("two edits fit comfortably");
        // Only the removed line of `a` is unpaired; the insertion lives in `b`.
        assert_eq!(pairs.iter().filter(|&&j| j == NONE).count(), 1);
        assert_eq!(pairs[70_000], NONE);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "aligning two edits in 200k lines took {:?}",
            started.elapsed()
        );
    }
}
