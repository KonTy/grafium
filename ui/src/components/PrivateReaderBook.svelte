<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import PrivateBookReader from "./PrivateBookReader.svelte";
  import { privateLibrary, privateBookJump, privateVisualPositions, readerNative, refreshPrivateLibrary, savePrivateBookmark, bookmarkLabel, bookmarkDate, privateBookLanguages, type ReaderBookmark } from "../lib/privateReader";
  import { playPrivateAudio, privatePlayback, bookmarkPrivatePlayback } from "../lib/privateReaderPlayback";
  import { isAndroidReader } from "../lib/privateReaderAndroid";
  import { startPrivateReadAloud } from "../lib/privateReaderVoice";
  let { bookId, onBack, onVoiceSettings }: { bookId: string; onBack: () => void; onVoiceSettings?: () => void } = $props();
  const book = $derived($privateLibrary.books.find(item => item.id === bookId));
  let busy = $state(false);
  let error = $state("");
  let message = $state("");
  let editing = $state("");
  let note = $state("");
  const android = isAndroidReader();
  let relinking = $state(false);
  let replacementBookId = $state("");
  let replacementPath = $state("");
  let audioRelinkChoice = $state(false);
  async function run(action: () => Promise<unknown>, success = "") {
    busy = true; error = ""; message = "";
    try { await action(); message = success; }
    catch (cause) { error = String(cause); }
    finally { busy = false; }
  }
  async function relink(directory?: boolean) {
    if (!book) return;
    if (android) { await refreshPrivateLibrary(true); relinking = true; return; }
    if (book.kind === "audio" && directory === undefined) { audioRelinkChoice = true; return; }
    const path = await open({ directory: directory === true, multiple: false,
      title: directory ? "Choose the top-level audiobook folder" : "Choose the replacement EPUB or loose root MP3" });
    if (typeof path !== "string") return;
    const library = $privateLibrary.libraryPath?.replace(/\/+$/, "");
    if (!library || !path.startsWith(`${library}/`)) throw new Error("Choose a replacement inside your current private library folder.");
    replacementPath = path.slice(library.length + 1);
    audioRelinkChoice = false;
    relinking = true;
  }
  async function reorder(index: number, direction: number) {
    if (!book) return;
    const trackIds = book.tracks.map(track => track.id);
    [trackIds[index], trackIds[index + direction]] = [trackIds[index + direction], trackIds[index]];
    await readerNative("reorder", { bookId, trackIds });
    await refreshPrivateLibrary();
  }
  async function jump(bookmark: ReaderBookmark) {
    if (!book) return;
    if (bookmark.position.locator) privateBookJump.set({ bookId, locator: bookmark.position.locator });
    else await playPrivateAudio(book, bookmark.position);
  }
  async function bookmark() {
    if ($privatePlayback.bookId === bookId && $privatePlayback.status !== "stopped") await bookmarkPrivatePlayback();
    else {
      const current = privateVisualPositions.get(bookId) ?? book?.position;
      if (!current) throw new Error("Open a passage or chapter before bookmarking.");
      await savePrivateBookmark(bookId, current);
    }
  }
</script>

