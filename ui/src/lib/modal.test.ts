// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { dismissOnBackdrop, dialogKeydown } from "./modal";

function buildDialog() {
  document.body.innerHTML = `
    <div id="backdrop">
      <div id="box"><input id="field" /></div>
    </div>`;
  return {
    backdrop: document.getElementById("backdrop") as HTMLElement,
    box: document.getElementById("box") as HTMLElement,
    field: document.getElementById("field") as HTMLElement,
  };
}

/** jsdom has no PointerEvent, and the action only reads `target`. */
function press(el: HTMLElement) {
  el.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
}

/**
 * The browser dispatches `click` at the nearest common ancestor of the press
 * and release targets, so a drag from inside the box to the backdrop produces
 * a click whose target is the backdrop itself.
 */
function clickAt(el: HTMLElement) {
  el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("dismissOnBackdrop", () => {
  it("closes when the backdrop is both pressed and released", () => {
    const { backdrop } = buildDialog();
    const close = vi.fn();
    dismissOnBackdrop(backdrop, close);

    press(backdrop);
    clickAt(backdrop);

    expect(close).toHaveBeenCalledTimes(1);
  });

  it("ignores clicks that originate inside the dialog box", () => {
    const { backdrop, field } = buildDialog();
    const close = vi.fn();
    dismissOnBackdrop(backdrop, close);

    press(field);
    clickAt(field);

    expect(close).not.toHaveBeenCalled();
  });

  it("does not close when a drag starts inside the box and releases on the backdrop", () => {
    const { backdrop, field } = buildDialog();
    const close = vi.fn();
    dismissOnBackdrop(backdrop, close);

    // Press inside, then the retargeted click lands on the backdrop.
    press(field);
    clickAt(backdrop);

    expect(close).not.toHaveBeenCalled();
  });

  it("re-arms after a rejected drag so a later genuine click still closes", () => {
    const { backdrop, field } = buildDialog();
    const close = vi.fn();
    dismissOnBackdrop(backdrop, close);

    press(field);
    clickAt(backdrop);
    expect(close).not.toHaveBeenCalled();

    press(backdrop);
    clickAt(backdrop);
    expect(close).toHaveBeenCalledTimes(1);
  });

  it("never stops propagation, so delegated handlers inside the dialog still fire", () => {
    const { backdrop, field } = buildDialog();
    dismissOnBackdrop(backdrop, vi.fn());
    const delegated = vi.fn();
    document.addEventListener("click", delegated);

    press(field);
    clickAt(field);

    expect(delegated).toHaveBeenCalledTimes(1);
    document.removeEventListener("click", delegated);
  });

  it("stops listening once destroyed", () => {
    const { backdrop } = buildDialog();
    const close = vi.fn();
    const handle = dismissOnBackdrop(backdrop, close);

    handle.destroy();
    press(backdrop);
    clickAt(backdrop);

    expect(close).not.toHaveBeenCalled();
  });

  it("uses the latest callback after an update", () => {
    const { backdrop } = buildDialog();
    const first = vi.fn();
    const second = vi.fn();
    const handle = dismissOnBackdrop(backdrop, first);

    handle.update(second);
    press(backdrop);
    clickAt(backdrop);

    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
  });
});

describe("dialogKeydown", () => {
  it("closes on Escape and stops the event", () => {
    const close = vi.fn();
    const event = new KeyboardEvent("keydown", { key: "Escape", cancelable: true });
    const stop = vi.spyOn(event, "stopPropagation");

    dialogKeydown(close)(event);

    expect(close).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
    expect(stop).toHaveBeenCalled();
  });

  it("leaves other keys alone so typing still reaches inputs", () => {
    const close = vi.fn();
    const event = new KeyboardEvent("keydown", { key: "a", cancelable: true });

    dialogKeydown(close)(event);

    expect(close).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });
});

describe("dialogKeydown Tab containment", () => {
  function mountDialog(inner: string) {
    const box = document.createElement("div");
    box.setAttribute("role", "dialog");
    box.setAttribute("aria-modal", "true");
    box.tabIndex = -1;
    box.innerHTML = inner;
    box.addEventListener("keydown", dialogKeydown(() => {}));

    const outside = document.createElement("button");
    outside.textContent = "background";

    document.body.append(box, outside);
    // jsdom gives every element zero client rects, so the visibility filter
    // would drop everything. Report a real box for anything laid out.
    vi.spyOn(Element.prototype, "getClientRects").mockReturnValue([
      { width: 10, height: 10 },
    ] as unknown as DOMRectList);

    return { box, outside };
  }

  function tab(target: HTMLElement, shiftKey = false) {
    const event = new KeyboardEvent("keydown", { key: "Tab", shiftKey, bubbles: true, cancelable: true });
    target.dispatchEvent(event);
    return event;
  }

  afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = "";
  });

  it("wraps forward from the last control back to the first", () => {
    const { box } = mountDialog('<button id="a">a</button><input id="b" />');
    const first = box.querySelector<HTMLElement>("#a")!;
    const last = box.querySelector<HTMLElement>("#b")!;
    last.focus();

    const event = tab(last);

    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(first);
  });

  it("wraps backward from the first control round to the last", () => {
    const { box } = mountDialog('<button id="a">a</button><input id="b" />');
    const first = box.querySelector<HTMLElement>("#a")!;
    const last = box.querySelector<HTMLElement>("#b")!;
    first.focus();

    const event = tab(first, true);

    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(last);
  });

  it("does not intercept Tab moving between the dialog's own controls", () => {
    const { box } = mountDialog('<button id="a">a</button><input id="b" /><button id="c">c</button>');
    const middle = box.querySelector<HTMLElement>("#b")!;
    middle.focus();

    const event = tab(middle);

    expect(event.defaultPrevented).toBe(false);
  });

  it("keeps focus on the box when the dialog has no focusable controls", () => {
    const { box } = mountDialog("<p>nothing to focus</p>");
    box.focus();

    const event = tab(box);

    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(box);
  });

  it("never lets Tab reach a control behind the dialog", () => {
    const { box, outside } = mountDialog('<button id="a">a</button>');
    const only = box.querySelector<HTMLElement>("#a")!;
    only.focus();

    tab(only);
    expect(document.activeElement).toBe(only);
    expect(document.activeElement).not.toBe(outside);
  });

  it("skips disabled controls when picking the wrap targets", () => {
    const { box } = mountDialog('<button id="a">a</button><button id="b" disabled>b</button>');
    const first = box.querySelector<HTMLElement>("#a")!;
    first.focus();

    const event = tab(first);

    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(first);
  });
});
