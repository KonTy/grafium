<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { themes, applyTheme, getThemeById } from "../lib/themes";
  import { getAppTheme, setAppTheme, getSmplosTheme, getAppVersion, findOrphanedAssets, deleteAssets, getGraphInfo, reindexCurrent } from "../lib/api";
  import type { OrphanedAsset } from "../lib/api";
  import { keymap_manager } from "../lib/keymap";
  import type { Shortcut } from "../lib/keymap";
  import AISettings from "./AISettings.svelte";

  interface SyncTarget {
    id: string;
    name: string;
    backend_type: string;
    auto_sync: boolean;
    config: any;
  }

  let currentThemeId = $state("auto");
  let smplosThemeName = $state<string | null>(null);
  let appVersion = $state("...");
  let graphPath = $state("...");
  let reindexing = $state(false);
  let reindexStatus = $state("");

  async function reindexGraphNow() {
    reindexing = true;
    reindexStatus = "";
    try {
      await reindexCurrent();
      reindexStatus = "Re-index complete.";
    } catch (e) {
      reindexStatus = `Re-index failed: ${e}`;
    } finally {
      reindexing = false;
    }
  }

  // Asset cleanup state
  let orphanedAssets = $state<OrphanedAsset[]>([]);
  let assetScanDone = $state(false);
  let assetDeleting = $state(false);

  async function scanOrphanedAssets() {
    try {
      orphanedAssets = await findOrphanedAssets();
      assetScanDone = true;
    } catch (e) {
      console.error("Failed to scan assets:", e);
    }
  }

  async function deleteAllOrphans() {
    if (orphanedAssets.length === 0) return;
    assetDeleting = true;
    try {
      await deleteAssets(orphanedAssets.map((a) => a.filename));
      orphanedAssets = [];
    } catch (e) {
      console.error("Failed to delete assets:", e);
    } finally {
      assetDeleting = false;
    }
  }

  async function deleteSingleOrphan(filename: string) {
    assetDeleting = true;
    try {
      await deleteAssets([filename]);
      orphanedAssets = orphanedAssets.filter((a) => a.filename !== filename);
    } catch (e) {
      console.error("Failed to delete asset:", e);
    } finally {
      assetDeleting = false;
    }
  }

  function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  // Sync state
  let syncTargets = $state<SyncTarget[]>([]);
  let syncLoading = $state(false);
  let syncMessage = $state("");
  let showAddSync = $state(false);
  let addSyncType: "filesystem" | "webdav" = $state("filesystem");
  let addSyncName = $state("");
  let addSyncPath = $state("");
  let addSyncUrl = $state("");
  let addSyncUsername = $state("");
  let addSyncPassword = $state("");

  $effect(() => {
    loadCurrentTheme();
    loadSyncTargets();
    getAppVersion().then((v) => (appVersion = v)).catch(() => {});
    getGraphInfo().then((info) => (graphPath = info.path)).catch(() => {});
  });

  async function loadCurrentTheme() {
    try {
      const [appTheme, smplos] = await Promise.all([getAppTheme(), getSmplosTheme()]);
      currentThemeId = appTheme;
      smplosThemeName = smplos;
    } catch (e) {
      console.error("Failed to load theme settings:", e);
    }
  }

  function autoSwatches(): { accent: string; bg: string; fg: string } {
    if (smplosThemeName) {
      const t = getThemeById(smplosThemeName);
      if (t) return { accent: t.colors.accent, bg: t.colors.bgPrimary, fg: t.colors.textPrimary };
    }
    return { accent: "#89b4fa", bg: "#1e1e2e", fg: "#cdd6f4" };
  }

  function resolvedThemeId(): string {
    if (currentThemeId === "auto") {
      return smplosThemeName ?? "catppuccin";
    }
    return currentThemeId;
  }

  async function selectTheme(themeId: string) {
    currentThemeId = themeId;
    const resolved = resolvedThemeId();
    const theme = getThemeById(resolved);
    if (theme) {
      applyTheme(theme.colors);
    }
    try {
      await setAppTheme(themeId);
    } catch (e) {
      console.error("Failed to save theme:", e);
    }
  }

  // Sync functions
  async function loadSyncTargets() {
    try {
      syncTargets = await invoke("sync_list_targets");
    } catch (e) {
      syncTargets = [];
    }
  }

  async function addSyncTarget() {
    syncLoading = true;
    syncMessage = "";
    try {
      if (addSyncType === "filesystem") {
        await invoke("sync_add_filesystem_target", { name: addSyncName, path: addSyncPath });
      } else {
        await invoke("sync_add_webdav_target", {
          name: addSyncName,
          url: addSyncUrl,
          username: addSyncUsername,
          password: addSyncPassword,
        });
      }
      showAddSync = false;
      addSyncName = "";
      addSyncPath = "";
      addSyncUrl = "";
      addSyncUsername = "";
      addSyncPassword = "";
      await loadSyncTargets();
      syncMessage = "Target added successfully";
    } catch (e: any) {
      syncMessage = `Error: ${e}`;
    } finally {
      syncLoading = false;
    }
  }

  async function removeSyncTarget(id: string) {
    try {
      await invoke("sync_remove_target", { targetId: id });
      await loadSyncTargets();
    } catch (e: any) {
      syncMessage = `Error: ${e}`;
    }
  }

  async function runSync(targetId: string) {
    syncLoading = true;
    syncMessage = "";
    try {
      const result: any = await invoke("sync_run", { targetId });
      const parts = [];
      if (result.pushed.length) parts.push(`↑ ${result.pushed.length} pushed`);
      if (result.pulled.length) parts.push(`↓ ${result.pulled.length} pulled`);
      if (result.conflicts.length) parts.push(`⚡ ${result.conflicts.length} conflicts`);
      if (result.deleted_remote.length) parts.push(`🗑 ${result.deleted_remote.length} deleted remote`);
      if (result.deleted_local.length) parts.push(`🗑 ${result.deleted_local.length} deleted local`);
      if (result.errors.length) parts.push(`❌ ${result.errors.length} errors`);
      syncMessage = parts.length ? parts.join(", ") : "Everything in sync ✓";
    } catch (e: any) {
      syncMessage = `Sync failed: ${e}`;
    } finally {
      syncLoading = false;
    }
  }

  // Keymap
  function getShortcutsByCategory(): Map<string, Shortcut[]> {
    const shortcuts = keymap_manager.getShortcuts();
    const map = new Map<string, Shortcut[]>();
    for (const s of shortcuts) {
      const cat = s.category || "other";
      if (!map.has(cat)) map.set(cat, []);
      map.get(cat)!.push(s);
    }
    return map;
  }

  function formatBinding(binding: string): string {
    return binding
      .replace(/mod/g, "Ctrl")
      .replace(/\+/g, " + ")
      .replace(/ {2}/g, "  then  ");
  }
