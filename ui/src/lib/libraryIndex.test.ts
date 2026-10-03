import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  acceptLibraryIndexStatus, acceptLibrarySearchHits, acceptLibrarySources, createStaleLibrarySearchGuard,
  formatLibrarySourcePosition, formatLibraryTimestamp, libraryIndexSettingsSet,
  libraryIndexStart, libraryIndexStatus, librarySearch, subscribeLibraryIndexUpdated,
} from "./libraryIndex";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

const status = {
  enabled: true, transcribeMedia: true, running: false, jobId: null,
  items: { total: 3, indexed: 2, pending: 1, failed: 0, titleOnly: 0 }, chunks: 42,
  semantic: "ready", semanticReason: null, transcription: "ready", transcriptionReason: null,
  lastIndexedAt: 123456, errors: [],
};
const hit = {
  bookId: "b1", title: "Fuel video", chunkId: "chunk-1", kind: "video", snippet: "replace the fuel filter",
  trackId: "t1", startMs: 65000, endMs: 90000, chapter: null, quote: null, score: 0.7, match: "both",
} as const;

beforeEach(() => {
  vi.resetAllMocks();
  mocks.listen.mockResolvedValue(vi.fn());
});

describe("library index IPC wrappers", () => {
  it("validates status, truncates oversized strings, and drops malformed entries", () => {
    expect(acceptLibraryIndexStatus(status).items.indexed).toBe(2);
    expect(() => acceptLibraryIndexStatus({ ...status, semantic: "cloud" })).toThrow(/Invalid Library index status/);
    const accepted = acceptLibraryIndexStatus({ ...status, errors: [
      { bookId: "book", title: "T".repeat(1000), message: "M".repeat(1000) },
      { bookId: "bad", title: "", message: "missing title" },
    ] });
    expect(accepted.errors).toHaveLength(1);
    expect(accepted.errors[0].title).toMatch(/…$/);
    expect(accepted.errors[0].message.length).toBeLessThanOrEqual(500);
    expect(acceptLibrarySearchHits([hit, { ...hit, bookId: "" }, { ...hit, bookId: "b2", snippet: "x".repeat(301) }]))
      .toEqual([expect.objectContaining({ bookId: "b1" }), expect.objectContaining({ bookId: "b2", snippet: expect.stringMatching(/…$/) })]);
  });



  it("accepts contract-shaped camelCase status and snake_case Library source payloads", () => {
    const rustSearchJson = `[{"bookId":"book","title":"Manual","chunkId":"chunk-a","kind":"epub","snippet":"Fuel filter","trackId":null,"startMs":null,"endMs":null,"chapter":"Chapter","quote":"Fuel filter","score":1,"match":"keyword"}]`;
    const rustStatusJson = `{"enabled":true,"transcribeMedia":true,"running":false,"jobId":null,"items":{"total":1,"indexed":1,"pending":0,"failed":0,"titleOnly":0},"chunks":4,"semantic":"ready","semanticReason":null,"transcription":"ready","transcriptionReason":null,"lastIndexedAt":123,"errors":[]}`;
    const rustSourcesJson = `[{"index":1,"book_id":"book","title":"Manual","kind":"epub","track_id":null,"start_ms":null,"end_ms":null,"chapter":"Chapter","quote":"Fuel filter"}]`;
    expect(acceptLibraryIndexStatus(JSON.parse(rustStatusJson))).toMatchObject({ enabled: true, items: { total: 1 } });
    expect(acceptLibrarySearchHits(JSON.parse(rustSearchJson))).toEqual([expect.objectContaining({ chunkId: "chunk-a", match: "keyword" })]);
    expect(acceptLibrarySources(JSON.parse(rustSourcesJson))).toEqual([expect.objectContaining({ bookId: "book", chapter: "Chapter" })]);
  });

  it("calls fixed command names with camelCase args", async () => {
    mocks.invoke.mockResolvedValueOnce(status).mockResolvedValueOnce({ ...status, enabled: false }).mockResolvedValueOnce("job-1").mockResolvedValueOnce([hit]);
    await expect(libraryIndexStatus()).resolves.toMatchObject({ enabled: true });
    await expect(libraryIndexSettingsSet(false, true)).resolves.toMatchObject({ enabled: false });
    await expect(libraryIndexStart(true)).resolves.toBe("job-1");
    await expect(librarySearch(" fuel ", 200)).resolves.toHaveLength(1);
    expect(mocks.invoke).toHaveBeenNthCalledWith(1, "library_index_status");
    expect(mocks.invoke).toHaveBeenNthCalledWith(2, "library_index_settings_set", { enabled: false, transcribeMedia: true });
    expect(mocks.invoke).toHaveBeenNthCalledWith(3, "library_index_start", { rebuild: true });
    expect(mocks.invoke).toHaveBeenNthCalledWith(4, "library_search", { query: "fuel", limit: 100 });
  });

  it("subscribes to library-index-updated with validated payloads", async () => {
    let callback: (event: { payload: unknown }) => void = () => {};
    mocks.listen.mockImplementation(async (_name, cb) => { callback = cb; return vi.fn(); });
    const handler = vi.fn();
    await subscribeLibraryIndexUpdated(handler);
    callback({ payload: status });
    expect(mocks.listen).toHaveBeenCalledWith("library-index-updated", expect.any(Function));
    expect(handler).toHaveBeenCalledWith(expect.objectContaining({ chunks: 42 }));
  });

  it("formats timestamps and source positions", () => {
    expect(formatLibraryTimestamp(65_000)).toBe("1:05");
    expect(formatLibraryTimestamp(3_665_000)).toBe("1:01:05");
    expect(acceptLibrarySearchHits([hit])[0].chunkId).toBe("chunk-1");
    expect(acceptLibrarySearchHits([{ ...hit, chunkId: 42 }])[0].chunkId).toBeNull();
    expect(formatLibrarySourcePosition(hit)).toBe("1:05");
    expect(formatLibrarySourcePosition({ kind: "epub", startMs: null, chapter: "Chapter 2" })).toBe("Chapter 2");
  });

  it("ignores stale search responses", async () => {
    let resolveFirst: (hits: typeof hit[]) => void = () => {};
    const searcher = vi.fn()
      .mockImplementationOnce(() => new Promise<typeof hit[]>((resolve) => { resolveFirst = resolve; }))
      .mockResolvedValueOnce([{ ...hit, bookId: "new" }]);
    const guarded = createStaleLibrarySearchGuard(searcher);
    const first = guarded("fuel");
    const second = guarded("filter");
    resolveFirst([hit]);
    await expect(second).resolves.toEqual([expect.objectContaining({ bookId: "new" })]);
    await expect(first).resolves.toBeNull();
  });

  it("turns stale failures and cleared queries into null", async () => {
    let rejectFirst: (cause: Error) => void = () => {};
    let resolveThird: (hits: typeof hit[]) => void = () => {};
    const searcher = vi.fn()
      .mockImplementationOnce(() => new Promise<typeof hit[]>((_resolve, reject) => { rejectFirst = reject; }))
      .mockResolvedValueOnce([hit])
      .mockImplementationOnce(() => new Promise<typeof hit[]>((resolve) => { resolveThird = resolve; }));
    const guarded = createStaleLibrarySearchGuard(searcher);
    const first = guarded("old");
    const second = guarded("new");
    rejectFirst(new Error("old failure"));
    await expect(second).resolves.toEqual([hit]);
    await expect(first).resolves.toBeNull();
    const third = guarded("soon-cleared");
    guarded.cancel();
    resolveThird([hit]);
    await expect(third).resolves.toBeNull();
  });
});
