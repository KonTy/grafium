# Studies

Open **Studies** in the navigation to build your own learning list. Studies is
separate from Tasks: adding an item does not create a task, duplicate your notes,
or change a source file.
The command palette includes **Go to Studies**; in navigation mode, press `g s`.

## Add and organize

Use **Add to Studies** on a page or original book, or add an item from Studies.
Choose a graph page (including a link-created placeholder), original book,
flashcard topic, audio/video source, YouTube video, or website.
For a graph link such as `[[Supplements]]`, choose its page in the picker.

Give each row an arbitrary **Topic** such as Health, General, or Chinese.
This is an organizational label, separate from the tag that selects flashcards.
Use the topic filter to focus the list and its time cards. Editing a topic
does not retag your graph or move cards between review decks.
Removing a study entry removes its study history, not its book, page, or media.

## Resume where you stopped

- **Pages and converted books:** open the item to restore its saved block and
  offset when available, with proportional scroll position as a fallback.
  The percentage is a scroll-position estimate, not a count of words learned.
  Editing or folding a source can change the apparent position.
- **Original books:** use the existing reader's saved page or EPUB location.
  The row shows the latest position reported while studying. The original book
  file is not modified. See [[Help - Books]].
- **Flashcards:** opening a topic starts the cards currently due under the
  existing spaced-repetition schedule. Reveal and grade as usual; progress
  describes the latest review session, not permanent mastery. See [[Help - Flashcards]].
- **Audio and video:** use a graph-relative media path or a direct HTTP(S)
  media URL. The player resumes at the saved timestamp after metadata loads.
  Formats/codecs depend on your platform. A web page containing a player is not
  a direct media URL. Press Play if automatic playback is blocked.
- **YouTube:** add a video URL, then open the row to load its embedded player.
  Grafium saves the timestamp when the player reports it. Network access is
  required; age, account, region, embedding restrictions, and WebView identity
  requirements may prevent playback. Use the external link if embedding fails.
- **Websites:** open in your browser and save a manual checkpoint in Grafium,
  such as a section heading or page number. Grafium does not observe your
  external browser, resume its scroll position, or estimate time spent there.

External content is contacted only when opened, not while browsing the study
list. Opening a YouTube player contacts YouTube; direct remote media contacts
its host. No media is downloaded into your graph merely by adding its URL.
YouTube uses an isolated local HTTP wrapper to identify the embedded player.
That wrapper serves only the player shell: it cannot read your graph or invoke
Grafium commands, and does not expose source files through its local address.

## Active time, not an attendance timer

Time is recorded only for an item explicitly opened from Studies, while Grafium
is visible and focused. A session bar shows its status and **Pause clock** /
**Resume clock** controls. Use **Back to Studies** when finished.

Reading and flashcards pause after **90 seconds** without a reading-position
change or a card interaction. The next interaction resumes counting without
adding the idle gap. This is an estimate: it can miss slow reading and cannot
know whether you are paying attention. Scrolling the mouse over an unrelated
panel does not count as reading.

Audio/video time follows playback with fresh player progress, not mouse motion.
Paused, buffering, hidden, and suspended time is not intentionally counted;
short player-reporting delays may affect the estimate. Seeking forward does not
add the skipped duration to study time. Website checkpoints do not run a clock.

Time and progress are saved periodically and when leaving a session. An abrupt
process termination may lose the latest few seconds. Save failures are shown
explicitly: keep Grafium open and use **Retry saving**. Studies data belongs to
the current graph; back up its application metadata/database along with sources.
The list and history are not a replacement for source-file backups.

F1 uses bundled current guidance even for an existing Welcome graph. Existing
tutorial notes are never refreshed or overwritten by this feature.
