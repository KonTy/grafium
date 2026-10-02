<script lang="ts">
  import { privatePlayback, pausePrivatePlayback, resumePrivatePlayback, stopPrivatePlayback, bookmarkPrivatePlayback, skipPrivateAudio } from "../lib/privateReaderPlayback";
  import { readerTime } from "../lib/privateReader";
  import { formatBinding } from "../lib/shortcuts";
  import { BOOKMARK_SHORTCUT } from "../lib/readerHotkeys";
  let { onOpen }: { onOpen: (bookId: string) => void } = $props();
  let busy = $state(false);
  let feedback = $state("");
  let error = $state("");
  async function run(action: () => Promise<void>, success = "") {
    busy = true; feedback = ""; error = "";
    try { await action(); feedback = success; }
    catch (cause) { error = String(cause); }
    finally { busy = false; }
  }
</script>

{#if $privatePlayback.bookId && ($privatePlayback.status !== "stopped" || error || $privatePlayback.error)}
  <section class="reader-bar" aria-label="Private reader playback" data-help-context="reader">
    <div class="identity">
      <span class="privacy">LIBRARY · {$privatePlayback.mode === "tts" ? "READ ALOUD" : "AUDIO"}</span>
      <button class="title" onclick={() => onOpen($privatePlayback.bookId!)}>{$privatePlayback.title}</button>
      <small>{$privatePlayback.status} · {readerTime($privatePlayback.position?.offsetMs ?? 0)}</small>
    </div>
    <div class="actions">
      {#if $privatePlayback.mode === "audio"}
        <button disabled={busy || $privatePlayback.status === "loading"} aria-label="Back 15 seconds" onclick={() => run(() => skipPrivateAudio(-15000))}>−15s</button>
        <button disabled={busy || $privatePlayback.status === "loading"} aria-label="Forward 15 seconds" onclick={() => run(() => skipPrivateAudio(15000))}>+15s</button>
      {/if}
      <button title={`Bookmark playback (${formatBinding(BOOKMARK_SHORTCUT)} when no visual book is open)`} disabled={busy || $privatePlayback.status === "loading"} onclick={() => run(bookmarkPrivatePlayback, "Bookmark saved on this device.")}>Bookmark</button>
      <button disabled={busy || $privatePlayback.status === "loading"} onclick={() => run($privatePlayback.status === "playing" ? pausePrivatePlayback : resumePrivatePlayback)}>{$privatePlayback.status === "playing" ? "Pause" : "Resume"}</button>
      <button disabled={busy} onclick={() => run(stopPrivatePlayback)}>Stop</button>
    </div>
    {#if error || $privatePlayback.error}<p role="alert">{error || $privatePlayback.error}</p>
    {:else if feedback}<p role="status">{feedback}</p>{/if}
  </section>
{/if}

<style>
  .reader-bar { flex: 0 0 auto; display: flex; align-items: center; flex-wrap: wrap; gap: 10px 20px; padding: 10px 18px; background: var(--bg-secondary); border-bottom: 1px solid var(--border); color: var(--text-primary); z-index: 2; }
  .identity { display: flex; flex: 1; min-width: 120px; flex-direction: column; gap: 3px; overflow: hidden; }
  .privacy { font-size: 9px; color: var(--accent); letter-spacing: .1em; }
  button { font: inherit; color: var(--text-primary); border: 1px solid var(--border); background: var(--bg-primary); border-radius: 6px; padding: 7px 12px; cursor: pointer; }
  .title { font-weight: 600; text-align: left; border: 0; padding: 0; background: transparent; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .actions { display: flex; flex-wrap: wrap; gap: 8px; } small { color: var(--text-muted); font-size: 11px; }
  button:disabled { opacity: .55; cursor: default; } button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  p { flex-basis: 100%; font-size: 12px; margin: 0; overflow-wrap: anywhere; } [role="alert"] { color: var(--danger, #c44); }
</style>
