//! Turning the flat page table into the hierarchical trees the sidebar shows.
//!
//! Pages are stored flat — one row per title — even though users clearly think
//! in hierarchies: `[[tech/linux/systemd]]` reads as a path, and the `/`
//! auto-creates `tech` and `tech/linux` as their own pages. The storage layer
//! deliberately keeps them flat (it makes every other query — search, links,
//! backlinks — a plain lookup instead of a recursive walk), so the *tree* is
//! purely a presentation concern reconstructed on demand. That is what this
//! module does.
//!
//! Two things drove the shape of the code:
//!
//!   1. **These are pure functions over already-fetched pages, not database
//!      queries.** Rebuilding a tree touches every page, so doing it with
//!      recursive SQL (or one query per node) would be exactly the O(n²) trap
//!      the flat schema exists to avoid. Instead a caller fetches the page set
//!      once and hands it here. The bonus is that the interesting logic —
//!      synthesizing missing parents, sorting, counting — is unit-testable
//!      without a database at all.
//!   2. **A path segment with no page of its own must still appear.** If the
//!      only page is `tech/linux/systemd` but nothing ever created `tech`, the
//!      tree still needs a `tech` node to hang the branch on, or the page
//!      vanishes from the sidebar entirely. Those synthesized nodes carry no
//!      [`TreeNode::page_id`], so the UI renders them as un-navigable grouping
//!      rows rather than dangling links.
//!
//! The namespace tree and the tag tree are the *same* construction over
//! different page sets, so they share one core ([`build_tree`]): the namespace
//! tree nests every non-journal page by its title, and the tag tree nests the
//! pages that are used as tags by their tag path. Keeping them one algorithm is
//! why the frontend can render both with a single component and a source
//! toggle.

use crate::models::Page;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One node in a page tree, with its children already sorted.
///
/// This is the wire shape the frontend consumes verbatim, so its field names
/// are frozen and it serializes with serde's default **snake_case** (no
/// `rename_all`) to match every other command payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeNode {
    /// Full title for a real page (`"tech/linux"`), or the synthesized path for
    /// a grouping node that has no page of its own (still `"tech/linux"`, just
    /// with no page behind it). Unique within the tree, so the UI can use it as
    /// a stable key for persisted expansion state.
    pub key: String,
    /// Last path segment — what the UI displays (`"linux"`). Split out so the
    /// UI never has to re-parse `key`, and so it stays right even for the
    /// degenerate titles handled in [`build_tree`].
    pub label: String,
    /// `Some` when a real page exists at this exact path; `None` for a pure
    /// grouping node. The UI keys navigation off this: a `None` node only
    /// expands, it never pretends to be a page you can open.
    pub page_id: Option<String>,
    pub children: Vec<TreeNode>,
    /// Count of real pages at or below this node (itself included when it is a
    /// real page). Drives the badge that tells a user how much a collapsed
    /// branch is hiding, which a raw child count could not — a branch can be
    /// one grouping node deep yet hold fifty pages.
    pub descendant_count: usize,
    /// Newest `updated_at` at or below this node, in epoch milliseconds.
    ///
    /// A folder takes its newest page's timestamp so that sorting by recency
    /// floats a book you touched this morning to the top, rather than stranding
    /// it wherever its name happens to fall. `0` for a grouping node whose
    /// subtree somehow contains no page.
    pub updated_at: i64,
}

/// Which taxonomy a tree presents. Kept as a plain enum (rather than two
/// unrelated functions the caller has to remember) so a UI toggle can pass the
/// choice around as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreeKind {
    Namespace,
    Tag,
}

/// Build the namespace tree: every page nested under its title path.
///
/// Journals are excluded on purpose. They are dated captures (`2024-06-01`),
/// not a place in a deliberate hierarchy, so folding them in would bury the
/// actual structure under a wall of date-named roots — and their `/`-free
/// titles would otherwise each land as a top-level node. Excluding them here
/// (rather than asking the caller to pre-filter) keeps the rule in one place
/// and makes it testable without a database.
pub fn build_namespace_tree(pages: &[Page]) -> Vec<TreeNode> {
    build_tree(
        pages
            .iter()
            .filter(|page| !page.is_journal)
            .map(|page| (page.title.as_str(), page.id.as_str(), page.updated_at)),
    )
}

