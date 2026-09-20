import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import MobileEditorBar from "./MobileEditorBar.svelte";

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

beforeEach(() => {
  vi.useFakeTimers();
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  host.remove();
  vi.useRealTimers();
});

function pointer(button: HTMLButtonElement, type: string) {
  button.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true }));
  flushSync();
}

function render() {
  const onTime = vi.fn();
  const onTimeLocation = vi.fn();
  const noop = vi.fn();
  component = mount(MobileEditorBar, {
    target: host,
    props: {
      onTodo: noop,
      onTime,
      onTimeLocation,
      onOutdent: noop,
      onIndent: noop,
      onLink: noop,
      onTag: noop,
      onSlash: noop,
      onHide: noop,
    },
  }) as Record<string, unknown>;
  flushSync();
  const button = document.querySelector<HTMLButtonElement>(
    'button[aria-label="Insert current time; hold for time and location"]',
  );
  if (!button) throw new Error("timestamp button was not rendered");
  return { button, onTime, onTimeLocation };
}

describe("MobileEditorBar timestamp gesture", () => {
  it("inserts only the time after a normal tap", () => {
    const { button, onTime, onTimeLocation } = render();
    pointer(button, "pointerdown");
    vi.advanceTimersByTime(200);
    pointer(button, "pointerup");
    expect(onTime).toHaveBeenCalledOnce();
    expect(onTimeLocation).not.toHaveBeenCalled();
  });

  it("inserts time and location after a long press without also firing tap", () => {
    const { button, onTime, onTimeLocation } = render();
    pointer(button, "pointerdown");
    vi.advanceTimersByTime(600);
    expect(onTimeLocation).toHaveBeenCalledOnce();
    pointer(button, "pointerup");
    expect(onTime).not.toHaveBeenCalled();
  });

  it("does nothing when the pointer is cancelled", () => {
    const { button, onTime, onTimeLocation } = render();
    pointer(button, "pointerdown");
    pointer(button, "pointercancel");
    vi.runAllTimers();
    expect(onTime).not.toHaveBeenCalled();
    expect(onTimeLocation).not.toHaveBeenCalled();
  });
});
