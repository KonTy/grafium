import { describe, expect, it } from "vitest";
import {
  clearPendingEditPageEnd,
  dispatchEditPageEnd,
  formatLocalClockTime,
  peekPendingEditPageEnd,
  PERSONAL_DIARY_LINK,
  personalDiarySnippet,
  timeStampSnippet,
} from "./editorInsert";

describe("formatLocalClockTime", () => {
  it("pads hours and minutes to HH:MM", () => {
    expect(formatLocalClockTime(new Date(2026, 0, 2, 9, 7))).toBe("09:07");
    expect(formatLocalClockTime(new Date(2026, 0, 2, 14, 3))).toBe("14:03");
  });
});

describe("snippets", () => {
  it("puts the caret on the next line after the time", () => {
    expect(timeStampSnippet(new Date(2026, 0, 2, 14, 7))).toBe("14:07\n");
  });

  it("inserts the personal diary wiki link then a newline", () => {
    expect(personalDiarySnippet()).toBe("[[personal/diary]]\n");
    expect(PERSONAL_DIARY_LINK).toBe("[[personal/diary]]");
  });
});

describe("pending edit-page-end", () => {
  it("matches the target page and can be consumed", () => {
    dispatchEditPageEnd({ pageTitle: "2026-04-08", insert: "14:07\n" });
    expect(peekPendingEditPageEnd({ id: "p1", title: "other" })).toBeNull();
    expect(peekPendingEditPageEnd({ id: "p1", title: "2026-04-08" })).toEqual({
      pageTitle: "2026-04-08",
      insert: "14:07\n",
    });
    clearPendingEditPageEnd();
    expect(peekPendingEditPageEnd({ id: "p1", title: "2026-04-08" })).toBeNull();
  });
});