<section class="private-detail" data-help-context="reader">
  <button onclick={onBack}>← Private library</button>
  {#if book}
    <header><div><p class="eyebrow">APP-PRIVATE · {book.kind === "audio" ? "AUDIOBOOK" : "EPUB"}</p><h1>{book.title}</h1></div>
      <div class="actions"><button disabled={busy || !book.available || !book.position} onclick={() => run(bookmark, "Bookmark saved on this device.")}>Bookmark</button><button disabled={busy} onclick={() => run(relink)}>Relink source…</button></div>
    </header>
    <p class="privacy">Your books and automatic bookmarks stay out of graph sync and AI. Journal notes you intentionally write with [[Book title]] remain ordinary graph content.</p>
    {#if !book.available}<p class="unavailable">Source unavailable. Progress and bookmarks have been retained. Reconnect the library or relink this book; a different chapter will never be chosen silently.</p>{/if}
    {#if book.error}<p class="error" role="alert">{book.error}</p>{/if}
    {#if audioRelinkChoice}
      <section class="relink" aria-label="Choose audiobook replacement type">
        <p>Choose the entire top-level audiobook folder, even if it contains only one chapter. Use a file only for an MP3 stored directly in the library root.</p>
        <div class="actions">
          <button disabled={busy} onclick={() => run(() => relink(true))}>Choose audiobook folder…</button>
          <button disabled={busy} onclick={() => run(() => relink(false))}>Choose loose root MP3…</button>
          <button onclick={() => audioRelinkChoice = false}>Cancel relink</button>
        </div>
      </section>
    {/if}
    {#if relinking}
      <form class="relink" onsubmit={event => { event.preventDefault(); void run(async () => {
        await readerNative("relink", android ? { bookId, replacementBookId } : { bookId, relativePath: replacementPath, confirmReplacement: true });
        await refreshPrivateLibrary(); relinking = false;
      }, "Source relinked; private history retained."); }}>
        {#if android}<label>Discovered replacement source<select bind:value={replacementBookId}><option value="" disabled>Choose a source…</option>{#each $privateLibrary.books.filter(item => item.id !== bookId && item.available && item.kind === book!.kind) as replacement}<option value={replacement.id}>{replacement.title}</option>{/each}</select></label>
        {:else}<p>Replacement: <strong>{replacementPath}</strong></p>{/if}
        <p class="hint">Confirm this is the intended source for “{book.title}”. Relinking retains history, but a changed source may invalidate older passages or chapter positions. Source files are never modified.</p>
        <div class="actions"><button disabled={busy || (android ? !replacementBookId : !replacementPath)}>Confirm relink</button><button type="button" onclick={() => relinking = false}>Cancel</button></div>
      </form>
    {/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if message}<p role="status">{message}</p>{/if}
    {#if book.kind === "audio"}
      <div class="actions"><button class="primary" disabled={busy || !book.available} onclick={() => run(() => playPrivateAudio(book!))}>{book.position ? "Resume audiobook" : "Play audiobook"}</button><span class="hint">Playback continues outside Studies and across graph switches.</span></div>
      <details class="chapters" open><summary>Chapters · {book.tracks.length}</summary><ol>
        {#each book.tracks as track, index (track.id)}
          <li><button class="chapter" disabled={busy || !book.available || track.available === false} onclick={() => run(() => playPrivateAudio(book!, { trackId: track.id, offsetMs: 0 }))}>{track.title}</button>
            <small>{track.relativePath}{track.available === false ? " · Source unavailable" : ""}</small><span class="order">
              <button aria-label={`Move ${track.title} earlier`} disabled={busy || index === 0} onclick={() => run(() => reorder(index, -1))}>↑</button>
              <button aria-label={`Move ${track.title} later`} disabled={busy || index === book!.tracks.length - 1} onclick={() => run(() => reorder(index, 1))}>↓</button>
            </span></li>
        {/each}
      </ol></details>
    {:else if book.available}
      {#if $privateBookLanguages[bookId]}
        <p class="hint">Book language suggestion: <strong>{$privateBookLanguages[bookId]}</strong>.
          {#if onVoiceSettings}<button class="text-button" onclick={onVoiceSettings}>Choose voice and language…</button>{/if}
          Metadata never changes your voice or downloads a model automatically.</p>
      {/if}
      <div class="actions">
        <button disabled={busy} onclick={() => run(() => startPrivateReadAloud(bookId))}>Resume read aloud</button>
        <button disabled={busy} onclick={() => run(() => startPrivateReadAloud(bookId, true))}>Read aloud from start</button>
        <span class="hint">{android ? "Your complete narration queue and progress are owned by the native offline service after preparation." : "Uses your installed offline voice. Playback continues when you leave this page."}</span>
      </div>
      <PrivateBookReader {bookId} />
    {/if}
    <section class="bookmarks" aria-label="Private bookmarks">
      <h2>Private bookmarks <span>{book.bookmarks.length}</span></h2>
      {#if !book.bookmarks.length}<p class="hint">Capture a passage or listening position here or from the global reader toolbar.</p>{/if}
      <ul>{#each book.bookmarks as mark (mark.id)}
        <li><div class="bookmark-heading"><button disabled={busy || !book.available} onclick={() => run(() => jump(mark))}>{bookmarkLabel(book, mark.position)}</button><time datetime={bookmarkDate(mark.createdAt)}>{bookmarkDate(mark.createdAt) ? new Date(mark.createdAt).toLocaleString() : "Unknown date"}</time></div>
          {#if editing === mark.id}
            <form onsubmit={event => { event.preventDefault(); void run(async () => {
              await readerNative("update_bookmark", { bookId, bookmarkId: mark.id, note }); await refreshPrivateLibrary(); editing = "";
            }, "Private note saved."); }}><label>Private bookmark note<textarea bind:value={note} maxlength="4096" rows="3"></textarea></label><div class="actions"><button disabled={busy}>Save private note</button><button type="button" onclick={() => editing = ""}>Cancel</button></div></form>
          {:else}
            {#if mark.note}<p class="note">{mark.note}</p>{/if}
            <button class="text-button" onclick={() => { editing = mark.id; note = mark.note; }}>{mark.note ? "Edit private note" : "Add private note"}</button>
          {/if}
        </li>
      {/each}</ul>
    </section>
  {:else}<p role="alert">This private book is not in the local library. Return to Studies and rescan.</p>{/if}
</section>

<style>
  .private-detail { max-width: 1100px; margin: auto; padding: 24px clamp(16px, 4vw, 48px); color: var(--text-primary); }
  header, .actions, .bookmark-heading { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; } header { justify-content: space-between; margin-top: 20px; }
  h1 { margin: 5px 0; font-size: 27px; overflow-wrap: anywhere; } h2 { font-size: 18px; } h2 span { color: var(--text-muted); font-weight: 400; }
  .eyebrow { color: var(--accent); font-size: 9px; letter-spacing: .1em; } .privacy, .hint, small, time { color: var(--text-muted); font-size: 12px; line-height: 1.6; }
  button, textarea { font: inherit; color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); border-radius: 6px; padding: 8px 12px; }
  button { cursor: pointer; } button:disabled { opacity: .5; cursor: default; } button:focus-visible, textarea:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .primary { color: var(--accent); } .chapters, .bookmarks { margin-top: 24px; } summary { cursor: pointer; font-weight: 600; }
  ol, ul { list-style: none; padding: 0; } li { border-bottom: 1px solid var(--border); padding: 12px 0; } ol li { display: grid; grid-template-columns: 1fr auto; align-items: center; gap: 4px 12px; }
  .chapter { border: 0; padding: 2px 0; text-align: left; background: none; } ol small { grid-column: 1; overflow-wrap: anywhere; } .order { display: flex; gap: 5px; grid-column: 2; grid-row: 1 / 3; }
  .bookmark-heading { justify-content: space-between; } .note { white-space: pre-wrap; overflow-wrap: anywhere; }
  .text-button { border: 0; padding: 5px 0; color: var(--accent); background: none; } label { display: flex; flex-direction: column; gap: 6px; margin: 12px 0; } textarea { resize: vertical; }
  .unavailable { padding: 12px; border: 1px solid var(--border); border-radius: 8px; } .error { color: var(--danger, #c44); overflow-wrap: anywhere; }
</style>
