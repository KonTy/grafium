# Books: originals and editable copies

For linked external EPUBs and audiobook folders without graph copies, use
**Settings > Library location** and the top-level Library instead.
That app-private reader does not index books for graph AI or sync them.
See [[Help - Private Reader]]. The import workflow below intentionally makes
graph copies and retains its existing graph search, annotation, and sync behavior.

Use **Import books** (`Alt+B`) to choose a file or a folder. Folder imports scan
subfolders. The dialog keeps its explanations behind the **?** button beside its
title; choices and errors stay visible. Set **Convert to editable Markdown**
before importing:

- **Unchecked (default)** copies EPUB, FB2, MOBI, AZW3, or PDF into
  the graph and opens it under **Books**. The copied source stays unchanged.
- **Checked** uses the existing document converter and
  creates editable pages under **Books**. The original stays outside the graph.
  Layout, images, and structure may change during conversion.

Neither mode replaces books or annotations you already own. Converting material
is a separate operation, not a prerequisite for reading an original.

Original-book import also reads a matching adjacent annotation file when present:
choose `1.epub` beside `1.jsonld` to bring both into another graph. Grafium
validates the book identity, source fingerprint, and supported sidecar format
before writing anything. Malformed or mismatched companions reject the import
rather than silently losing notes. Reimporting a valid pair combines revision
histories with existing annotations, preserving deletions and unresolved
conflicts instead of overwriting local notes. The graph's managed copy uses
`books/<book-id>/original.epub` and `original.jsonld` (with the appropriate book
extension). Importing a book without a companion does not create an empty
annotation file. The sidecar does not embed referenced media; copy that media
separately and preserve its paths relative to the managed book folder.

Original import accepts files up to **128 MiB** and checks EPUB archive expansion
limits. The extracted index is limited to **16 MiB of text / 20,000 blocks**.
An oversized original is rejected; an extraction limit produces a partial-index
warning, not a claim that the whole book was indexed.

## Read and annotate

For converted Markdown books, use the Notes controls described in
[[Help - Editor]]: selected words or blocks attach to a new blank note,
**Delete note…** removes one saved annotation, and **Delete all notes on this
page…** removes only that source's annotations after confirmation.

Open an original from **Books** to use the reader rather than the Markdown
editor. Use chapter navigation or page controls to move through it. The reader
remembers your last location.

The reader starts with its controls hidden. Tap the reading surface, press **F8**,
or click the small corner button to reveal navigation, text size, and **Book notes**.
Use **Fullscreen** or **F11** for an unobstructed page; **Escape** leaves fullscreen.
Swipe horizontally to turn pages without opening the controls.
Reflowable text follows Grafium's theme and scales its actual lettering when
you change text size. Images and fixed-layout/PDF page artwork retain their
original colors; PDFs use page zoom rather than text reflow.

Use the arrow icons or **Arrow Left / Arrow Right** to turn pages; arrows follow
the book's reading direction. **Page Up / Page Down** always mean previous/next.
Shortcuts leave text selections, editable fields, menus, and other panes alone.
They also work while the controls are hidden.

For reflowable EPUB, FB2, MOBI, and AZW3, choose **Pages** or **Continuous scroll**
in the reading bar. Scroll mode moves through chapters automatically at their
boundaries; its page keys advance an overlapping screenful rather than skipping
a chapter when the lettering is large. The layout preference survives restart.
Text wraps to the available width, including narrow phone-sized windows.
Grafium's **Wide mode** (**Alt+W**) changes the available reading width; book
columns follow that width rather than retaining a fixed desktop column limit.
Fullscreen and window resizing also reflow the text without reopening the book.
PDF page keys first move through an enlarged page before changing pages.
The mouse wheel turns pages in **Pages** mode. In **Continuous scroll**, or on
an enlarged PDF page, it scrolls the current content before crossing its boundary.
Zoom gestures and nested scrollable content keep their own behavior.
Reflowable **Text size** is remembered across books and restarts; PDF zoom is separate.
PDF and fixed-layout EPUB do not offer text reflow; DjVu is not an original-reader
format. PDF may contain selectable text, not only images; reliable conversion or
OCR is a separate workflow, not lossless reflow of the original.

The book's **Bionic** button, **Menu > Bionic reading**, or **Ctrl/Cmd+Alt+B**
enables **Bionic reading**, emphasizing
word beginnings. The shortcut also toggles the same preference for notes; there
is no separate top-bar B button. The book control remains visible but disabled
while loading or for fixed-layout pages; hover for the reason or shortcut.
This is optional presentation, not a change
to book text or a guaranteed reading-speed improvement. Code, math, and artwork
are left alone. Positions, quoted selections, and passage-note anchors remain
compatible when changing Bionic mode, layout, text size, or window width.

Select a passage, open the right panel's **Notes** tab, and choose
**Use selection**. Write your thoughts and **Save note**. A note can also apply
to the whole book without a selected passage. Open a saved note's passage to
return to its location.
Private **Ctrl/Cmd+Alt+M** bookmarks belong to the external Library, not these
graph-imported originals. Use **Book notes** for passages in graph books.

