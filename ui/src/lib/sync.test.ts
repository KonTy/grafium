import { describe, it, expect, vi, afterEach } from "vitest";
const ipc = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), toast: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: ipc.listen }));
vi.mock("./toast.svelte", () => ({ showToast: ipc.toast }));
import { summarizeSyncResult, getSyncConflictState, resolveSyncConflict, initSyncMonitor, type SyncResult } from "./sync";

afterEach(() => {
  ipc.invoke.mockReset();
  ipc.listen.mockReset();
  ipc.toast.mockReset();
});

function result(overrides: Partial<SyncResult> = {}): SyncResult {
  return {
    pushed: [],
    pulled: [],
    conflicts: [],
    deleted_remote: [],
    deleted_local: [],
    errors: [],
    ...overrides,
  };
}

describe("summarizeSyncResult", () => {
  it("reports a clean sync when nothing changed", () => {
    expect(summarizeSyncResult(result())).toBe("Everything in sync ✓");
  });

  describe("revision-checked sync IPC", () => {
    it("passes missing sides as null rather than confusing them with an empty file", async () => {
      const snapshot = { graph_path: "/graph", rel_path: "assets/image.png", local_hash: null, remote_hash: "empty-sha", local_size: null, remote_size: 0 };
      ipc.invoke.mockResolvedValueOnce(snapshot);
      const view = await getSyncConflictState("target", snapshot.rel_path);
      expect(ipc.invoke).toHaveBeenLastCalledWith("sync_get_conflict_state", { targetId: "target", relPath: snapshot.rel_path });
      await resolveSyncConflict("target", view, "local");
      expect(ipc.invoke).toHaveBeenLastCalledWith("sync_resolve_conflict", {
        graphPath: "/graph", targetId: "target", relPath: snapshot.rel_path, expectedLocalHash: null, expectedRemoteHash: "empty-sha", chosen: "local",
      });
    });

    it("does not announce success when reconciliation or deletion sync reports errors", async () => {
      const handlers = new Map<string, (event: unknown) => void>();
      const stop = vi.fn();
      ipc.listen.mockImplementation(async (name, handler) => { handlers.set(name, handler); return stop; });
      const unlisten = await initSyncMonitor();
      handlers.get("sync-completed")!({ payload: {
        target_name: "USB", pushed: 0, pulled: 1, conflicts: 0, errors: 1, deleted_local: 1, deleted_remote: 2,
      } });
      expect(ipc.toast).toHaveBeenCalledWith(expect.stringContaining("1 errors"), "error");
      expect(ipc.toast.mock.calls[0][0]).toContain("1 deleted local");
      expect(ipc.toast.mock.calls[0][0]).toContain("2 deleted remote");
      unlisten();
      expect(stop).toHaveBeenCalledTimes(3);
    });
  });

  it("counts pushed and pulled files", () => {
    const summary = summarizeSyncResult(result({ pushed: ["a.md"], pulled: ["b.md", "c.md"] }));
    expect(summary).toBe("↑ 1 pushed, ↓ 2 pulled");
  });

  it("never hides conflicts", () => {
    const summary = summarizeSyncResult(result({ pushed: ["a.md"], conflicts: ["b.md"] }));
    expect(summary).toContain("1 conflicts");
  });

  it("never hides errors", () => {
    const summary = summarizeSyncResult(result({ pulled: ["a.md"], errors: ["boom"] }));
    expect(summary).toContain("1 errors");
  });

  it("distinguishes remote from local deletions", () => {
    const summary = summarizeSyncResult(
      result({ deleted_remote: ["a.md"], deleted_local: ["b.md", "c.md"] })
    );
    expect(summary).toContain("1 deleted remote");
    expect(summary).toContain("2 deleted local");
  });

  it("reports every category when all are present", () => {
    const summary = summarizeSyncResult(
      result({
        pushed: ["a"],
        pulled: ["b"],
        conflicts: ["c"],
        deleted_remote: ["d"],
        deleted_local: ["e"],
        errors: ["f"],
      })
    );
    for (const fragment of [
      "1 pushed",
      "1 pulled",
      "1 conflicts",
      "1 deleted remote",
      "1 deleted local",
      "1 errors",
    ]) {
      expect(summary).toContain(fragment);
    }
  });

  it("does not claim success when only errors occurred", () => {
    const summary = summarizeSyncResult(result({ errors: ["network down"] }));
    expect(summary).not.toContain("Everything in sync");
  });
});
