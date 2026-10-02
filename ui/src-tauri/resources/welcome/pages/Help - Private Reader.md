# Private reader

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
you work elsewhere in Grafium. Stop retains your place. A bookmark is a private
saved location, not an automatically created note or graph page. A save error
means the location was not confirmed as durable; do not assume it was saved.
Listening checkpoints normally run every four seconds on Linux and every three
seconds in the Android audio service, with saves at playback transitions. The
recovery target is at most five seconds of listening when the app is scheduled
normally and private storage is writable. An abrupt process termination,
stalled storage, or operating-system suspension can exceed that target.
Bookmarks acknowledged as saved are durable independently of the next periodic
checkpoint.

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
