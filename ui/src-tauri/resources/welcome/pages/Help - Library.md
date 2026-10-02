# Library

**Library** is your app-private media shelf, separate from your graph and from
**Studies**, your learning plan. Open it from the main navigation or use `g l`
in navigation mode. Tasks remains a separate workspace.
Use **Ctrl/Cmd+F** to focus Library's wide search field and filter your shelf.
The media-type filter (for example, Audio) is remembered on this device when
you return from a book and after restarting Grafium.

## Your active pile

Resume reading, listening, or watching from the saved position. Actual use moves
an item toward the top of the recent pile; browsing its details, rescanning,
or favoriting it does not count as reading. Favorite items for easy access.
Progress reflects a playback or reading position, not mastery of the material.
Unknown media duration must not be mistaken for completion.

Visual books open with their controls hidden. A single tap, **F8**, or the small
corner button reveals the reading bar, including bookmarks and **Fullscreen**
(**F11**, **Escape** to leave). Swipe to turn pages. Text-based books follow the
Grafium theme; illustrations retain their original colors. See
[[Help - Private Reader]] for text sizing, gestures, and read-aloud options.

Choose a local folder in **Settings > Library location** for EPUBs, audiobooks,
downloaded podcast audio, and supported videos. Files stay in that folder:
Library does not import them into the graph or modify the originals.
Use **Rescan** after adding files. Missing or replaced sources retain their
history and require reconnecting or explicitly relinking.

You can also add supported YouTube links and direct HTTP(S) audio/video links.
These are bookmarks to online media, not downloads. Playback contacts the
provider and requires a working connection; codec and embedding restrictions
can prevent playback. Listing saved links does not fetch their media.
Podcast-feed subscriptions are not part of this feature.

## Bookmarks and journal notes

Bookmark during playback without naming it or interrupting the media. Review
the saved position later, replay it, or add a private bookmark comment.
In a visual EPUB, **Ctrl/Cmd+Alt+M** saves selected text or, without a selection,
the visible page. Compact bookmark rows use a short label; click to return and
highlight the passage. The row's **…** offers **Go to**, **Edit**, confirmed
**Delete…**, and **Journal note…**. Outside a visual book, the shortcut bookmarks
active playback. Reading controls show their keyboard hints on hover.
**Write to journal** deliberately inserts a link and any selected bookmark
comment into today's journal for further writing. That note is ordinary graph
content and follows the graph's sharing and sync rules.

Journal links use stable Library item and bookmark IDs, not private filesystem
paths or automatic `[[Book title]]` pages. Another device needs the matching
private Library history and access to the source to follow them. A missing
item or bookmark is reported rather than opening a different source.

Private audio/read-aloud playback can continue while you use other workspaces.
Video and embedded-player behavior depends on the available player/platform;
do not assume a hidden or unsupported player is still playing.
Local video is currently supported on desktop, not Android. Online audio/video
on Android plays only while its Library player is open in the foreground;
it does not use the native background audiobook service.
For offline speech, backup/restore, and opt-in Android volume-key bookmarking,
see [[Help - Private Reader]]. Phone hardware behavior still requires physical
device testing; this change does not establish screen-off compatibility.

## Add to a learning plan

Use **Add to Studies** and choose a topic, then confirm **Add study**. This
action is in the visual reader's first-row **…** menu, alongside Favorite,
Relink source, and read-aloud options. You do not need to scroll past bookmarks.
The plan
stores a reference to the Library item, not another copy of its media, position,
or bookmarks. Opening that plan entry uses the same Library reader.
Removing the study entry removes its plan/time history, not the Library item.
See [[Help - Studies]] for active-time tracking.

Library files and automatic history stay outside graph search, Chat, and graph
sync. Back up both your original media and private Library records; a graph
backup alone is not a backup of your Library.
