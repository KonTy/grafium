import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { showToast } from "./toast.svelte";

export interface SyncTarget {
  id: string;
  name: string;
  backend_type: string;
  auto_sync: boolean;
  config: Record<string, unknown>;
}

export interface SyncResult {
  pushed: string[];
  pulled: string[];
  conflicts: string[];
  deleted_remote: string[];
  deleted_local: string[];
  errors: string[];
}

export function listSyncTargets(): Promise<SyncTarget[]> {
  return invoke("sync_list_targets");
}

export function addFilesystemTarget(name: string, path: string): Promise<void> {
  return invoke("sync_add_filesystem_target", { name, path });
}

export function addWebdavTarget(
  name: string,
  url: string,
  username: string,
  password: string
): Promise<void> {
  return invoke("sync_add_webdav_target", { name, url, username, password });
}

export function removeSyncTarget(targetId: string): Promise<void> {
  return invoke("sync_remove_target", { targetId });
}

export function runSyncTarget(targetId: string): Promise<SyncResult> {
  return invoke("sync_run", { targetId });
}

/**
 * Renders a sync result as a short status line.
 *
 * Kept here rather than in a component because both the app menu and the
 * settings page report sync outcomes, and a summary that silently omitted a
 * category (conflicts or errors especially) would be actively misleading.
 */
export function summarizeSyncResult(result: SyncResult): string {
  const parts: string[] = [];
  if (result.pushed.length) parts.push(`↑ ${result.pushed.length} pushed`);
  if (result.pulled.length) parts.push(`↓ ${result.pulled.length} pulled`);
  if (result.conflicts.length) parts.push(`⚡ ${result.conflicts.length} conflicts`);
  if (result.deleted_remote.length) parts.push(`🗑 ${result.deleted_remote.length} deleted remote`);
  if (result.deleted_local.length) parts.push(`🗑 ${result.deleted_local.length} deleted local`);
  if (result.errors.length) parts.push(`❌ ${result.errors.length} errors`);
  return parts.length ? parts.join(", ") : "Everything in sync ✓";
}


export async function initSyncMonitor(): Promise<UnlistenFn> {
  const unlistenAvailable = await listen<{ target_id: string; target_name: string }>(
    "sync-target-available",
    (event) => {
      showToast(`Sync target connected: ${event.payload.target_name}`, "info");
    }
  );

  const unlistenCompleted = await listen<{
    target_name: string;
    pushed: number;
    pulled: number;
    conflicts: number;
  }>("sync-completed", (event) => {
    const { target_name, pushed, pulled, conflicts } = event.payload;
    const parts: string[] = [];
    if (pushed) parts.push(`↑ ${pushed} pushed`);
    if (pulled) parts.push(`↓ ${pulled} pulled`);
    if (conflicts) parts.push(`⚡ ${conflicts} conflicts`);
    const summary = parts.length ? parts.join(", ") : "Everything in sync";
    
    showToast(
      `Auto-sync complete (${target_name}): ${summary}`, 
      conflicts > 0 ? "error" : "success"
    );
  });

  const unlistenError = await listen<{ target_name: string; error: string }>(
    "sync-error",
    (event) => {
      showToast(`Auto-sync failed (${event.payload.target_name}): ${event.payload.error}`, "error");
    }
  );

  return () => {
    unlistenAvailable();
    unlistenCompleted();
    unlistenError();
  };
}
