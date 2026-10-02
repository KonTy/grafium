<script lang="ts">
  import { privatePlayback, pausePrivatePlayback, resumePrivatePlayback, stopPrivatePlayback, bookmarkPrivatePlayback, skipPrivateAudio, seekPrivateAudioPosition, setPrivatePlaybackRate } from "../lib/privateReaderPlayback";
  import { PLAYBACK_RATES, mediaPlaybackRate, speechPlaybackRate } from "../lib/readerPlaybackPreferences";
  import { readerTime } from "../lib/privateReader";
  import { formatBinding } from "../lib/shortcuts";
  import { BOOKMARK_SHORTCUT } from "../lib/readerHotkeys";
  let { onOpen, bookId }: { onOpen?: (bookId: string) => void; bookId?: string } = $props();
  let busy = $state(false);
  let stopping = $state(false);
  let feedback = $state("");
  let error = $state("");
  const duration = $derived($privatePlayback.durationMs ?? 0);
  const canSeek = $derived($privatePlayback.mode === "audio" && $privatePlayback.seekable && duration > 0);
  const rate = $derived($privatePlayback.playbackRate ?? ($privatePlayback.mode === "tts" ? $speechPlaybackRate : $mediaPlaybackRate));
  async function stop() {
    stopping = true; feedback = ""; error = "";
    try { await stopPrivatePlayback(); }
    catch (cause) { error = String(cause); }
    finally { stopping = false; }
  }
  async function run(action: () => Promise<void>, success = "") {
    busy = true; feedback = ""; error = "";
    try { await action(); feedback = success; }
    catch (cause) { error = String(cause); }
    finally { busy = false; }
  }
</script>

{#if $privatePlayback.bookId && (!bookId || bookId === $privatePlayback.bookId) && ($privatePlayback.status !== "stopped" || error || $privatePlayback.error)}
  <section class="reader-bar" aria-label={bookId ? "Audiobook playback" : "Library playback"} data-help-context="reader">
    <div class="identity">
      <span class="privacy">LIBRARY · {$privatePlayback.mode === "tts" ? "READ ALOUD" : "AUDIO"}</span>
      {#if onOpen}<button class="title" onclick={() => onOpen?.($privatePlayback.bookId!)}>{$privatePlayback.title}</button>
      {:else}<strong>{$privatePlayback.title}</strong>{/if}
      <small>{$privatePlayback.status} · {readerTime($privatePlayback.position?.offsetMs ?? 0)}{duration > 0 ? ` / ${readerTime(duration)}` : ""}</small>
    </div>
    <div class="actions">
      <label>Speed
        <select aria-label={$privatePlayback.mode === "tts" ? "Read-aloud speed" : "Audio playback speed"}
          title="Remembered playback speed; read aloud has its own setting"
          value={rate} disabled={busy || stopping || $privatePlayback.status === "loading"}
          onchange={event => {
            const selected = Number(event.currentTarget.value);
            event.currentTarget.value = String(rate);
            void run(() => setPrivatePlaybackRate(selected));
          }}>
          {#if !PLAYBACK_RATES.includes(rate)}<option value={rate}>{rate}×</option>{/if}
          {#each PLAYBACK_RATES as speed}<option value={speed}>{speed}×</option>{/each}
        </select>
      </label>
      {#if $privatePlayback.mode === "audio"}
        <button disabled={busy || stopping || !canSeek || $privatePlayback.status === "loading"} aria-label="Back 15 seconds" onclick={() => run(() => skipPrivateAudio(-15000))}>−15s</button>
        <button disabled={busy || stopping || !canSeek || $privatePlayback.status === "loading"} aria-label="Forward 15 seconds" onclick={() => run(() => skipPrivateAudio(15000))}>+15s</button>
      {/if}
      <button title={`Bookmark playback (${formatBinding(BOOKMARK_SHORTCUT)} when no visual book is open)`} disabled={busy || $privatePlayback.status === "loading"} onclick={() => run(bookmarkPrivatePlayback, "Bookmark saved on this device.")}>Bookmark</button>
      <button disabled={busy || stopping || $privatePlayback.status === "loading" || $privatePlayback.status === "stopped"} onclick={() => run($privatePlayback.status === "playing" ? pausePrivatePlayback : resumePrivatePlayback)}>{$privatePlayback.status === "playing" ? "Pause" : "Resume"}</button>
      <button class="stop" disabled={stopping} onclick={stop}>{stopping ? "Stopping…" : "Stop"}</button>
    </div>
    <div class="timeline">
      <label>Seek {$privatePlayback.mode === "audio" ? "audio" : "read aloud"}
        <input type="range" min="0" max={duration || 1} step="1000"
          value={Math.min($privatePlayback.position?.offsetMs ?? 0, duration)}
          aria-valuetext={`${readerTime($privatePlayback.position?.offsetMs ?? 0)}${duration ? ` of ${readerTime(duration)}` : ""}`}
          disabled={!canSeek || busy || stopping || $privatePlayback.status === "loading" || $privatePlayback.status === "stopped"}
          onchange={event => run(() => seekPrivateAudioPosition(Number(event.currentTarget.value)))} />
      </label>
      {#if !canSeek}<small>{$privatePlayback.mode === "tts" ? "Read aloud uses passage positions; timeline seeking is unavailable." : duration ? "Seeking unavailable for this source." : "Duration unknown; seeking unavailable."}</small>{/if}
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
  select { font: inherit; color: var(--text-primary); border: 1px solid var(--border); background: var(--bg-primary); border-radius: 6px; padding: 7px; }
  select:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .title { font-weight: 600; text-align: left; border: 0; padding: 0; background: transparent; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .actions { display: flex; flex-wrap: wrap; gap: 8px; } small { color: var(--text-muted); font-size: 11px; }
  .stop { border-color: var(--accent); font-weight: 600; }
  .timeline { flex: 1 0 100%; min-width: 0; }
  label { display: flex; align-items: center; gap: 12px; font-size: 12px; }
  input { flex: 1; min-width: 60px; accent-color: var(--accent); }
  input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  @media (pointer: coarse) { button { min-height: 44px; } input { min-height: 32px; } }
  button:disabled { opacity: .55; cursor: default; } button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  p { flex-basis: 100%; font-size: 12px; margin: 0; overflow-wrap: anywhere; } [role="alert"] { color: var(--danger, #c44); }
</style>
