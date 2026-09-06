import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import { toasts, showToast, dismissToast, describeError } from "./toast.svelte";

describe("toast store", () => {
  beforeEach(() => {
    toasts.splice(0, toasts.length);
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("adds a toast with the requested severity", () => {
    showToast("Could not save", "error");
    expect(toasts).toHaveLength(1);
    expect(toasts[0].message).toBe("Could not save");
    expect(toasts[0].severity).toBe("error");
  });

  it("defaults to error severity", () => {
    showToast("Something broke");
    expect(toasts[0].severity).toBe("error");
  });

  it("gives each toast a distinct id so repeats stack", () => {
    showToast("same message");
    showToast("same message");
    expect(toasts).toHaveLength(2);
    expect(toasts[0].id).not.toBe(toasts[1].id);
  });

  it("dismisses only the requested toast", () => {
    showToast("first");
    showToast("second");
    dismissToast(toasts[0].id);
    expect(toasts).toHaveLength(1);
    expect(toasts[0].message).toBe("second");
  });

  it("ignores a dismiss for an unknown id", () => {
    showToast("only");
    dismissToast(9999);
    expect(toasts).toHaveLength(1);
  });

  it("auto-dismisses after the timeout", () => {
    showToast("transient");
    expect(toasts).toHaveLength(1);
    vi.advanceTimersByTime(6000);
    expect(toasts).toHaveLength(0);
  });

  it("auto-dismiss removes the right toast when others were dismissed first", () => {
    showToast("first");
    showToast("second");
    dismissToast(toasts[0].id);
    vi.advanceTimersByTime(6000);
    expect(toasts).toHaveLength(0);
  });
});

describe("describeError", () => {
  it("uses the message of an Error", () => {
    expect(describeError(new Error("disk full"))).toBe("disk full");
  });

  it("stringifies non-Error values", () => {
    expect(describeError("plain string")).toBe("plain string");
    expect(describeError(42)).toBe("42");
  });
});
