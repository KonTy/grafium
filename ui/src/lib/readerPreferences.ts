import { writable } from "svelte/store";
import type { ReaderFlow } from "./readerNavigation";
import { showToast } from "./toast.svelte";

const FLOW_KEY = "grafium.reader.flow";
export const readerFlow = writable<ReaderFlow>("paginated");

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
