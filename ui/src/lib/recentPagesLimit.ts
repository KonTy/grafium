import { writable } from "svelte/store";

export const RECENT_PAGES_DEFAULT = 10;
export const RECENT_PAGES_MAX = 50;
const STORAGE_KEY = "grafium.sidebar.recentPagesLimit";

/** Whole number of recent pages to show; 0 hides the sidebar section. */
export function normalizeRecentPagesLimit(value: unknown): number {
  if (value === null || value === undefined || (typeof value === "string" && !value.trim())) {
    return RECENT_PAGES_DEFAULT;
  }
  const number = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(number)) return RECENT_PAGES_DEFAULT;
  return Math.min(RECENT_PAGES_MAX, Math.max(0, Math.round(number)));
}

function stored(): number {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw === null ? RECENT_PAGES_DEFAULT : normalizeRecentPagesLimit(raw);
  } catch (error) {
    console.warn("[sidebar] Could not read the recent pages limit:", error);
    return RECENT_PAGES_DEFAULT;
  }
}

export const recentPagesLimit = writable(stored());

export function setRecentPagesLimit(value: unknown): number {
  const limit = normalizeRecentPagesLimit(value);
  recentPagesLimit.set(limit);
  try {
    localStorage.setItem(STORAGE_KEY, String(limit));
  } catch (error) {
    console.warn("[sidebar] Could not remember the recent pages limit:", error);
  }
  return limit;
}
