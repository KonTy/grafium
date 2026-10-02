# Library reading and listening

The private library is separate from your graph's imported Books and study
history. Choose **Library location** in Settings, then open the private library
in the top-level **Library**. Audio and EPUB originals are linked from that location and are
not copied into your graph. Nothing is automatically added to your journal.

Library also organizes supported video and online-media entries. Its active
pile, favorites, shared progress, journal links, and reference-only Study plans
are described in [[Help - Library]].

## Library layout and missing sources

Each folder immediately inside the library represents an audiobook; its chapter
files may be inside nested folders such as Disc 1 and Disc 2. Chapters use natural
numeric order: 1, 2, 10 rather than 1, 10, 2. Each loose MP3 directly in the library
is a separate book. EPUB files are discovered recursively as individual books.
Check the chapter order before listening. The chapter-order controls change
only private library metadata; they do not rename or move the external files.

The library scans when you open it and when its window regains focus.
Use **Rescan** to discover files added while the library remains open.
Keep source files in place while listening.
An unavailable disk, revoked Android folder permission, or changed source is
not an empty book: progress and bookmarks must be retained until access is
restored. Re-select the library or use the available relink controls rather than
deleting your history. Removing a library entry is not permission to delete its
external files.

On Android choose a local device or removable-storage folder. Folder access
uses a persisted read grant; permission can still be revoked by Android or the
storage provider. Cloud document providers are not an offline source. On Linux,
Grafium cannot prove that a mounted directory is not backed by a remote service:
choose a genuinely local folder if your books must remain on this device.

## Playback and bookmarks

The app-wide player provides **Bookmark**, **Pause/Resume**, and **Stop** while
you work elsewhere in Grafium. The open audiobook also has its own visible
**Play/Resume**, **Pause**, **Stop**, and seek controls, so you do not have to find
the app toolbar to stop it. Stop retains your place and remains available while
another player operation is pending. Playback does not start merely by opening
a book.

Use the seek slider to move within the **current chapter**, not across the entire
audiobook. It becomes available when the player reports a finite duration and
supports seeking. Unknown duration or an unseekable stream shows a reason rather
than a working-looking timeline. Read-aloud uses saved passages and does not
offer a misleading seconds-based seek slider.

A bookmark is a private
saved location, not an automatically created note or graph page. A save error
means the location was not confirmed as durable; do not assume it was saved.
Listening checkpoints normally run every four seconds on Linux and every three
seconds in the Android audio service, with saves at playback transitions. The
recovery target is at most five seconds of listening when the app is scheduled
normally and private storage is writable. An abrupt process termination,
stalled storage, or operating-system suspension can exceed that target.
Bookmarks acknowledged as saved are durable independently of the next periodic
checkpoint.

Use **Ctrl/Cmd+Alt+M** or **Bookmark** in a Library book. Selected EPUB text saves
that exact word or passage; without a selection, the bookmark saves the visible
page. Creating a visual bookmark does not replace the saved narration voice or
spoken offset. Outside a visual book, the shortcut bookmarks active playback.
Graph-imported original books use **Book notes** instead.

Bookmarks are compact single-line rows. Automatic EPUB labels use one or two
words; click a row to return and select its saved passage. Its **…** menu offers
**Go to**, **Edit**, **Delete…**, and **Journal note…**. Editing changes the private
label/comment, not the passage anchor. Deletion requires confirmation and does
not delete the source book. Hover a row for the full label, position, and time.
Journal notes require your review before entering ordinary graph storage.

Android headphone play/pause controls address the native player. Android
background playback, interruptions, and button delivery still need testing on
your actual handset. A browser test is not proof of screen-off behavior.

## Phone volume keys

Physical volume-key bookmarking is an optional Accessibility-service feature.
Enable it explicitly in Android settings only if you want the configured key
gesture, and enable the mapping in **Settings → Library location**. Choose
Volume up or Volume down and hold that key for 700 milliseconds during playback.
A short press still adjusts volume. The service does not request
screen-content retrieval or gesture
injection. Turn it off to restore normal handling; it must not intercept keys
when there is no active reader playback.

**Locked-screen and screen-off volume-key delivery is unverified on physical
Samsung, Pixel, and Vivo devices.** Android or the manufacturer may reserve or
not deliver these events to an Accessibility service. Enabling the service
does not establish compatibility. Test unlocked, locked with the display on,
and display off separately. Headphone and notification controls are not proof
that the phone's own volume keys work. Grafium does not infer bookmarks by
watching system volume changes and does not require root or system changes.

## EPUB and offline speech

### Just the book

Books open without a title banner or permanent controls. Tap once on a reading
surface, press **F8**, or use the small corner button to show or hide the reading
bar. The bar overlays the page without restarting the book or changing its place.
Links, selected text, and pinch/vertical-scroll gestures do not toggle it.

