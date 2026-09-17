import { describe, expect, it } from "vitest";
import { FOLLOW_THRESHOLD_PX, isNearBottom } from "./scrollToBottom";

function pane(scrollTop: number, clientHeight: number, scrollHeight: number) {
  return { scrollTop, clientHeight, scrollHeight } as HTMLElement;
}

describe("isNearBottom", () => {
  it("is true when pinned to the very bottom", () => {
    expect(isNearBottom(pane(600, 400, 1000))).toBe(true);
  });

  it("is true for a short pane that cannot scroll", () => {
    expect(isNearBottom(pane(0, 400, 300))).toBe(true);
  });

  it("stays true inside the follow threshold", () => {
    expect(isNearBottom(pane(600 - FOLLOW_THRESHOLD_PX + 1, 400, 1000))).toBe(true);
  });

  it("turns false once the reader has scrolled away", () => {
    expect(isNearBottom(pane(600 - FOLLOW_THRESHOLD_PX - 1, 400, 1000))).toBe(false);
  });

  it("treats a missing element as following, so the first paint scrolls", () => {
    expect(isNearBottom(null)).toBe(true);
  });

  it("keeps following across the bottom padding that the fix added", () => {
    // The pane grew 14px of padding; that must not read as "scrolled away".
    expect(isNearBottom(pane(586, 400, 1014))).toBe(true);
  });
});
