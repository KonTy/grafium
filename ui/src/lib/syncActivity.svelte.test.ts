import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => []) }));

import {
  RECENT_SYNC_WINDOW_MS,
  beginSyncRun,
  endSyncRun,
  markSyncCompleted,
  resetSyncActivity,
  setConflictCount,
  shouldShowConflicts,
  syncActivity,
  initSyncActivity,
} from "./syncActivity.svelte";

describe("sync activity store", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetSyncActivity();
  });

  afterEach(() => {
    resetSyncActivity();
    vi.useRealTimers();
    vi.mocked(listen).mockReset();
  });

  it("hides conflicts on a graph that has never synced or conflicted", () => {
    expect(shouldShowConflicts()).toBe(false);
  });

  it("shows conflicts while a sync is in flight", () => {
    beginSyncRun();
    expect(shouldShowConflicts()).toBe(true);
  });

  it("counts concurrent runs so one finishing does not hide an ongoing sync", () => {
    beginSyncRun();
    beginSyncRun();
    endSyncRun();
    expect(syncActivity.running).toBe(1);
    expect(shouldShowConflicts()).toBe(true);
  });

  it("never lets the run counter go negative", () => {
    endSyncRun();
    endSyncRun();
    expect(syncActivity.running).toBe(0);
  });

  it("keeps conflicts reachable for a grace period after a sync, then hides them", () => {
    markSyncCompleted();
    expect(shouldShowConflicts()).toBe(true);
    vi.advanceTimersByTime(RECENT_SYNC_WINDOW_MS - 1);
    expect(shouldShowConflicts()).toBe(true);
    vi.advanceTimersByTime(1);
    expect(shouldShowConflicts()).toBe(false);
  });

  it("keeps outstanding conflicts visible long after the grace period", () => {
    markSyncCompleted();
    setConflictCount(2);
    vi.advanceTimersByTime(RECENT_SYNC_WINDOW_MS * 10);
    expect(syncActivity.recentSync).toBe(false);
    expect(shouldShowConflicts()).toBe(true);
  });

  it("hides conflicts once the last one is resolved and the grace period lapses", () => {
    setConflictCount(1);
    markSyncCompleted();
    setConflictCount(0);
    vi.advanceTimersByTime(RECENT_SYNC_WINDOW_MS);
    expect(shouldShowConflicts()).toBe(false);
  });

  it("restarts the grace period on each completion rather than expiring mid-sync-series", () => {
    markSyncCompleted();
    vi.advanceTimersByTime(RECENT_SYNC_WINDOW_MS - 10);
    markSyncCompleted();
    vi.advanceTimersByTime(RECENT_SYNC_WINDOW_MS - 10);
    expect(shouldShowConflicts()).toBe(true);
  });
  it("does not turn annotation merge candidates into file-level conflicts", async () => {
    const handlers = new Map<string, (event: unknown) => void>();
    vi.mocked(listen).mockImplementation(async (name, handler) => {
      handlers.set(name, handler as (event: unknown) => void);
      return vi.fn();
    });
    const unlisten = await initSyncActivity();
    handlers.get("sync-completed")!({ payload: { annotation_conflicts: 2, conflicts: 0 } });
    await Promise.resolve();
    expect(syncActivity.conflictCount).toBe(0);
    vi.advanceTimersByTime(RECENT_SYNC_WINDOW_MS);
    expect(shouldShowConflicts()).toBe(false);
    unlisten();
  });
});
