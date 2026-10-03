import { afterEach, describe, expect, it, vi } from "vitest";
import { mount, unmount } from "svelte";
import ChatMessageBubble from "./ChatMessageBubble.svelte";

vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn() }));

let component: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (component) await unmount(component); component = undefined; document.body.replaceChildren(); });

describe("ChatMessageBubble Library citations", () => {
  it("renders Library source chips and opens them via the shared handler", () => {
    const onOpenLibrarySource = vi.fn();
    component = mount(ChatMessageBubble, { target: document.body, props: { message: {
      role: "assistant", content: "Use [1].", librarySources: [{
        index: 1, bookId: "book", title: "Repair video", kind: "video", trackId: "track", startMs: 65000, endMs: 90000,
        chapter: null, quote: null,
      }],
    }, onOpenLibrarySource } });
    const chip = document.querySelector<HTMLButtonElement>(".library-source-chip")!;
    expect(chip.textContent).toContain("Repair video");
    expect(chip.textContent).toContain("1:05");
    chip.click();
    expect(onOpenLibrarySource).toHaveBeenCalledWith(expect.objectContaining({ bookId: "book", startMs: 65000 }));
  });
});