</script>

<div class="settings-page">
  <h1 class="settings-title">Settings</h1>

  <!-- General Section -->
  <details class="settings-section" open>
    <summary class="section-header">
      <svg class="chevron" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M9 18l6-6-6-6"></path>
      </svg>
      <span class="section-title">General</span>
    </summary>
    <div class="section-content">
      <div class="setting-row">
        <span class="setting-label">Graph location</span>
        <span class="setting-value">{graphPath}</span>
      </div>
      <div class="setting-row">
        <span class="setting-label">Index</span>
        <div style="display:flex; flex-direction:column; align-items:flex-end; gap:6px;">
          <button class="sync-btn" onclick={reindexGraphNow} disabled={reindexing}>
            {reindexing ? "Re-indexing..." : "Re-index Graph (Manual)"}
          </button>
          {#if reindexStatus}
            <span class="setting-value">{reindexStatus}</span>
          {/if}
        </div>
      </div>
    </div>
  </details>

  <!-- Sync Section -->
  <details class="settings-section">
    <summary class="section-header">
      <svg class="chevron" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M9 18l6-6-6-6"></path>
      </svg>
      <span class="section-title">Sync</span>
    </summary>
    <div class="section-content">
    <p class="section-desc">Sync your notes to a USB drive, network share, or WebDAV server.</p>

    {#if syncMessage}
      <div class="sync-message" class:error={syncMessage.startsWith("Error") || syncMessage.startsWith("Sync failed")}>
        {syncMessage}
      </div>
    {/if}

    <div class="sync-targets">
      {#if syncTargets.length === 0}
        <div class="sync-empty">
          <p>No sync targets configured.</p>
        </div>
      {:else}
        {#each syncTargets as target}
          <div class="sync-target-card">
            <div class="sync-target-info">
              <div class="sync-target-name">
                {#if target.backend_type === "filesystem"}
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
                  </svg>
                {:else}
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <circle cx="12" cy="12" r="10"></circle>
                    <line x1="2" y1="12" x2="22" y2="12"></line>
                    <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"></path>
                  </svg>
                {/if}
                <span>{target.name}</span>
              </div>
              <div class="sync-target-detail">
                {#if target.config.path}
                  {target.config.path}
                {:else if target.config.url}
                  {target.config.url}
                {/if}
              </div>
            </div>
            <div class="sync-target-actions">
              <button class="sync-btn sync-btn-run" onclick={() => runSync(target.id)} disabled={syncLoading}>
                {syncLoading ? "Syncing..." : "Sync Now"}
              </button>
              <button class="sync-btn sync-btn-remove" onclick={() => removeSyncTarget(target.id)}>
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <polyline points="3 6 5 6 21 6"></polyline>
                  <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>
                </svg>
              </button>
            </div>
          </div>
        {/each}
      {/if}
    </div>

    {#if !showAddSync}
      <button class="sync-add-btn" onclick={() => showAddSync = true}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <line x1="12" y1="5" x2="12" y2="19"></line>
          <line x1="5" y1="12" x2="19" y2="12"></line>
        </svg>
        Add Sync Target
      </button>
    {:else}
      <div class="sync-add-form">
        <h4 class="sync-form-title">Add Sync Target</h4>
        <div class="sync-type-toggle">
          <button class="sync-type-btn" class:active={addSyncType === "filesystem"} onclick={() => addSyncType = "filesystem"}>
            Filesystem
          </button>
          <button class="sync-type-btn" class:active={addSyncType === "webdav"} onclick={() => addSyncType = "webdav"}>
            WebDAV
          </button>
        </div>
        <div class="sync-form-fields">
          <label class="sync-field">
            <span class="sync-field-label">Name</span>
            <input type="text" bind:value={addSyncName} placeholder="e.g. USB Stick, Nextcloud" />
          </label>
          {#if addSyncType === "filesystem"}
            <label class="sync-field">
              <span class="sync-field-label">Path</span>
              <input type="text" bind:value={addSyncPath} placeholder="/media/usb/notes" />
            </label>
          {:else}
            <label class="sync-field">
              <span class="sync-field-label">WebDAV URL</span>
              <input type="text" bind:value={addSyncUrl} placeholder="https://cloud.example.com/remote.php/dav/..." />
            </label>
            <label class="sync-field">
              <span class="sync-field-label">Username</span>
              <input type="text" bind:value={addSyncUsername} placeholder="username" />
            </label>
            <label class="sync-field">
              <span class="sync-field-label">Password</span>
              <input type="password" bind:value={addSyncPassword} placeholder="password" />
            </label>
          {/if}
        </div>
        <div class="sync-form-actions">
          <button class="sync-btn sync-btn-cancel" onclick={() => showAddSync = false}>Cancel</button>
          <button class="sync-btn sync-btn-save" onclick={addSyncTarget} disabled={syncLoading || !addSyncName}>
            {syncLoading ? "Adding..." : "Add Target"}
          </button>
        </div>
      </div>
    {/if}
    </div>
  </details>

  <!-- AI / Knowledge Section -->
  <details class="settings-section">
    <summary class="section-header">
      <svg class="chevron" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M9 18l6-6-6-6"></path>
      </svg>
      <span class="section-title">AI / Knowledge Engine</span>
    </summary>
    <div class="section-content">
      <AISettings />
    </div>
  </details>

  <!-- Theme Section -->
  <details class="settings-section">
    <summary class="section-header">
      <svg class="chevron" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M9 18l6-6-6-6"></path>
      </svg>
      <span class="section-title">Theme</span>
    </summary>
    <div class="section-content">
      <p class="section-desc">
        {#if smplosThemeName}
          smplos detected — current system theme: <strong>{smplosThemeName}</strong>
        {:else}
          Select a color theme for Grafium.
        {/if}
      </p>

      <div class="theme-grid">
      <button
        class="theme-card"
        class:active={currentThemeId === "auto"}
        onclick={() => selectTheme("auto")}
      >
        <div class="theme-swatches">
          <div class="swatch" style="background: {autoSwatches().accent}"></div>
          <div class="swatch" style="background: {autoSwatches().bg}"></div>
          <div class="swatch" style="background: {autoSwatches().fg}"></div>
        </div>
        <span class="theme-name">Auto{smplosThemeName ? ` (${smplosThemeName})` : ""}</span>
      </button>

      {#each themes as theme (theme.id)}
        <button
          class="theme-card"
          class:active={currentThemeId === theme.id}
          onclick={() => selectTheme(theme.id)}
        >
          <div class="theme-swatches">
            <div class="swatch" style="background: {theme.colors.accent}"></div>
            <div class="swatch" style="background: {theme.colors.bgPrimary}"></div>
            <div class="swatch" style="background: {theme.colors.textPrimary}"></div>
          </div>
          <span class="theme-name">{theme.name}</span>
        </button>
      {/each}
    </div>
    </div>
  </details>

  <!-- Keyboard Shortcuts Section -->
  <details class="settings-section">
    <summary class="section-header">
      <svg class="chevron" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M9 18l6-6-6-6"></path>
      </svg>
      <span class="section-title">Keyboard Shortcuts</span>
    </summary>
    <div class="section-content">
    <p class="section-desc">Press <kbd>Escape</kbd> in a block editor to return to navigation mode.</p>

    <div class="keymap-list">
      {#each [...getShortcutsByCategory()] as [category, shortcuts]}
        <div class="keymap-category">
          <h3 class="keymap-category-title">{category}</h3>
          {#each shortcuts as shortcut}
            <div class="keymap-row">
              <span class="keymap-desc">{shortcut.description || shortcut.binding}</span>
              <kbd class="keymap-binding">{formatBinding(shortcut.binding)}</kbd>
            </div>
          {/each}
        </div>
      {/each}
    </div>
    </div>
  </details>

  <!-- Asset Cleanup Section -->
  <details class="settings-section">
    <summary class="section-header">
      <svg class="chevron" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <polyline points="9 18 15 12 9 6"></polyline>
      </svg>
      <span class="section-title">Asset Cleanup</span>
    </summary>
    <div class="section-content">
      <p class="setting-desc">Find and remove images in assets/ that are no longer referenced by any block.</p>
      <button class="sync-btn" onclick={scanOrphanedAssets}>
        {assetScanDone ? "Re-scan" : "Scan for orphaned assets"}
      </button>

      {#if assetScanDone}
        {#if orphanedAssets.length === 0}
          <p class="setting-desc" style="margin-top: 8px; color: var(--accent);">No orphaned assets found.</p>
        {:else}
          <p class="setting-desc" style="margin-top: 8px;">Found {orphanedAssets.length} orphaned file{orphanedAssets.length > 1 ? "s" : ""} ({formatBytes(orphanedAssets.reduce((s, a) => s + a.size, 0))} total)</p>
          <button class="sync-btn sync-btn-remove" onclick={deleteAllOrphans} disabled={assetDeleting}>
            {assetDeleting ? "Deleting..." : `Delete all ${orphanedAssets.length} orphans`}
          </button>
          <div class="orphan-list">
            {#each orphanedAssets as asset}
              <div class="orphan-item">
                <span class="orphan-name">{asset.filename}</span>
                <span class="orphan-size">{formatBytes(asset.size)}</span>
                <button class="orphan-delete" onclick={() => deleteSingleOrphan(asset.filename)} disabled={assetDeleting}>✕</button>
              </div>
            {/each}
          </div>
        {/if}
      {/if}
    </div>
  </details>

  <!-- About Section -->
  <details class="settings-section">
    <summary class="section-header">
      <svg class="chevron" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M9 18l6-6-6-6"></path>
      </svg>
      <span class="section-title">About</span>
    </summary>
    <div class="section-content">
    <div class="about-info">
      <span class="about-app-name">Grafium</span>
      <span class="about-version">v{appVersion}</span>
    </div>
    <p class="section-desc">
      A fast, file-first personal knowledge management app.
      Your notes are stored as plain markdown files — edit them anywhere.
    </p>
    <div class="about-details">
      <div class="detail-row">
        <span class="detail-label">Stack</span>
        <span class="detail-value">Rust + Tauri 2 + Svelte 5</span>
      </div>
      <div class="detail-row">
        <span class="detail-label">Storage</span>
        <span class="detail-value">Markdown files + SQLite index</span>
      </div>
      <div class="detail-row">
        <span class="detail-label">License</span>
        <span class="detail-value">MIT</span>
      </div>
    </div>
    </div>
  </details>
</div>

<style>
  .settings-page {
    padding: 32px 48px;
    max-width: 800px;
  }

  .settings-title {
    font-size: 24px;
    font-weight: 700;
    color: var(--text-primary);
    margin-bottom: 24px;
  }

  .settings-section {
    margin-bottom: 8px;
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: hidden;
  }

  .settings-section[open] {
    margin-bottom: 16px;
  }

  .section-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 16px;
    cursor: pointer;
    user-select: none;
    list-style: none;
    background: var(--bg-secondary);
    transition: background 0.15s;
  }

  .section-header:hover {
    background: var(--bg-hover);
  }

  .section-header::-webkit-details-marker {
    display: none;
  }

  .section-header::marker {
    display: none;
    content: "";
  }

  .chevron {
    transition: transform 0.2s ease;
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .settings-section[open] > .section-header .chevron {
    transform: rotate(90deg);
  }

  .section-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .section-content {
    padding: 16px;
  }

  .setting-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
  }

  .setting-label {
    font-size: 13px;
    color: var(--text-secondary);
  }

  .setting-value {
    font-size: 12px;
    color: var(--text-muted);
    font-family: monospace;
    word-break: break-all;
    text-align: right;
  }

  .section-desc {
    font-size: 13px;
    color: var(--text-muted);
    margin-bottom: 16px;
  }

  .section-desc strong {
    color: var(--accent);
  }

  .section-desc kbd {
    font-family: "SF Mono", "Fira Code", "JetBrains Mono", monospace;
    font-size: 11px;
    padding: 2px 5px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--text-primary);
  }

  /* Theme grid */
  .theme-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 12px;
  }

  .theme-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 12px;
    background: var(--bg-secondary);
    border: 2px solid var(--border);
    border-radius: 10px;
    cursor: pointer;
    transition: border-color 0.15s, transform 0.1s;
  }

  .theme-card:hover {
    border-color: var(--text-muted);
    transform: translateY(-1px);
  }

  .theme-card.active {
    border-color: var(--accent);
  }

  .theme-swatches {
    display: flex;
    gap: 4px;
  }

  .swatch {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    border: 1px solid rgba(128, 128, 128, 0.3);
  }

  .theme-name {
    font-size: 12px;
    color: var(--text-secondary);
    font-weight: 500;
    text-align: center;
  }

  .theme-card.active .theme-name {
    color: var(--accent);
  }

  /* Sync */
  .sync-message {
    padding: 8px 12px;
    border-radius: 6px;
    font-size: 12px;
    background: rgba(76, 175, 80, 0.1);
    color: #4caf50;
    border: 1px solid rgba(76, 175, 80, 0.2);
    margin-bottom: 12px;
  }

  .sync-message.error {
    background: rgba(244, 67, 54, 0.1);
    color: #f44336;
    border-color: rgba(244, 67, 54, 0.2);
  }

  .sync-targets {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-bottom: 12px;
  }

  .sync-empty {
    text-align: center;
    padding: 16px;
    color: var(--text-muted);
    font-size: 13px;
  }

  .sync-target-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  .sync-target-info {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .sync-target-name {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .sync-target-detail {
    font-size: 11px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .sync-target-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .sync-btn {
    padding: 5px 10px;
    border-radius: 5px;
    font-size: 11px;
    font-weight: 500;
    cursor: pointer;
    border: 1px solid var(--border);
    background: var(--bg-secondary);
    color: var(--text-secondary);
    transition: all 0.15s;
  }

  .sync-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .sync-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .sync-btn-run {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
  }

  .sync-btn-run:hover {
    opacity: 0.9;
    background: var(--accent);
    color: #fff;
  }

  .sync-btn-remove {
    padding: 5px 6px;
    color: var(--text-muted);
  }

  .sync-btn-remove:hover {
    color: #f44336;
    background: rgba(244, 67, 54, 0.1);
    border-color: rgba(244, 67, 54, 0.2);
  }

  .sync-add-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    background: none;
    border: 1px dashed var(--border);
    border-radius: 8px;
    color: var(--text-muted);
    font-size: 13px;
    cursor: pointer;
    transition: all 0.15s;
    width: 100%;
    justify-content: center;
  }

  .sync-add-btn:hover {
    border-color: var(--accent);
    color: var(--accent);
  }

  .sync-add-form {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 16px;
  }

  .sync-form-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
    margin: 0 0 12px;
  }

  .sync-type-toggle {
    display: flex;
    gap: 6px;
    margin-bottom: 12px;
  }

  .sync-type-btn {
    padding: 6px 12px;
    border-radius: 5px;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    border: 1px solid var(--border);
    background: var(--bg-primary);
    color: var(--text-secondary);
    transition: all 0.15s;
  }

  .sync-type-btn:hover {
    border-color: var(--text-muted);
  }

  .sync-type-btn.active {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
  }

  .sync-form-fields {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-bottom: 14px;
  }

  .sync-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .sync-field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .sync-field input {
    padding: 7px 10px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg-input);
    color: var(--text-primary);
    font-size: 13px;
    outline: none;
    transition: border-color 0.15s;
  }

  .sync-field input:focus {
    border-color: var(--accent);
  }

  .sync-field input::placeholder {
    color: var(--text-muted);
    opacity: 0.6;
  }

  .sync-form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .sync-btn-cancel {
    background: var(--bg-primary);
  }

  .sync-btn-save {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
  }

  .sync-btn-save:hover {
    opacity: 0.9;
    background: var(--accent);
    color: #fff;
  }

  /* Keymap */
  .keymap-list {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .keymap-category-title {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-muted);
    margin-bottom: 8px;
  }

  .keymap-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 0;
  }

  .keymap-row + .keymap-row {
    border-top: 1px solid var(--border);
  }

  .keymap-desc {
    font-size: 13px;
    color: var(--text-secondary);
  }

  .keymap-binding {
    font-family: "SF Mono", "Fira Code", "JetBrains Mono", monospace;
    font-size: 11px;
    padding: 3px 8px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text-primary);
    white-space: nowrap;
  }

  /* About */
  .about-info {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin-bottom: 8px;
  }

  .about-app-name {
    font-size: 16px;
    font-weight: 700;
    color: var(--text-primary);
  }

  .about-version {
    font-size: 13px;
    color: var(--accent);
    font-weight: 600;
  }

  .about-details {
    background: var(--bg-secondary);
    border-radius: 8px;
    padding: 12px 16px;
  }

  .detail-row {
    display: flex;
    justify-content: space-between;
    padding: 6px 0;
    font-size: 12px;
  }

  .detail-row + .detail-row {
    border-top: 1px solid var(--border);
  }

  .detail-label {
    color: var(--text-muted);
    font-weight: 500;
  }

  .detail-value {
    color: var(--text-primary);
  }

  /* Asset Cleanup */
  .orphan-list {
    margin-top: 8px;
    max-height: 200px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .orphan-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
  }

  .orphan-item:last-child {
    border-bottom: none;
  }

  .orphan-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-primary);
  }

  .orphan-size {
    color: var(--text-muted);
    font-size: 11px;
    white-space: nowrap;
  }

  .orphan-delete {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 14px;
  }

  .orphan-delete:hover {
    background: rgba(255, 80, 80, 0.2);
    color: #ff5050;
  }
</style>
