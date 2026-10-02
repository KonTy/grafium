# Help - Settings

Settings controls Grafium's appearance, graph behavior, indexing, AI features,
and synchronization.

Settings keeps controls and current results visible, with explanations behind
small **?** buttons. Click or keyboard-activate a **?** to open that topic's help.
Use **Close** or **Escape** to return to the same button; Tab stays within the
help dialog. **Filter settings** also searches the hidden help text without
expanding it. **F1** still opens the full contextual help page, including from
inside a topic dialog.

Errors, operational status, and warnings in action confirmations stay visible.
In Asset Cleanup, **Completion history** has its own help button explaining
the preview-and-backup operation; its preview and completion results remain
beside the controls. **Unused attachments** and **Asset trash** each have their
own help button rather than repeating instructions above every list.

On Linux, local deployment installs the matching Grafium launcher and icon
files. New windows use the stable `grafium` application ID, even when the native
executable is named `grafium-bin`. If an already-running window retains a generic
icon after an update, close it normally and reopen Grafium.
The smplOS menu has its own application index. Local deployment refreshes that
index when its helper is available and uses a distinct local icon name so an
older system icon cannot override it. Close and reopen the menu after an update.

**Mobile location format** controls what an Android long-press on the editor
bar's **Time** button inserts. OpenStreetMap link is the default; plain
coordinates and a device `geo:` map link are also available.

## Theme and colors

Open **Theme** to choose a palette. The color swatches preview its accents on
its own background. All built-in themes use vivid reds, blues, purples, cyan,
orange, and pink for headings, tags, and callouts, with deeper shades on light
surfaces and brighter shades on dark surfaces so the text stays readable.

Markdown heading levels use red, blue, purple, cyan, orange, and pink in order
from H1 to H6. The colors are presentation only; your Markdown is unchanged.
Matrix keeps its green text, black canvas, and terminal font, with colorful
headings and highlights rather than an all-green page. OLED keeps its true-black
background. Selecting another theme updates these colors immediately.

**Auto** follows a recognized smplOS theme, including its background opacity on
supported Linux desktops. The desktop can show through the window background;
text and icons are not faded, and switching focus does not change the opacity.
Explicitly selecting a Grafium palette keeps it opaque, even when smplOS changes.
Auto reads the actual background, foreground, and semantic colors from the current
smplOS palette, including custom theme names and changes without renaming a theme.
The Grafium system theme is OLED black with silver-grey text. Its Auto preview
shows the same colors as the app. Explicit palette selections remain independent
of system changes.

Without smplOS or native transparency support, Auto stays opaque. Missing or
malformed system palettes show a diagnostic and use an opaque built-in palette
matching the name when available, otherwise the platform default.

smplOS themes set `app_background_opacity` in `current/theme/colors.toml` to a
decimal from `0.0` (clear background) to `1.0` (opaque). When absent, Grafium uses
`popup_opacity`, then `1.0`. An invalid explicit value is reported in the log and
uses `1.0`; it does not fall through to an older value. Changes to the palette,
including replacement without a theme rename, are picked up while Grafium runs.

Menus, dialog cards, code surfaces, original book/PDF pages, media, and the 3D
space scene retain their readability backgrounds. Ordinary notes and editors,
the 2D graph background, and window chrome can show the desktop. Contrast depends
on what is behind a transparent window; use an explicit palette for opaque reading.
The compositor must leave Grafium's whole-window opacity at `1.0`.

Helper text, settings labels, and inline-code text use readable solid colors.
On transparent backgrounds, supporting text uses the strength of normal body
text instead of faint grey. This does not fade images or change background
opacity. Wallpaper can still affect contrast; choose an explicit opaque palette
when you need predictable reading contrast.

On Linux with WebKitGTK 2.44 or newer, Grafium uses shared-memory rendering to
preserve background alpha without unstable GPU-DMABUF transport. Older engines
remain opaque. Settings explains when an explicit `WEBKIT_DISABLE_DMABUF_RENDERER`
or `WEBKIT_DISABLE_COMPOSITING_MODE` environment override prevents transparency.
Grafium does not erase these overrides: remove them and quit/reopen to use its
default renderer. Whole-window compositor fading is not a substitute because it
also fades text and icons.

Enabling native transparency requires a rebuilt Grafium and one normal quit and
reopen. Later theme opacity changes are live. This does not migrate or refresh
existing tutorial graphs, and installing a build does not restart a running app.

## Private library

**Library location** selects an external audiobook/EPUB library for the private
reader in Library. This is an app-level setting, independent of the active graph.
Source media is not copied into the graph. Automatic bookmarks and progress stay
outside graph sync and AI indexing. Deliberately written journal notes such as
`[[Book title]]` remain ordinary graph content with ordinary sharing settings.
See [[Help - Private Reader]] for folder layout, local voices, backups, and
Android's unverified locked-screen volume-key compatibility.

## AI / Knowledge Engine

Open this section to configure an embedded local model, Ollama, an
OpenAI-compatible endpoint, or a cloud provider. Test the connection, select
generation and embedding models separately, and index only the graph content
you want AI to retrieve. See [[AI Setup And Privacy]] for provider, privacy,
and troubleshooting guidance.

**Native model recovery** lists models whose worker exit was not confirmed or
which stopped abnormally. **Allow one GPU attempt** authorizes only that model's
next attempt; it does not disable memory admission. **Request GPU for Chat**
requests offload for the selected chat model. Active native jobs must finish
before their worker can be evicted for a retry.

## Re-indexing the graph

