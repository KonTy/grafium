import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import PendingMergeList from "./PendingMergeList.svelte";

let mounted: ReturnType<typeof mount> | undefined;

afterEach(async () => {
  if (mounted) await unmount(mounted);
  mounted = undefined;
  document.body.replaceChildren();
});

const labels = () => [...document.querySelectorAll("button")].map((b) => b.textContent?.replace(/\s+/g, " ").trim());

describe("notes waiting for a merge", () => {
  it("lists each held note with its marked spots and opens the one that is chosen", () => {
    const onOpen = vi.fn();
    const notes = [
      { rel_path: "pages/todo.md", conflicts: 2 },
      { rel_path: "journals/2026_01_02.md", conflicts: 1 },
    ];
    mounted = mount(PendingMergeList, { target: document.body, props: { notes, onOpen } });
    expect(labels()).toEqual(["pages/todo.md 2 spots to merge", "journals/2026_01_02.md 1 spot to merge"]);
    document.querySelector<HTMLButtonElement>('button[aria-label="Open journals/2026_01_02.md"]')!.click();
    flushSync();
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onOpen).toHaveBeenCalledWith(notes[1]);
  });

  it("says how to finish: edit the spots, delete the marker lines, sync again", () => {
    mounted = mount(PendingMergeList, {
      target: document.body,
      props: { notes: [{ rel_path: "pages/todo.md", conflicts: 1 }], onOpen: vi.fn() },
    });
    expect(document.body.textContent).toContain("Edit each marked spot, delete its marker lines, then sync again.");
  });
});
