import { readFileSync } from "node:fs";
import { join } from "node:path";
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import DelegationFixture from "./fixtures/DelegationFixture.svelte";

const source = readFileSync(join(process.cwd(), "src/components/BlockEditor.svelte"), "utf8");

/**
 * Svelte 5 does not attach `onclick` to the element it is written on. For
 * delegated events it registers one dispatcher on the mount root and on
 * `document`, then walks from `event.target` upwards invoking the handlers it
 * finds. A *native* `stopPropagation()` listener on an intermediate element
 * therefore does not just stop ancestors: it stops the event ever reaching the
 * dispatcher, so every delegated handler on the path — including ones on
 * elements *below* the stopping element — silently stops working.
 *
 * `.query-block` used to carry exactly that pattern, which killed its own
 * "Edit query", "Re-run query" and "Show more" buttons. The guard was also
 * redundant, because `handleClick` already returns early for query blocks.
 *
 * These run against the real Svelte runtime, so they stay honest if Svelte ever
 * changes how it delegates.
 */
describe("native stopPropagation vs Svelte's delegated dispatcher", () => {
  let target: HTMLDivElement | undefined;
  let component: Record<string, unknown> | undefined;

  function render(stopNatively: boolean) {
    const onInner = vi.fn();
    target = document.createElement("div");
    document.body.append(target);
    component = mount(DelegationFixture, { target, props: { stopNatively, onInner } });
    flushSync();
    const button = target.querySelector<HTMLButtonElement>("button.inner")!;
    return { button, onInner };
  }

  afterEach(() => {
    if (component) unmount(component);
    target?.remove();
    component = undefined;
    target = undefined;
  });

  it("silences a delegated handler on an element below the native stop", () => {
    const { button, onInner } = render(true);

    button.click();
    flushSync();

    expect(onInner).not.toHaveBeenCalled();
  });

  it("delivers the same click once the native stop is gone", () => {
    const { button, onInner } = render(false);

    button.click();
    flushSync();

    expect(onInner).toHaveBeenCalledTimes(1);
  });
});

/**
 * The invariant these pin is structural, so they are written to survive
 * renaming: they assert the query block carries no element reference and no
 * click handler of its own, rather than looking for one particular variable.
 */
describe("BlockEditor query block stays clickable", () => {
  function queryBlockTag() {
    const tag = source.match(/<div class="query-block"[^>]*>/);
    expect(tag, "the query block wrapper should still exist").not.toBeNull();
    return tag![0];
  }

  it("gives the query block wrapper no click handler and no element binding", () => {
    const tag = queryBlockTag();
    expect(tag).not.toContain("bind:this");
    expect(tag).not.toContain("onclick");
    expect(tag).not.toContain("onpointerdown");
  });

  it("registers no native listener against a query-block element reference", () => {
    // A reintroduced guard would need a `bind:this` on the wrapper to reach the
    // element; the tag assertion above covers the name-agnostic case, and this
    // keeps the original variable from quietly coming back too.
    expect(source).not.toContain("queryBlockEl");
  });

  it("keeps the query block's own controls on delegated handlers", () => {
    for (const control of ["query-edit-btn", "query-refresh", "query-more-btn"]) {
      const button = source.match(new RegExp(`class="${control}"[^>]*`));
      expect(button?.[0], `${control} should still have an onclick`).toContain("onclick=");
    }
  });

  it("relies on handleClick opting out of query blocks instead of stopping the event", () => {
    expect(source).toContain("if (!isEditing && !queryExpression) {");
  });
});
