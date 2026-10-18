import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => []) }));

import {
  RECENT_SYNC_WINDOW_MS,
  beginSyncRun,
  endSyncRun,
  markSyncCompleted,
  refreshConflictCount,
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

  describe("probing the backend", () => {
    const conflict = { target_id: "t", target_name: "USB", rel_path: "assets/a.png", backup_path: "b", recorded_at: 1 };
    const held = { rel_path: "pages/todo.md", conflicts: 2 };

    function backend(answers: Record<string, unknown>) {
      vi.mocked(invoke).mockImplementation(async (command: string) => {
        const answer = answers[command];
        if (answer instanceof Error) throw answer;
        return answer ?? [];
      });
    }

    afterEach(() => {
      vi.mocked(invoke).mockImplementation(async () => []);
    });

    it("counts notes holding merge markers next to whole-file conflicts", async () => {
      backend({ sync_list_conflicts: [conflict], sync_list_pending_merges: [held, { ...held, rel_path: "pages/b.md" }] });
      expect(await refreshConflictCount()).toBe(3);
      expect(shouldShowConflicts()).toBe(true);
    });

    it("keeps a held note visible when there are no whole-file conflicts", async () => {
      backend({ sync_list_pending_merges: [held] });
      await refreshConflictCount();
      expect(syncActivity.conflictCount).toBe(1);
    });

    it("keeps the previous count when either probe fails", async () => {
      const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
      setConflictCount(3);
      backend({ sync_list_conflicts: [], sync_list_pending_merges: new Error("unavailable") });
      expect(await refreshConflictCount()).toBe(3);
      quiet.mockRestore();
    });
  });
});