/// Build the tag tree: the pages that are *used as tags* nested by their tag
/// path.
///
/// In this codebase a tag is a real page (writing `#tech/linux` creates the
/// `tech/linux` page and points a tag-typed link at it), so the tag tree is the
/// exact same construction as the namespace tree over a narrower set: the pages
/// that some block tags. The caller supplies that set — hence `tag_pages`,
/// which is the concrete reading of the contract's `pages_with_tags`. A segment
/// that is only an ancestor of a tag and never tagged itself (the `tech` above,
/// if nothing tags `#tech` directly) surfaces as a grouping node, the same way
/// a missing intermediate does in the namespace tree.
pub fn build_tag_tree(tag_pages: &[Page]) -> Vec<TreeNode> {
    build_tree(
        tag_pages
            .iter()
            .map(|page| (page.title.as_str(), page.id.as_str(), page.updated_at)),
    )
}

/// Scratch node used while assembling the tree in an arena.
///
/// Children are held as arena indices rather than owned `TreeNode`s so that a
/// deep branch is built with cheap `usize` pushes and never moved or cloned
/// mid-build; the owned tree is materialized once at the end by [`assemble`].
struct Scratch {
    key: String,
    label: String,
    page_id: Option<String>,
    children: Vec<usize>,
    updated_at: i64,
}

/// The shared core behind both public builders.
///
/// Takes `(title, page_id)` for every real page that should appear and returns
/// the sorted roots. The rules it enforces — normalize `\` to `/`, nest on `/`,
/// synthesize missing intermediates as `page_id: None`, sort case-insensitively
/// and stably by label — are the contract both trees obey.
///
/// ## Complexity
///
/// One arena entry is created per distinct path prefix, and each entry is
/// linked to its parent exactly once (at creation), so there are no duplicate
/// edges to scrub later. Construction is `O(T)` where `T` is the total number
/// of path segments across all titles (≈ pages × average depth). Materializing
/// the tree sorts each node's children once; summed over the tree that is
/// `O(N log N)` for `N` nodes (`N ≤ T`). Overall `O(T log T)` — linearithmic,
/// with no per-page rescan that would make it quadratic on a large graph.
/// The key a title nests under: backslashes normalized and empty segments
/// dropped. A title equal to its own canonical key is "tidy" and owns that
/// node; anything else is a messy spelling of it.
fn canonical_key(title: &str) -> String {
    title
        .replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn build_tree<'a>(entries: impl Iterator<Item = (&'a str, &'a str, i64)>) -> Vec<TreeNode> {
    let mut arena: Vec<Scratch> = Vec::new();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();

    // Order matters when two titles normalize onto one key (`a//b` and `a/b`,
    // or `tech\linux` and `tech/linux`). Both pages are real — titles are
    // UNIQUE in the database — so the node has to belong to one of them
    // *predictably*, not to whichever the iterator happened to yield first.
    // Pages whose title already equals its normalized key are processed first
    // and therefore own it; the messy spellings fall through to a literal node
    // of their own below.
    let mut ordered: Vec<(&str, &str, i64)> = entries.collect();
    ordered.sort_by_key(|(title, _, _)| canonical_key(title) != *title);

    for (title, page_id, updated_at) in ordered {
        let normalized = title.replace('\\', "/");

        // Empty segments (leading/trailing/doubled slashes) are dropped so
        // `a//b` collapses to `a/b` and a stray trailing slash is ignored. That
        // keeps normal titles' keys identical to their titles while making the
        // nesting robust to messy input.
        let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();

        // A title that is *only* separators ("//", "/", "") has no usable
        // segment, but the page is real and must not silently disappear. Give
        // it a single literal root node keyed by its normalized title so it is
        // still reachable instead of dropped.
        if segments.is_empty() {
            let idx = get_or_create(
                &mut arena,
                &mut index_of,
                &mut roots,
                None,
                &normalized,
                &normalized,
            );
            arena[idx].page_id = Some(page_id.to_string());
            arena[idx].updated_at = arena[idx].updated_at.max(updated_at);
            continue;
        }

        let mut prefix = String::new();
        let mut parent: Option<usize> = None;
        let last = segments.len() - 1;
        for (depth, segment) in segments.iter().enumerate() {
            if depth > 0 {
                prefix.push('/');
            }
            prefix.push_str(segment);

            let idx = get_or_create(
                &mut arena,
                &mut index_of,
                &mut roots,
                parent,
                &prefix,
                segment,
            );
            // Only the final segment corresponds to the page itself; ancestors
            // stay grouping nodes unless they are themselves some page's final
            // segment (processed on their own iteration, in any order).
            if depth == last {
                // Two distinct pages can normalize onto one key — `a//b` and
                // `a/b`, or `tech\linux` and `tech/linux` — and overwriting
                // here made the loser vanish from the tree entirely, with no
                // error and no way to reach it. Titles are UNIQUE in the
                // database, so both pages are real; the first claim keeps the
                // shared node and the other gets its own node keyed by its
                // literal title, so every page stays reachable.
                if arena[idx].page_id.is_none() {
                    arena[idx].page_id = Some(page_id.to_string());
                    arena[idx].updated_at = arena[idx].updated_at.max(updated_at);
                } else if arena[idx].key != title {
                    let literal =
                        get_or_create(&mut arena, &mut index_of, &mut roots, parent, title, title);
                    arena[literal].page_id = Some(page_id.to_string());
                    // Without this the page is dated 0 and sinks to the bottom
                    // of a recency sort for good.
                    arena[literal].updated_at = arena[literal].updated_at.max(updated_at);
                }
            }
            parent = Some(idx);
        }
    }

    sort_indices(&arena, &mut roots);
    roots.iter().map(|&idx| assemble(&arena, idx)).collect()
}

