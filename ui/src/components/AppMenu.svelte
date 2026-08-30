<script lang="ts">
  import { getAppVersion } from "../lib/api";
  import {
    listSyncTargets,
    addFilesystemTarget,
    addWebdavTarget,
    removeSyncTarget as removeSyncTargetById,
    runSyncTarget,
    summarizeSyncResult,
    type SyncTarget,
  } from "../lib/sync";
  import { getShortcutsByCategory, formatBinding } from "../lib/shortcuts";
  import { describeError } from "../lib/toast.svelte";

  interface Props {
    uiZoom?: number;
    onZoomIn?: () => void;
    onZoomOut?: () => void;
    onZoomReset?: () => void;
  }

  let {
    uiZoom = 1,
    onZoomIn = () => {},
    onZoomOut = () => {},
    onZoomReset = () => {},
  }: Props = $props();

  let menuOpen = $state(false);
  let showAbout = $state(false);
  let showSettings = $state(false);
  let settingsTab: "general" | "keymap" | "sync" = $state("general");
  let appVersion = $state("...");

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
    getAppVersion().then((v) => (appVersion = v)).catch(() => {});
  });

  // Listen for toggle-settings event from hotkeys
  $effect(() => {
    const handler = () => { showSettings = true; };
    window.addEventListener("toggle-settings", handler);
    return () => window.removeEventListener("toggle-settings", handler);
  });

  function toggleMenu() {
    menuOpen = !menuOpen;
  }

  function closeMenu() {
    menuOpen = false;
  }

  function openSettings() {
    closeMenu();
    showSettings = true;
    settingsTab = "general";
    loadSyncTargets();
  }

  function closeSettings() {
    showSettings = false;
    showAddSync = false;
  }

  async function loadSyncTargets() {
    try {
      syncTargets = await listSyncTargets();
    } catch {
      syncTargets = [];
    }
  }

  async function addSyncTarget() {
    syncLoading = true;
    syncMessage = "";
    try {
      if (addSyncType === "filesystem") {
        await addFilesystemTarget(addSyncName, addSyncPath);
      } else {
        await addWebdavTarget(addSyncName, addSyncUrl, addSyncUsername, addSyncPassword);
      }
      showAddSync = false;
      addSyncName = "";
      addSyncPath = "";
      addSyncUrl = "";
      addSyncUsername = "";
      addSyncPassword = "";
      await loadSyncTargets();
      syncMessage = "Target added successfully";
    } catch (e) {
      syncMessage = `Error: ${describeError(e)}`;
    } finally {
      syncLoading = false;
    }
  }

  async function removeSyncTarget(id: string) {
    try {
      await removeSyncTargetById(id);
      await loadSyncTargets();
    } catch (e) {
      syncMessage = `Error: ${describeError(e)}`;
    }
  }

  async function runSync(targetId: string) {
    syncLoading = true;
    syncMessage = "";
    try {
      syncMessage = summarizeSyncResult(await runSyncTarget(targetId));
    } catch (e) {
      syncMessage = `Sync failed: ${describeError(e)}`;
    } finally {
      syncLoading = false;
    }
  }

  function openAbout() {
    closeMenu();
    showAbout = true;
  }

  function closeAbout() {
    showAbout = false;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      closeMenu();
      closeAbout();
      closeSettings();
    }
  }

</script>

<svelte:window onkeydown={handleKeydown} />

