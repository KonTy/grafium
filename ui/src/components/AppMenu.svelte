<script lang="ts">
  import { getAppVersion } from "../lib/api";

  interface Props {
    uiZoom?: number;
    onZoomIn?: () => void;
    onZoomOut?: () => void;
    onZoomReset?: () => void;
    onOpenSettings?: () => void;
  }

  let {
    uiZoom = 1,
    onZoomIn = () => {},
    onZoomOut = () => {},
    onZoomReset = () => {},
    onOpenSettings = () => {},
  }: Props = $props();

  let menuOpen = $state(false);
  let showAbout = $state(false);
  let appVersion = $state("...");

  $effect(() => {
    getAppVersion().then((v) => (appVersion = v)).catch(() => {});
  });

  // Listen for toggle-settings event from hotkeys -- navigates to the same
  // Settings page as the left sidebar's Settings link (no separate dialog).
  $effect(() => {
    const handler = () => { onOpenSettings(); };
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
    onOpenSettings();
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

</style>