/// Look up the node for `key`, creating and parenting it on first sight.
///
/// Creating-and-linking in the same step is what guarantees each node is linked
/// to its parent exactly once, so child lists never need de-duplication no
/// matter how many pages share a prefix.
fn get_or_create(
    arena: &mut Vec<Scratch>,
    index_of: &mut HashMap<String, usize>,
    roots: &mut Vec<usize>,
    parent: Option<usize>,
    key: &str,
    label: &str,
) -> usize {
    if let Some(&idx) = index_of.get(key) {
        return idx;
    }
    let idx = arena.len();
    arena.push(Scratch {
        key: key.to_string(),
        label: label.to_string(),
        page_id: None,
        children: Vec::new(),
        // Grouping nodes carry no date of their own; `assemble` rolls up the
        // newest date from the pages underneath.
        updated_at: 0,
    });
    index_of.insert(key.to_string(), idx);
    match parent {
        Some(p) => arena[p].children.push(idx),
        None => roots.push(idx),
    }
    idx
}

/// Sort a sibling group: branches first, then case-insensitively by label.
///
/// Branches lead because they are the structure of the tree — with a few
/// hundred leaf pages, folders sorted alphabetically among them are scattered
/// and effectively unfindable. Every file manager and IDE orders this way for
/// the same reason.
///
/// `sort_by_cached_key` is stable, so siblings whose labels differ only in case
/// (the one way distinct siblings can compare equal — same-cased siblings would
/// share a key and be the same node) keep their insertion order, which is the
/// "stable for equal labels" the contract asks for. Caching the key also folds
/// each label once instead of on every comparison.
fn sort_indices(arena: &[Scratch], indices: &mut [usize]) {
    indices.sort_by_cached_key(|&idx| {
        (
            arena[idx].children.is_empty(),
            arena[idx].label.to_lowercase(),
        )
    });
}

