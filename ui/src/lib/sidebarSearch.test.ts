import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  searchPageTitles: vi.fn(),
  searchFts: vi.fn(),
}));

import { searchFts, searchPageTitles } from "./api";
import { createSidebarSearchController, runSidebarSearch } from "./sidebarSearch";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

describe("sidebar search controller", () => {
  const page = { id: "page-1", title: "Grafium", is_journal: false };
  const block = {
    id: "block-1",
    page_id: "page-1",
    parent_id: null,
    order_index: 0,
    content: "Grafium block",
    block_type: "text",
    properties: {},
    created_at: "0",
    updated_at: "0",
  };
  const mockSearchPageTitles = vi.mocked(searchPageTitles);
  const mockSearchFts = vi.mocked(searchFts);

  beforeEach(() => {
    mockSearchPageTitles.mockReset();
    mockSearchFts.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("debounces search execution until typing pauses", async () => {
    vi.useFakeTimers();
    mockSearchPageTitles.mockResolvedValue([page]);
    mockSearchFts.mockResolvedValue([block]);
    const apply = vi.fn();
    const clear = vi.fn();
    const controller = createSidebarSearchController({
      debounceMs: 120,
      run: runSidebarSearch,
      apply,
      clear,
    });

    controller.submit("gr");
    await vi.advanceTimersByTimeAsync(119);
    expect(mockSearchPageTitles).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);
    await Promise.resolve();
    await Promise.resolve();

    expect(mockSearchPageTitles).toHaveBeenCalledTimes(1);
    expect(mockSearchPageTitles).toHaveBeenCalledWith("gr", 10);
    expect(mockSearchFts).toHaveBeenCalledWith("gr", 20);
    expect(apply).toHaveBeenCalledWith("gr", [
      { kind: "page", page },
      { kind: "block", block },
    ]);
    expect(clear).not.toHaveBeenCalled();
  });

  it("applies only the latest overlapping result", async () => {
    vi.useFakeTimers();
    const requests = new Map<string, ReturnType<typeof deferred<(typeof page)[]>>>();
    const apply = vi.fn();
    const controller = createSidebarSearchController({
      debounceMs: 120,
      run: runSidebarSearch,
      clear: vi.fn(),
      apply,
    });
    mockSearchPageTitles.mockImplementation((query: string) => {
      const request = deferred<(typeof page)[]>();
      requests.set(query, request);
      return request.promise;
    });
    mockSearchFts.mockResolvedValue([]);

    controller.submit("gr");
    await vi.advanceTimersByTimeAsync(120);

    controller.submit("graf");
    await vi.advanceTimersByTimeAsync(120);

    requests.get("graf")!.resolve([{ ...page, id: "page-2", title: "Graf Search" }]);
    await Promise.resolve();
    await Promise.resolve();

    requests.get("gr")!.resolve([page]);
    await Promise.resolve();
    await Promise.resolve();

    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply).toHaveBeenCalledWith("graf", [
      { kind: "page", page: { ...page, id: "page-2", title: "Graf Search" } },
    ]);
  });

  it("uses indexed title search without scanning blocks for one-character queries", async () => {
    mockSearchPageTitles.mockResolvedValue([page]);
    mockSearchFts.mockResolvedValue([block]);

    await expect(runSidebarSearch("g")).resolves.toEqual([
      { kind: "page", page },
    ]);
    expect(mockSearchPageTitles).toHaveBeenCalledWith("g", 10);
    expect(mockSearchFts).not.toHaveBeenCalled();
  });
});
