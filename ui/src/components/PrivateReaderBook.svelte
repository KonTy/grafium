<script lang="ts">
  import { onDestroy, tick, untrack } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import PrivateBookReader from "./PrivateBookReader.svelte";
  import LibraryMedia from "./LibraryMedia.svelte";
  import PrivateReaderToolbar from "./PrivateReaderToolbar.svelte";
  import SettingsHelp from "./SettingsHelp.svelte";
  import ReaderMenu from "./ReaderMenu.svelte";
  import { showToast } from "../lib/toast.svelte";
  import { requestLibraryMedia } from "../lib/library";
  import { privateLibrary, privateBookJump, privateVisualPositions, readerNative, refreshPrivateLibrary, savePrivateBookmark, bookmarkLabel, bookmarkDate, bookmarkExcerpt, compactBookmarkLabel, privateBookLanguages, setPrivateFavorite, type ReaderBookmark, type ReaderBook, type ReaderProgress } from "../lib/privateReader";
  import { formatBinding } from "../lib/shortcuts";
  import { BOOKMARK_SHORTCUT } from "../lib/readerHotkeys";
  import { playPrivateAudio, privatePlayback, bookmarkPrivatePlayback } from "../lib/privateReaderPlayback";
  import { isAndroidReader } from "../lib/privateReaderAndroid";
  import { startPrivateReadAloud } from "../lib/privateReaderVoice";
  let { bookId, onBack, onVoiceSettings, onAddToStudies, onJournalNote, initialBookmarkId, onPlayback, onActivity, onProgress }: {
    bookId: string; onBack: () => void; onVoiceSettings?: () => void;
    onAddToStudies?: (book: ReaderBook) => void;
    onJournalNote?: (book: ReaderBook, bookmark: ReaderBookmark) => void;
    initialBookmarkId?: string | null; onPlayback?: (playing: boolean) => void; onActivity?: () => void;
    onProgress?: (progress: ReaderProgress) => void;
  } = $props();
  const book = $derived($privateLibrary.books.find(item => item.id === bookId));
  let busy = $state(false);
  let error = $state("");
  let message = $state("");
  let editing = $state("");
  let note = $state("");
  let deleting = $state("");
  let visualReader = $state<PrivateBookReader>();
  let disposed = false;
  onDestroy(() => { disposed = true; });
  const android = isAndroidReader();
  let relinking = $state(false);
  let replacementBookId = $state("");
  let replacementPath = $state("");
  let audioRelinkChoice = $state(false);
  let consumedBookmark = "";
  let ignoredBookmark = $state<string | null>(null);
  const missingInitialBookmark = $derived(!!initialBookmarkId && !!book
    && !book.bookmarks.some(mark => mark.id === initialBookmarkId) && ignoredBookmark !== initialBookmarkId);
  const reading = $derived(book?.kind === "epub" && book.available && !missingInitialBookmark);
  let reportedPlayback = "";
  let reportedProgress = "";
  $effect(() => {
    if (!book?.available) return;
    const receive = (event: Event) => {
      if (event.defaultPrevented) return;
      event.preventDefault();
      if (!busy) void run(bookmark, "Bookmark saved on this device.");
    };
    window.addEventListener("grafium-bookmark", receive);
    return () => window.removeEventListener("grafium-bookmark", receive);
  });
  $effect(() => {
    const id = initialBookmarkId;
    const current = book;
    if (!id || !current || consumedBookmark === `${bookId}:${id}`) return;
    consumedBookmark = `${bookId}:${id}`;
    untrack(() => {
      const mark = current.bookmarks.find(mark => mark.id === id);
      if (mark && current.available) void run(() => jump(mark));
    });
  });
  $effect(() => {
    if (!book || book.kind === "video" || book.kind === "youtube" || (book.sourceUrl && android)) return;
    const state = $privatePlayback;
    const playing = state.bookId === bookId && state.status === "playing";
    const position = state.bookId === bookId ? state.position : null;
    // Observations for the Study clock only; Library owns the durable checkpoint.
    const progress: ReaderProgress | null = position ? {
      position: position.offsetMs / 1000, total: 0,
      anchor: position.locator?.kind === "epub" ? position.locator.cfi : position.trackId ?? "",
      label: bookmarkLabel(book, position),
    } : null;
    const playbackKey = `${bookId}:${playing}`;
    const progressKey = `${bookId}:${JSON.stringify(progress)}`;
    untrack(() => {
      const changedPlayback = playbackKey !== reportedPlayback;
      const changedProgress = progressKey !== reportedProgress;
      reportedPlayback = playbackKey; reportedProgress = progressKey;
      if (changedPlayback) onPlayback?.(playing);
      if (changedProgress && progress) onProgress?.(progress);
      if (playing && (changedPlayback || changedProgress)) onActivity?.();
    });
  });
  async function run(action: () => Promise<unknown>, success = "") {
    busy = true; error = ""; message = "";
    try { await action(); if (success && reading) showToast(success); else message = success; }
    catch (cause) { error = String(cause); if (disposed) showToast(error, "error"); }
    finally { busy = false; }
  }
  async function relink(directory?: boolean) {
    if (!book) return;
    if (android) { await refreshPrivateLibrary(true); relinking = true; return; }
    if (book.kind === "audio" && directory === undefined) { audioRelinkChoice = true; return; }
    const path = await open({ directory: directory === true, multiple: false,
      title: directory ? "Choose the top-level audiobook folder" : "Choose the replacement EPUB, video, or loose audio file" });
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
    if (bookmark.position.locator) privateBookJump.set({ bookId, locator: bookmark.position.locator, select: true });
    else if (book.kind === "video" || book.kind === "youtube" || (book.sourceUrl && android)) requestLibraryMedia(bookId, bookmark.position);
    else await playPrivateAudio(book, bookmark.position);
  }
  async function bookmark() {
    if (book?.kind === "epub") {
      if (!reading || !visualReader) throw new Error("Open an available EPUB before bookmarking.");
      const captured = await visualReader.captureBookmark();
      await savePrivateBookmark(bookId, { offsetMs: 0, locator: captured.location }, bookmarkExcerpt(captured.quote));
    } else if ($privatePlayback.bookId === bookId && $privatePlayback.status !== "stopped") await bookmarkPrivatePlayback();
    else {
      const current = privateVisualPositions.get(bookId) ?? book?.position;
      if (!current) throw new Error("Open a passage or chapter before bookmarking.");
      await savePrivateBookmark(bookId, current);
    }
  }
