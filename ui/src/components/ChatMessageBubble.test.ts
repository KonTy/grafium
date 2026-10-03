import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import ChatMessageBubble from "./ChatMessageBubble.svelte";

let component: ReturnType<typeof mount> | undefined;
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
});

describe("message sender icons", () => {
  it.each([
    ["user", "You"],
    ["assistant", "Grafium AI"],
  ] as const)("keeps the %s name as text and its icon decorative", (role, label) => {
    component = mount(ChatMessageBubble, {
      target: document.body,
      props: { message: { role, content: "A selectable message." } },
    });
    flushSync();
    const sender = document.querySelector(".msg-sender")!;
    expect(sender.textContent?.trim()).toBe(label);
    expect(sender.querySelectorAll("svg")).toHaveLength(1);
    expect(sender.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
    expect(sender.querySelector("svg")?.getAttribute("focusable")).toBe("false");
    expect(sender.querySelector("[aria-label], title")).toBeNull();
    expect(document.querySelector(".msg-content")?.textContent?.trim()).toBe("A selectable message.");
    const copy = document.querySelector(".copy-btn");
    if (role === "assistant") {
      expect(copy?.textContent?.trim()).toBe("Copy");
      expect(copy?.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
    } else {
      expect(copy).toBeNull();
    }
  });

  it("does not introduce a duplicate sender while a trail owns the empty streaming answer", () => {
    component = mount(ChatMessageBubble, {
      target: document.body,
      props: { message: { role: "assistant", content: "" }, streaming: true, trailed: true },
    });
    flushSync();
    expect(document.querySelector(".msg-sender")).toBeNull();
  });
});
