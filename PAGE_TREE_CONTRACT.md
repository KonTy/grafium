# Page Tree & Collections — API contract

Frozen interface so backend and frontend can be built in parallel. **Do not
change a name or shape here without saying so loudly in your final report.**

## Background (already true, do not rebuild)

- `[[a/b/c]]` and `#a/b/c` already auto-create `a`, `a/b`, `a/b/c` as real
  pages. `\` normalizes to `/`. Verified working.
- `Database::get_parent_page(title)` and `get_child_pages(parent_title)` exist.
- `Page.properties` is a `serde_json::Value` — usable for marking collections
  without a schema migration.
- `Block` has `order` and belongs to a page — ordering already exists.

The gap is **presentation**, not storage.

## Rust types (`core/src/knowledge/tree.rs` or similar)

```rust
/// One node in a page tree. Children are already sorted.
pub struct TreeNode {
    /// Full title for a real page (`"tech/linux"`), or the synthesized path
    /// for a grouping node that has no page of its own.
    pub key: String,
    /// Last path segment — what the UI displays (`"linux"`).
    pub label: String,
    /// `Some` when a real page exists at this path; `None` for a pure grouping
    /// node. The UI must not offer navigation for `None`.
    pub page_id: Option<String>,
    pub children: Vec<TreeNode>,
    /// Pages at or below this node, for a count badge.
    pub descendant_count: usize,
}

pub enum TreeKind { Namespace, Tag }
```

`build_namespace_tree(pages) -> Vec<TreeNode>` and
`build_tag_tree(pages_with_tags) -> Vec<TreeNode>` must be **pure functions**
over already-fetched data, so they are unit-testable without a database.

Rules both trees must obey:
- Nest on `/` after normalizing `\` → `/`.
- A missing intermediate is a grouping node (`page_id: None`), never dropped.
- Pages with no `/` are roots.
- Sort case-insensitively by `label`; stable for equal labels.
- Journal pages are excluded from the namespace tree (they are dated, not
  organized).

## Collections (books/projects) — phase 3

A collection is an ordinary page marked with **flat string** properties:

```json
{ "collection": "book", "collection-status": "draft" }
```

**Flat, not nested, and this is load-bearing.** The markdown serializer only
emits `key:: value` for *string* values, and indexing a file replaces a page's
properties with whatever the parser read back — so a nested marker was written
to the database and then silently erased by the next reindex, watcher event or
sync pull, with the collections list simply coming back empty and no error to
explain it. A flat string round-trips through markdown, which also means the
marking travels between devices in the file itself.

Both decoders — `collection_of` in Rust and `getCollectionKind` in TypeScript —
read this same wire data and must stay byte-compatible.

Its **ordered members are its blocks**, each containing a `[[page link]]`.
Ordering, drag-to-reorder, and free-form brainstorm text therefore come from
the existing block editor for free. **Do not invent a membership table.**

```rust
pub struct CollectionInfo { pub kind: String, pub status: Option<String> }
pub fn collection_of(page: &Page) -> Option<CollectionInfo>;
pub fn mark_collection(props: &mut serde_json::Value, kind: &str);
pub fn clear_collection(props: &mut serde_json::Value);
```

## Tauri commands

```
pages_namespace_tree() -> Vec<TreeNodeDto>
pages_tag_tree() -> Vec<TreeNodeDto>
page_set_collection(page_id: String, kind: Option<String>) -> ()   // None clears
pages_list_collections() -> Vec<CollectionSummaryDto>              // id, title, kind, member_count
```

Payloads are **snake_case** (serde default, no `rename_all`), matching every
existing command. Tauri maps camelCase JS argument names to snake_case Rust
params — `pageId` in JS, `page_id` in Rust — exactly as `ai_ask_stream` does.

## Frontend (`ui/src/lib/pageTree.ts`)

```ts
export interface TreeNode { key, label, page_id, children, descendant_count }
export function pagesNamespaceTree(): Promise<TreeNode[]>
export function pagesTagTree(): Promise<TreeNode[]>
export function pageSetCollection(pageId: string, kind: string | null): Promise<void>
export function pagesListCollections(): Promise<CollectionSummary[]>
```

## UI surfaces

1. **Sidebar** — collapsible tree above/replacing the flat page list. Expansion
   state persists across restarts. Clicking a node with a `page_id` navigates;
   a grouping node only expands.
2. **All Pages** — same tree, with a Tree/List toggle. Reuse the same component
   as the sidebar; do not write it twice.
3. **Tree source toggle** — Namespace / Tags.
4. **Collections** — a page can be marked a collection from its page menu.
   Collection pages get a header showing kind and member count, and their
   linked members render as an ordered list. A "My Books" namespace plus
   per-book collection pages is the intended workflow; do not build a separate
   folder system.
