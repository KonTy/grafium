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
  /** Adjacent annotation files combined without selecting a conflicting note head. */
  merged?: string[];
  /** Books with unresolved annotation heads, not file-level local/remote conflicts. */
  annotation_conflicts?: string[];
  deleted_remote: string[];
  deleted_local: string[];
  errors: string[];
}
export interface SyncCompleted {
  target_name: string;
  pushed: number;
  pulled: number;
  conflicts: number;
  merged?: number;
  annotation_conflicts?: number;
  deleted_local?: number;
  deleted_remote?: number;
  errors?: number;
}

const annotationMergeNotice = (count: number) =>
  `${count} ${count === 1 ? "book has" : "books have"} notes to merge; open Book notes`;

export interface SyncConflict {
  target_id: string;
  target_name: string;
  rel_path: string;
  backup_path: string;
  recorded_at: number;
}

export interface SyncConflictState {
  graph_path: string;
  rel_path: string;
  /** null means deleted; an empty file has a non-null hash and size 0. */
  local_hash: string | null;
  remote_hash: string | null;
  local_size: number | null;
  remote_size: number | null;
}

export type SyncConflictSide = "local" | "remote";

export function getSyncConflictState(targetId: string, relPath: string): Promise<SyncConflictState> {
  return invoke("sync_get_conflict_state", { targetId, relPath });
}

export function resolveSyncConflict(
  targetId: string, snapshot: SyncConflictState, chosen: SyncConflictSide,
): Promise<SyncResult> {
  return invoke("sync_resolve_conflict", {
    graphPath: snapshot.graph_path, targetId, relPath: snapshot.rel_path,
    expectedLocalHash: snapshot.local_hash, expectedRemoteHash: snapshot.remote_hash, chosen,
  });
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

export function listSyncConflicts(): Promise<SyncConflict[]> {
  return invoke("sync_list_conflicts");
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
  if (result.merged?.length) parts.push(`↔ ${result.merged.length} annotation files merged`);
  if (result.conflicts.length) parts.push(`⚡ ${result.conflicts.length} conflicts`);
  if (result.annotation_conflicts?.length) parts.push(annotationMergeNotice(new Set(result.annotation_conflicts).size));
  if (result.deleted_remote.length) parts.push(`🗑 ${result.deleted_remote.length} deleted remote`);
  if (result.deleted_local.length) parts.push(`🗑 ${result.deleted_local.length} deleted local`);
  if (result.errors.length) parts.push(`❌ ${result.errors.length} errors`);
  return parts.length ? parts.join(", ") : "Everything in sync ✓";
}


export async function initSyncMonitor(): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = [];
  try {
    unlisteners.push(await listen<{ target_id: string; target_name: string }>(
      "sync-target-available",
      (event) => {
        showToast(`Sync target connected: ${event.payload.target_name}`, "info");
      }
    ));

    unlisteners.push(await listen<SyncCompleted>("sync-completed", (event) => {
      const { target_name, pushed, pulled, conflicts, merged = 0, annotation_conflicts = 0,
        deleted_local = 0, deleted_remote = 0, errors = 0 } = event.payload;
      const parts: string[] = [];
      if (pushed) parts.push(`↑ ${pushed} pushed`);
      if (pulled) parts.push(`↓ ${pulled} pulled`);
      if (merged) parts.push(`↔ ${merged} annotation files merged`);
      if (conflicts) parts.push(`⚡ ${conflicts} conflicts`);
      if (annotation_conflicts) parts.push(annotationMergeNotice(annotation_conflicts));
      if (deleted_local) parts.push(`🗑 ${deleted_local} deleted local`);
      if (deleted_remote) parts.push(`🗑 ${deleted_remote} deleted remote`);
      if (errors) parts.push(`❌ ${errors} errors`);
      const summary = parts.length ? parts.join(", ") : "Everything in sync";

      showToast(
        `Sync complete (${target_name}): ${summary}`,
        conflicts > 0 || errors > 0 ? "error" : annotation_conflicts > 0 ? "info" : "success"
      );
    }));

    unlisteners.push(await listen<{ target_name: string; error: string }>(
      "sync-error",
      (event) => {
        showToast(`Auto-sync failed (${event.payload.target_name}): ${event.payload.error}`, "error");
      }
    ));
  } catch (error) {
    for (const unlisten of unlisteners) unlisten();
    throw error;
  }

  return () => {
    for (const unlisten of unlisteners) unlisten();
  };
}
