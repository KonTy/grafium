import { readFileSync } from "node:fs";
import { join } from "node:path";
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import RenderedClickFixture from "./fixtures/RenderedClickFixture.svelte";

const source = readFileSync(join(process.cwd(), "src/components/BlockEditor.svelte"), "utf8");

/**
 * `.block-content` has always carried its `handleClick` (click-to-edit) as a
 * *native* listener. `.rendered-content` sits inside it and its handler is
 * written to claim clicks on task checkboxes, flashcard images, markdown table
 * headers, in-page anchors, page links and tags — every branch calls
 * `stopPropagation()` precisely so those clicks do not also drop the block into
 * the editor.
 *
 * While `.rendered-content` used a Svelte `onclick`, those calls were dead:
 * Svelte delegates, so the handler ran at the `#app`/`document` dispatcher long
 * after the ancestor's native `handleClick` had already switched to editing.
 * Registering `.rendered-content`'s handler natively puts it back in real
 * bubbling order, ahead of the ancestor, so the stops finally do what they say.
 *
 * These mount the real Svelte runtime because the whole behaviour is a property
 * of Svelte's event delegation, not of the markup.
 */
describe("rendered content claims its own clicks", () => {
  let target: HTMLDivElement | undefined;
  let component: Record<string, unknown> | undefined;

  function render(renderedNative: boolean) {
    const onAncestor = vi.fn();
    const onRendered = vi.fn();
    target = document.createElement("div");
    document.body.append(target);
    component = mount(RenderedClickFixture, {
      target,
      props: { renderedNative, onAncestor, onRendered },
    });
    flushSync();
    const checkbox = target.querySelector<HTMLInputElement>("input.task-checkbox")!;
    return { checkbox, onAncestor, onRendered };
  }

  afterEach(() => {
    if (component) unmount(component);
    target?.remove();
    component = undefined;
    target = undefined;
  });

  it("stops the ancestor's native handler when wired natively", () => {
    const { checkbox, onAncestor, onRendered } = render(true);

    checkbox.click();
    flushSync();

    expect(onRendered).toHaveBeenCalledTimes(1);
    expect(onAncestor).not.toHaveBeenCalled();
  });

  it("cannot stop that ancestor while wired as a delegated onclick", () => {
    const { checkbox, onAncestor, onRendered } = render(false);

    checkbox.click();
    flushSync();

    // Both run, ancestor first: the delegated stop arrives far too late. This
    // is the behaviour the native wiring replaces.
    expect(onAncestor).toHaveBeenCalledTimes(1);
    expect(onRendered).toHaveBeenCalledTimes(1);
  });
});

describe("BlockEditor keeps rendered-content handlers native", () => {
  it("does not put rendered-content's handlers back on delegated attributes", () => {
    const tag = source.match(/<div\s+class="rendered-content"[^>]*>/s)
      ?? source.match(/<div[^>]*class="rendered-content"[^>]*>/s);
    expect(tag, "the rendered-content container should still exist").not.toBeNull();
    for (const attribute of ["onclick=", "onpointerdown=", "onkeydown=", "oncontextmenu="]) {
      expect(tag![0], `rendered-content must not use ${attribute}`).not.toContain(attribute);
    }
  });

  it("registers them against the bound element instead", () => {
    expect(source).toContain('addEventListener("click", handleRenderedClick)');
    expect(source).toContain("const el = renderedEl;");
  });
});
