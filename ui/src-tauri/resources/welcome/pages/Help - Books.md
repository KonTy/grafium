# Books: originals and editable copies

Use **Import books** (`Alt+B`) to choose a file or a folder. Folder imports scan
subfolders. Set **Convert to editable Markdown** before importing:

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

Open an original from **Books** to use the reader rather than the Markdown
editor. Use chapter navigation or page controls to move through it. The reader
remembers your last location.

Select a passage, open the right panel's **Notes** tab, and choose
**Use selection**. Write your thoughts and **Save note**. A note can also apply
to the whole book without a selected passage. Open a saved note's passage to
return to its location.

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
