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
| Right reference panel | `Ctrl+Shift+B` / `Cmd+Shift+B` |

Use the command palette to discover commands for journals, dates, timestamps,
imports, themes, graphs, and navigation. On macOS, `Cmd` replaces `Ctrl`.

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
