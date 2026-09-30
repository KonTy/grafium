import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
const native = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: native.invoke }));
import BookImportDialog from "./BookImportDialog.svelte";
let mounted: ReturnType<typeof mount> | undefined;
afterEach(async () => {
  if (mounted) await unmount(mounted);
  mounted = undefined; document.body.replaceChildren(); native.invoke.mockReset();
});
describe("book import conversion checkbox", () => {
  it.each([false, true])("queues the %s conversion choice without a mode dropdown", async convert => {
    native.invoke.mockResolvedValue("job");
    const queued = vi.fn(), choose = vi.fn().mockResolvedValue("/library/book.epub");
    mounted = mount(BookImportDialog, { target: document.body, props: {
      onChooseFile: choose, onChooseFolder: vi.fn(), onClose: vi.fn(), onQueued: queued,
    } });
    flushSync();
    const checkbox = document.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    expect(checkbox.checked).toBe(false);
    expect(checkbox.closest("label")?.textContent).toContain("Convert to editable Markdown");
    expect(document.querySelector("select")).toBeNull();
    if (convert) { checkbox.click(); flushSync(); }
    const button = (label: string) => [...document.querySelectorAll("button")].find(b => b.textContent?.trim() === label)!;
    button("Choose file").click();
    await vi.waitFor(() => expect(choose).toHaveBeenCalledWith(convert ? "markdown" : "original"));
    await vi.waitFor(() => expect(button(convert ? "Convert and import" : "Add to Books").disabled).toBe(false));
    button(convert ? "Convert and import" : "Add to Books").click();
    await vi.waitFor(() => expect(queued).toHaveBeenCalledOnce());
    expect(native.invoke).toHaveBeenCalledWith(convert ? "books_import_directory" : "books_import_originals",
      convert ? { sourceDir: "/library/book.epub" } : { sourcePaths: ["/library/book.epub"] });
  });
});