/// Materialize the owned `TreeNode` for one arena entry.
///
/// Iterative rather than recursive on purpose. Depth here is the segment count
/// of a page title, which is user data — a hashtag like `#a/a/a/…` nested
/// thousands deep is enough to overflow the stack, and this runs while the
/// graph mutex is held, so a panic would poison it and take down every later
/// command rather than failing one request.
///
/// Two passes over an explicit stack: descend marking nodes, then build each
/// node once its children are already built. Children are sorted here (not at
/// insertion) so the arena can be built in one cheap pass, and
/// `descendant_count` is summed on the way back up, counting a node itself
/// only when a real page sits there.
fn assemble(arena: &[Scratch], root: usize) -> TreeNode {
    enum Step {
        Descend(usize),
        Build(usize),
    }

    let mut stack = vec![Step::Descend(root)];
    // Finished subtrees, keyed by arena index, consumed by their parent.
    let mut built: std::collections::HashMap<usize, TreeNode> = std::collections::HashMap::new();
    // Child order per node, computed once during the descent.
    let mut ordered: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();

    while let Some(step) = stack.pop() {
        match step {
            Step::Descend(idx) => {
                let mut child_indices = arena[idx].children.clone();
                sort_indices(arena, &mut child_indices);
                // Build runs after every child, because the stack pops in
                // reverse order of pushes.
                stack.push(Step::Build(idx));
                for &child in &child_indices {
                    stack.push(Step::Descend(child));
                }
                ordered.insert(idx, child_indices);
            }
            Step::Build(idx) => {
                let child_indices = ordered.remove(&idx).unwrap_or_default();
                let children: Vec<TreeNode> = child_indices
                    .into_iter()
                    .filter_map(|child| built.remove(&child))
                    .collect();

                let mut descendant_count = usize::from(arena[idx].page_id.is_some());
                let mut updated_at = arena[idx].updated_at;
                for child in &children {
                    descendant_count += child.descendant_count;
                    updated_at = updated_at.max(child.updated_at);
                }

                built.insert(
                    idx,
                    TreeNode {
                        key: arena[idx].key.clone(),
                        label: arena[idx].label.clone(),
                        page_id: arena[idx].page_id.clone(),
                        children,
                        descendant_count,
                        updated_at,
                    },
                );
            }
        }
    }

    built.remove(&root).expect("root is always built")
}

#[cfg(test)]
mod collision_tests {
    use super::*;
    use crate::models::Page;

    fn page(id: &str, title: &str) -> Page {
        Page {
            id: id.to_string(),
            title: title.to_string(),
            file_path: None,
            created_at: 0,
            updated_at: 0,
            is_journal: false,
            properties: serde_json::json!({}),
        }
    }

    fn page_at(id: &str, title: &str, updated_at: i64) -> Page {
        Page {
            updated_at,
            ..page(id, title)
        }
    }

    /// A folder is only as old as its newest page.
    ///
    /// Sorting by recency is meant to surface what you touched last. If a
    /// folder reported no date of its own, a book edited this morning would
    /// sink to the bottom of a recency sort and the feature would be useless
    /// for exactly the case it exists for.
    #[test]
    fn a_folder_takes_the_date_of_its_newest_page() {
        let pages = vec![
            page_at("1", "mybooks/coolbook/toc", 100),
            page_at("2", "mybooks/coolbook/plot", 900),
            page_at("3", "mybooks/oldbook/toc", 50),
            page_at("4", "loose", 500),
        ];

        let tree = build_namespace_tree(&pages);
        let mybooks = tree.iter().find(|n| n.label == "mybooks").unwrap();
        assert_eq!(mybooks.updated_at, 900, "newest page anywhere below it");

        let coolbook = mybooks
            .children
            .iter()
            .find(|n| n.label == "coolbook")
            .unwrap();
        assert_eq!(coolbook.updated_at, 900);

        let oldbook = mybooks
            .children
            .iter()
            .find(|n| n.label == "oldbook")
            .unwrap();
        assert_eq!(oldbook.updated_at, 50, "unaffected by a sibling branch");

        let loose = tree.iter().find(|n| n.label == "loose").unwrap();
        assert_eq!(loose.updated_at, 500);
    }

    /// Folders lead, then pages, each alphabetically.
    ///
    /// With a few hundred loose pages, a folder sorted alphabetically among
    /// them is scattered somewhere in the middle and effectively unfindable.
    #[test]
    fn branches_sort_before_loose_pages() {
        let pages = vec![
            page("1", "absorption"),
            page("2", "zebra"),
            page("3", "mybooks/coolbook/toc"),
            page("4", "biology"),
            page("5", "tech/linux"),
        ];

        let labels: Vec<String> = build_namespace_tree(&pages)
            .into_iter()
            .map(|n| n.label)
            .collect();

        assert_eq!(labels, vec!["mybooks", "tech", "absorption", "biology", "zebra"]);
    }

