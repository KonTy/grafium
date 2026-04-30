<script lang="ts">
  import { getAppVersion } from "../lib/api";
  import { keymap_manager } from "../lib/keymap";
  import type { Shortcut } from "../lib/keymap";

  let menuOpen = $state(false);
  let showAbout = $state(false);
  let showSettings = $state(false);
  let settingsTab: "keymap" | "general" = $state("keymap");
  let appVersion = $state("...");

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
    settingsTab = "keymap";
  }

  function closeSettings() {
    showSettings = false;
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
        <h2 class="about-title">Logseq Clone</h2>
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
      <div class="settings-header">
        <h2 class="settings-title">Settings</h2>
        <button class="settings-close" onclick={closeSettings}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M18 6 6 18M6 6l12 12"></path>
          </svg>
        </button>
      </div>

      <div class="settings-tabs">
        <button class="settings-tab" class:active={settingsTab === "keymap"} onclick={() => settingsTab = "keymap"}>
          Keyboard Shortcuts
        </button>
        <button class="settings-tab" class:active={settingsTab === "general"} onclick={() => settingsTab = "general"}>
          General
        </button>
      </div>

      <div class="settings-content">
        {#if settingsTab === "keymap"}
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
        {:else}
          <div class="general-settings">
            <p class="settings-placeholder">More settings coming soon.</p>
          </div>
        {/if}
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
    width: 560px;
    max-width: 90vw;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  }

  .settings-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 20px 24px 0;
  }

  .settings-title {
    font-size: 18px;
    font-weight: 700;
    color: var(--text-primary);
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

  .settings-tabs {
    display: flex;
    gap: 0;
    padding: 16px 24px 0;
    border-bottom: 1px solid var(--border);
  }

  .settings-tab {
    padding: 8px 16px;
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--text-muted);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    margin-bottom: -1px;
  }

  .settings-tab:hover {
    color: var(--text-secondary);
  }

  .settings-tab.active {
    color: var(--accent);
    border-bottom-color: var(--accent);
  }

  .settings-content {
    padding: 16px 24px 24px;
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
</style>
