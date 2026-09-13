import { describe, expect, it, vi } from "vitest";
import { lazyComponent } from "./lazy";

describe("lazyComponent", () => {
  it("does not import before first use and shares the pending and resolved promise", async () => {
    const component = { default: {} };
    const importModule = vi.fn(async () => component);
    const load = lazyComponent(importModule);
    expect(importModule).not.toHaveBeenCalled();
    const first = load();
    expect(load()).toBe(first);
    expect(await first).toBe(component);
    expect(load()).toBe(first);
    expect(importModule).toHaveBeenCalledTimes(1);
  });

  it("surfaces a failed import and allows an explicit retry", async () => {
    const component = { default: {} };
    const importModule = vi.fn()
      .mockRejectedValueOnce(new Error("Network unavailable"))
      .mockResolvedValueOnce(component);
    const load = lazyComponent(importModule);
    await expect(load()).rejects.toThrow("Network unavailable");
    expect(importModule).toHaveBeenCalledTimes(1);
    expect(await load()).toBe(component);
    expect(importModule).toHaveBeenCalledTimes(2);
  });
});
