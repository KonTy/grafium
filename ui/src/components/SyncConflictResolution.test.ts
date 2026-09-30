import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));
import SyncConflictResolution from "./SyncConflictResolution.svelte";

let mounted: ReturnType<typeof mount> | undefined;
const conflict = { target_id: "usb", target_name: "USB", rel_path: "assets/binary.png", backup_path: "assets/binary.conflict_hash.png", recorded_at: 1 };
const success = { pushed: ["assets/binary.png"], pulled: [], conflicts: [], deleted_remote: [], deleted_local: [], errors: [] };
const button = (text: string) => [...document.querySelectorAll("button")].find(b => b.textContent?.trim() === text)!;

afterEach(async () => {
  if (mounted) await unmount(mounted);
  mounted = undefined;
  document.body.replaceChildren();
  ipc.invoke.mockReset();
});

describe("explicit conflict choice", () => {
  it("requires an honest confirmation and sends unchanged binary revisions", async () => {
    const onResolved = vi.fn();
    const onClose = vi.fn();
    ipc.invoke.mockResolvedValueOnce({ graph_path: "/graph", rel_path: conflict.rel_path, local_hash: "png=======hash", remote_hash: "remote", local_size: 42, remote_size: 80 });
    ipc.invoke.mockResolvedValueOnce(success);
    mounted = mount(SyncConflictResolution, { target: document.body, props: { conflict, onClose, onResolved } });
    await vi.waitFor(() => expect(document.querySelectorAll('input[type="radio"]').length).toBe(2));
    expect(document.querySelector('[role="dialog"]')?.getAttribute("data-help-context")).toBe("sync");
    expect(button("Confirm choice").disabled).toBe(true);
    (document.querySelector('input[value="local"]') as HTMLInputElement).click();
    flushSync();
    expect(document.body.textContent).toContain("Confirm replacing the remote file");
    expect(ipc.invoke).toHaveBeenCalledTimes(1);
    button("Confirm choice").click();
    await vi.waitFor(() => expect(onResolved).toHaveBeenCalledTimes(1));
    expect(ipc.invoke).toHaveBeenLastCalledWith("sync_resolve_conflict", {
      graphPath: "/graph", targetId: "usb", relPath: conflict.rel_path, expectedLocalHash: "png=======hash", expectedRemoteHash: "remote", chosen: "local",
    });
  });

  it("shows deletion explicitly and refuses to retry a stale choice", async () => {
    const onResolved = vi.fn();
    ipc.invoke.mockResolvedValueOnce({ graph_path: "/graph", rel_path: conflict.rel_path, local_hash: null, remote_hash: "remote", local_size: null, remote_size: 0 });
    ipc.invoke.mockRejectedValueOnce(new Error("Sync revision changed"));
    mounted = mount(SyncConflictResolution, { target: document.body, props: { conflict, onClose: vi.fn(), onResolved } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("Local: deleted"));
    expect(document.body.textContent).toContain("Remote: 0 bytes");
    (document.querySelector('input[value="local"]') as HTMLInputElement).click(); flushSync();
    expect(document.body.textContent).toContain("Confirm deletion of the remote file");
    button("Confirm choice").click();
    await vi.waitFor(() => expect(document.querySelector('[role="alert"]')?.textContent).toContain("Reload versions"));
    expect(button("Confirm choice").disabled).toBe(true);
    expect(onResolved).not.toHaveBeenCalled();
    expect(ipc.invoke).toHaveBeenLastCalledWith("sync_resolve_conflict", {
      graphPath: "/graph", targetId: "usb", relPath: conflict.rel_path, expectedLocalHash: null, expectedRemoteHash: "remote", chosen: "local",
    });
  });

  it("keeps reconciliation errors visible even when the file choice was applied", async () => {
    const onResolved = vi.fn();
    ipc.invoke.mockResolvedValueOnce({ graph_path: "/graph", rel_path: conflict.rel_path, local_hash: "l", remote_hash: "r", local_size: 1, remote_size: 2 });
    ipc.invoke.mockResolvedValueOnce({ ...success, errors: ["Reconcile failed"] });
    mounted = mount(SyncConflictResolution, { target: document.body, props: { conflict, onClose: vi.fn(), onResolved } });
    await vi.waitFor(() => expect(document.querySelectorAll('input[type="radio"]').length).toBe(2));
    (document.querySelector('input[value="remote"]') as HTMLInputElement).click(); flushSync();
    button("Confirm choice").click();
    await vi.waitFor(() => expect(document.querySelector('[role="alert"]')?.textContent).toContain("Reconcile failed"));
    expect(onResolved).not.toHaveBeenCalled();
    button("Close").click(); flushSync();
    expect(onResolved).toHaveBeenCalledTimes(1);
  });
});