<div class="app-menu-container">
  <button class="menu-trigger" onclick={toggleMenu} title="Menu">
    <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor">
      <circle cx="5" cy="12" r="2"></circle>
      <circle cx="12" cy="12" r="2"></circle>
      <circle cx="19" cy="12" r="2"></circle>
    </svg>
  </button>

  {#if menuOpen}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="menu-backdrop" onclick={closeMenu}></div>
    <div class="menu-dropdown">
      <div class="zoom-section">
        <div class="zoom-label">Zoom {Math.round(uiZoom * 100)}%</div>
        <div class="zoom-controls">
          <button class="zoom-btn" onclick={() => { closeMenu(); onZoomOut(); }} title="Zoom out (Ctrl+-)">−</button>
          <button class="zoom-reset" onclick={() => { closeMenu(); onZoomReset(); }} title="Reset zoom (Ctrl+0)">100%</button>
          <button class="zoom-btn" onclick={() => { closeMenu(); onZoomIn(); }} title="Zoom in (Ctrl+Plus)">+</button>
        </div>
      </div>
      <div class="menu-separator"></div>
      <button class="menu-item" onclick={openSettings}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="12" r="3"></circle>
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
        </svg>
        <span>Settings</span>
      </button>
      <div class="menu-separator"></div>
      <button class="menu-item" onclick={openAbout}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="12" r="10"></circle>
          <line x1="12" y1="16" x2="12" y2="12"></line>
          <line x1="12" y1="8" x2="12.01" y2="8"></line>
        </svg>
        <span>About</span>
      </button>
    </div>
  {/if}
</div>

{#if showAbout}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="dialog-backdrop" onclick={closeAbout}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog" onclick={(e) => e.stopPropagation()}>
      <div class="about-header">
        <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" stroke-width="1.5">
          <circle cx="12" cy="12" r="3"></circle>
          <circle cx="4" cy="4" r="2"></circle>
          <circle cx="20" cy="4" r="2"></circle>
          <circle cx="4" cy="20" r="2"></circle>
          <circle cx="20" cy="20" r="2"></circle>
          <line x1="6" y1="6" x2="10" y2="10"></line>
          <line x1="18" y1="6" x2="14" y2="10"></line>
          <line x1="6" y1="18" x2="10" y2="14"></line>
          <line x1="18" y1="18" x2="14" y2="14"></line>
        </svg>
        <h2 class="about-title">Grafium</h2>
      </div>
      <div class="about-version">v{appVersion}</div>
      <p class="about-desc">
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
      <div class="about-actions">
        <button class="btn btn-primary" onclick={closeAbout}>Close</button>
      </div>
    </div>
  </div>
{/if}

{#if showSettings}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="dialog-backdrop" onclick={closeSettings}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="settings-dialog" onclick={(e) => e.stopPropagation()}>
      <div class="settings-layout">
        <div class="settings-sidebar">
          <div class="settings-sidebar-header">
            <h2 class="settings-title">Settings</h2>
          </div>
          <nav class="settings-nav">
            <button class="settings-nav-item" class:active={settingsTab === "general"} onclick={() => settingsTab = "general"}>
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <circle cx="12" cy="12" r="3"></circle>
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
              </svg>
              <span>General</span>
            </button>
            <button class="settings-nav-item" class:active={settingsTab === "sync"} onclick={() => settingsTab = "sync"}>
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="23 4 23 10 17 10"></polyline>
                <polyline points="1 20 1 14 7 14"></polyline>
                <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"></path>
              </svg>
              <span>Sync</span>
            </button>
            <button class="settings-nav-item" class:active={settingsTab === "keymap"} onclick={() => settingsTab = "keymap"}>
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <rect x="2" y="4" width="20" height="16" rx="2" ry="2"></rect>
                <path d="M6 8h.001M10 8h.001M14 8h.001M18 8h.001M8 12h.001M12 12h.001M16 12h.001M7 16h10"></path>
              </svg>
              <span>Keyboard Shortcuts</span>
            </button>
          </nav>
        </div>

        <div class="settings-main">
          <div class="settings-main-header">
            <h3 class="settings-section-title">
              {#if settingsTab === "general"}General{:else if settingsTab === "sync"}Sync{:else}Keyboard Shortcuts{/if}
            </h3>
            <button class="settings-close" onclick={closeSettings}>
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M18 6 6 18M6 6l12 12"></path>
              </svg>
            </button>
          </div>

          <div class="settings-content">
            {#if settingsTab === "general"}
              <div class="general-settings">
                <p class="settings-placeholder">More settings coming soon.</p>
              </div>

            {:else if settingsTab === "sync"}
              <div class="sync-settings">
                {#if syncMessage}
                  <div class="sync-message" class:error={syncMessage.startsWith("Error") || syncMessage.startsWith("Sync failed")}>
                    {syncMessage}
                  </div>
                {/if}

                <div class="sync-targets">
                  {#if syncTargets.length === 0}
                    <div class="sync-empty">
                      <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="var(--text-muted)" stroke-width="1.5">
                        <polyline points="23 4 23 10 17 10"></polyline>
                        <polyline points="1 20 1 14 7 14"></polyline>
                        <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"></path>
                      </svg>
                      <p>No sync targets configured</p>
                      <p class="sync-empty-hint">Add a USB drive, network share, or WebDAV server to sync your notes across devices.</p>
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
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
                        </svg>
                        Filesystem
                      </button>
                      <button class="sync-type-btn" class:active={addSyncType === "webdav"} onclick={() => addSyncType = "webdav"}>
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                          <circle cx="12" cy="12" r="10"></circle>
                          <line x1="2" y1="12" x2="22" y2="12"></line>
                          <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"></path>
                        </svg>
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
                          <input type="text" bind:value={addSyncPath} placeholder="/media/usb/notes or /mnt/share/notes" />
                        </label>
                      {:else}
                        <label class="sync-field">
                          <span class="sync-field-label">WebDAV URL</span>
                          <input type="text" bind:value={addSyncUrl} placeholder="https://cloud.example.com/remote.php/dav/files/user/Notes" />
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

            {:else if settingsTab === "keymap"}
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
              <p class="keymap-hint">Shortcuts work in navigation mode. Press <kbd>Escape</kbd> in a block editor to return to navigation mode.</p>
            {/if}
          </div>
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .app-menu-container {
    position: relative;
  }

  .menu-trigger {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    padding: 6px;
    border-radius: 4px;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: all 0.15s;
  }

  .menu-trigger:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .menu-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 99;
  }

  .menu-dropdown {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    z-index: 100;
    padding: 4px;
    min-width: 160px;
  }

  .menu-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 12px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-secondary);
    font-size: 13px;
    cursor: pointer;
    text-align: left;
  }

  .menu-item:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .zoom-section {
    padding: 10px 12px 8px;
  }

  .zoom-label {
    font-size: 12px;
    color: var(--text-secondary);
    margin-bottom: 8px;
  }

  .zoom-controls {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .zoom-btn,
  .zoom-reset {
    height: 28px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg-secondary);
    color: var(--text-primary);
    cursor: pointer;
  }

  .zoom-btn {
    width: 30px;
    font-size: 18px;
    line-height: 1;
  }

  .zoom-reset {
    flex: 1;
    min-width: 64px;
    padding: 0 10px;
    font-size: 12px;
  }

  .zoom-btn:hover,
  .zoom-reset:hover {
    background: var(--bg-hover);
  }

  .menu-separator {
    height: 1px;
    background: var(--border);
    margin: 4px 8px;
  }

  .dialog-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }

  .dialog {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 32px;
    width: 380px;
    max-width: 90vw;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
    text-align: center;
  }

  .about-header {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    margin-bottom: 8px;
  }

  .about-title {
    font-size: 20px;
    font-weight: 700;
    color: var(--text-primary);
  }

  .about-version {
    font-size: 14px;
    color: var(--accent);
    font-weight: 600;
    margin-bottom: 16px;
  }

  .about-desc {
    font-size: 13px;
    color: var(--text-secondary);
    line-height: 1.6;
    margin-bottom: 20px;
  }

  .about-details {
    text-align: left;
    background: var(--bg-primary);
    border-radius: 8px;
    padding: 12px 16px;
    margin-bottom: 20px;
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

  .about-actions {
    display: flex;
    justify-content: center;
  }

  .btn {
    padding: 8px 20px;
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    border: none;
  }

  .btn-primary {
    background: var(--accent);
    color: #fff;
  }

  .btn-primary:hover {
    opacity: 0.9;
  }

  /* Settings dialog */
  .settings-dialog {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 12px;
    width: 720px;
    max-width: 90vw;
    height: 520px;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
    overflow: hidden;
  }

  .settings-layout {
    display: flex;
    height: 100%;
  }

  .settings-sidebar {
    width: 200px;
    min-width: 200px;
    background: var(--bg-primary);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    padding: 16px 0;
  }

  .settings-sidebar-header {
    padding: 0 16px 16px;
  }

  .settings-title {
    font-size: 14px;
    font-weight: 700;
    color: var(--text-primary);
    margin: 0;
  }

  .settings-nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 0 8px;
  }

  .settings-nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-secondary);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    text-align: left;
    transition: all 0.15s;
  }

  .settings-nav-item:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .settings-nav-item.active {
    background: var(--accent);
    color: #fff;
  }

  .settings-nav-item.active svg {
    stroke: #fff;
  }

  .settings-main {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .settings-main-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border);
  }

  .settings-section-title {
    font-size: 15px;
    font-weight: 600;
    color: var(--text-primary);
    margin: 0;
  }

  .settings-close {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
  }

  .settings-close:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .settings-content {
    padding: 16px 20px;
    overflow-y: auto;
    flex: 1;
  }

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
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text-primary);
    white-space: nowrap;
  }

  .keymap-hint {
    margin-top: 16px;
    font-size: 12px;
    color: var(--text-muted);
    line-height: 1.5;
  }

  .keymap-hint kbd {
    font-family: "SF Mono", "Fira Code", "JetBrains Mono", monospace;
    font-size: 11px;
    padding: 2px 5px;
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--text-primary);
  }

  .settings-placeholder {
    color: var(--text-muted);
    font-size: 14px;
    text-align: center;
    padding: 40px 0;
  }

  /* Sync settings */
  .sync-settings {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .sync-message {
    padding: 8px 12px;
    border-radius: 6px;
    font-size: 12px;
    background: rgba(76, 175, 80, 0.1);
    color: #4caf50;
    border: 1px solid rgba(76, 175, 80, 0.2);
  }

  .sync-message.error {
    background: rgba(244, 67, 54, 0.1);
    color: #f44336;
    border-color: rgba(244, 67, 54, 0.2);
  }

  .sync-empty {
    text-align: center;
    padding: 24px 16px;
    color: var(--text-muted);
    font-size: 13px;
  }

  .sync-empty svg {
    margin-bottom: 12px;
    opacity: 0.5;
  }

  .sync-empty p {
    margin: 4px 0;
  }

  .sync-empty-hint {
    font-size: 12px;
    opacity: 0.7;
  }

  .sync-targets {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .sync-target-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px;
    background: var(--bg-primary);
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
    background: color-mix(in srgb, var(--accent) 5%, transparent);
  }

  .sync-add-form {
    background: var(--bg-primary);
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
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border-radius: 5px;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    border: 1px solid var(--border);
    background: var(--bg-secondary);
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
    background: var(--bg-secondary);
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
    background: var(--bg-secondary);
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
</style>