New book notes share one adjacent JSON-LD file: `1.epub` has `1.jsonld`
(not `1.annotations.jsonld`). Markdown note bodies, quoted text, passage
locators, source fingerprints, and revision history live there, not in SQLite
alone. **Open note**, search results, and annotation links open the dedicated
book-note editor, not a generic Markdown editor for the JSON-LD file.
**Return to book** opens the original reader. Existing standalone Markdown
annotations remain supported in their existing locations; they are not
silently moved, nor are their attachment paths rewritten.
Saving a note never inserts footnotes into the original EPUB or PDF. Save before
closing Grafium: unsaved drafts are not a backup. If a note changes externally,
resolve the conflict instead of overwriting it. If the source changes or
disappears, notes are preserved but their passage may be marked orphaned.
Their indexed note pages still open even if the original book page is gone.
When the source is unavailable, existing note bodies remain editable using
their saved attachments. New notes, new attachments, and passage navigation
require restoring the source. For an orphan conflict, explicitly choose a
candidate attachment before either merging or confirming deletion; the saved
candidate fingerprint is retained rather than guessed from a missing book.

## Resolve annotation conflicts

Sync combines independent notes and compatible revision histories in the
adjacent file. Competing edits to the same note, including deletion versus an
edit, remain distinct candidates in **Notes**. Each candidate shows its Markdown,
quote, attachment, source fingerprint, update time, and deletion state.

Compare all candidates. **Use candidate … text and attachment** starts from a
chosen version; **attachment only** keeps your composer text. Edit or combine
the Markdown yourself. Explicitly choose the intended attachment, **Use
selection** from the current book, or **Whole-book note**. Then click
**Resolve with merged note**. To keep a deletion instead, use **Resolve as
deleted…** and confirm deletion of all reviewed candidates. The deletion record
uses the current book with no passage attachment when available, or the
explicitly selected candidate attachment when the original is missing;
unsaved composer text stays
in a new draft. Ordinary Save and
Remove never resolve these conflicts.

If another edit arrives, resolution is refused and the candidates reload.
Your merged draft stays in this session: review the new versions and choose
the attachment again before retrying. Navigating away does not save or discard
that draft. The original book remains unchanged throughout.

Reflowable EPUB, FB2, and MOBI books change pagination with font and window size;
their original file is still preserved. PDF retains its page layout. Fixed-layout
ebooks may not support the same passage-selection and highlighting features as
reflowable books. DRM-protected ebooks are not supported.

## Search and AI

Grafium extracts text into its rebuildable index without converting the source
into an editable Markdown page. That text is available to graph search and
page/book AI context. Embeddings use your configured indexing and model settings;
copying a book does not install or enable an AI model.

**Re-index** includes every imported original stored under this graph's `books/`
folder, together with Markdown pages and companion notes. It reads the graph's
copy, so the external file you originally imported is no longer required.
Books remain graph sources until you remove those graph copies.

An explicit reindex extracts book text again even when the source bytes are
unchanged. It repairs missing search data and retries extraction that previously
failed, for example after installing Calibre. It preserves book and note
identities, highlights, favorites, flashcard review progress, and handwriting
recognition data; originals and annotation files are not rewritten.
Vector refresh is queued for the configured embedding model. Reindexing does
not enable a model or recreate a removed book from its external source.

AI search indexing runs in the background after a book import finishes, so the
import job says **AI search index is building in the background** when an
embedding model is ready. Large batches — a book, a sync pull, a rebuild —
appear in the activity list as **Building AI search index**, showing the book
being embedded and how many passages are done. It closes with **AI search index
is up to date** once the queue is empty. Until then Chat can already use the
extracted text, but semantic matches from that book may be incomplete. Small
edits index quietly. If the embedding model becomes unavailable the job pauses
and indexing resumes automatically later.

Extraction can be incomplete or unavailable, even when the book can be read.
MOBI/AZW3 indexing may need Calibre's `ebook-convert`. Original PDF indexing uses
embedded text, not OCR. Image-only scans can still be displayed; use the separate
Markdown/OCR conversion workflow if you need their text extracted. Check import-job details
and the reader's indexing warning rather than assuming every page was indexed.
AI answers based on partial extraction are not a complete review of a book.

Reader highlights are not Markdown block selections. Use the book/page context
for AI; selecting a reader passage does not silently change Chat's context.
Your own notes remain separate graph pages, not part of the author's book text.
See [[AI Setup And Privacy]] before sending book content to a model service.

## Files, sync, and deletion

Originals, book metadata, and adjacent `.jsonld` annotation files are stored under
`books/` inside the graph. Back up and sync these and any legacy Markdown notes,
not just the original
ebook. SQLite and vectors are indexes, not the only copy of your annotations.
JSON-LD uses Web Annotation concepts plus Grafium's revision data; it does not
promise that every ebook reader can import the notes or preserve their locators.
Relative media paths in new notes start beside the book; legacy Markdown
attachment paths keep their existing interpretation.

Removing an original removes its indexed source content. Companion notes are
kept, with an unavailable source, unless you explicitly delete those notes too.
Reimporting the same external original can restore a missing graph copy without
replacing its notes. If the existing graph copy contains different bytes,
Grafium refuses to silently substitute or overwrite it.
Links and tags still used by other documents must not be removed merely because
one source was deleted. See [[Your Files]] and [[Help - Sync]].
