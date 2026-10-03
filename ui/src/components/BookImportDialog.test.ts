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

describe("book import explanations", () => {
  it("keeps details behind a help button while choices and errors stay visible", async () => {
    const saved = (["showModal", "close"] as const).map(method =>
      [method, Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, method)] as const);
    Object.defineProperties(HTMLDialogElement.prototype, {
      showModal: { configurable: true, value(this: HTMLDialogElement) { this.open = true; } },
      close: { configurable: true, value(this: HTMLDialogElement) { this.open = false; } },
    });
    const choose = vi.fn().mockRejectedValue(new Error("picker unavailable"));
    mounted = mount(BookImportDialog, { target: document.body, props: {
      onChooseFile: choose, onChooseFolder: vi.fn(), onClose: vi.fn(), onQueued: vi.fn(),
    } });
    flushSync();
    const visibleText = () => [...document.querySelectorAll(".book-import-dialog p")]
      .filter(p => !p.closest("[hidden], dialog")).map(p => p.textContent).join(" ");
    expect(visibleText()).not.toContain("DRM-protected");
    expect(visibleText()).not.toContain("Folder imports scan subfolders");
    expect(document.querySelector("[data-settings-help-text]")?.textContent).toContain("DRM-protected");
    const help = document.querySelector<HTMLButtonElement>('button[aria-label="Help: Importing books"]')!;
    help.click();
    await vi.waitFor(() => expect(document.querySelector("dialog")?.open).toBe(true));
    expect(document.querySelector("dialog")?.textContent).toContain("Folder imports scan subfolders");
    document.querySelector<HTMLButtonElement>("dialog button")!.click();
    flushSync();
    [...document.querySelectorAll("button")].find(b => b.textContent?.trim() === "Choose file")!.click();
    await vi.waitFor(() => expect(document.querySelector('[role="alert"]')?.textContent).toContain("picker unavailable"));
    expect(document.querySelector('[role="alert"]')?.closest("[hidden], dialog")).toBeNull();
    expect(document.querySelector(".actions .primary")?.textContent?.trim()).toBe("Add to Books");
    for (const [method, descriptor] of saved) {
      if (descriptor) Object.defineProperty(HTMLDialogElement.prototype, method, descriptor);
      else Reflect.deleteProperty(HTMLDialogElement.prototype, method);
    }
  });
});
