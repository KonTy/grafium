import { mount, unmount, flushSync, tick } from "svelte";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import AssetTrash from "./AssetTrash.svelte";
import type { AssetTrashResult, AssetTrashScan } from "../lib/api";
import { helpPageTitle } from "../lib/help";
import settingsHelp from "../../src-tauri/resources/welcome/pages/Help - Settings.md?raw";
import cleanupSource from "./AssetCleanup.svelte?raw";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const assets = [
  { filename: "assets/archive.zip", trash_filename: ".grafium/asset-trash/batch-a/assets/archive.zip", size: 2048, sha256: "a".repeat(64) },
  { filename: "assets/archive.zip", trash_filename: ".grafium/asset-trash/batch-b/assets/archive.zip", size: 123, sha256: "b".repeat(64) },
];
const preview: AssetTrashScan = { graph_path: "/synthetic/graph", assets };
let host: HTMLDivElement;
let component: ReturnType<typeof mount>;
const dialogMethods = ["showModal", "close"] as const;
const descriptors = dialogMethods.map((method) => Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, method));

beforeAll(() => {
  Object.defineProperties(HTMLDialogElement.prototype, {
    showModal: { configurable: true, value(this: HTMLDialogElement) { this.open = true; } },
    close: { configurable: true, value(this: HTMLDialogElement) { this.open = false; } },
  });
});
afterAll(() => {
  dialogMethods.forEach((method, index) => {
    if (descriptors[index]) Object.defineProperty(HTMLDialogElement.prototype, method, descriptors[index]!);
    else Reflect.deleteProperty(HTMLDialogElement.prototype, method);
  });
});
beforeEach(() => {
  vi.mocked(invoke).mockReset();
  host = document.createElement("div");
  document.body.append(host);
  component = mount(AssetTrash, { target: host });
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
async function list() {
  vi.mocked(invoke).mockResolvedValueOnce(preview);
  await click(button("List trash"));
  await vi.waitFor(() => expect(host.querySelectorAll(".trash-list li")).toHaveLength(2));
}
async function confirm(action: "restore" | "purge", result: AssetTrashResult) {
  vi.mocked(invoke).mockResolvedValueOnce(result);
  await click(button(action === "restore" ? "Confirm restore" : "Confirm permanent deletion"));
  await vi.waitFor(() => expect(host.querySelector("dialog")).toBeNull());
}

describe("persistent asset trash", () => {
  it("loads explicitly, shows original/trash paths and sizes, and reuses Settings F1 guidance", async () => {
    expect(invoke).not.toHaveBeenCalled();
    let resolve!: (value: AssetTrashScan) => void;
    vi.mocked(invoke).mockReturnValueOnce(new Promise<AssetTrashScan>((done) => { resolve = done; }));
    await click(button("List trash"));
    expect(button("Loading trash…").disabled).toBe(true);
    expect(invoke).toHaveBeenLastCalledWith("list_asset_trash", {});
    resolve(preview);
    await vi.waitFor(() => expect(host.querySelectorAll(".trash-list li")).toHaveLength(2));
    expect(host.textContent).toContain(assets[0].filename);
    expect(host.textContent).toContain(assets[0].trash_filename);
    expect(host.textContent).toContain(assets[1].trash_filename);
    expect(host.textContent).toContain(assets[0].sha256);
    expect(host.textContent).toContain(assets[1].sha256);
    expect(host.textContent).toContain("2.0 KB");
    expect(host.textContent).toContain("123 B");
    expect(button("Restore selected (0)…").disabled).toBe(true);
    expect(button("Permanently delete selected (0)…").disabled).toBe(true);
    expect(button("Refresh trash").closest("[data-help-context]")?.getAttribute("data-help-context")).toBe("settings");
    expect(helpPageTitle("settings")).toBe("Help - Settings");
    expect(cleanupSource).toContain("<AssetTrash />");
    expect(settingsHelp).toContain("Undo cannot recover permanently deleted attachments");
    expect(settingsHelp).toContain("**Never overwrite an existing original**");
  });

  it("focuses Cancel in permanent deletion confirmation; Cancel and Escape leave bytes untouched", async () => {
    await list();
    const trigger = button("Permanently delete all 2…");
    await click(trigger);
    const dialog = host.querySelector("dialog")!;
    expect(dialog.open).toBe(true);
    expect(dialog.getAttribute("aria-label")).toBe("Confirm permanent asset deletion");
    expect(dialog.textContent).toContain("Undo cannot recover permanently deleted attachments");
    expect(dialog.textContent).toContain("This cannot be undone");
    expect(dialog.textContent).toContain(preview.graph_path);
    expect(dialog.textContent).toContain(assets[0].trash_filename);
    expect(document.activeElement).toBe(button("Cancel"));
    expect(invoke).toHaveBeenCalledTimes(1);
    await click(button("Cancel"));
    expect(host.querySelector("dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger);
    await click(trigger);
    button("Cancel").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await tick();
    expect(host.querySelector("dialog")).toBeNull();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(host.querySelectorAll(".trash-list li")).toHaveLength(2);
  });

  it("restores only selected graph-bound snapshots and retains conflicts without optimistic success", async () => {
    await list();
    await click(checkbox(`Select trash ${assets[1].trash_filename}`));
    expect(checkbox("Select all trash").indeterminate).toBe(true);
    await click(button("Restore selected (1)…"));
    expect(host.querySelector("dialog")?.textContent).toContain("Existing originals are never overwritten");
    expect(host.querySelector("dialog")?.textContent).not.toContain(assets[0].trash_filename);
    expect(host.querySelector("dialog")?.textContent).toContain(assets[1].sha256);
    expect(host.querySelector("dialog")?.textContent).not.toContain(assets[0].sha256);
    let resolve!: (value: AssetTrashResult) => void;
    vi.mocked(invoke).mockReturnValueOnce(new Promise<AssetTrashResult>((done) => { resolve = done; }));
    await click(button("Confirm restore"));
    expect(invoke).toHaveBeenLastCalledWith("restore_trashed_assets", { graphPath: preview.graph_path, assets: [assets[1]] });
    expect(host.textContent).not.toContain("Restored 1 file");
    expect(host.querySelectorAll(".trash-list li")).toHaveLength(2);
    expect(button("Working…").disabled).toBe(true);
    expect(button("Cancel").disabled).toBe(true);
    expect(button("Refresh trash").disabled).toBe(true);
    host.querySelector("dialog")!.dispatchEvent(new Event("cancel", { cancelable: true }));
    await tick();
    expect(host.querySelector("dialog")).not.toBeNull();
    resolve({ restored: [], purged: [], errors: ["assets/archive.zip: original already exists"] });
    await vi.waitFor(() => expect(host.querySelector("dialog")).toBeNull());
    expect(host.querySelectorAll(".trash-list li")).toHaveLength(2);
    expect(host.textContent).toContain("Restored 0 files.");
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("original already exists");
    expect(checkbox(`Select trash ${assets[1].trash_filename}`).checked).toBe(true);
  });

  it("selects all but removes only confirmed restored trash ids, not same-name copies", async () => {
    await list();
    await click(checkbox("Select all trash"));
    await click(button("Restore selected (2)…"));
    await confirm("restore", { restored: [assets[0].trash_filename], purged: [], errors: ["second copy: conflict"] });
    expect(invoke).toHaveBeenLastCalledWith("restore_trashed_assets", { graphPath: preview.graph_path, assets });
    expect(host.querySelectorAll(".trash-list li")).toHaveLength(1);
    expect(host.querySelector(".trash-list")?.textContent).toContain(assets[1].trash_filename);
    expect(checkbox(`Select trash ${assets[1].trash_filename}`).checked).toBe(true);
    expect(host.textContent).toContain("Restored 1 file.");
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("second copy: conflict");
  });

  it("purges only selected ids and retains partial errors in the preview", async () => {
    await list();
    await click(checkbox("Select all trash"));
    await click(button("Permanently delete selected (2)…"));
    await confirm("purge", { restored: [], purged: [assets[1].trash_filename], errors: ["first copy: contents changed"] });
    expect(invoke).toHaveBeenLastCalledWith("purge_trashed_assets", { graphPath: preview.graph_path, assets });
    expect(host.querySelectorAll(".trash-list li")).toHaveLength(1);
    expect(host.querySelector(".trash-list")?.textContent).toContain(assets[0].trash_filename);
    expect(host.textContent).toContain("Permanently deleted 1 file.");
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("contents changed");
  });

  it("waits for confirmed all-files permanent deletion before clearing the preview", async () => {
    await list();
    await click(button("Permanently delete all 2…"));
    let resolve!: (value: AssetTrashResult) => void;
    vi.mocked(invoke).mockReturnValueOnce(new Promise<AssetTrashResult>((done) => { resolve = done; }));
    await click(button("Confirm permanent deletion"));
    expect(invoke).toHaveBeenLastCalledWith("purge_trashed_assets", { graphPath: preview.graph_path, assets });
    expect(host.querySelectorAll(".trash-list li")).toHaveLength(2);
    expect(host.textContent).not.toContain("Permanently deleted 2 files.");
    resolve({ restored: [], purged: assets.map((asset) => asset.trash_filename), errors: [] });
    await vi.waitFor(() => expect(host.querySelector("dialog")).toBeNull());
    expect(host.querySelector(".trash-list")).toBeNull();
    expect(host.textContent).toContain("No files remain in this trash preview.");
    expect(host.textContent).toContain("Permanently deleted 2 files.");
    expect(document.activeElement).toBe(button("Refresh trash"));
  });

  it.each(["restore", "purge"] as const)("retains every entry on a rejected %s and requires a refresh", async (action) => {
    await list();
    await click(button(action === "restore" ? "Restore all 2…" : "Permanently delete all 2…"));
    vi.mocked(invoke).mockRejectedValueOnce("graph changed");
    await click(button(action === "restore" ? "Confirm restore" : "Confirm permanent deletion"));
    expect(host.querySelector("dialog")).toBeNull();
    expect(host.querySelectorAll(".trash-list li")).toHaveLength(2);
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("graph changed");
    expect(host.textContent).not.toContain("Restored 2 files.");
    expect(host.textContent).not.toContain("Permanently deleted 2 files.");
    expect(button("Restore all 2…").disabled).toBe(true);
    expect(button("Permanently delete all 2…").disabled).toBe(true);
  });

  it("retains the old preview on list failure but disables stale operations", async () => {
    vi.mocked(invoke).mockRejectedValueOnce("unreadable trash");
    await click(button("List trash"));
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("unreadable trash");
    await list();
    vi.mocked(invoke).mockRejectedValueOnce("graph unavailable");
    await click(button("Refresh trash"));
    expect(host.querySelectorAll(".trash-list li")).toHaveLength(2);
    expect(button("Restore all 2…").disabled).toBe(true);
    expect(button("Permanently delete all 2…").disabled).toBe(true);
    expect(button("Refresh trash").disabled).toBe(false);
  });

  it("refreshes graph binding for subsequent operations while keeping graph-labelled receipts", async () => {
    await list();
    await click(button("Restore all 2…"));
    await confirm("restore", { restored: assets.map((asset) => asset.trash_filename), purged: [], errors: [] });
    vi.mocked(invoke).mockResolvedValueOnce({ graph_path: "/synthetic/other", assets: [assets[0]] });
    await click(button("Refresh trash"));
    await click(button("Restore all 1…"));
    expect(host.querySelector("dialog")?.textContent).toContain("/synthetic/other");
    await confirm("restore", { restored: [assets[0].trash_filename], purged: [], errors: [] });
    expect(invoke).toHaveBeenLastCalledWith("restore_trashed_assets", { graphPath: "/synthetic/other", assets: [assets[0]] });
    const receipts = [...host.querySelectorAll(".result")];
    expect(receipts[0].textContent).toContain(preview.graph_path);
    expect(receipts[1].textContent).toContain("/synthetic/other");
  });
});
