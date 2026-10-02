import { writable } from "svelte/store";
import type { ReaderFlow } from "./readerNavigation";
import { showToast } from "./toast.svelte";

const FLOW_KEY = "grafium.reader.flow";
const SIZE_KEY = "grafium.reader.textSize";
export const readerFlow = writable<ReaderFlow>("paginated");
export const READER_TEXT_SIZES = [75, 90, 100, 115, 125, 130, 150, 175, 200];
export const readerTextSize = writable(100);

export function loadReaderTextSizePreference(): void {
  try {
    const value = window.localStorage.getItem(SIZE_KEY);
    if (value === null) readerTextSize.set(100);
    else if (READER_TEXT_SIZES.includes(Number(value))) readerTextSize.set(Number(value));
    else console.warn("Invalid saved book text size; keeping the current size.");
  } catch (error) { console.warn("Could not load book text size:", error); }
}

export function setReaderTextSize(value: number): void {
  if (!READER_TEXT_SIZES.includes(value)) throw new Error("Unsupported book text size.");
  readerTextSize.set(value);
  try { window.localStorage.setItem(SIZE_KEY, String(value)); }
  catch (error) { showToast(`Book text size will not survive a restart: ${String(error)}`, "error"); }
}

export function loadReaderFlowPreference(): void {
  try {
    const value = window.localStorage.getItem(FLOW_KEY);
    if (value === null || value === "paginated" || value === "scrolled") readerFlow.set(value ?? "paginated");
    else console.warn("Invalid saved book reading layout; keeping the current layout.");
  } catch (error) {
    console.warn("Could not load book reading layout:", error);
  }
}

export function setReaderFlow(value: ReaderFlow): void {
  readerFlow.set(value);
  try { window.localStorage.setItem(FLOW_KEY, value); }
  catch (error) { showToast(`Reading layout will not survive a restart: ${String(error)}`, "error"); }
}
