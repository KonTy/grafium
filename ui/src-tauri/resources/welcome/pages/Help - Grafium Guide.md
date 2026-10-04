# Help - Grafium Guide

Press **F1** anywhere in Grafium to open the guide for the screen you are
using. Help lives in the built-in Welcome graph so it remains available while
you work in another graph.

## Everyday navigation

| Action | Shortcut |
| --- | --- |
| Contextual help | `F1` |
| Command palette | `Ctrl+Shift+P` / `Cmd+Shift+P` |
| Search | `Ctrl+K` / `Cmd+K` |
| Current page links | `Ctrl+L` / `Cmd+L` |
| Focus or collapse the left sidebar | `Ctrl+B` / `Cmd+B` |
| Right reference panel | `Ctrl+Shift+B` / `Cmd+Shift+B` |

Use the command palette to discover commands for journals, dates, timestamps,
imports, themes, graphs, and navigation. On macOS, `Cmd` replaces `Ctrl`.

These are the default keys. Every shortcut is listed in **Settings > Keyboard
Shortcuts**, grouped by where it works (General, Navigation, Layout, Journal,
Editor, Chat, Library & reading, Flashcards, Menus & dialogs). Press **+** beside an
action and then the keys you want, remove a key with **×**, or reset it.
Changes are saved on this device for every graph. If the keys already do
something else, Grafium names every action using them and asks before moving
them; resetting an action never takes back a key another action uses now. Keys
that make typing, undo and menus work, such as Enter, Tab, the arrows, Escape,
`Ctrl+Z` and `Ctrl+Y`, are listed as built in and cannot be changed. The command
palette lists every command, including ones you removed all keys from.

Collapsing the left sidebar keeps its main navigation available as an icon rail.
Use its top button or `Ctrl+B` / `Cmd+B` to expand it again. Zen mode is the
only layout mode that hides the navigation chrome completely.
`Ctrl+B` / `Cmd+B` focuses the active navigation item, not a search box.
The left panel keeps navigation, favorites, and recent pages; use `Ctrl+K` /
`Cmd+K` for the single graph-search dialog. See [[Help - Search]].

On Android, swipe inward from the left or right edge to move back through
Grafium's page history. If there is no earlier Grafium page, the app stays open.
When a dialog or menu is open, the same gesture closes that first.

On desktop, Grafium runs as one window. Starting it again from a launcher or
the terminal brings the open window forward, or highlights it in the task bar
where the desktop does not allow that, instead of opening a second copy: two
copies editing one graph would keep re-reading each other's saves and make
typing stall.

## Dialogs and menus

Every dialog can be driven from the keyboard.

| Action | Shortcut |
| --- | --- |
| Close the open dialog or menu | `Escape` |
| Move between a dialog's controls | `Tab` / `Shift+Tab` |
| Move through a menu you opened from a button | `↑` / `↓` |
| Choose the highlighted item | `Enter` |

Arrow keys work in menus that take the keyboard when they open: the app menu,
the graph menu, and the `⋯` menu. A right-click menu stays where the pointer
is and only listens for `Escape`, so your place in the text is not disturbed.

Opening a dialog puts the cursor where you are most likely to start, and `Tab`
cycles within that dialog rather than wandering into the page behind it. Click
a dialog's shaded surround to dismiss it; a selection that merely finishes
outside the dialog will not discard what you typed.

## AI and privacy

AI is optional. Chat can answer questions about selected notes, retrieve
supporting blocks, summarize, rewrite, suggest links for review, help create
study material, and run cited Internet research. Open **Settings → AI /
Knowledge Engine** to configure an embedded local model, Ollama, an
OpenAI-compatible endpoint, or a cloud provider. Generation and embedding
models are configured separately.

Local graph scope controls retrieval, not where a cloud model runs. Prompts and
retrieved excerpts sent to a provider are subject to that provider's handling.
Read [[AI Setup And Privacy]] before connecting an account or downloading a
model.

## Sync and privacy

Sync supports independent USB, filesystem, file-server, and WebDAV targets.
Configure them in **Settings → Sync**, sync one target at a time, and review
manual conflicts. Sync copies files to the selected destination; it is not a
backup or live collaboration. Read [[Sync And Privacy]] before syncing a real
graph.

## Background jobs

Click the bell in the title bar, or **Jobs** in the sidebar, to open the
**Jobs** page. It shows imports, indexing, progress, and errors, grouped as
running, failed, completed, and cancelled jobs, newest first, with when each
one started and finished. While jobs run, the bell counts them; otherwise its
number counts jobs that completed or failed since you last opened Jobs. The
page marks those results **New** and clears the number. Cancelled jobs are
listed but never counted. What you have seen is remembered on this device.

The last 100 finished jobs stay there across restarts. The history is kept in
Grafium's app data on this device; it is not part of any graph and is not
synced. A job that was still running when Grafium closed is listed as
**Cancelled**, "Stopped when Grafium closed". Automatic Library indexing that
found nothing new leaves no entry. **Clear history** removes every finished job
at once; running jobs stay. The **Running** count shimmers only while at least
one job is running, and stops when the last job finishes, fails, or is
cancelled. With reduced motion enabled, the count updates without animation.

## The Grafium model

Grafium is a local-first graph of Markdown pages and journals. Blocks are
addressable pieces of knowledge. `[[Links]]` connect pages, and the graph views
visualize those connections. Tasks, flashcards, books, media, and reading notes
all remain part of the same local graph.

## Explore next

- [[Help - Journal Guide]]
- [[Help - Editor]]
- [[Help - Graph]]
- [[Help - Tasks]]
- [[Imports And Media]]
- [[Your Files]]
- [[Learning/Reading Notes]]