    /// A page that also has children is structure too, so it leads as well.
    #[test]
    fn a_page_with_children_sorts_with_the_folders() {
        let pages = vec![
            page("1", "aaa"),
            page("2", "tech"),
            page("3", "tech/linux"),
        ];

        let labels: Vec<String> = build_namespace_tree(&pages)
            .into_iter()
            .map(|n| n.label)
            .collect();

        assert_eq!(labels, vec!["tech", "aaa"]);
    }

    fn all_page_ids(nodes: &[TreeNode], out: &mut Vec<String>) {
        for n in nodes {
            if let Some(id) = &n.page_id {
                out.push(id.clone());
            }
            all_page_ids(&n.children, out);
        }
    }

    /// Titles are UNIQUE in the database, so two pages that merely *normalize*
    /// onto the same key are both real. Overwriting made one vanish from the
    /// sidebar with no error and no route to it.
    #[test]
    fn pages_that_normalize_to_the_same_key_are_both_reachable() {
        for (a, b) in [
            ("a//b", "a/b"),
            ("tech\\linux", "tech/linux"),
            ("/tech/x/", "tech/x"),
        ] {
            let pages = vec![page("id-a", a), page("id-b", b)];
            let tree = build_namespace_tree(&pages);
            let mut ids = Vec::new();
            all_page_ids(&tree, &mut ids);
            ids.sort();
            assert_eq!(
                ids,
                vec!["id-a".to_string(), "id-b".to_string()],
                "both {a:?} and {b:?} must appear in the tree"
            );
        }
    }