**Re-index Graph (Manual)** and the graph menu's **Re-index** rebuild search data
from Markdown pages, journals, graph knowledge, and all imported originals in
`books/`. Companion reading notes are included. The graph's book copies are the
sources; their former external locations are not needed.

Reindex retries original-book extraction even for unchanged files and queues
vector refresh for the configured embedding model. Existing page identities,
favorites, annotations, review progress, and handwriting recognition are
preserved. Removed sources are cleaned from the index, not recreated.
An invalid source is reported without erasing unrelated books or notes.
See [[Help - Books]] for format and extraction limits.

## Asset Cleanup

Deleting text or blocks automatically moves attachments that become unreferenced
into persistent graph-local trash. **Undo** restores references and attachment
files while those files remain in trash; shared references are kept
conservatively. Whole-page asset cleanup and manual cleanup below use the same
trash. Restoring an attachment does not restore a deleted page.

AI chat history does not keep pages or attachments alive. Messages, quoted
links, citations, and conversation context are historical snapshots, not
owners. They do not prevent page deletion, unused-placeholder removal, or
attachment cleanup, so old chat links may stop opening. Save content into a
note if it and its attachments should be kept. Real note and annotation
references, properties, and active Studies/audio/handwriting attachments still
protect shared files. Use **Scan for orphaned assets** to find files previously
retained only by chat history; no conversation needs to be deleted.

If automatic cleanup fails or moves only some attachments, Grafium shows an
error notification for that graph and keeps the successful text edit and its
Undo history. Review the warning, then scan or refresh Asset Cleanup before
retrying. Attachment restoration failures instead stop Undo so it can be
retried after resolving the error; Grafium does not silently complete an Undo
with missing attachments.
Undo/Redo history is cleared when switching graphs; it cannot be used on a
different graph with copied page or block IDs.

Open **Asset Cleanup** to find unreferenced attachments, including ZIP archives
and other non-image files. **Save pending edits first**: unsaved editor drafts
cannot be checked. The scan checks indexed references and saved graph text,
including Markdown, JSON-LD reading notes, and configuration. Filename matching
is deliberately conservative: a reference to a name can retain multiple files
with that name even in different folders.
References inside binary archives or books are not inspected; this is a
conservative scan of supported sources, not an exhaustive check of every
possible reference. Symlinks or unreadable text sources can prevent a safe scan
and are reported rather than silently ignored.

Choose **Scan for orphaned assets**, then review the full graph-relative paths
and sizes. Tick individual files or **Select all**, then choose **Move selected
to trash…**. You can also move one file or all preview candidates. Every move
requires an explicit confirmation. Grafium binds the preview to its graph,
checks references again, and refuses files whose contents changed or which have
become referenced. Scan failures and per-file move errors are shown; only files
confirmed moved disappear from the preview. Re-scan after resolving errors.

This is recoverable removal, not permanent deletion. Files move into a unique
directory under `.grafium/asset-trash/`, retaining their original relative folder
structure. The result lists the files moved and the absolute recovery directory.
Under **Asset trash**, choose **List trash** or **Refresh trash** to review the
original path, full trash path (including its batch ID), size, and SHA-256
fingerprint of each file. **Open containing folder** beside a file opens that
specific trash copy's folder in your system's default file manager (the
directory MIME handler on Linux). It does not restore or delete anything.
External file-browser access is unavailable on Android and reports an error.
Refresh trash after changing files outside Grafium; deleting them externally
also removes the bytes that Undo needs.
Select individual copies or
**Select all trash**, then choose **Restore selected…** or **Restore all…**.
Restoration returns files to their original paths, not deleted notes.
**Never overwrite an existing original**: Grafium refuses conflicts and keeps
the trash copy. Failed or partial attempts can leave recovery copies even when
the original file was not moved. Each preview and confirmation names its graph.
Only confirmed restorations or permanent deletions leave the preview; errors
remain visible and failed entries remain available for review. Refresh after
resolving an error or after moving more files to trash.

If several versions share an original path, automatic Undo refuses to guess.
Compare their batch paths, sizes, and fingerprints; select the intended copy,
**Restore** it, then retry Undo. Grafium records that explicit version choice
persistently; other copies stay in trash. Purged-version records also prevent
a different file from silently substituting for permanently deleted bytes.

**Moving to trash does not free disk space.** To reclaim it, use
**Permanently delete selected…** or **Permanently delete all…** and review the
focused confirmation. **Cancel** is focused first; Cancel or **Escape** leaves
the files untouched. **Permanent deletion destroys the attachment bytes.
Undo cannot recover permanently deleted attachments**. Undo that needs a
purged attachment fails explicitly rather than silently restoring broken
references. Tiny deletion records remain after purge to recognize these cases;
the attachment bytes are gone. Keep an independent backup if needed. There is no
automatic purge, expiration, or disk-pressure deletion.

The `.grafium` trash is device-local and is never synced, but removal of each
original attachment will sync to your configured destinations. Keep backups and
review the preview carefully. Another device does not receive these recovery
copies. Press **F1** inside Asset Cleanup for this help.

## Sync

Add filesystem or WebDAV targets here. USB drives and file-server folders are
filesystem targets. Configure each destination separately and use **Sync Now**
when it is available. See [[Sync And Privacy]] before syncing a real graph.

Press **F1** while focused inside a Settings section for section-specific help.
For sync instructions, focus a control in the Sync section and press F1 again.

Grafium's source code is licensed under the GNU Affero General Public License
version 3. Modified versions made available over a network must also make their
corresponding source available to their users.
