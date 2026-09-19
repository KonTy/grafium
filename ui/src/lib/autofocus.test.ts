import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { autofocus } from "./autofocus";

/**
 * The action's whole point is that focus comes back when the thing that stole
 * it goes away. The first version only restored when `document.activeElement`
 * was still the node itself, which is the one state that is *not* reliably true
 * during teardown: removing a focused element drops focus to `<body>` first, and
 * arrow-key navigation inside a menu moves it to a child. Both looked like
 * "somebody else has the focus now", so the action quietly did nothing and the
 * user was dumped at the top of the document.
 */

let host: HTMLElement;
let trigger: HTMLButtonElement;

beforeEach(() => {
  host = document.createElement("div");
  document.body.appendChild(host);
  trigger = document.createElement("button");
  host.appendChild(trigger);
  trigger.focus();
});

afterEach(() => {
  host.remove();
});

function mountPanel(): { node: HTMLElement; destroy: () => void } {
  const node = document.createElement("div");
  node.tabIndex = -1;
  const child = document.createElement("button");
  node.appendChild(child);
  host.appendChild(node);
  const action = autofocus(node);
  return {
    node,
    destroy: () => {
      action.destroy();
      node.remove();
    },
  };
}

describe("autofocus", () => {
  it("takes the focus when it mounts", () => {
    const panel = mountPanel();
    expect(document.activeElement).toBe(panel.node);
    panel.destroy();
  });

  it("gives focus back to whatever had it before", () => {
    const panel = mountPanel();
    panel.destroy();
    expect(document.activeElement).toBe(trigger);
  });

  it("gives focus back when the node is detached before teardown runs", () => {
    const panel = mountPanel();
    // Mirrors the real order: the element leaves the DOM, the browser resets
    // activeElement to <body>, and only then does the action tear down.
    panel.node.remove();
    expect(document.activeElement).toBe(document.body);
    panel.destroy();
    expect(document.activeElement).toBe(trigger);
  });

  it("gives focus back when focus moved to one of its children", () => {
    const panel = mountPanel();
    panel.node.querySelector("button")!.focus();
    panel.destroy();
    expect(document.activeElement).toBe(trigger);
  });

  it("leaves focus alone when the user moved it somewhere else", () => {
    const elsewhere = document.createElement("button");
    host.appendChild(elsewhere);
    const panel = mountPanel();

    elsewhere.focus();
    panel.destroy();

    expect(document.activeElement).toBe(elsewhere);
  });

  it("does nothing when the previous element is gone", () => {
    const panel = mountPanel();
    trigger.remove();
    panel.destroy();
    expect(document.activeElement).toBe(document.body);
  });
});

describe("autofocus in a real component teardown", () => {
  it("restores focus when Svelte unmounts the component holding it", async () => {
    const { default: Fixture } = await import("../components/fixtures/AutofocusFixture.svelte");
    const component = mount(Fixture, { target: host, props: {} });
    flushSync();

    const open = host.querySelector<HTMLButtonElement>(".open")!;
    open.focus();
    open.click();
    flushSync();

    const panel = host.querySelector<HTMLElement>(".panel")!;
    expect(document.activeElement).toBe(panel);

    host.querySelector<HTMLButtonElement>(".close")!.click();
    flushSync();

    expect(document.activeElement).toBe(open);
    unmount(component);
  });
});
