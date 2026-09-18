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

Grafium never resolves conflicts automatically. When a sync is running — and for
a few minutes afterwards — a **Conflicts** tab appears in the right panel. It
also comes back on its own, with a count, whenever conflicts are still
unresolved, including after a restart. Once everything is resolved the tab goes
away again, so it does not take up space on a graph that has never conflicted.

To resolve one: open the **Conflicts** tab, click the note, edit the normal note
into the version you want, and save.
Sync carries your notes: `pages/`, `journals/`, `knowledge/` and `assets/`.
It deliberately leaves everything else behind, including the search index and
your **Chat** conversations. Chats stay on the machine they happened on — a
transcript can quote notes the other end has no business receiving, and a
half-finished thread is not something you want merged on a shared drive. If an
answer is worth keeping, ask Chat to save it into a page and that page syncs
like any other note.
