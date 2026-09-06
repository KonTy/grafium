import { describe, it, expect } from "vitest";
import { summarizeSyncResult, type SyncResult } from "./sync";

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