Use **Fullscreen** in the bar or **F11** to fill the display; **Escape** leaves
fullscreen (or closes the bar when not fullscreen). The bar holds chapter
navigation, text size, **Bookmark**, Library navigation, and expandable
**Bookmarks**. The **…** on the first controls row opens favorites, Studies,
relinking, voice settings, and read-aloud without scrolling past bookmarks.
Errors remain visible rather than hiding there. Hover controls for keyboard hints.

Swipe horizontally in **Pages** mode or use the arrow icons. **Arrow Left /
Arrow Right** follow the book's reading direction; **Page Up / Page Down** always
go previous/next, including with the bar hidden. Selected text, editing controls,
menus, and other panes keep their own keys.

Choose **Continuous scroll** to read vertically, continuing into adjacent
chapters at their boundaries. Page keys move an overlapping screenful of text
instead of skipping the rest of a long chapter. Layout is remembered across
restarts, and reflowable EPUB text wraps to wide or narrow windows.
Use Grafium's **Wide mode** (**Alt+W**) outside fullscreen to widen or narrow
the reading area. Book text follows its available width rather than a fixed
desktop column limit; resizing and fullscreen keep the same book mounted.
The mouse wheel turns pages in **Pages** mode, or scrolls within the chapter
before crossing to the next/previous chapter in **Continuous scroll**.
Touch behavior depends on your WebView and device; physical phone validation
is still needed.

Use **B** in the reading bar or **Ctrl/Cmd+Alt+B** for optional **Bionic reading**:
word beginnings become bold without rewriting the book. The global shortcut also
controls this preference for rendered notes; the top-bar B button is removed.
Code, math, and artwork are unchanged. Bookmarks and narration passage anchors
work with Bionic on or off. Appearance changes do not overwrite a saved voice
or an offset within a spoken passage. Bionic is a preference, not a promised
reading-speed improvement.

Reflowable book text follows Grafium's background, text, and link colors, including
live theme changes. **Text size** scales the actual letters, including books with
publisher-defined fixed font sizes, rather than merely widening line spacing.
Your chosen text size is remembered across books and app restarts.
Illustrations, scanned PDF pages, and fixed-layout artwork retain their original
colors. Reading appearance never rewrites the source file.
Fixed-layout EPUB does not offer continuous text reflow or Bionic mode.
For graph-imported FB2, MOBI, AZW3, and PDF capabilities, see [[Help - Books]];
the external private ebook library currently discovers EPUB files.

The visual EPUB reader blocks book scripts and external resources. DRM is not
supported. The private reader does not import EPUB text into graph search,
embeddings, or Chat. Use a local, compatible voice for read-aloud; unavailable
engines or voices must report an error rather than use cloud or system-default
speech as a fallback.

Open the EPUB before starting read-aloud. Narration prepares text from the
whole book, not just the visible page, and uses the same passage locations as
the visual reader. If a visual position is not an exact saved narration
boundary, choose **Read aloud from start** rather than assuming the app can
guess the correct spoken passage. Changing a book or renderer version can
invalidate a saved location; the reader must not silently reinterpret it.

Resuming with the same voice preserves the saved offset within the spoken
passage. Choosing another voice, or resuming an older position without a saved
voice identity, starts that passage again because different voices have
different timings.

Voice models require their own license and compatibility information. Import
local voice files for a fully offline setup, or explicitly authorize a model
download. Downloading a voice contacts its server but must not send book text.
Voice/language availability depends on installed models, not the system's
cloud speech settings. A system voice described as "offline" does not prove
that its provider cannot receive or retain the text.

### Linux runtime setup

Linux uses Piper in a dedicated Python virtual environment, with Bubblewrap
network and filesystem isolation. Grafium does not install a runtime
automatically. With Python's virtual-environment support installed, an explicit
setup can use:

```sh
python3 -m venv "$HOME/.local/share/grafium-piper"
"$HOME/.local/share/grafium-piper/bin/pip" install "piper-tts==1.8.0"
```

These commands download software, not books. In the offline-voice settings,
choose that environment's `bin/piper` executable. Use a dedicated environment,
not a directory containing personal documents. Its Python interpreter must
come from the system `/usr` installation. Bubblewrap and permission to create
its isolation namespaces are also required; a blocked sandbox is an error,
not permission to run speech without isolation.

### Voice packages on either platform

In **Settings > Library**, open **?** beside **Get offline voices** for a complete
worked example, download buttons, and copyable commands. Those commands are not
executed by Grafium and do not request administrator access.

