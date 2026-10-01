# Help - Editor

Press **F1** while editing for quick editor help.

- **Enter** creates a block; **Shift+Enter** adds a line inside it.
- **Tab** indents; **Shift+Tab** outdents.
- Click the compact triangle beside a heading to collapse or expand its
  children. The full gutter area remains clickable.
- Blocks already have an outline bullet. A block starting with a Markdown list
  (`- text`, `* text`, or `+ text`) uses that bullet for its first item instead
  of showing two. Further items and nested lists keep their own markers.
  Source text is unchanged; `_ text` is literal text, not a Markdown list.
- Markdown list markers have distinct shapes: `* text` shows a dot,
  `- text` a diamond, and `+ text` a square. This always applies to rendered
  lists, including nested lists and books; ordinary blocks keep their dot.
  Other Markdown editors may display all three as the same bullet.
- Type `/` for commands.
- Link pages with `[[Page Title]]` and add tags such as `#project`.
- Headings from `#` through `######` use theme-aware red, blue, purple, cyan,
  orange, and pink. Tags and callouts also use the selected theme's palette;
  change it in **Settings > Theme** without modifying your notes.
- Use the right panel for references, search, and questions.
- Pasting from a web page converts supported rich content to Markdown,
  including tables, links, formatting, lists, and images. Copying an image
  directly or pasting a screenshot saves it into the graph's assets, including
  on desktop webviews that do not expose image clipboard files. Page-local
  images remain portable when the graph is copied between desktop and Android.
  A pasted
  HTML table leaves edit mode after saving so the rendered table appears
  immediately.
- On Android, tap **Time** in the editor bar to insert the current local time.
  Hold **Time** to request the current location and insert both. Grafium asks
  for Android location permission only when you use the hold action. In the
  Journal on both desktop and Android, **Go to date** and **Go to link** sit
  in the top toolbar immediately before Search, without a separate row above
  your notes. Zen mode keeps the two buttons above the journal while the
  top toolbar is hidden.

Clicking a block's text opens it for editing. Controls inside a rendered block
act on themselves instead: ticking a task checkbox, sorting a table column, or
following a page link, a tag or an in-page anchor does what you clicked without
also opening the editor. A plain web link still opens the block for editing
after it follows the link. Query result rows that carry a block reference are
reachable with **Tab** and open with **Enter**.

## Deleted attachments and Undo

Deleting text or blocks moves newly unreferenced attachments into persistent
graph-local trash, rather than destroying them. Shared references are checked
conservatively. **Undo** restores the deleted references and their files while
the attachment copies remain in trash.
Switching graphs clears Undo/Redo and the editors' local text history, even
when the graphs contain copied pages with the same IDs. Finish an in-progress
block operation before switching graphs.

Open **Settings > Asset Cleanup > Asset trash** to list, restore, or explicitly
permanently delete selected or all trash copies. Restore never overwrites an
existing original. Trash is not automatically purged and still uses disk space.
**Undo cannot recover permanently deleted attachment bytes**: an Undo that
needs a purged attachment fails rather than silently restoring broken
references. Other restoration errors stop Undo so you can resolve the error
and retry. Automatic cleanup failures show an error notification without
discarding the successful edit or its Undo history. Trash is device-local and
never synced. See [[Help - Settings]] for confirmation, backup, and partial
failure guidance.
If Undo reports several attachment versions at the same path, use Asset trash
to compare their batch paths, sizes, and SHA-256 fingerprints. Restore only the
intended copy, then retry Undo. Your explicit version choice is recorded;
the other copies stay in trash rather than being guessed or deleted.

## Reading notes and selected passages

Open the right panel's **Notes** tab to annotate a page, converted book, or
journal day. Select words or several blocks before writing a new note:
the blank composer automatically attaches that selection and shows its quote.
Whole-block selections made with the mouse or **Shift+Up/Down** attach those
blocks, not the entire page. Only one page or journal day can be annotated at
once. Unsupported selections show an error instead of silently saving a
page-level note.

Once you start writing, the attachment is kept. **Use selection** explicitly
replaces a new draft's quote; **Make page-level note** deliberately removes it.
Check the quote preview, write your Markdown, then **Save note**. With no
selection, a new note applies to the whole current source. Saved notes are
footnotes inside its Markdown file, not just database records. Original-format
ebooks use adjacent JSON-LD instead; see [[Help - Books]].

**Delete note…** is available on each saved note card and in its editor.
**Delete all notes on this page…** is under **More actions** in the Notes
toolbar. Both open a centered confirmation dialog without scrolling the notes
list. A single-note confirmation previews the note being deleted; bulk deletion
shows the note count and page. **Cancel** is focused first; Cancel or **Escape**
returns to the same place without changing anything. Bulk deletion targets only the current
page or journal day, even when **Notes scope** is **All notes**, and includes
legacy separate-file annotations on that source. It does not delete the page,
source prose, ordinary footnotes, or other pages' notes. Unsaved note edits are
kept as new drafts rather than silently discarded. Save them before closing.
Storage guidance and file paths are available under **About reading notes**,
**Storage details**, or a saved note's **Details**, rather than repeated beside
every action. Attachment warnings remain visible.

Deletion checks the reviewed revisions. If a note changed since confirmation,
refresh and review it before retrying instead of overwriting someone else's
edit. Before deleting, Grafium keeps verified recovery copies beside the affected
source files as `.reading-note-<uuid>.deleted`. These copies are not indexed as
notes. If only some files can be changed, the error reports what was deleted and
what still needs attention; it does not claim that everything succeeded.

See [[Writing]] for more examples.
