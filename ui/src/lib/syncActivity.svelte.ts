/**
 * Sync activity tracking.
 *
 * The reading panel's Conflicts tab used to be permanent, which is noise for
 * the common case of a graph that has never conflicted — most users would never
 * see anything but "No unresolved sync conflicts." The tab only earns its space
 * while a sync is running, while conflicts are actually outstanding, or for a
 * short grace period after a sync so the outcome can be confirmed.
 *
 * That rule needs state that outlives any one panel instance (conflicts survive
 * restarts; syncs are triggered from Settings and from the auto-sync watcher),
 * so it lives in a module-level store rather than component state.
 */

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { listSyncConflicts } from "./sync";

/**
 * How long the Conflicts tab stays reachable after a sync finishes.
 *
 * Long enough to answer "did that sync conflict?" without hunting, short enough
 * that the tab doesn't quietly become permanent again.
 */
export const RECENT_SYNC_WINDOW_MS = 5 * 60 * 1000;

export const syncActivity = $state({
  /** Sync runs currently in flight. A counter, because targets sync independently. */
  running: 0,
  /** Unresolved conflicts as of the last refresh, across all targets. */
  conflictCount: 0,
  /** Epoch ms of the most recent sync completion, or null if none observed yet. */
  lastCompletedAt: null as number | null,
  /** True during {@link RECENT_SYNC_WINDOW_MS} after a sync completes. */
  recentSync: false,
});

let recentSyncTimer: ReturnType<typeof setTimeout> | null = null;

/** Mark a sync run as started. Pair with {@link endSyncRun} in a `finally`. */
export function beginSyncRun(): void {
  syncActivity.running += 1;
}

/** Mark a sync run as finished, whether it succeeded or failed. */
export function endSyncRun(): void {
  syncActivity.running = Math.max(0, syncActivity.running - 1);
  markSyncCompleted();
}

/**
 * Record that a sync finished and re-check conflicts.
 *
 * Also called for syncs this window did not start (auto-sync in the backend),
 * which is why it is separate from {@link endSyncRun}.
 */
export function markSyncCompleted(now: number = Date.now()): void {
  syncActivity.lastCompletedAt = now;
  syncActivity.recentSync = true;
  if (recentSyncTimer) clearTimeout(recentSyncTimer);
  recentSyncTimer = setTimeout(() => {
    syncActivity.recentSync = false;
    recentSyncTimer = null;
  }, RECENT_SYNC_WINDOW_MS);
  void refreshConflictCount();
}

/**
 * Re-read the unresolved conflict count from the backend.
 *
 * A failed probe deliberately leaves the previous count alone: making
 * outstanding conflicts silently invisible is far worse than a stale number.
 */
export async function refreshConflictCount(): Promise<number> {
  try {
    const conflicts = await listSyncConflicts();
    syncActivity.conflictCount = conflicts.length;
  } catch (error) {
    console.error("Failed to read sync conflicts:", error);
  }
  return syncActivity.conflictCount;
}

/** Update the count from a list the caller already fetched. */
export function setConflictCount(count: number): void {
  syncActivity.conflictCount = count;
}

/** Whether the Conflicts tab should be offered right now. */
export function shouldShowConflicts(): boolean {
  return syncActivity.running > 0 || syncActivity.conflictCount > 0 || syncActivity.recentSync;
}

/** Test seam: drop any pending grace-period timer and reset the store. */
export function resetSyncActivity(): void {
  if (recentSyncTimer) clearTimeout(recentSyncTimer);
  recentSyncTimer = null;
  syncActivity.running = 0;
  syncActivity.conflictCount = 0;
  syncActivity.lastCompletedAt = null;
  syncActivity.recentSync = false;
}

/**
 * Start mirroring backend sync events into the store.
 *
 * The initial conflict probe matters on its own: conflicts recorded in a
 * previous session are still unresolved on disk, and the tab has to come back
 * for them without waiting for another sync.
 */
export async function initSyncActivity(): Promise<UnlistenFn> {
  void refreshConflictCount();

  const unlistenCompleted = await listen("sync-completed", () => {
    markSyncCompleted();
  });
  const unlistenError = await listen("sync-error", () => {
    markSyncCompleted();
  });

  return () => {
    unlistenCompleted();
    unlistenError();
  };
}
