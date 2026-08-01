import { afterEach, describe, expect, it, vi } from "vitest";
import { createSidebarSearchController } from "./sidebarSearch";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

describe("sidebar search controller", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("debounces search execution until typing pauses", async () => {
    vi.useFakeTimers();
    const run = vi.fn(async (query: string) => query.toUpperCase());
    const apply = vi.fn();
    const clear = vi.fn();
    const controller = createSidebarSearchController({
      debounceMs: 120,
      run,
      apply,
      clear,
    });

    controller.submit("gr");
    await vi.advanceTimersByTimeAsync(119);
    expect(run).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);
    await Promise.resolve();

    expect(run).toHaveBeenCalledTimes(1);
    expect(run).toHaveBeenCalledWith("gr");
    expect(apply).toHaveBeenCalledWith("gr", "GR");
    expect(clear).not.toHaveBeenCalled();
  });

  it("applies only the latest overlapping result", async () => {
    vi.useFakeTimers();
    const requests = new Map<string, ReturnType<typeof deferred<string>>>();
    const apply = vi.fn();
    const controller = createSidebarSearchController({
      debounceMs: 120,
      run: vi.fn((query: string) => {
        const request = deferred<string>();
        requests.set(query, request);
        return request.promise;
      }),
      apply,
      clear: vi.fn(),
    });

    controller.submit("gr");
    await vi.advanceTimersByTimeAsync(120);

    controller.submit("graf");
    await vi.advanceTimersByTimeAsync(120);

    requests.get("graf")!.resolve("newest");
    await Promise.resolve();

    requests.get("gr")!.resolve("stale");
    await Promise.resolve();

    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply).toHaveBeenCalledWith("graf", "newest");
  });
});
