import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
import ReferencePanel from "./ReferencePanel.svelte";
import { helpPageTitle, isHelpContext } from "../lib/help";
import { resetSyncActivity, syncActivity } from "../lib/syncActivity.svelte";

let mounted: ReturnType<typeof mount> | undefined;

const wholeFile = { target_id: "usb", target_name: "USB", rel_path: "assets/a.png", backup_path: "assets/a.conflict.png", recorded_at: 1 };
const held = { rel_path: "pages/todo.md", conflicts: 2 };

function backend(answers: { conflicts?: unknown[]; pendingMerges?: unknown[] }) {
  ipc.invoke.mockImplementation(async (command: string, args: Record<string, unknown>) => {
    if (command === "sync_list_conflicts") return answers.conflicts ?? [];
    if (command === "sync_list_pending_merges") return answers.pendingMerges ?? [];
    if (command === "get_page") return { id: `page-for-${args.title}`, title: args.title };
    throw new Error(`unexpected command ${command}`);
  });
}

function openConflictsTab(onNavigate = vi.fn()) {
  mounted = mount(ReferencePanel, { target: document.body, props: { visible: true, initialTab: "conflicts", onNavigate } });
  return onNavigate;
}

afterEach(async () => {
  if (mounted) await unmount(mounted);
  mounted = undefined;
  document.body.replaceChildren();
  ipc.invoke.mockReset();
  resetSyncActivity();
});

describe("Conflicts tab", () => {
  it("lists notes holding merge markers and opens the chosen note", async () => {
    backend({ pendingMerges: [held] });
    const onNavigate = openConflictsTab();
    await vi.waitFor(() => expect(document.body.textContent).toContain("pages/todo.md"));
    expect(document.body.textContent).toContain("2 spots to merge");
    expect(document.body.textContent).not.toContain("No unresolved sync conflicts.");
    // Editing the note is how a held note is resolved, so the whole-file rule must not be shown.
    expect(document.body.textContent).not.toContain("Editing a note alone does not resolve");
    expect(syncActivity.conflictCount).toBe(1);

    document.querySelector<HTMLButtonElement>('button[aria-label="Open pages/todo.md"]')!.click();
    await vi.waitFor(() => expect(onNavigate).toHaveBeenCalledWith({ id: "page-for-todo" }));
  });

  it("shows whole-file conflicts and held notes together and counts both", async () => {
    backend({ conflicts: [wholeFile], pendingMerges: [held] });
    openConflictsTab();
    await vi.waitFor(() => expect(document.body.textContent).toContain("assets/a.png"));
    expect(document.body.textContent).toContain("pages/todo.md");
    expect(document.body.textContent).toContain("Editing a note alone does not resolve its conflict.");
    expect(document.querySelector('button[aria-label="Resolve assets/a.png"]')).not.toBeNull();
    flushSync();
    expect(syncActivity.conflictCount).toBe(2);
    expect(document.querySelector('[role="tab"][aria-selected="true"]')?.textContent?.trim()).toBe("Conflicts (2)");
  });

  it("says so plainly when nothing is waiting", async () => {
    backend({});
    openConflictsTab();
    await vi.waitFor(() => expect(document.body.textContent).toContain("No unresolved sync conflicts."));
    expect(document.body.textContent).not.toContain("Editing a note alone");
  });

  it("routes F1 to the Sync help from anywhere in the tab, and only there", async () => {
    backend({ pendingMerges: [held] });
    openConflictsTab();
    await vi.waitFor(() => expect(document.body.textContent).toContain("pages/todo.md"));
    const panel = document.querySelector("aside.reference-panel")!;
    expect(isHelpContext(panel.getAttribute("data-help-context"))).toBe(true);
    expect(helpPageTitle("sync")).toBe("Help - Sync");
    expect(panel.getAttribute("data-help-context")).toBe("sync");
    // The tab button is inside the panel, so F1 there finds the same context.
    expect(document.querySelector('[role="tab"][aria-selected="true"]')?.closest("[data-help-context]")).toBe(panel);

    document.querySelector<HTMLButtonElement>('[role="tablist"] button:first-child')!.click();
    flushSync();
    expect(document.querySelector("aside.reference-panel")!.hasAttribute("data-help-context")).toBe(false);
  });

  it("explains an unavailable note instead of failing silently", async () => {
    backend({ pendingMerges: [held] });
    ipc.invoke.mockImplementation(async (command: string) => {
      if (command === "sync_list_pending_merges") return [held];
      if (command === "sync_list_conflicts") return [];
      throw new Error("no such page");
    });
    openConflictsTab();
    await vi.waitFor(() => expect(document.body.textContent).toContain("pages/todo.md"));
    document.querySelector<HTMLButtonElement>('button[aria-label="Open pages/todo.md"]')!.click();
    await vi.waitFor(() => expect(document.querySelector('[role="alert"]')?.textContent).toContain("This note is unavailable."));
  });
});
