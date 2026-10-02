<script lang="ts">
  import { untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { open as openExternal } from "@tauri-apps/plugin-shell";
  import type { StudyItem, StudyProgress } from "../lib/studies";
  import { finiteStudyProgress, normalizeStudySource, studyPercent, studyTime, webStudyUrl, youtubeVideoId } from "../lib/studySources";
  import { PLAYBACK_RATES, mediaPlaybackRate, setMediaPlaybackRate, applyReaderPlaybackRate } from "../lib/readerPlaybackPreferences";

  let { graphPath = "", item, onProgress, onPlayback, onActivity, autoplay = true, resolveMedia, helpContext = "studies" }: {
    graphPath?: string; item: Pick<StudyItem, "id" | "kind" | "source" | "title" | "progress">;
    onProgress: (progress: StudyProgress, active: boolean) => void;
    onPlayback: (playing: boolean) => void; onActivity?: () => void;
    autoplay?: boolean; resolveMedia?: () => Promise<string>; helpContext?: string;
  } = $props();
  const frameId = $props.id();
  let media = $state<HTMLMediaElement>();
  let frame = $state<HTMLIFrameElement>();
  let mediaSource = $state("");
  let embedSource = $state("");
  let transportOrigin = $state("");
  let externalSource = $state("");
  let error = $state("");
  let rateError = $state("");
  let appliedPlaybackRate = $state(1);
  let loading = $state(false);
  let ready = $state(false);
  let resumeFailed = $state(false);
  let resumeNotice = $state("");
  let checkpoint = $state("");
  let checkpointPercent = $state(0);
  let checkpointSaved = $state(false);
  let progress = $state<StudyProgress>({ position: 0, total: 0, anchor: "", label: "" });
  let resumePosition = 0;
  let resumed = false;
  let publishedPosition: number | null = null;
  let autoPlayRequested = false;
  let generation = 0;
  let playing = $state(false);
  let stopped = $state(false);
  let seekable = $state(false);
  const sourceIdentity = $derived(`${graphPath}\0${item.id}\0${item.kind}\0${item.source}`);

  $effect(() => {
    sourceIdentity;
    const { graph, id, kind, source } = untrack(() => ({ graph: graphPath, id: item.id, kind: item.kind, source: item.source }));
    const current = ++generation;
    untrack(() => {
      onPlayback(false);
      playing = false;
      stopped = false; seekable = false;
      mediaSource = ""; embedSource = ""; transportOrigin = ""; externalSource = ""; error = ""; ready = false;
      rateError = ""; appliedPlaybackRate = $mediaPlaybackRate;
      resumed = false; autoPlayRequested = false; resumeFailed = false; resumeNotice = ""; publishedPosition = null; loading = false; checkpointSaved = false;
      progress = finiteStudyProgress(item.progress.position, item.progress.total, item.progress.label, item.progress.anchor);
      resumePosition = progress.position; checkpoint = progress.label; checkpointPercent = studyPercent(progress);
    });
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let poll: ReturnType<typeof setInterval> | undefined;
    try {
      if (kind === "library") throw new Error("Open this reference in Library to use its shared reader and saved position.");
      const normalized = resolveMedia ? source : normalizeStudySource(kind, source);
      if (kind === "website") externalSource = normalized;
      else if (kind === "youtube") {
        externalSource = normalized;
        loading = true;
        void invoke<string>("study_youtube_embed", { videoId: youtubeVideoId(normalized), start: resumePosition }).then(source => {
          if (cancelled || generation !== current || graphPath !== graph || item.id !== id) return;
          const url = new URL(source);
          if (url.protocol !== "http:" || url.hostname !== "127.0.0.1" || !url.port || url.username || url.password) {
            throw new Error("The video player returned an invalid local address.");
          }
          transportOrigin = url.origin;
          embedSource = url.href;
        }).catch(cause => {
          if (!cancelled && generation === current) {
            if (timer) clearTimeout(timer);
            if (poll) clearInterval(poll);
            loading = false;
            error = `Could not open the YouTube player: ${String(cause)} Open the source in your browser instead.`;
          }
        });
        timer = setTimeout(() => {
          if (!cancelled && !ready) { loading = false; error = "YouTube did not become available. Embedding may be blocked by this video, your network, or the app's webview. Open it in your browser instead."; onPlayback(false); }
        }, 15000);
        // The isolated local wrapper relays the iframe API; no remote script runs in the app.
        poll = setInterval(() => {
          if (!cancelled && !ready) postToYoutube({ event: "listening", id: frameId, channel: "widget" });
        }, 500);
      } else if (kind === "audio" || kind === "video") {
        if (resolveMedia) {
          loading = true;
          void untrack(() => resolveMedia!()).then(url => {
            if (!cancelled && generation === current) mediaSource = url;
          }).catch(cause => {
            if (!cancelled && generation === current) error = `Could not open the library media: ${String(cause)}`;
          }).finally(() => { if (!cancelled && generation === current) loading = false; });
        } else if (/^https?:/i.test(normalized)) mediaSource = normalized;
        else {
          loading = true;
          // Same byte-loading path used by hydrated graph media. Unlike the shared
          // Markdown cache, this request cannot reuse an asset from another graph.
          void invoke<string>("read_asset_data_url", { path: normalized, graphPath: graph }).then(url => {
            if (cancelled || generation !== current || graphPath !== graph || item.id !== id) return;
            if (!/^data:(?:audio|video|application\/octet-stream)[^,]*;base64,/i.test(url)) throw new Error("The graph asset is not playable media.");
            mediaSource = url;
          }).catch(cause => {
            if (!cancelled && generation === current) error = `Could not load the graph media: ${String(cause)}`;
          }).finally(() => { if (!cancelled && generation === current) loading = false; });
        }
        if (/^https?:/i.test(normalized)) externalSource = normalized;
      } else error = "This source opens in the page, book, or flashcard reader.";
    } catch (cause) { error = String(cause); }
    return () => {
      untrack(() => {
        if (media) { media.pause(); media.removeAttribute("src"); media.load(); }
        if (frame?.isConnected) youtubeCommand("pauseVideo");
      });
      cancelled = true; ++generation;
      if (timer) clearTimeout(timer);
      if (poll) clearInterval(poll);
      untrack(() => { onPlayback(false); });
    };
  });

  $effect(() => {
    const target = media;
    const rate = $mediaPlaybackRate;
    if (target) untrack(() => applySpeed(target, rate));
  });

  function applySpeed(target: HTMLMediaElement, rate: number): boolean {
    try {
      applyReaderPlaybackRate(target, rate);
      rateError = "";
      return true;
    } catch (cause) {
      rateError = `Could not change playback speed: ${String(cause)}`;
      return false;
    } finally {
      appliedPlaybackRate = target.playbackRate;
    }
  }
  function changeSpeed(event: Event) {
    const select = event.currentTarget as HTMLSelectElement;
    const rate = Number(select.value);
    if (!media || item.kind === "youtube") return;
    if (applySpeed(media, rate)) {
      try { setMediaPlaybackRate(rate); }
      catch (cause) { rateError = `Could not save playback speed: ${String(cause)}`; }
    }
    // A rejected rate may leave state unchanged, so also restore the native select.
    select.value = String(appliedPlaybackRate);
  }
  function mediaRateChanged() {
    if (media) appliedPlaybackRate = media.playbackRate;
  }

  function postToYoutube(data: Record<string, unknown>) {
    if (transportOrigin) frame?.contentWindow?.postMessage(JSON.stringify(data), transportOrigin);
  }
  function youtubeCommand(func: string, args: unknown[] = []) {
    postToYoutube({ event: "command", func, args, id: frameId, channel: "widget" });
  }
  function listenToYoutube() {
    postToYoutube({ event: "listening", id: frameId, channel: "widget" });
    youtubeCommand("addEventListener", ["onStateChange"]);
    youtubeCommand("addEventListener", ["onError"]);
  }
  function publish(position: number, total: number) {
    const next = finiteStudyProgress(position, total);
    next.label = next.total ? `${studyTime(next.position)} / ${studyTime(next.total)}` : studyTime(next.position);
    progress = next;
    if (publishedPosition === next.position) return;
    publishedPosition = next.position;
    onProgress(next, playing);
  }
  function handleYoutubeMessage(event: MessageEvent) {
    if (item.kind !== "youtube" || !transportOrigin || !frame?.contentWindow
      || event.source !== frame.contentWindow || event.origin !== transportOrigin) return;
    let data: { event?: string; info?: unknown };
    try { data = typeof event.data === "string" ? JSON.parse(event.data) : event.data; } catch { return; }
    if (!data || typeof data !== "object") return;
    if (data.event === "onReady") {
      ready = true; loading = false; error = "";
      youtubeCommand("addEventListener", ["onStateChange"]);
      youtubeCommand("addEventListener", ["onError"]);
      if (autoplay && !autoPlayRequested) { autoPlayRequested = true; youtubeCommand("playVideo"); }
    } else if (data.event === "onError") {
      playing = false; onPlayback(false); loading = false;
      error = "This YouTube video is unavailable or cannot be embedded. Open it in your browser instead.";
    } else if (data.event === "onStateChange") {
      youtubeState(data.info);
    } else if (data.event === "infoDelivery" && data.info && typeof data.info === "object") {
      const info = data.info as Record<string, unknown>;
      if (typeof info.playerState === "number") youtubeState(info.playerState);
      if (typeof info.duration === "number" && Number.isFinite(info.duration) && info.duration > 0) {
        progress = finiteStudyProgress(progress.position, info.duration, progress.label, progress.anchor);
        if (!resumed && resumePosition > info.duration) {
          resumePosition = typeof info.currentTime === "number" && Number.isFinite(info.currentTime)
            ? Math.max(0, Math.min(info.currentTime, info.duration)) : 0;
          resumed = true;
          resumeNotice = "Your saved position is beyond this video's current duration. Continuing from the player's available position.";
        }
      }
      if (typeof info.currentTime === "number" && Number.isFinite(info.currentTime) && info.currentTime >= 0) {
        const duration = typeof info.duration === "number" && Number.isFinite(info.duration) && info.duration > 0 ? info.duration : progress.total;
        // Initial API reports can say zero before the start parameter is applied.
        if (resumed || info.currentTime >= resumePosition || resumePosition === 0) {
          resumed = true; publish(info.currentTime, duration);
        }
      }
    }
  }
  function youtubeState(state: unknown) {
    if (typeof state !== "number" || ![-1, 0, 1, 2, 3, 5].includes(state)) return;
    ready = true; loading = false;
    playing = state === 1;
    onPlayback(state === 1);
    if (state === 1) { error = ""; onActivity?.(); }
  }
  function metadataReady() {
    if (!media || media.readyState < 1) return;
    const speedApplied = applySpeed(media, $mediaPlaybackRate);
    ready = true;
    updateSeekable();
    if (!resumed) {
      try {
        media.currentTime = Number.isFinite(media.duration) && media.duration > 0 ? Math.min(resumePosition, media.duration) : resumePosition;
        resumed = true;
        if (resumeFailed) { resumeFailed = false; error = ""; }
      } catch (cause) {
        resumeFailed = true;
        onPlayback(false);
        error = `Could not restore your saved position. Retry resume or start from the beginning. ${String(cause)}`;
        return;
      }
    }
    publish(media.currentTime, media.duration);
    if (autoplay && !autoPlayRequested && speedApplied) {
      autoPlayRequested = true;
      const current = generation;
      void media.play().catch(cause => {
        if (current !== generation) return;
        if (cause instanceof DOMException && cause.name === "NotAllowedError")
          resumeNotice = "Your place is restored. Press Play to begin; automatic playback is blocked by this WebView.";
        else error = `Could not start playback. Try the player's Play control. ${String(cause)}`;
      });
    }
  }
  function mediaProgress() {
    if (media && resumed) { updateSeekable(); publish(media.currentTime, media.duration); }
  }
  function updateSeekable() {
    seekable = !!media && Number.isFinite(media.duration) && media.duration > 0 && media.seekable.length > 0;
  }
  async function play() {
    const current = generation;
    autoPlayRequested = true; stopped = false;
    if (item.kind === "youtube") { youtubeCommand("playVideo"); return; }
    if (!media) return;
    try { await media.play(); }
    catch (cause) { if (current === generation) error = `Could not start playback: ${String(cause)}`; }
  }
  function pause(stop = false) {
    autoPlayRequested = true;
    stopped = stop;
    if (item.kind === "youtube") youtubeCommand("pauseVideo");
    else media?.pause();
    mediaStopped();
  }
  export function pausePlayback() { pause(); }
  function seek(event: Event) {
    const target = Number((event.currentTarget as HTMLInputElement).value);
    if (!Number.isFinite(target) || progress.total <= 0 || !media || !seekable) {
      error = "Seeking is unavailable for this source.";
      return;
    }
    try {
      let available = false;
      for (let index = 0; index < media.seekable.length; index++)
        if (target >= media.seekable.start(index) && target <= media.seekable.end(index)) available = true;
      if (!available) throw new Error("This position is outside the media's available seek range.");
      media.currentTime = target;
      mediaProgress();
    } catch (cause) { error = `Could not seek: ${String(cause)}`; }
  }
  function mediaPlaying() { playing = true; stopped = false; if (!resumeFailed) error = ""; onPlayback(true); onActivity?.(); }
  function mediaStopped() { mediaProgress(); playing = false; onPlayback(false); }
  function mediaError() {
    playing = false; seekable = false;
    onPlayback(false);
    error = "The media could not be played. Check that the asset exists or the URL points directly to a supported audio/video file.";
  }
  async function startFromBeginning() {
    if (!media) return;
    const current = generation;
    try {
      if (media.currentTime !== 0) media.currentTime = 0;
      resumePosition = 0; resumed = true; resumeFailed = false; error = "";
      publish(media.currentTime, media.duration);
      await media.play();
    } catch (cause) {
      if (current === generation) {
        onPlayback(false);
        error = `Could not start playback. Try the player's Play control. ${String(cause)}`;
      }
    }
  }
  function saveCheckpoint(event: SubmitEvent) {
    event.preventDefault();
    checkpointSaved = false;
    const label = checkpoint.trim();
    if (new TextEncoder().encode(label).length > 1024) {
      error = "The checkpoint is too long. Shorten it to at most 1,024 UTF-8 bytes.";
      return;
    }
    error = "";
    const percent = Number.isFinite(checkpointPercent) ? Math.max(0, Math.min(100, checkpointPercent)) : 0;
    onProgress(finiteStudyProgress(percent, 100, label, ""), false);
    checkpointSaved = true;
  }
  async function openInBrowser() {
    const current = generation;
    try {
      const url = webStudyUrl(externalSource);
      if (item.kind === "youtube" && progress.position > 0) url.searchParams.set("t", `${Math.floor(progress.position)}s`);
      await openExternal(url.href);
    }
    catch (cause) { if (current === generation) error = `Could not open the browser: ${String(cause)}`; }
  }
</script>

<svelte:window onmessage={handleYoutubeMessage} />
<div class="study-media" data-help-context={helpContext}>
  {#key sourceIdentity}
  {#if item.kind === "website"}
    <section class="website">
      <div class="website-icon" aria-hidden="true">↗</div><h2>Continue in your browser</h2>
      <p>This website opens outside Grafium. Browser activity and reading time are not tracked.</p>
      <p class="source">{externalSource || item.source}</p>
      <button class="primary" disabled={!externalSource} onclick={openInBrowser}>Open website ↗</button>
      <form onsubmit={saveCheckpoint}>
        <h3>Your checkpoint</h3>
        <label>Where did you leave off?<input bind:value={checkpoint} oninput={() => checkpointSaved = false} placeholder="Chapter 3, exercise 2, or a short note…" maxlength="1024" /></label>
        <label>Progress (%)<input type="number" min="0" max="100" step="1" bind:value={checkpointPercent} oninput={() => checkpointSaved = false} /></label>
        <button type="submit">Save checkpoint</button>
        {#if checkpointSaved}<small role="status">Checkpoint updated. Study-session save status appears above.</small>{/if}
      </form>
    </section>
  {:else if item.kind === "youtube"}
    {#if embedSource}
      <iframe bind:this={frame} title={`YouTube: ${item.title}`} src={embedSource}
        sandbox="allow-scripts allow-same-origin" allow="autoplay; encrypted-media; fullscreen; picture-in-picture"
        referrerpolicy="strict-origin-when-cross-origin" allowfullscreen onload={listenToYoutube}></iframe>
      <div class="playback-note"><span>Resuming your video. If it does not start, press Play. Your saved place: {studyTime(progress.position)}.</span>
        <button disabled={!ready} onclick={() => { youtubeCommand("seekTo", [resumePosition, true]); youtubeCommand("playVideo"); onActivity?.(); }}>Play from saved place</button>
      </div>
      <p class="muted">If playback is blocked, press Play in the video. Only playback reported by the embedded player is tracked.</p>
    {/if}
  {:else if (item.kind === "audio" || item.kind === "video") && mediaSource}
    <div class:audio-player={item.kind === "audio"}>
      {#if item.kind === "audio"}
        <div class="audio-heading"><span aria-hidden="true">♫</span><h2>{item.title}</h2></div>
        <audio bind:this={media} src={mediaSource} controls preload="metadata" onloadedmetadata={metadataReady} ondurationchange={metadataReady} onratechange={mediaRateChanged} onprogress={updateSeekable} ontimeupdate={mediaProgress} onseeked={mediaProgress} onplaying={mediaPlaying} onpause={mediaStopped} onended={mediaStopped} onwaiting={() => onPlayback(false)} onstalled={() => onPlayback(false)} onerror={mediaError}></audio>
      {:else}
        <!-- This is a user-selected source, not an authored video with supplied captions. -->
        <!-- svelte-ignore a11y_media_has_caption -->
        <video bind:this={media} src={mediaSource} controls playsinline preload="metadata" onloadedmetadata={metadataReady} ondurationchange={metadataReady} onratechange={mediaRateChanged} onprogress={updateSeekable} ontimeupdate={mediaProgress} onseeked={mediaProgress} onplaying={mediaPlaying} onpause={mediaStopped} onended={mediaStopped} onwaiting={() => onPlayback(false)} onstalled={() => onPlayback(false)} onerror={mediaError}></video>
      {/if}
    </div>
    <p class="muted">Press Play to begin. Your saved place is restored once the media is ready.</p>
  {/if}
  {#if item.kind === "audio" || item.kind === "video" || item.kind === "youtube"}
    <section class="transport" aria-label="Media playback controls">
      <div class="transport-actions">
        <button disabled={!ready || playing || resumeFailed} onclick={play}>{progress.position > 0 ? "Resume" : "Play"}</button>
        <button disabled={!playing} onclick={() => pause()}>Pause</button>
        <button class="stop" onclick={() => pause(true)}>Stop</button>
        <label class="speed-control">Speed
          <select value={item.kind === "youtube" ? "provider" : appliedPlaybackRate}
            disabled={item.kind === "youtube" || !media}
            aria-describedby={item.kind === "youtube" ? `${frameId}-speed-provider` : rateError ? `${frameId}-speed-error` : undefined}
            onchange={changeSpeed}>
            {#if item.kind === "youtube"}
              <option value="provider">YouTube controls</option>
            {:else}
              {#each PLAYBACK_RATES as rate}<option value={rate}>{rate}×</option>{/each}
              {#if !PLAYBACK_RATES.some(rate => rate === appliedPlaybackRate)}
                <option value={appliedPlaybackRate}>{appliedPlaybackRate}× (actual)</option>
              {/if}
            {/if}
          </select>
        </label>
        <span role="status">{stopped ? "Stopped" : playing ? "Playing" : ready ? "Paused" : "Loading"} · {studyTime(progress.position)}{progress.total > 0 ? ` / ${studyTime(progress.total)}` : ""}</span>
      </div>
      {#if item.kind === "youtube"}<p class="muted" id={`${frameId}-speed-provider`}>Use YouTube's own speed controls; available speeds are set by YouTube.</p>{/if}
      {#if rateError}<p class="error" role="alert" id={`${frameId}-speed-error`}>{rateError}</p>{/if}
      <label>Seek {item.kind === "audio" ? "audio" : "video"}
        <input type="range" min="0" max={progress.total || 1} step="1" value={progress.position}
          aria-valuetext={`${studyTime(progress.position)}${progress.total ? ` of ${studyTime(progress.total)}` : ""}`}
          disabled={!ready || progress.total <= 0 || !seekable} onchange={seek} />
      </label>
      {#if item.kind === "youtube"}<small>Use the video's own timeline; embedded seeking availability is not reported.</small>
      {:else if progress.total <= 0}<small>Duration unknown; seeking unavailable.</small>
      {:else if !seekable}<small>Seeking unavailable for this source.</small>{/if}
    </section>
  {/if}
  {#if loading}<p class="muted" role="status">Loading player…</p>{/if}
  {#if resumeNotice}<p class="muted" role="status">{resumeNotice}</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if resumeFailed}<div class="resume-actions"><button onclick={metadataReady}>Retry resume</button><button onclick={startFromBeginning}>Start from beginning</button></div>{/if}
  {#if externalSource && item.kind !== "website"}<button class="external" onclick={openInBrowser}>Open source in browser ↗</button><small class="browser-note">Playback outside Grafium is not tracked.</small>{/if}
  {/key}
</div>

<style>
  .study-media { box-sizing: border-box; width: 100%; max-width: 1000px; margin: 0 auto; padding: 24px; color: var(--text-primary); }
  iframe, video { display: block; width: 100%; aspect-ratio: 16 / 9; border: 1px solid var(--border); border-radius: 12px; background: #000; max-height: 65vh; }
  audio { width: 100%; } .audio-player { border: 1px solid var(--border); border-radius: 12px; padding: 32px; background: var(--bg-secondary); }
  .audio-heading { display: flex; align-items: center; gap: 16px; margin-bottom: 24px; } .audio-heading span { font-size: 40px; color: var(--accent); } h2 { font-size: 20px; margin: 0; }
  .playback-note { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-top: 16px; font-size: 12px; color: var(--text-secondary); }
  .muted, small, .browser-note { color: var(--text-muted); font-size: 12px; } .error { color: var(--accent-red, #e78284); overflow-wrap: anywhere; }
  button, input, select { font: inherit; color: var(--text-primary); border: 1px solid var(--border); border-radius: 7px; background: var(--bg-primary); padding: 9px 12px; }
  button { cursor: pointer; } button:hover { background: var(--bg-hover, var(--bg-secondary)); } button:disabled { opacity: .55; cursor: default; }
  button:focus-visible, input:focus-visible, select:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; } .primary { color: var(--accent); border-color: var(--accent); }
  .external { font-size: 12px; margin: 8px 0; } .browser-note { display: block; }
  .resume-actions { display: flex; flex-wrap: wrap; gap: 8px; margin-bottom: 12px; }
  .transport { margin: 16px 0; padding: 12px; border: 1px solid var(--border); border-radius: 8px; background: var(--bg-secondary); }
  .transport-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; margin-bottom: 12px; }
  .transport-actions span { font-size: 12px; }
  .speed-control { width: auto; flex-direction: row; align-items: center; }
  .stop { border-color: var(--accent); font-weight: 600; }
  .transport input { padding: 0; accent-color: var(--accent); width: 100%; }
  @media (pointer: coarse) { .transport button, .transport select { min-height: 44px; } .transport input { min-height: 32px; } }
  .website { border: 1px solid var(--border); border-radius: 12px; padding: 28px; background: var(--bg-secondary); }
  .website-icon { font-size: 30px; color: var(--accent); margin-bottom: 12px; } .website p { color: var(--text-secondary); line-height: 1.6; font-size: 13px; } .source { overflow-wrap: anywhere; }
  form { border-top: 1px solid var(--border); margin-top: 28px; padding-top: 8px; display: flex; flex-direction: column; align-items: flex-start; gap: 12px; } h3 { margin: 12px 0 0; font-size: 15px; }
  label { display: flex; flex-direction: column; gap: 6px; font-size: 12px; width: 100%; color: var(--text-secondary); } input { min-width: 0; } input[type="number"] { width: 90px; }
  @media (max-width: 600px) { .playback-note { align-items: flex-start; flex-direction: column; } .audio-player, .website { padding: 20px; } }
</style>
