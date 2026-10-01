import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { fetchStudyLinkTitle, listStudies, recordStudyActivity, removeStudy, saveStudy, type StudyItem } from "./studies";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const mockedInvoke = vi.mocked(invoke);
const item: StudyItem = {
  id: "study", title: "Reading", topic: "", kind: "page", source: "page",
  progress: { position: 0, total: 0, anchor: "", label: "" }, createdAt: "", updatedAt: "",
};

describe("Studies command contract", () => {
  beforeEach(() => { vi.resetAllMocks(); });
  it("looks up link titles without a graph argument or mutation", async () => {
    mockedInvoke.mockResolvedValueOnce("A decoded title");
    expect(await fetchStudyLinkTitle("https://example.com/watch?v=1")).toBe("A decoded title");
    expect(mockedInvoke).toHaveBeenCalledTimes(1);
    expect(mockedInvoke).toHaveBeenCalledWith("study_link_title", { url: "https://example.com/watch?v=1" });
  });
  it("surfaces title lookup failures without retrying or inventing a fallback", async () => {
    mockedInvoke.mockRejectedValueOnce("Study title unavailable: private address");
    await expect(fetchStudyLinkTitle("http://127.0.0.1/private"))
      .rejects.toBe("Study title unavailable: private address");
    expect(mockedInvoke).toHaveBeenCalledTimes(1);
  });
  it("scopes every read and metadata operation to the explicit graph", async () => {
    mockedInvoke.mockResolvedValueOnce({ items: [], days: [] });
    expect(await listStudies("/a")).toEqual({ items: [], days: [] });
    expect(mockedInvoke).toHaveBeenLastCalledWith("list_studies", { graphPath: "/a" });
    mockedInvoke.mockResolvedValueOnce(item);
    expect(await saveStudy("/a", item)).toEqual(item);
    expect(mockedInvoke).toHaveBeenLastCalledWith("save_study", { graphPath: "/a", item });
    await removeStudy("/a", "study");
    expect(mockedInvoke).toHaveBeenLastCalledWith("remove_study", { graphPath: "/a", id: "study" });
  });
  it("returns durable topic history even when no study rows remain", async () => {
    mockedInvoke.mockResolvedValueOnce({ items: [], days: [], topics: ["Astronomy", "Physics"] });
    expect(await listStudies("/a")).toEqual({ items: [], days: [], topics: ["Astronomy", "Physics"] });
  });
  it("retries with exactly the same receipt and captured progress", async () => {
    mockedInvoke.mockRejectedValueOnce("lost response").mockResolvedValueOnce(undefined);
    const progress = { ...item.progress, position: 2 };
    const pending = recordStudyActivity("/a", "study", 30, "2026-09-30", progress);
    progress.position = 100;
    await pending;
    expect(mockedInvoke).toHaveBeenCalledTimes(2);
    expect(mockedInvoke.mock.calls[0]).toEqual(mockedInvoke.mock.calls[1]);
    expect(mockedInvoke.mock.calls[0]).toEqual(["record_study_activity", {
      graphPath: "/a", id: "study", seconds: 30, day: "2026-09-30",
      progress: { ...item.progress, position: 2 }, requestId: expect.any(String),
    }]);
    const previous = (mockedInvoke.mock.calls[0][1] as { requestId: string }).requestId;
    await recordStudyActivity("/a", "study", 0, "2026-09-30", null);
    expect((mockedInvoke.mock.calls[2][1] as { requestId: string }).requestId).not.toBe(previous);
  });
  it("propagates explicit backend errors instead of returning empty data", async () => {
    mockedInvoke.mockRejectedValue("The active graph changed");
    await expect(listStudies("/old")).rejects.toBe("The active graph changed");
    await expect(recordStudyActivity("/old", "study", 0, "2026-09-30", null))
      .rejects.toBe("The active graph changed");
    expect(mockedInvoke).toHaveBeenCalledTimes(3);
  });
  it("preserves fractional seconds and a caller-owned receipt across later retries", async () => {
    const requestId = crypto.randomUUID();
    const seconds = 0.123456789;
    const progress = { ...item.progress, position: 1.25 };
    mockedInvoke.mockRejectedValueOnce("lost response").mockRejectedValueOnce("still disconnected");
    await expect(recordStudyActivity("/a", "study", seconds, "2026-09-30", progress, requestId))
      .rejects.toBe("still disconnected");
    mockedInvoke.mockResolvedValueOnce(undefined);
    await recordStudyActivity("/a", "study", seconds, "2026-09-30", progress, requestId);
    expect(mockedInvoke).toHaveBeenCalledTimes(3);
    for (const call of mockedInvoke.mock.calls) {
      expect(call).toEqual(["record_study_activity", {
        graphPath: "/a", id: "study", seconds, day: "2026-09-30", progress, requestId,
      }]);
    }
  });
});