The example is **LJ Speech high**, a single US-English voice at **22,050 Hz**.
The [model author](https://brycebeattie.com/files/tts/) declares that model public
domain; the [dataset](https://keithito.com/LJ-Speech-Dataset/#license) is public
domain in the US. Review jurisdiction-specific terms and preserve bundled
phonemizer notices. This is not a license to redistribute copyrighted books.
Other voices have different licenses; do not reuse this example's license fields.

1. **Linux:** get `en_US-ljspeech-high.onnx` (about 114 MB) and its matching
   `en_US-ljspeech-high.onnx.json` from the
   [official Piper voice folder](https://huggingface.co/rhasspy/piper-voices/tree/main/en/en_US/ljspeech/high).
   Keep both filenames unchanged together in a new folder. Install/configure
   Piper as described above; the model alone is not the engine.
2. **Android:** the engine is already embedded. Download the
   [Sherpa-converted archive](https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-ljspeech-high.tar.bz2)
   (about 116 MB) and extract it on a computer. Keep
   `en_US-ljspeech-high.onnx`, `tokens.txt`, and the complete `espeak-ng-data/`
   directory with its subdirectories. Follow the
   [upstream model instructions](https://k2-fsa.github.io/sherpa/onnx/tts/all/English/vits-piper-en_US-ljspeech-high.html).
   Do not substitute the Linux ONNX, import the compressed archive, or install an
   upstream APK instead of a voice.
3. **Both:** save the voice folder's `MODEL_CARD` next to the ONNX file, with no
   added `.txt` extension. In that directory, run the **Copy package preparation
   command** from the platform-specific setup guide. It uses Python 3 in a
   POSIX terminal (Linux, macOS, or WSL); Android packages can be prepared on a
   computer and then copied intact to local phone storage.
   The helper reads only local files, calculates sizes/hashes, and creates
   `manifest.json`. It never downloads or converts a model and refuses to replace
   an existing manifest. Original voice files stay unchanged.
4. **Import offline model…:** on Linux select the new `manifest.json`; on Android
   select the complete prepared folder. A Piper `.onnx.json` is a model
   configuration, **not** Grafium's manifest. Select the installed voice and
   matching language, then **Save voice and language**.
5. Open an EPUB and choose **… > Read aloud from start**. Missing files, incompatible
   models, or blocked Linux isolation produce an error, not a cloud fallback.

There is not yet a one-click voice catalog. Download buttons open your browser
and contact the hosting service, without sending book content. Hugging Face and
GitHub model URLs normally redirect, so use browser download and local import;
they are not compatible with the redirect-free **Advanced: download from a
manifest** control. The generated offline-import manifest has no download URLs
and is not intended for that advanced control.

For other languages, browse [Piper samples](https://rhasspy.github.io/piper-samples/)
and [Piper voice documentation](https://github.com/OHF-Voice/piper1-gpl/blob/main/docs/VOICES.md).
Android additionally requires a matching converted Piper/VITS voice from the
[Sherpa catalog](https://k2-fsa.github.io/sherpa/onnx/tts/pretrained_models/index.html);
not every engine in that catalog is supported. Adjust the helper's ID, name,
language, sample rate and license to match that voice's actual metadata.
Reading web samples contacts the provider; synthesis of your books after setup
uses local files. Locally calculated hashes do not authenticate the publisher.

Linux packages declare runtime `piper-onnx-v1`; Android packages declare
`sherpa-vits-v1`. The Android app includes its native engine and does not use the
Android system TTS provider. A voice for one runtime is not automatically a
complete package for the other.

Each package contains `manifest.json` and its declared artifacts. The manifest
uses `schema_version: 1`, a package `id`, display `name`, BCP-47 `language`,
`runtime`, `sample_rate`, `license`, `license_url`, and an `artifacts` array.
Each artifact declares its relative `path`, `role`, exact byte count `bytes`,
and hexadecimal `sha256`; downloads additionally require `url`. Piper needs
the ONNX model, its JSON configuration, and license/model-card files. Android
VITS packages additionally need the matching token table and any required
phonemizer data. Do not omit a model's required language data.
Android requires a converted Sherpa-compatible Piper/VITS model with the
engine's required metadata; renaming a raw Piper model or adding a token file
does not make it compatible.

Review the publisher and the model's own license before import. You can inspect
local artifact sizes with `wc -c` and hashes with `sha256sum`. A matching hash
only proves that files match the supplied manifest, not that the publisher is
trustworthy or the license permits your intended use.

On Linux, **Import offline model** selects the manifest alongside its files.
On Android, select the local package folder. Import leaves those originals
unchanged and stores the verified voice separately from graph content.
Choose an installed voice and its supported language explicitly; book-language
metadata is a suggestion, not permission to download or switch voices.

In-app downloads require direct public HTTPS artifact URLs and explicit
consent. Redirects and query-bearing URLs are rejected. For a provider that
uses signed redirects, download its files deliberately outside Grafium and
import a local package instead. No book text is included in model requests.

## Privacy and backups

Library records, reading positions, and automatic bookmarks live in application
storage, outside graph sync and AI indexing. Graph-folder backups alone do not
include this private state. Preserve the external originals separately and use
the reader's local backup controls where available. Exported backups may contain
private titles, paths, notes, and locations: protect their destination yourself.

If you deliberately write `[[Book title]]`, tags, quotations, or notes in your
journal, that text is ordinary graph content. It follows your graph's sync and
AI settings. Private reading does not make those manually written notes private.

Application backup exclusions cannot control a user's separately synchronized
source folder or every manufacturer/whole-device backup policy. Local synthesis
does not make Grafium's unrelated web, AI, or sync features network-free.
See [[AI Setup And Privacy]] and [[Sync And Privacy]].
