# Help - Sync

Use **Settings → Sync** to add one or more independent filesystem or WebDAV
targets. A filesystem target can be a USB drive or network folder.

1. Add each location separately.
2. Mount one location.
3. Click **Sync Now** for that target.
4. Sync before disconnecting and after connecting it elsewhere.

A first sync merges both sides: remote-only files come down, local-only files
go up, and a file that exists on both sides with different contents becomes a
conflict rather than an overwrite. For a page or journal that means marker
lines in the note; see **Notes edited on two devices**. Nothing is replaced
silently. If you want a USB graph on its own rather than merged into the graph
you already have here, create an empty graph first and sync into that.

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

## Notes edited on two devices

When the same page or journal changed on both devices, sync compares the two
versions block by block instead of choosing one:

- **Different places combine on their own.** The merged note is saved here and
  uploaded. Differences that do not change the text, such as trailing spaces or
  the hidden block ids Grafium adds, never count as a conflict.
- **The same place is never guessed.** Grafium writes marker lines into the note
  on **this device only** and lists the note in the **Conflicts** tab. Sync's
  status reports **notes need merging; open Conflicts**.

The marker lines look like this, where the dots stand for a short code Grafium
adds:

```text
<<<<<<< YOURS (merge ········)
what this device has
======= (merge ········)
what the other device has
>>>>>>> THEIRS (merge ········)
```

To resolve a note, open it from the **Conflicts** tab. Each marker line is its
own block, shown in colour. Keep the text you want, delete the rest, and delete
all three marker lines. Repeat for every marked spot, then run **Sync Now**
again to upload the finished note.

Markers never travel. A note that still has a marker line is neither uploaded
nor replaced, so the other device never receives half-finished text and the two
devices cannot end up chasing each other's fixes. Your finished note arrives
there as an ordinary edit. Both original versions are kept under
`sync-recovery/` in this graph's metadata folder.

Reading notes, `knowledge/` files, assets, books, notes over 2 MiB and files
that are not UTF-8 text are not merged this way; they use the choice below.

## Other file conflicts

Other conflicts are not resolved automatically. When a sync is running — and for
a few minutes afterwards — a **Conflicts** tab appears in the right panel. It
also comes back on its own, with a count, whenever a note has marker lines or a
conflict is unresolved, including after a restart. Once everything is resolved
the tab goes away again, so it does not take up space on a graph that has never
conflicted.

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

## Recovery copies are cleaned up

Copies in `sync-recovery/` are a safety net, not an archive. Grafium trims them
after every sync that finishes without errors, so they cannot pile up:

- A copy is kept for **30 days**. If the copies together grow past about
  **256 MB**, the oldest go first.
- A copy is **never removed while its note still has an open conflict or marker
  lines**, however old it is. A copy made in the last hour is never removed
  either.
- Resolving a note starts a new 30 days for its copies, so there is time to
  change your mind.
- The saved starting versions Grafium uses to merge notes are deleted once
  nothing can use them, for example after you delete the note.
- Removing a sync target forgets its sync records right away. Its recovery
  copies, including any for conflicts you never resolved, are then trimmed by
  the same rules after the next sync.

Only files in the graph's metadata folder are deleted. Your notes, assets and
books are never touched. To keep a copy longer, move it out of `sync-recovery/`
before it expires.
