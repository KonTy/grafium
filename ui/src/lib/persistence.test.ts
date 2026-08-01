import { describe, expect, it, vi } from "vitest";
import {
  buildSaveContext,
  persistBlockContentIfChanged,
  persistThen,
} from "./persistence";

describe("persistence regressions", () => {
  it("builds save context with changed=false when content is identical", () => {
    const ctx = buildSaveContext("b1", "p1", "same", "same");
    expect(ctx.changed).toBe(false);
    expect(ctx.before).toBe("same");
    expect(ctx.after).toBe("same");
  });

  it("persists and mutates local block when content changed", async () => {
    const block = { id: "b1", content: "" };
    const update = vi.fn(async () => {});

    const changed = await persistBlockContentIfChanged(block, "[[test]]", update);

    expect(changed).toBe(true);
    expect(block.content).toBe("[[test]]");
    expect(update).toHaveBeenCalledTimes(1);
    expect(update).toHaveBeenCalledWith("b1", "[[test]]");
  });

  it("does not call update when content unchanged", async () => {
    const block = { id: "b1", content: "[[test]]" };
    const update = vi.fn(async () => {});

    const changed = await persistBlockContentIfChanged(block, "[[test]]", update);

    expect(changed).toBe(false);
    expect(update).not.toHaveBeenCalled();
  });

  it("rejects and leaves local block content unchanged when the save fails", async () => {
    const block = { id: "b1", content: "before" };
    const update = vi.fn(async () => {
      throw new Error("disk full");
    });

    await expect(
      persistBlockContentIfChanged(block, "after", update)
    ).rejects.toThrow("disk full");

    expect(block.content).toBe("before");
    expect(update).toHaveBeenCalledWith("b1", "after");
  });

  it("runs persistence before structural operation", async () => {
    const order: string[] = [];

    await persistThen(
      async () => {
        order.push("persist");
      },
      async () => {
        order.push("move");
      }
    );

    expect(order).toEqual(["persist", "move"]);
  });
});
