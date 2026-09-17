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

## Conflicts and deletion safety

Grafium does not silently choose a winner. Conflicting edits remain available
for manual review in the right-panel **Conflicts** tab, which appears while a
sync runs and stays available — with a count — for as long as anything is
unresolved. Keep the normal local note, compare the preserved conflict copy,
edit the note into the version you want, and sync again.

Do not treat an empty or unmounted target as a valid source. Verify the target
path and sync summary before accepting deletion changes.

## Privacy

Filesystem sync sends files to the selected location. WebDAV sends them to the
configured server and its operators. Sync can include Markdown, assets, book
files, and other graph files; it does not make cloud AI private.

Use encryption or access controls on removable drives and servers. Keep a
separate, dated backup before the first sync and before bulk cleanup.
