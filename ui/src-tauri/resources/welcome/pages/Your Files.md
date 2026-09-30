Grafium keeps notes as local Markdown and indexes them in SQLite for search, links, tasks, and queries. You can inspect and edit the Markdown with another editor.

- `pages/` holds ordinary pages; slash namespaces map to nested folders.
- `journals/` holds daily pages.
- `assets/` holds shared media, including this graph's original orbit SVG.
- `books/` holds original-format books, portable book metadata, and one adjacent
  JSON-LD annotation file per book: `1.epub` → `1.jsonld`. Markdown note bodies,
  quotes, locators, and revision history live in that file. Extracted book text
  is indexed without rewriting the original.
- `pages/Reading Notes/Books/` can hold older standalone Markdown annotations.
  These remain supported and are not silently migrated. Back up them and the
  adjacent JSON-LD files along with the books. New note media references are
  relative to the book folder; existing Markdown references keep their meaning.
- Graph metadata holds the index and state that does not live in plain note text.

An original book and its matching Grafium JSON-LD file can be copied together
and imported into another graph. Keep matching basenames, such as `1.epub` and
`1.jsonld`. Import validates the pair before writing and unions valid revision
histories on reimport; it does not replace local notes or resurrect deletions.
Managed graph copies are named `books/<book-id>/original.<extension>` and
`original.jsonld`.

## Pages that are not files

Not every page in this graph has a file behind it. Writing `[[Orbits]]` or `#astronomy` creates the page immediately so the link resolves and backlinks work, but nothing is written to `pages/` until the page has content. These are **placeholders**: real entries in the index, with real backlinks, and no `.md` file on disk.

This is why a page can appear in **All Pages** and in the namespace tree, yet be missing when you look in the folder with another editor. Nothing is lost — there was never a file.

**All Pages** has a filter for exactly this question:

- **All** — everything, placeholders included. The default.
- **Files** — only pages with a source file on disk: Markdown or an original book.
  Use this when you want the list to match what a file manager shows.
- **Placeholders** — only pages that a link or tag named and nobody has written. A useful to-do list: each one is somewhere you meant to say something.

The filter applies to list and tree view alike, and is remembered per graph. Note that **Placeholders** empties the namespace tree of most of its structure, and **Files** empties the tag tree almost entirely — tag pages are usually the placeholders.

The namespace tree keeps Grafium's **Books**, **ImportedMedia**, and **Reading Notes** folders at the top in a fixed order. Their distinct book, media, and note icons make those app-managed locations recognizable. This does not affect the tag tree or same-named folders nested elsewhere.

The file watcher notices external source changes, removals, and folder moves.
Opening a graph reconciles changes made while it was closed, even when the
number of files stayed the same. Avoid editing the same page simultaneously in
two editors. Watching local edits and synchronizing a graph are separate features.

## What deleting a source removes

Deleting an indexed source removes its indexed text, outgoing links and tags,
and source-owned task, card, property, and handwriting-search entries.
Vector cleanup does not require AI to be enabled or an embedding model to be
loaded. It runs in the background; a replacement embedding is only generated
when a model is configured.

A title can remain as an empty **placeholder** when another document still
links to it. That preserves the other document's reference, not the deleted
source's words. Shared tags, authored pages, favorites, and other owned state
are not garbage-collected merely because a source stopped mentioning them.

Original-book deletion removes the graph's copy, not the external file it was
imported from. Adjacent JSON-LD and legacy Markdown notes remain and can become orphaned.
Likewise, deleting a source is not secure erasure of quotations you saved in
other notes, previous Chat answers, or preserved sync-conflict copies.
Those are separate records and require separate, explicit decisions.
Files named as `.conflict_...` copies and hidden recovery/staging files are
recovery artifacts, not additional authoritative pages. They are kept out of
normal indexing and sync so they cannot duplicate a note's identity.

Re-indexing and routine sync reconcile source content rather than discarding
unrelated favorites or flashcard review progress. An error opening a graph is
not permission to delete its database; Grafium preserves it and reports the
failure instead.

Manual **Re-index** includes all originals in `books/`, not just `.md` files.
It retries text extraction, rebuilds search tables, and queues vector refresh
without depending on the external files originally imported. A missing graph
copy is removed from the index, never silently restored from elsewhere.

## Sync to another location

Grafium supports **filesystem** and **WebDAV** sync targets. Configure the target in **Settings**, where **Sync Now** lets you request a sync. The sync monitor starts with the app and can automatically sync a configured target when it becomes available. This is not continuous collaborative editing.

Independent original-book notes combine during sync. Competing edits and
deletion-versus-edit candidates stay visible in the book's **Notes** until you
manually merge and explicitly resolve them; see [[Help - Books]].

A WebDAV target involves a remote server: choose one you trust with your notes and configure its access carefully.

Sync is not a substitute for an independent backup. Check the destination and the result before relying on another copy, especially before changing targets or moving a graph.

## Back up more than the text

Keep a separate backup of the whole graph, including assets and metadata. Markdown alone does not preserve every review, preference, or index-backed state. App-level configuration may live outside the graph; record your graph locations and settings too. For example, the saved theme lives in `grafium/theme.txt` under the platform's configuration directory, not inside the graph.

Before a bulk import, move, or cleanup, make a backup and verify that you can read it. A second copy on the same disk is not protection against losing that disk.

The Welcome seed only populates an empty default graph. It does not refresh older tutorials or replace edited sample notes. Use [[Create Your Own Graph]] for a separate workspace, and [[Imports And Media]] to bring files into it.
