import { mount, unmount, flushSync, tick } from "svelte";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import AssetCleanup from "./AssetCleanup.svelte";
import { helpPageTitle } from "../lib/help";
import type { AssetCleanupResult, AssetCleanupScan } from "../lib/api";
import settingsHelp from "../../src-tauri/resources/welcome/pages/Help - Settings.md?raw";
import settingsSource from "./Settings.svelte?raw";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const assets = [
  { filename: "assets/archive.zip", size: 2048, sha256: "a".repeat(64) },
  { filename: "pages/book/assets/image.png", size: 123, sha256: "b".repeat(64) },
];
const preview: AssetCleanupScan = { graph_path: "/synthetic/graph", assets };
let host: HTMLDivElement;
let component: ReturnType<typeof mount>;

const dialogMethods = ["showModal", "close"] as const;
const originalDescriptors = dialogMethods.map((method) => Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, method));
beforeAll(() => {
  Object.defineProperties(HTMLDialogElement.prototype, {
    showModal: { configurable: true, value(this: HTMLDialogElement) { this.open = true; } },
    close: { configurable: true, value(this: HTMLDialogElement) { this.open = false; } },
  });
});
afterAll(() => {
  dialogMethods.forEach((method, index) => {
    const descriptor = originalDescriptors[index];
    if (descriptor) Object.defineProperty(HTMLDialogElement.prototype, method, descriptor);
    else Reflect.deleteProperty(HTMLDialogElement.prototype, method);
  });
});

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  host = document.createElement("div");
  document.body.append(host);
  component = mount(AssetCleanup, { target: host });
  flushSync();
});

afterEach(async () => {
  await unmount(component);
  host.remove();
  vi.restoreAllMocks();
});

function button(text: string, root: ParentNode = host) {
  const found = [...root.querySelectorAll("button")].find((item) => item.textContent?.trim() === text);
  expect(found, `button: ${text}`).toBeDefined();
  return found!;
}

