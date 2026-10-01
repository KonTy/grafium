# Help - Editor

Press **F1** while editing for quick editor help.

- **Enter** creates a block; **Shift+Enter** adds a line inside it.
- **Tab** indents; **Shift+Tab** outdents.
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
  Journal, **Go to date** and **Go to link** sit in the title bar immediately
  before Search so the journal itself keeps more room for notes.

Clicking a block's text opens it for editing. Controls inside a rendered block
act on themselves instead: ticking a task checkbox, sorting a table column, or
following a page link, a tag or an in-page anchor does what you clicked without
also opening the editor. A plain web link still opens the block for editing
after it follows the link. Query result rows that carry a block reference are
reachable with **Tab** and open with **Enter**.

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
**Delete all notes on this page…** is in the Notes toolbar. Both ask for
confirmation; Cancel changes nothing. Bulk deletion targets only the current
page or journal day, even when **Notes scope** is **All notes**, and includes
legacy separate-file annotations on that source. It does not delete the page,
source prose, ordinary footnotes, or other pages' notes. Unsaved note edits are
kept as new drafts rather than silently discarded. Save them before closing.

Deletion checks the reviewed revisions. If a note changed since confirmation,
refresh and review it before retrying instead of overwriting someone else's
edit. Before deleting, Grafium keeps verified recovery copies beside the affected
source files as `.reading-note-<uuid>.deleted`. These copies are not indexed as
notes. If only some files can be changed, the error reports what was deleted and
what still needs attention; it does not claim that everything succeeded.

See [[Writing]] for more examples.
