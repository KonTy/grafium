import { describe, expect, it } from "vitest";
import {
  bulletToTodoContent,
  isTaskContent,
  normalizeTaskPrefix,
  splitImeEnterContent,
  taskToBulletContent,
} from "./taskSyntax";

describe("task syntax helpers", () => {
  it("detects Logseq and Markdown task prefixes", () => {
    expect(isTaskContent("TODO ship it")).toBe(true);
    expect(isTaskContent("DONE [#A] shipped")).toBe(true);
    expect(isTaskContent("[ ] imported checkbox")).toBe(true);
    expect(isTaskContent("- [x] full markdown checkbox")).toBe(true);
    expect(isTaskContent("plain bullet text")).toBe(false);
  });

  it("turns task content into normal bullet content", () => {
    expect(taskToBulletContent("TODO [#A] Write migration")).toBe("Write migration");
    expect(taskToBulletContent("DONE archived idea")).toBe("archived idea");
    expect(taskToBulletContent("[ ] imported checkbox")).toBe("imported checkbox");
  });

  it("removes task-only scheduling and logbook metadata when turning tasks into bullets", () => {
    expect(
      taskToBulletContent(
        [
          "TODO [#B] Stopped project idea",
          "SCHEDULED: <2026-09-09 Wed>",
          "DEADLINE: <2026-09-10 Thu>",
          ":LOGBOOK:",
          "CLOCK: [2026-09-09 Wed 10:00]--[2026-09-09 Wed 11:00] =>  01:00",
          ":END:",
          "Keep this note for later",
        ].join("\n")
      )
    ).toBe("Stopped project idea\nKeep this note for later");
  });

  it("turns normal bullet content into a TODO without changing existing tasks", () => {
    expect(bulletToTodoContent("Call dentist")).toBe("TODO Call dentist");
    expect(bulletToTodoContent("  Indented note")).toBe("  TODO Indented note");
    expect(bulletToTodoContent("TODO already a task")).toBe("TODO already a task");
  });

  it("uppercases a task keyword even without a trailing space", () => {
    expect(normalizeTaskPrefix("todo")).toBe("TODO");
    expect(normalizeTaskPrefix("todo buy milk")).toBe("TODO buy milk");
    expect(isTaskContent("TODO")).toBe(true);
    expect(normalizeTaskPrefix("today is sunny")).toBe("today is sunny");
  });

  it("splits an IME newline so the leftover line becomes a new block", () => {
    expect(splitImeEnterContent("TODO buy milk")).toEqual({
      head: "TODO buy milk",
      remainder: "",
    });
    expect(splitImeEnterContent("TODO first\nTODO hide AI")).toEqual({
      head: "TODO first",
      remainder: "TODO hide AI",
    });
    expect(splitImeEnterContent("todo\n")).toEqual({
      head: "TODO",
      remainder: "",
    });
  });
});