function checkbox(name: string) {
  return [...host.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')]
    .find((item) => item.getAttribute("aria-label") === name)!;
}

async function click(element: HTMLElement) {
  element.focus();
  element.click();
  await tick();
  flushSync();
}

async function scan() {
  vi.mocked(invoke).mockResolvedValueOnce(preview);
  await click(button("Scan for orphaned assets"));
  await vi.waitFor(() => expect(host.textContent).toContain("2 candidate files"));
}

async function confirm(result: AssetCleanupResult) {
  vi.mocked(invoke).mockResolvedValueOnce(result);
  await click(button("Confirm move to trash"));
  await vi.waitFor(() => expect(host.querySelector("dialog")).toBeNull());
}

describe("recoverable asset cleanup", () => {
  it("scans explicitly, shows busy state, full ZIP paths and sizes, and uses Settings F1 help", async () => {
    expect(invoke).not.toHaveBeenCalled();
    expect(host.textContent).toContain("Save pending edits first");
    expect(host.textContent).toContain("No disk space is freed");
    expect(host.textContent).toContain("References inside binary archives or books are not inspected");
    let resolve!: (value: AssetCleanupScan) => void;
    vi.mocked(invoke).mockReturnValueOnce(new Promise<AssetCleanupScan>((done) => { resolve = done; }));
    await click(button("Scan for orphaned assets"));
    expect(button("Scanning…").disabled).toBe(true);
    expect(invoke).toHaveBeenCalledWith("find_orphaned_assets", {});
    resolve(preview);
    await tick();
    await vi.waitFor(() => expect(host.textContent).toContain("assets/archive.zip"));
    expect(host.textContent).toContain("pages/book/assets/image.png");
    expect(host.textContent).toContain("2.0 KB");
    expect(host.textContent).toContain("123 B");
    expect(checkbox("Select all candidates").checked).toBe(false);
    expect(button("Move selected (0) to trash…").disabled).toBe(true);
    expect(button("Re-scan").closest("[data-help-context]")?.getAttribute("data-help-context")).toBe("settings");
    expect(helpPageTitle("settings")).toBe("Help - Settings");
    expect(settingsSource).toContain('<AssetCleanup />');
    expect(settingsHelp).toContain("## Asset Cleanup");
    expect(settingsHelp).toContain(".grafium/asset-trash/");
    expect(settingsHelp).toContain("Never overwrite an");
    expect(settingsHelp).toContain("References inside binary archives or books are not inspected");
  });

  it("requires confirmation for one file and lets Cancel or Escape preserve the preview", async () => {
    await scan();
    const move = host.querySelector<HTMLButtonElement>('[aria-label="Move assets/archive.zip to trash"]')!;
    await click(move);
    const dialog = host.querySelector("dialog")!;
    expect(dialog.open).toBe(true);
    expect(dialog.textContent).toContain("assets/archive.zip");
    expect(dialog.textContent).not.toContain("pages/book/assets/image.png");
    expect(document.activeElement).toBe(button("Cancel"));
    expect(invoke).toHaveBeenCalledTimes(1);
    await click(button("Cancel"));
    expect(host.querySelector("dialog")).toBeNull();
    expect(document.activeElement).toBe(move);
    await click(move);
    button("Cancel").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await tick();
    expect(host.querySelector("dialog")).toBeNull();
    expect(host.querySelectorAll(".asset-list li")).toHaveLength(2);
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("sends only selected snapshots with the scanned graph and retains per-file refusals", async () => {
    await scan();
    await click(checkbox("Select assets/archive.zip"));
    expect(checkbox("Select all candidates").indeterminate).toBe(true);
    await click(button("Move selected (1) to trash…"));
    await confirm({ moved: [], trash_path: null, errors: ["assets/archive.zip: now referenced"] });
    expect(invoke).toHaveBeenLastCalledWith("trash_assets", { graphPath: preview.graph_path, assets: [assets[0]] });
    expect(host.querySelectorAll(".asset-list li")).toHaveLength(2);
    expect(host.textContent).toContain("Moved 0 files to trash.");
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("assets/archive.zip: now referenced");
    expect(checkbox("Select assets/archive.zip").checked).toBe(true);
  });

  it("selects all and removes only successful moves while showing recovery paths and errors", async () => {
    await scan();
    await click(checkbox("Select all candidates"));
    expect(checkbox("Select assets/archive.zip").checked).toBe(true);
    expect(checkbox("Select pages/book/assets/image.png").checked).toBe(true);
    await click(button("Move selected (2) to trash…"));
    await confirm({
      moved: [assets[0].filename],
      trash_path: "/synthetic/graph/.grafium/asset-trash/batch-id",
      errors: ["pages/book/assets/image.png: file changed"],
    });
    expect(invoke).toHaveBeenLastCalledWith("trash_assets", { graphPath: preview.graph_path, assets });
    expect(host.querySelectorAll(".asset-list li")).toHaveLength(1);
    expect(checkbox("Select pages/book/assets/image.png").checked).toBe(true);
    expect(host.textContent).toContain("Moved 1 file to trash.");
    expect(host.textContent).toContain("/synthetic/graph/.grafium/asset-trash/batch-id");
    expect(host.textContent).toContain("List trash or Refresh trash");
    expect(host.textContent).toContain("Never overwrite an existing original");
    expect(host.textContent).toContain("recovery copies even when the original was not moved");
    expect(host.textContent).toContain("pages/book/assets/image.png: file changed");
  });

  it("requires explicit all-files confirmation and prevents repeat actions while moving", async () => {
    await scan();
    await click(button("Move all 2 to trash…"));
    expect(host.querySelector("dialog")?.textContent).toContain("Move 2 files to trash?");
    expect(invoke).toHaveBeenCalledTimes(1);
    let resolve!: (value: AssetCleanupResult) => void;
    vi.mocked(invoke).mockReturnValueOnce(new Promise<AssetCleanupResult>((done) => { resolve = done; }));
    await click(button("Confirm move to trash"));
    expect(button("Moving…").disabled).toBe(true);
    expect(button("Cancel").disabled).toBe(true);
    expect(button("Re-scan").disabled).toBe(true);
    host.querySelector("dialog")!.dispatchEvent(new Event("cancel", { cancelable: true }));
    await tick();
    expect(host.querySelector("dialog")).not.toBeNull();
    expect(invoke).toHaveBeenCalledTimes(2);
    resolve({ moved: assets.map((asset) => asset.filename), trash_path: "/synthetic/trash/all", errors: [] });
    await vi.waitFor(() => expect(host.querySelector("dialog")).toBeNull());
    expect(host.textContent).toContain("Moved 2 files to trash.");
    expect(host.textContent).toContain("No candidates remain in this preview.");
    expect(host.querySelector(".asset-list")).toBeNull();
  });

  it("shows scan failures without discarding a previous preview or allowing stale actions", async () => {
    vi.mocked(invoke).mockRejectedValueOnce("unreadable source");
    await click(button("Scan for orphaned assets"));
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("unreadable source");
    await scan();
    vi.mocked(invoke).mockRejectedValueOnce("cannot read graph");
    await click(button("Re-scan"));
    expect(host.querySelectorAll(".asset-list li")).toHaveLength(2);
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("cannot read graph");
    expect(button("Move all 2 to trash…").disabled).toBe(true);
    expect(button("Re-scan").disabled).toBe(false);
  });

  it("keeps every candidate on a rejected move and requires a fresh scan", async () => {
    await scan();
    await click(button("Move all 2 to trash…"));
    vi.mocked(invoke).mockRejectedValueOnce("graph changed");
    await click(button("Confirm move to trash"));
    expect(host.querySelector("dialog")).toBeNull();
    expect(host.querySelectorAll(".asset-list li")).toHaveLength(2);
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("graph changed");
    expect(host.textContent).not.toContain("Moved 2 files");
    expect(button("Move all 2 to trash…").disabled).toBe(true);
  });

  it("keeps recovery receipts after rescanning and updates the graph-bound preview", async () => {
    await scan();
    await click(host.querySelector<HTMLButtonElement>('[aria-label="Move assets/archive.zip to trash"]')!);
    await confirm({ moved: [assets[0].filename], trash_path: "/synthetic/trash/first", errors: [] });
    vi.mocked(invoke).mockResolvedValueOnce({ graph_path: "/synthetic/other", assets: [assets[1]] });
    await click(button("Re-scan"));
    expect(host.textContent).toContain("/synthetic/trash/first");
    await click(button("Move all 1 to trash…"));
    await confirm({ moved: [assets[1].filename], trash_path: "/synthetic/trash/second", errors: [] });
    expect(invoke).toHaveBeenLastCalledWith("trash_assets", { graphPath: "/synthetic/other", assets: [assets[1]] });
    expect(host.textContent).toContain("/synthetic/trash/first");
    expect(host.textContent).toContain("/synthetic/trash/second");
  });
});