    /// Depth comes from a page title, which is user data — a deeply nested
    /// hashtag must not overflow the stack while the graph mutex is held.
    #[test]
    fn a_pathologically_deep_title_does_not_overflow_the_stack() {
        let deep = std::iter::repeat("a")
            .take(5_000)
            .collect::<Vec<_>>()
            .join("/");
        let pages = vec![page("deep", &deep)];
        let tree = build_namespace_tree(&pages);
        assert_eq!(tree.len(), 1);

        // Walk it iteratively; recursing here would defeat the point.
        let mut depth = 0usize;
        let mut cur = &tree[0];
        while let Some(next) = cur.children.first() {
            depth += 1;
            cur = next;
        }
        assert_eq!(depth, 4_999);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a regular page with the given title. Only `title`, `id`, and
    /// `is_journal` matter to the builders, so the rest take cheap defaults.
    fn page(id: &str, title: &str) -> Page {
        Page {
            id: id.to_string(),
            title: title.to_string(),
            file_path: None,
            created_at: 0,
            updated_at: 0,
            is_journal: false,
            properties: serde_json::json!({}),
        }
    }

    fn journal(id: &str, title: &str) -> Page {
        Page {
            is_journal: true,
            ..page(id, title)
        }
    }

    /// Find a node by its `key` anywhere in a forest (depth-first).
    fn find<'a>(nodes: &'a [TreeNode], key: &str) -> Option<&'a TreeNode> {
        for node in nodes {
            if node.key == key {
                return Some(node);
            }
            if let Some(found) = find(&node.children, key) {
                return Some(found);
            }
        }
        None
    }

    fn labels(nodes: &[TreeNode]) -> Vec<String> {
        nodes.iter().map(|n| n.label.clone()).collect()
    }

    #[test]
    fn empty_graph_yields_no_nodes() {
        assert!(build_namespace_tree(&[]).is_empty());
        assert!(build_tag_tree(&[]).is_empty());
    }

    #[test]
    fn page_without_slash_is_a_root_with_its_own_id() {
        let tree = build_namespace_tree(&[page("p1", "Inbox")]);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].key, "Inbox");
        assert_eq!(tree[0].label, "Inbox");
        assert_eq!(tree[0].page_id.as_deref(), Some("p1"));
        assert_eq!(tree[0].descendant_count, 1);
        assert!(tree[0].children.is_empty());
    }

    #[test]
    fn deep_nesting_chains_nodes_and_key_matches_full_title() {
        let pages = [
            page("a", "a"),
            page("ab", "a/b"),
            page("abc", "a/b/c"),
            page("abcd", "a/b/c/d"),
            page("abcde", "a/b/c/d/e"),
        ];
        let tree = build_namespace_tree(&pages);

        // A single root that funnels down one child at a time to the leaf.
        assert_eq!(tree.len(), 1);
        let leaf = find(&tree, "a/b/c/d/e").expect("deep leaf present");
        assert_eq!(leaf.label, "e");
        assert_eq!(leaf.page_id.as_deref(), Some("abcde"));
        assert!(leaf.children.is_empty());
    }

    #[test]
    fn descendant_count_includes_self_and_all_descendants() {
        let pages = [
            page("a", "a"),
            page("ab", "a/b"),
            page("abc", "a/b/c"),
            page("abcd", "a/b/c/d"),
            page("abcde", "a/b/c/d/e"),
        ];
        let tree = build_namespace_tree(&pages);

        assert_eq!(find(&tree, "a").unwrap().descendant_count, 5);
        assert_eq!(find(&tree, "a/b").unwrap().descendant_count, 4);
        assert_eq!(find(&tree, "a/b/c/d/e").unwrap().descendant_count, 1);
    }

    #[test]
    fn missing_intermediate_becomes_a_grouping_node() {
        // Only the deep leaf exists; nothing ever created `tech` or `tech/linux`.
        let tree = build_namespace_tree(&[page("leaf", "tech/linux/systemd")]);

        let tech = find(&tree, "tech").expect("synthesized parent present");
        assert_eq!(tech.page_id, None, "grouping node has no page");
        assert_eq!(tech.label, "tech");
        // The branch it hangs still counts the one real page beneath it.
        assert_eq!(tech.descendant_count, 1);

        let mid = find(&tree, "tech/linux").unwrap();
        assert_eq!(mid.page_id, None);

        let leaf = find(&tree, "tech/linux/systemd").unwrap();
        assert_eq!(leaf.page_id.as_deref(), Some("leaf"));
    }

    #[test]
    fn real_intermediate_keeps_its_page_id_regardless_of_order() {
        // The child is listed before the parent, to prove leaf-vs-intermediate
        // resolution does not depend on processing order.
        let tree = build_namespace_tree(&[page("child", "tech/linux"), page("parent", "tech")]);
        assert_eq!(
            find(&tree, "tech").unwrap().page_id.as_deref(),
            Some("parent")
        );
        assert_eq!(
            find(&tree, "tech/linux").unwrap().page_id.as_deref(),
            Some("child")
        );
    }

    #[test]
    fn backslashes_normalize_to_forward_slashes() {
        let tree = build_namespace_tree(&[page("leaf", "tech\\linux")]);
        assert!(find(&tree, "tech").is_some());
        let leaf = find(&tree, "tech/linux").expect("backslash path normalized");
        assert_eq!(leaf.label, "linux");
        assert_eq!(leaf.page_id.as_deref(), Some("leaf"));
    }

    #[test]
    fn backslash_titles_nest_under_the_same_parent() {
        // `tech\linux` nests as `tech/linux` — the separator is normalized, so
        // both spellings live under one `tech` parent rather than creating a
        // second root literally named "tech\linux".
        let tree = build_namespace_tree(&[page("a", "tech\\linux"), page("b", "tech/linux")]);
        let tech = find(&tree, "tech").unwrap();

        // Both pages are real rows with UNIQUE titles, so both must remain
        // reachable. An earlier version merged them and silently dropped one,
        // which removed a page from the sidebar with no error.
        assert_eq!(tech.descendant_count, 2, "neither page may be dropped");
        let ids: Vec<&str> = tech
            .children
            .iter()
            .filter_map(|c| c.page_id.as_deref())
            .collect();
        assert!(ids.contains(&"a") && ids.contains(&"b"));
    }

    #[test]
    fn siblings_sort_case_insensitively() {
        let tree =
            build_namespace_tree(&[page("b", "Banana"), page("a", "apple"), page("c", "Cherry")]);
        assert_eq!(labels(&tree), vec!["apple", "Banana", "Cherry"]);
    }

    #[test]
    fn equal_labels_are_sorted_stably_by_insertion_order() {
        // Two siblings whose labels differ only in case compare equal
        // case-insensitively, so a stable sort must preserve the order they
        // were supplied in.
        let forward = build_namespace_tree(&[page("f", "root/Foo"), page("g", "root/foo")]);
        assert_eq!(
            labels(&find(&forward, "root").unwrap().children),
            vec!["Foo", "foo"]
        );

        let reversed = build_namespace_tree(&[page("g", "root/foo"), page("f", "root/Foo")]);
        assert_eq!(
            labels(&find(&reversed, "root").unwrap().children),
            vec!["foo", "Foo"]
        );
    }

    #[test]
    fn duplicate_labels_under_different_parents_stay_distinct() {
        let tree = build_namespace_tree(&[page("w", "projects/web"), page("d", "docs/web")]);

        let under_projects = find(&tree, "projects/web").expect("projects/web present");
        let under_docs = find(&tree, "docs/web").expect("docs/web present");
        assert_eq!(under_projects.label, "web");
        assert_eq!(under_docs.label, "web");
        assert_eq!(under_projects.page_id.as_deref(), Some("w"));
        assert_eq!(under_docs.page_id.as_deref(), Some("d"));
    }

    #[test]
    fn journals_are_excluded_from_the_namespace_tree() {
        let tree = build_namespace_tree(&[
            page("w", "Work"),
            journal("j1", "2024-06-01"),
            journal("j2", "2024-06-02"),
        ]);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].key, "Work");
    }

    #[test]
    fn title_of_only_separators_is_preserved_not_dropped() {
        // "//" is nonsense as a path, but it is a real page and must still be
        // reachable rather than silently swallowed.
        let tree = build_namespace_tree(&[page("weird", "//")]);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].key, "//");
        assert_eq!(tree[0].label, "//");
        assert_eq!(tree[0].page_id.as_deref(), Some("weird"));
        assert_eq!(tree[0].descendant_count, 1);
    }

    #[test]
    fn leading_and_trailing_slashes_are_trimmed() {
        let tree = build_namespace_tree(&[page("p", "/tech/linux/")]);
        // The stray edge slashes drop away, leaving the same shape as
        // `tech/linux` instead of blank-labelled phantom nodes.
        assert!(find(&tree, "tech").is_some());
        assert_eq!(
            find(&tree, "tech/linux").unwrap().page_id.as_deref(),
            Some("p")
        );
    }

    #[test]
    fn tag_tree_nests_tag_pages_by_path() {
        let tags = [
            page("t1", "recipe/italian"),
            page("t2", "recipe/mexican"),
            page("t3", "rust"),
        ];
        let tree = build_tag_tree(&tags);

        // `rust` (no slash) is a root; `recipe` groups its two children.
        assert_eq!(labels(&tree), vec!["recipe", "rust"]);
        let recipe = find(&tree, "recipe").unwrap();
        assert_eq!(recipe.page_id, None);
        assert_eq!(recipe.descendant_count, 2);
        assert_eq!(labels(&recipe.children), vec!["italian", "mexican"]);
    }

    #[test]
    fn tag_tree_does_not_exclude_anything_the_way_namespace_does() {
        // The tag builder trusts its caller's set verbatim; it is the namespace
        // builder alone that drops journals.
        let tree = build_tag_tree(&[journal("j", "someday")]);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].key, "someday");
    }

    #[test]
    fn wide_forest_sorts_every_level() {
        let pages = [
            page("1", "Zeta/alpha"),
            page("2", "Zeta/Beta"),
            page("3", "alpha/Zed"),
            page("4", "alpha/aardvark"),
        ];
        let tree = build_namespace_tree(&pages);
        assert_eq!(labels(&tree), vec!["alpha", "Zeta"]);
        assert_eq!(
            labels(&find(&tree, "alpha").unwrap().children),
            vec!["aardvark", "Zed"]
        );
        assert_eq!(
            labels(&find(&tree, "Zeta").unwrap().children),
            vec!["alpha", "Beta"]
        );
    }
}
