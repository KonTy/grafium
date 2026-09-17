# Help - Sync

Use **Settings → Sync** to add one or more independent filesystem or WebDAV
targets. A filesystem target can be a USB drive or network folder.

1. Add each location separately.
2. Mount one location.
3. Click **Sync Now** for that target.
4. Sync before disconnecting and after connecting it elsewhere.

Grafium never resolves conflicts automatically. When a sync is running — and for
a few minutes afterwards — a **Conflicts** tab appears in the right panel. It
also comes back on its own, with a count, whenever conflicts are still
unresolved, including after a restart. Once everything is resolved the tab goes
away again, so it does not take up space on a graph that has never conflicted.

To resolve one: open the **Conflicts** tab, click the note, edit the normal note
into the version you want, and save.
