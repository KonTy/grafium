Bring in material when you have a reason to use it. This graph ships only Markdown and one original SVG; no book, audio clip, video, or AI model is silently downloaded.

## Books and documents

**Import books** keeps **Convert to editable Markdown** unchecked by default. This copies
EPUB, PDF, FB2, MOBI, or AZW3 into the graph and keeps the source unchanged.
Open it under **Books**, add notes in one adjacent JSON-LD file (`1.epub` →
`1.jsonld`), and use extracted text
for graph search and AI. Indexing limitations are reported; DRM-protected
ebooks are not supported.

To carry annotations to another graph, copy the original and its matching
Grafium JSON-LD file together, then import the book. For `1.epub`, its companion
must be `1.jsonld`. Valid pairs retain their notes and revision histories;
reimport combines histories without overwriting local edits or resurrecting
deleted notes. Invalid or mismatched companions are rejected before import
writes. Generic JSON-LD from another reader is not guaranteed compatible.
Referenced note media is not bundled into the sidecar; preserve those files and
their paths relative to the imported book folder separately.

Check **Convert to editable Markdown** to use the existing conversion workflow.
Folder import scans supported books recursively, creates editable pages under
`Books/`, extracts referenced media when available, and leaves the originals
outside the graph. EPUB, PDF, HTML, Markdown, TXT, and FB2 are supported directly;
additional formats use optional Calibre tools.

Imported books use a long page with paragraph blocks nested beneath detected chapter or section headings. Inspect the result: complex layouts and OCR can need correction.

- PDF text extraction and scanned-page handling use optional **Poppler** tools.
- Scanned PDFs need **Tesseract** for local OCR.
- **ImageMagick** can help crop obvious figure regions from scanned pages.

Tool availability and the source document determine what can be extracted; these are not guaranteed lossless conversions. Back up first and keep the originals.

See [[Help - Books]] for reader controls, notes, indexing, and the difference
between preserving a source and converting it.

## Rich media and voice

Attach real local images, audio, or video to your notes. In **Import media**, paste a supported web URL, type an absolute local path, or use **Choose file…** to select audio/video from a local or mounted drive. Local imports read the original in place for transcription; Grafium does not duplicate the potentially large source file into the graph.

Media imported by URL is filed under `ImportedMedia/`, not scattered across the graph. The title comes from whatever the site published, so a fresh import is rarely where you would have filed it yourself — collecting them in one folder gives you an inbox to work through. Move a page out once you have read it and decided where it belongs; nothing else writes to that folder, so whatever is still sitting there is still untriaged.

Importing into today's journal instead puts the transcript in the journal, as before, and does not use the folder.

Media imports run one at a time. Each one converts audio and transcribes it with
the same local model Chat uses, so running several at once would slow all of
them and Chat. Start as many as you like: later ones wait in **Jobs** as
*Queued*, start in the order you added them, and can be cancelled while they
wait. Library indexing also pauses its transcription while an import runs.

Audio notes can keep transcripts so spoken material becomes searchable. Transcription requires a Whisper model and suitable local resources; audio/video import may also need external conversion or download tools. Follow the app's setup and dependency messages rather than assuming every format works on a fresh install.

[[Learning/Flashcards]] shows the shipped image card and explains Anki `.apkg` imports. [[Learning/Reading Shelf]] demonstrates organizing the resulting notes as a collection. See [[Your Files]] before moving assets or originals.
