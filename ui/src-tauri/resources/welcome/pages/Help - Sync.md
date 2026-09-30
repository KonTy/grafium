# Help - Sync

Use **Settings → Sync** to add one or more independent filesystem or WebDAV
targets. A filesystem target can be a USB drive or network folder.

1. Add each location separately.
2. Mount one location.
3. Click **Sync Now** for that target.
4. Sync before disconnecting and after connecting it elsewhere.

A first sync merges both sides: remote-only files come down, local-only files
go up, and a file that exists on both sides with different contents becomes a
conflict rather than an overwrite. Nothing is replaced silently. If you want a
USB graph on its own rather than merged into the graph you already have here,
create an empty graph first and sync into that.

A target belongs to the graph it was first synced with. Point a graph at a
location holding a different graph and Grafium stops with an explanation
instead of merging or deleting. Targets are independent, so a USB drive and a
file server never have to be connected at the same time.

For adjacent original-book `.jsonld` annotations, sync combines independent notes
and compatible revisions, but never chooses between competing edits to one note.
Open that book's **Notes** to compare every candidate, manually combine the
Markdown, explicitly choose its attachment, and **Resolve with merged note**.
Deletion versus an edit also needs an explicit choice; **Resolve as deleted…**
requires confirmation. A stale choice is refused and your draft is kept while
candidates reload. See [[Help - Books]]. For example, `1.epub` keeps its notes in
`1.jsonld`; the original book is never rewritten.
Sync's status reports **books have notes to merge; open Book notes** separately
from ordinary file conflicts. This is not a request to choose one entire
JSON-LD file over the other: both sets of note candidates are already retained.

Other file conflicts are not resolved automatically. When a sync is running — and for
a few minutes afterwards — a **Conflicts** tab appears in the right panel. It
also comes back on its own, with a count, whenever conflicts are still
unresolved, including after a restart. Once everything is resolved the tab goes
away again, so it does not take up space on a graph that has never conflicted.

To resolve one, open the **Conflicts** tab and compare the versions. You may edit
the normal Markdown note first, but editing or opening a file does not by itself
resolve a conflict. Click **Resolve**, choose **Local** or **Remote**, then
**Confirm choice**. A missing file is shown as **deleted**, not as an empty file:
choosing that side confirms deletion on the other side.

The choice applies only if both versions still match the ones you reviewed.
If either changed, reload the versions and choose again. This works for original
books, images, and other binary files without modifying their selected bytes.
Both available versions are retained under `sync-recovery/` in this graph's
metadata folder. Recovery files are not indexed as notes or propagated by sync.

Failed, malformed, or incomplete remote inventories stop sync instead of being
treated as deletions. WebDAV replacement/deletion requires a strong server ETag
and a conditional request. Filesystem targets recheck revisions and retain
recovery copies, but cannot exclude an uncooperative external application
writing at exactly the same instant. Avoid simultaneous editing and keep backups.

Sync carries your notes: `pages/`, `journals/`, `knowledge/`, `assets/`, and the
managed originals, metadata, and reading positions in `books/`.
Adjacent JSON-LD book annotations and legacy Markdown notes both sync.
Malformed or incompatible annotation files remain ordinary file conflicts
instead of being silently replaced. Derived reader caches do not sync.
It deliberately leaves everything else behind, including the search index and
your **Chat** conversations. Chats stay on the machine they happened on — a
transcript can quote notes the other end has no business receiving, and a
half-finished thread is not something you want merged on a shared drive. If an
answer is worth keeping, ask Chat to save it into a page and that page syncs
like any other note.
