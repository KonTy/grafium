import { describe, expect, it } from "vitest";
import {
  deletionPrompt,
  deletionTakesEverything,
  deletionTargets,
  pruneSelection,
  selectRange,
} from "./chatSelection";

const threads = [
  { id: "a" },
  { id: "b" },
  { id: "c" },
  { id: "d" },
];

describe("deletionTargets", () => {
  it("takes everything when nothing is selected", () => {
    expect(deletionTargets(threads, new Set()).map((t) => t.id)).toEqual([
      "a",
      "b",
      "c",
      "d",
    ]);
  });

  it("takes only the selection when there is one", () => {
    expect(deletionTargets(threads, new Set(["b", "d"])).map((t) => t.id)).toEqual([
      "b",
      "d",
    ]);
  });

  it("returns the selection in list order, not click order", () => {
    // The confirmation counts these, and the caller deletes them in order;
    // neither should depend on which row happened to be clicked first.
    const clickedBackwards = new Set(["d", "a"]);
    expect(deletionTargets(threads, clickedBackwards).map((t) => t.id)).toEqual([
      "a",
      "d",
    ]);
  });

  // The dangerous direction is a stale id quietly turning "delete 2" into
  // "delete everything", so an id that no longer exists must simply vanish.
  it("ignores selected ids that no longer exist", () => {
    expect(deletionTargets(threads, new Set(["b", "gone"])).map((t) => t.id)).toEqual([
      "b",
    ]);
  });

  it("deletes nothing when every selected id is stale", () => {
    expect(deletionTargets(threads, new Set(["gone", "also-gone"]))).toEqual([]);
  });

  it("has nothing to delete in an empty list", () => {
    expect(deletionTargets([], new Set())).toEqual([]);
  });
});

describe("deletionTakesEverything", () => {
  it("is true when nothing is selected", () => {
    expect(deletionTakesEverything(4, 4)).toBe(true);
  });

  it("is false for a partial selection", () => {
    expect(deletionTakesEverything(4, 2)).toBe(false);
  });

  // Hand-selecting every row is still "all of them", and saying otherwise
  // would understate what the button is about to do.
  it("is true when the selection happens to cover every chat", () => {
    expect(deletionTakesEverything(3, 3)).toBe(true);
  });

  it("is false when there is nothing at all", () => {
    expect(deletionTakesEverything(0, 0)).toBe(false);
  });
});

describe("deletionPrompt", () => {
  it("names the total when taking everything", () => {
    expect(deletionPrompt(7, true, 0)).toBe("Delete all 7 chats? This cannot be undone.");
  });

  it("names the selection when taking part of the list", () => {
    expect(deletionPrompt(2, false, 0)).toBe(
      "Delete 2 selected chats? This cannot be undone.",
    );
  });

  it("stays readable for a single chat", () => {
    expect(deletionPrompt(1, true, 0)).toBe("Delete this chat? This cannot be undone.");
    expect(deletionPrompt(1, false, 0)).toBe(
      "Delete the selected chat? This cannot be undone.",
    );
  });

  // Killing work that is mid-answer is a separate consequence from losing the
  // transcript, and nothing else on screen says it is about to happen.
  it("warns that running conversations will be stopped", () => {
    expect(deletionPrompt(5, true, 2)).toBe(
      "Delete all 5 chats? 2 are still working and will be stopped. This cannot be undone.",
    );
    expect(deletionPrompt(3, false, 1)).toBe(
      "Delete 3 selected chats? One is still working and will be stopped. This cannot be undone.",
    );
  });

  it("says nothing about running work when none is running", () => {
    expect(deletionPrompt(4, true, 0)).not.toContain("working");
  });
});

describe("selectRange", () => {
  const ids = ["a", "b", "c", "d", "e"];

  it("covers both ends of a downward range", () => {
    expect(selectRange(ids, "b", "d")).toEqual(["b", "c", "d"]);
  });

  it("covers an upward range the same way", () => {
    expect(selectRange(ids, "d", "b")).toEqual(["b", "c", "d"]);
  });

  it("is just the one row when the anchor is the target", () => {
    expect(selectRange(ids, "c", "c")).toEqual(["c"]);
  });

  // Shift-clicking before anything is selected should not reach back to the
  // top of the list and sweep up chats the user never touched.
  it("selects only the clicked row when there is no anchor", () => {
    expect(selectRange(ids, null, "d")).toEqual(["d"]);
  });

  it("selects only the clicked row when the anchor has been deleted", () => {
    expect(selectRange(ids, "gone", "d")).toEqual(["d"]);
  });

  it("selects nothing when the target itself is unknown", () => {
    expect(selectRange(ids, "a", "gone")).toEqual([]);
  });
});

describe("pruneSelection", () => {
  it("keeps ids that still exist", () => {
    expect([...pruneSelection(new Set(["a", "c"]), ["a", "b", "c"])]).toEqual(["a", "c"]);
  });

  it("drops ids that have gone", () => {
    expect([...pruneSelection(new Set(["a", "gone"]), ["a", "b"])]).toEqual(["a"]);
  });

  it("empties the selection when the whole list is replaced", () => {
    expect(pruneSelection(new Set(["a", "b"]), ["x", "y"]).size).toBe(0);
  });

  it("does not mutate the set it was given", () => {
    const original = new Set(["a", "gone"]);
    pruneSelection(original, ["a"]);
    expect(original.size).toBe(2);
  });
});
