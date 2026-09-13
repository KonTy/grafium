# Imports And Media

Bring in material when you have a reason to use it. This graph ships only Markdown and one original SVG; no book, audio clip, video, or AI model is silently downloaded.

## Books and documents

Folder import scans supported books recursively: **EPUB, PDF, HTML, Markdown, TXT, and FB2**. It creates editable pages under `Books/`, extracts referenced media when available, and leaves the original files outside the graph.

Imported books use a long page with paragraph blocks nested beneath detected chapter or section headings. Inspect the result: complex layouts and OCR can need correction.

- PDF text extraction and scanned-page handling use optional **Poppler** tools.
- Scanned PDFs need **Tesseract** for local OCR.
- **ImageMagick** can help crop obvious figure regions from scanned pages.

Tool availability and the source document determine what can be extracted; these are not guaranteed lossless conversions. Back up first and keep the originals.

## Rich media and voice

Attach real local images, audio, or video to your notes. Supported links can render rich media in place; local paths must point to files you actually have.

Audio notes can keep transcripts so spoken material becomes searchable. Transcription requires a Whisper model and suitable local resources; audio/video import may also need external conversion or download tools. Follow the app's setup and dependency messages rather than assuming every format works on a fresh install.

[[Learning/Flashcards]] shows the shipped image card and explains Anki `.apkg` imports. [[Learning/Reading Shelf]] demonstrates organizing the resulting notes as a collection. See [[Your Files]] before moving assets or originals.
