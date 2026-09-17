import { describe, expect, it } from "vitest";
import { BUSY_STALL_MS, busyDisplay } from "./busyState";

describe("busyDisplay", () => {
  it("is completely idle when nothing is running", () => {
    expect(busyDisplay(null, 10_000)).toEqual({ shimmer: false, stalled: false, seconds: 0 });
  });

  it("shimmers while a run is inside its deadline", () => {
    const d = busyDisplay(1_000, 1_000 + 3_000);
    expect(d.shimmer).toBe(true);
    expect(d.stalled).toBe(false);
    expect(d.seconds).toBe(3);
  });

  it("stops shimmering once the run overruns", () => {
    const d = busyDisplay(0, BUSY_STALL_MS + 1);
    expect(d.shimmer).toBe(false);
    expect(d.stalled).toBe(true);
  });

  it("treats the deadline itself as still healthy", () => {
    expect(busyDisplay(0, BUSY_STALL_MS).stalled).toBe(false);
    expect(busyDisplay(0, BUSY_STALL_MS).shimmer).toBe(true);
  });

  it("never shimmers under reduced motion, even while healthy", () => {
    expect(busyDisplay(0, 500, true).shimmer).toBe(false);
  });

  it("still reports stall under reduced motion so callers can say so", () => {
    const d = busyDisplay(0, BUSY_STALL_MS + 5_000, true);
    expect(d.shimmer).toBe(false);
    expect(d.stalled).toBe(true);
  });

  it("honours a caller's shorter deadline", () => {
    expect(busyDisplay(0, 3_000, false, 2_000).stalled).toBe(true);
    expect(busyDisplay(0, 1_000, false, 2_000).stalled).toBe(false);
  });

  it("treats a non-positive deadline as no deadline at all", () => {
    const d = busyDisplay(0, 10 * BUSY_STALL_MS, false, 0);
    expect(d.stalled).toBe(false);
    expect(d.shimmer).toBe(true);
  });

  it("clamps a clock that jumps backwards instead of reporting negative time", () => {
    const d = busyDisplay(5_000, 1_000);
    expect(d.seconds).toBe(0);
    expect(d.stalled).toBe(false);
  });
});
