# Sync And Privacy

Sync copies graph files between independent locations. It is not live
collaboration and it is not a replacement for a separate backup.

## USB, file server, and WebDAV

1. Open **Settings → Sync**.
2. Add a filesystem target for a mounted USB drive or file-server folder, or
   add a WebDAV target for a server such as Nextcloud.
3. Give each location a distinct name.
4. Connect or mount one target, then choose **Sync Now**.
5. Safely disconnect the USB or wait for the file server before changing
   machines.
6. Configure the same target on the other machine and sync it there.

You can configure several targets at once. They keep independent sync state,
so you can sync to USB today and a file server tomorrow without both being
available simultaneously.

## Three common setups

### You have a graph on a USB stick and a fresh install

A first sync **merges** — it does not replace. Anything only on the USB is
pulled down, and anything already here is pushed up. So if you sync a USB graph
into the graph you were given at install, you get both, welcome pages included.

If you want the USB graph on its own, make an empty graph for it first:

1. Create a new graph and switch to it.
2. **Settings → Sync**, add a filesystem target pointing at the graph folder on
   the USB drive.
3. **Sync Now**. The USB contents come down into the empty graph.
4. Afterwards, sync before unplugging and again after plugging in elsewhere.

### You already have a graph here and want it on a USB stick

1. Take a dated backup first. Sync is not a backup.
2. **Settings → Sync**, add a filesystem target pointing at a folder on the
   drive. An empty folder is fine.
3. **Sync Now**. Everything is pushed up; nothing local is removed.
4. From then on it is the same two habits: sync before you unplug, sync after
   you plug in.

If the folder already holds a different Grafium graph, Grafium stops and says
so rather than merging two unrelated graphs. Point the target at an empty
folder instead.

### You want both a USB stick and a file server

Add them as separate targets and sync each one whenever it happens to be
available. Each target remembers its own state, so they never need to be
connected at the same time and neither falls behind permanently — the next sync
of a stale target catches it up.

One caution: a target is tied to the graph it was first synced with. Grafium
refuses to sync a graph to a location that belongs to a different graph,
because the only other option would be deleting one side.

## Conflicts and deletion safety

Grafium does not silently choose a winner. Conflicting edits remain available
for manual review in the right-panel **Conflicts** tab, which appears while a
sync runs and stays available — with a count — for as long as anything is
unresolved. Compare the normal local file and preserved versions, then click
**Resolve**. Choose the current **Local** or **Remote** version and confirm.
You can edit a Markdown note before choosing it, but editing alone does not
authorize an overwrite. Binary books and images keep their exact selected bytes.

The dialog shows missing files as **deleted**; choosing a deleted side confirms
deletion on the other side. If either version changes while you decide, reload
and choose again. Available versions are retained in the graph metadata folder's
`sync-recovery/` directory, outside the normal index and sync inventory.

Do not treat an empty or unmounted target as a valid source. Verify the target
path and sync summary before accepting deletion changes.

An HTTP error or invalid WebDAV inventory is an error, not an empty graph.
Conditional WebDAV replacements need a strong server ETag. Filesystem targets
recheck the file revision and keep recovery copies, but cannot prevent every
race with another application writing at exactly the same instant.

## Privacy

The app-level **private library** is not part of graph sync: linked external
audio/EPUB sources, private progress, and automatic bookmarks are not graph
assets or imported Books. Back up those sources and private application state
separately. Notes you intentionally write into journals, including `[[Book title]]`
links and quotations, still follow normal graph sharing. See [[Help - Private Reader]].

Filesystem sync sends files to the selected location. WebDAV sends them to the
configured server and its operators. Sync can include Markdown, assets, book
files, and other graph files; it does not make cloud AI private.

Use encryption or access controls on removable drives and servers. Keep a
separate, dated backup before the first sync and before bulk cleanup.
