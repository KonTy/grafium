# Help - Journal

The Journal is the fastest place to capture a thought. Each dated journal page
is made from **blocks**: small, addressable pieces of writing that can be
reordered, nested, linked, searched, and turned into work.

## A useful first session

1. Open **Journal** from the left sidebar (`h` then `j` also works).
2. Click an empty block and type a thought.
3. Press **Enter** for another block. Use **Tab** to nest it and
   **Shift+Tab** to move it back out.
4. Select a phrase and make it a link with `[[Page Name]]`, or use the link
   command to create a page from selected text.
5. Press **F1** again whenever you need this guide.

## Blocks and outlines

Blocks keep their own identity, so a journal can grow without becoming one
large text field. Try these operations:

- **Enter**: split or create the next block.
- **Shift+Enter**: insert a line inside the current block.
- **Tab / Shift+Tab**: indent and outdent.
- Drag or use the block controls to reorder, fold, and expand outlines.
- Shift-select blocks, then copy, cut, delete, or undo the whole selection.
- Leave a block to see its Markdown rendered; click it again to edit the source.

When deleting text or blocks removes an attachment's last reference, its file
moves to persistent graph-local trash. Undo restores the references and files
while they remain in trash; shared references are kept conservatively.
**Settings > Asset Cleanup > Asset trash** offers restore and explicit permanent
deletion. There is no automatic purge. **Undo cannot recover permanently deleted
attachment bytes.** See [[Help - Settings]].
Undo reports missing purged attachments instead of completing with broken
references; other restoration failures can be retried after resolving the
error. Cleanup warnings do not discard a successful edit or its Undo history.
Trash is device-local and never synced to your other devices.

## Links and connections

Write `[[Dinner ideas]]` to link to a page. If the page does not exist, Grafium
can create it when you follow the link. Links turn journal capture into a
connected graph: open **Graph** from the sidebar, or press **F1** there for
graph-specific guidance.

Use the top toolbar's link icon or `Ctrl+L` / `Cmd+L` to find and open any page
by name. Backlinks and related pages show how an idea connects without requiring
folders.

## Dates, times, and tasks

- Journal pages are dated automatically. Use **Go to date** (the calendar icon
  in the top toolbar, beside the link icon and Search) or **Ctrl/Cmd+G** to
  jump to another day. Desktop and Android share this compact placement,
  without an extra button row above your notes. In Zen mode, which hides the
  top toolbar, these two buttons remain available above the journal.
- To delete a day, hover its date label and click the trash button (always
  visible on touch screens), or right-click the date and choose **Delete page**.
  This removes that day's `.md` file after you confirm. Page deletion itself
  cannot be undone; newly unused attachments go to recoverable Asset trash.
- Use **Ctrl+Shift+P** / **Cmd+Shift+P** to open the command palette, then search
  for time, journal, insert, or navigation commands.
- Use the toolbar timestamp command to insert the current local date and time.
- Start a block with `TODO` to make work visible in Tasks. Add a date or
  `SCHEDULED` / `DEADLINE` property when the task needs a time.
- Tags such as `#inbox`, `#rehearse`, and `#digested` are ordinary Markdown
  markers that make later search and review easy.

## Finding your way around

- **Ctrl+K / Cmd+K**: global search.
- **Ctrl+Shift+B / Cmd+Shift+B**: toggle the right reference panel.
- **Ctrl+L / Cmd+L**: find and open a page by name.
- **Ctrl+Shift+P / Cmd+Shift+P**: command palette.
- **F1**: contextual help.
- Use the back and forward buttons to return to earlier pages and journal days.

## Bring information into the graph

Use the command palette or the **Import media** action to bring in images,
audio, and video. Grafium stores graph assets beside your notes and keeps media
references portable. Use **Import books** for a directory of books; imported
books become searchable reading pages, and the right panel can hold reading
notes without rewriting the source.

For a guided tour of files and books, open [[Your Files]] and
[[Imports And Media]]. For a complete command list, open the command palette and
type `help`.

## Try this exercise

Create these four blocks in today's journal:

```text
TODO Read about [[Graph connections]]
SCHEDULED 2026-09-17
The best idea from today is ...
Timestamp: use the timestamp command instead of typing the time
```

Then follow the link, open Graph, press `Ctrl+L`, and return with the back
button. This small loop demonstrates capture, connection, navigation, and
review.
