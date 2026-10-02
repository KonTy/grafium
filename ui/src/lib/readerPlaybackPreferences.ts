import { writable } from "svelte/store";
import { showToast } from "./toast.svelte";

export const PLAYBACK_RATES = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 2.5, 3, 3.5, 4];
export const mediaPlaybackRate = writable(1);
export const speechPlaybackRate = writable(1);
const preferences = [
  { key: "grafium.library.mediaPlaybackRate", label: "Audio/video speed", store: mediaPlaybackRate },
  { key: "grafium.library.speechPlaybackRate", label: "Read-aloud speed", store: speechPlaybackRate },
];

export function validateReaderPlaybackRate(rate: number): void {
  if (!Number.isFinite(rate) || rate < 0.5 || rate > 4)
    throw new Error("Playback speed must be between 0.5× and 4×.");
}

export function loadReaderPlaybackPreferences(): void {
  for (const preference of preferences) {
    try {
      const saved = localStorage.getItem(preference.key);
      const rate = saved === null ? 1 : Number(saved);
      validateReaderPlaybackRate(rate);
      preference.store.set(rate);
    } catch (cause) {
      preference.store.set(1);
      console.warn(`Could not restore ${preference.label}`, cause);
      showToast(`${preference.label} could not be restored; using 1×. ${String(cause)}`, "error");
    }
  }
}

function saveRate(index: number, rate: number): void {
  validateReaderPlaybackRate(rate);
  const preference = preferences[index];
  preference.store.set(rate);
  try { localStorage.setItem(preference.key, String(rate)); }
  catch (cause) { showToast(`${preference.label} changed for this session, but will not survive a restart: ${String(cause)}`, "error"); }
}
export function setMediaPlaybackRate(rate: number): void { saveRate(0, rate); }
export function setSpeechPlaybackRate(rate: number): void { saveRate(1, rate); }

export function applyReaderPlaybackRate(media: HTMLMediaElement, rate: number): void {
  validateReaderPlaybackRate(rate);
  const previous = media.playbackRate;
  const previousDefault = media.defaultPlaybackRate;
  try {
    media.preservesPitch = true;
    if ("webkitPreservesPitch" in media) media.webkitPreservesPitch = true;
    media.defaultPlaybackRate = rate;
    media.playbackRate = rate;
    if (!Number.isFinite(media.playbackRate) || Math.abs(media.playbackRate - rate) > 0.001)
      throw new Error(`The player did not accept ${rate}× playback speed.`);
  } catch (cause) {
    // Keep a failed rate change from silently changing the next loaded chapter.
    try { media.defaultPlaybackRate = previousDefault; media.playbackRate = previous; }
    catch (restoreError) { throw new Error(`Playback speed failed and could not be restored: ${String(restoreError)}. ${String(cause)}`); }
    throw new Error(`Could not change playback speed: ${String(cause)}`);
  }
}