</script>

{#snippet bookActions()}
  {#if book}
      <div class="actions">
        <button disabled={busy} aria-pressed={book.favorite ?? false} onclick={() => run(() => setPrivateFavorite(bookId, !book!.favorite))}>{book.favorite ? "★ Favorite" : "☆ Favorite"}</button>
        {#if onAddToStudies}<button onclick={() => onAddToStudies?.(book!)}>Add to Studies</button>{/if}
        {#if !reading}<button title={`Bookmark (${formatBinding(BOOKMARK_SHORTCUT)})`} disabled={busy || !book.available} onclick={() => run(bookmark, "Bookmark saved on this device.")}>Bookmark</button>{/if}
        {#if !book.sourceUrl}<button disabled={busy} onclick={() => run(relink)}>Relink source…</button>{/if}
        <SettingsHelp title="Library reading and bookmarks"><p>Sources, progress, and bookmarks stay outside graph sync and AI. Add a private comment to a bookmark here, or choose Journal note to review a draft before saving it to your graph. A Study references this same source and position.</p><p>Audio and read aloud continue outside Library and across graph switches. Videos, YouTube, and Android network audio stop when you leave their player. Local video currently requires desktop.</p></SettingsHelp>
      </div>
      {#if reading}
        <div class="actions">
          <button disabled={busy} onclick={() => run(() => startPrivateReadAloud(bookId))}>Resume read aloud</button>
          <button disabled={busy} onclick={() => run(() => startPrivateReadAloud(bookId, true))}>Read aloud from start</button>
          {#if onVoiceSettings}<button onclick={onVoiceSettings}>Choose voice and language…</button>{/if}
        </div>
        {#if $privateBookLanguages[bookId]}<small>Book language: {$privateBookLanguages[bookId]}</small>{/if}
      {/if}
  {/if}
{/snippet}

{#snippet privateBookmarks()}
  {#if book}
    <section class="bookmarks" aria-label="Bookmarks">
      {#if !book.bookmarks.length}<p class="hint">No bookmarks yet.</p>{/if}
      <ul>{#each book.bookmarks as mark (mark.id)}
        <li>
          <div class="bookmark-row">
            <button class="bookmark-jump" disabled={busy || !book.available}
              title={`${compactBookmarkLabel(book, mark)}\n${bookmarkLabel(book, mark.position)}\n${bookmarkDate(mark.createdAt) ? new Date(mark.createdAt).toLocaleString() : "Unknown date"}`}
              aria-label={`Go to bookmark: ${compactBookmarkLabel(book, mark)}`} onclick={() => run(() => jump(mark))}>
              <span>{compactBookmarkLabel(book, mark)}</span>
            </button>
            <ReaderMenu label={`Actions for bookmark: ${compactBookmarkLabel(book, mark)}`} heading="Bookmark actions">
              {#snippet children(close)}
                <button disabled={busy || !book.available} onclick={() => { close(); void run(() => jump(mark)); }}>Go to</button>
                <button disabled={busy} onclick={async () => {
                  close(); deleting = ""; editing = mark.id; note = mark.note;
                  await tick(); document.getElementById(`bookmark-note-${mark.id}`)?.focus();
                }}>Edit</button>
                <button disabled={busy} onclick={async () => {
                  close(); editing = ""; deleting = mark.id;
                  await tick(); document.getElementById(`bookmark-cancel-${mark.id}`)?.focus();
                }}>Delete…</button>
                {#if onJournalNote}<button onclick={() => { close(); onJournalNote?.(book!, mark); }}>Journal note…</button>{/if}
              {/snippet}
            </ReaderMenu>
          </div>
          {#if editing === mark.id}
            <form onsubmit={event => { event.preventDefault(); void run(async () => {
              await readerNative("update_bookmark", { bookId, bookmarkId: mark.id, note }); await refreshPrivateLibrary(); editing = "";
            }, "Bookmark saved."); }}>
              <label>Bookmark label / private note<textarea id={`bookmark-note-${mark.id}`} bind:value={note} maxlength="4096" rows="3"
                onkeydown={event => {
                  if (!busy && !event.repeat && !event.isComposing && !event.altKey
                    && (event.ctrlKey || event.metaKey) && event.key === "Enter") {
                    event.preventDefault(); event.currentTarget.form?.requestSubmit();
                  }
                }}></textarea></label>
              <div class="actions"><button title="Save (Ctrl/Cmd+Enter)" disabled={busy}>Save</button><button type="button" onclick={() => editing = ""}>Cancel</button></div>
            </form>
          {:else if deleting === mark.id}
            <div class="delete-bookmark" role="group" aria-label="Confirm bookmark deletion">
              <p>Delete “{compactBookmarkLabel(book, mark)}”? Only this bookmark and its private note will be removed. The book and reading position stay unchanged.</p>
              <div class="actions">
                <button id={`bookmark-cancel-${mark.id}`} disabled={busy} onclick={() => deleting = ""}>Cancel</button>
                <button disabled={busy} onclick={() => run(async () => {
                  await readerNative("delete_bookmark", { bookId, bookmarkId: mark.id });
                  await refreshPrivateLibrary(); deleting = "";
                }, "Bookmark deleted.")}>Delete bookmark</button>
              </div>
            </div>
          {/if}
        </li>
      {/each}</ul>
    </section>
  {/if}
{/snippet}

{#snippet readingStatus()}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
  {#if book}
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
  {/if}
{/snippet}

<section class="private-detail" class:reading data-help-context="reader">
  {#if !reading}<button onclick={onBack}>← Library</button>{/if}
  {#if book}
    {#if !reading}
    <header><div><p class="eyebrow">APP-PRIVATE · {book.kind.toUpperCase()}</p><h1>{book.title}</h1></div>
      {@render bookActions()}
    </header>
    {/if}
    {#if !book.available}<p class="unavailable">Source unavailable. Progress and bookmarks have been retained. Reconnect the library or relink this book; a different chapter will never be chosen silently.</p>{/if}
    {#if book.error}<p class="error" role="alert">{book.error}</p>{/if}
    {#if !reading}{@render readingStatus()}{/if}
    {#if missingInitialBookmark}
      <p class="error" role="alert">This bookmark is no longer in the local Library. No replacement position was opened.</p>
      <button onclick={() => ignoredBookmark = initialBookmarkId ?? null}>Open current saved place</button>
    {:else if book.kind === "audio" && !(book.sourceUrl && android)}
      {#if $privatePlayback.bookId === bookId && $privatePlayback.status !== "stopped"}
        <PrivateReaderToolbar {bookId} />
      {:else}
        <div class="actions"><button class="primary" disabled={busy || !book.available} onclick={() => run(() => playPrivateAudio(book!))}>{book.position ? "Resume audio" : "Play audio"}</button></div>
      {/if}
      {#if book.tracks.length}<details class="chapters" open><summary>Chapters · {book.tracks.length}</summary><ol>
        {#each book.tracks as track, index (track.id)}
          <li><button class="chapter" disabled={busy || !book.available || track.available === false} onclick={() => run(() => playPrivateAudio(book!, { trackId: track.id, offsetMs: 0 }))}>{track.title}</button>
            <small>{track.relativePath}{track.available === false ? " · Source unavailable" : ""}</small><span class="order">
              <button aria-label={`Move ${track.title} earlier`} disabled={busy || index === 0} onclick={() => run(() => reorder(index, -1))}>↑</button>
              <button aria-label={`Move ${track.title} later`} disabled={busy || index === book!.tracks.length - 1} onclick={() => run(() => reorder(index, 1))}>↓</button>
            </span></li>
        {/each}
      </ol></details>{/if}
    {:else if book.kind === "epub" && book.available}
      <PrivateBookReader bind:this={visualReader} {bookId} {onActivity} {onProgress} {onBack}
        onBookmark={() => { if (!busy) void run(bookmark, "Bookmark saved on this device."); }}
        actions={bookActions} bookmarks={privateBookmarks} status={readingStatus} />
    {:else if book.available}
      {#key bookId}<LibraryMedia {book} {onPlayback} {onActivity} {onProgress} />{/key}
    {/if}
    {#if !reading}<h2>Bookmarks <span>{book.bookmarks.length}</span></h2>{@render privateBookmarks()}{/if}
  {:else}<p role="alert">This source is not in the local library. Return to Library and rescan.</p>{/if}
</section>

<style>
  .private-detail { max-width: 1100px; margin: auto; padding: 24px clamp(16px, 4vw, 48px); color: var(--text-primary); }
  .private-detail.reading { display: flex; flex-direction: column; max-width: none; height: 100%; min-height: 0; padding: 0; margin: 0; }
  header, .actions { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; } header { justify-content: space-between; margin-top: 20px; }
  h1 { margin: 5px 0; font-size: 27px; overflow-wrap: anywhere; } h2 { font-size: 18px; } h2 span { color: var(--text-muted); font-weight: 400; }
  .eyebrow { color: var(--accent); font-size: 9px; letter-spacing: .1em; } .hint, small { color: var(--text-muted); font-size: 12px; line-height: 1.6; }
  button, textarea { font: inherit; color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); border-radius: 6px; padding: 8px 12px; }
  button { cursor: pointer; } button:disabled { opacity: .5; cursor: default; } button:focus-visible, textarea:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .primary { color: var(--accent); } .chapters { margin-top: 24px; } .bookmarks { margin-top: 8px; } summary { cursor: pointer; font-weight: 600; }
  ol, ul { list-style: none; padding: 0; } li { border-bottom: 1px solid var(--border); padding: 12px 0; } ol li { display: grid; grid-template-columns: 1fr auto; align-items: center; gap: 4px 12px; }
  .chapter { border: 0; padding: 2px 0; text-align: left; background: none; } ol small { grid-column: 1; overflow-wrap: anywhere; } .order { display: flex; gap: 5px; grid-column: 2; grid-row: 1 / 3; }
  .bookmarks li { padding: 2px 0; }
  .bookmark-row { display: flex; align-items: center; gap: 4px; min-width: 0; }
  .bookmark-jump { display: block; flex: 1; min-width: 0; border: 0; padding: 6px 4px; text-align: left; background: none; }
  .bookmark-jump span { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .bookmark-jump:hover { background: var(--bg-hover); }
  label { display: flex; flex-direction: column; gap: 6px; margin: 12px 0; } textarea { resize: vertical; }
  .delete-bookmark { padding: 8px; background: var(--bg-secondary); font-size: 13px; }
  @media (pointer: coarse) { .bookmark-jump { min-height: 44px; } }
  .unavailable { padding: 12px; border: 1px solid var(--border); border-radius: 8px; } .error { color: var(--danger, #c44); overflow-wrap: anywhere; }
</style>
