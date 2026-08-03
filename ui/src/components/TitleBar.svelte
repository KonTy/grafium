<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import AppMenu from "./AppMenu.svelte";

  interface Props {
    sidebarVisible?: boolean;
    uiZoom?: number;
    canGoBack?: boolean;
    canGoForward?: boolean;
    onGoBack?: () => void;
    onGoForward?: () => void;
    onToggleReferencePanel?: () => void;
    onOpenSearch?: () => void;
    onZoomIn?: () => void;
    onZoomOut?: () => void;
    onZoomReset?: () => void;
  }

  let {
    sidebarVisible = true,
    uiZoom = 1,
    canGoBack = false,
    canGoForward = false,
    onGoBack = () => {},
    onGoForward = () => {},
    onToggleReferencePanel = () => {},
    onOpenSearch = () => {},
    onZoomIn = () => {},
    onZoomOut = () => {},
    onZoomReset = () => {},
  }: Props = $props();

  const appWindow = getCurrentWindow();

  let isMaximized = $state(false);

  $effect(() => {
    appWindow.isMaximized().then((v) => (isMaximized = v));
  });

  async function minimize() {
    await appWindow.minimize();
  }

  async function toggleMaximize() {
    await appWindow.toggleMaximize();
    isMaximized = await appWindow.isMaximized();
  }

  async function close() {
    await appWindow.close();
  }
</script>

<div class="titlebar" data-tauri-drag-region>
  {#if sidebarVisible}
    <div class="titlebar-left" data-tauri-drag-region>
    </div>
  {/if}

  <div class="titlebar-right" data-tauri-drag-region>
    <AppMenu {uiZoom} onZoomIn={onZoomIn} onZoomOut={onZoomOut} onZoomReset={onZoomReset} />
    <div class="nav-controls" data-tauri-drag-region="false">
      <button class="titlebar-btn nav-btn" data-tauri-drag-region="false" onclick={onGoBack} title="Back" disabled={!canGoBack}>
        <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
          <path d="M7.5 2.5L4 6l3.5 3.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
      </button>
      <button class="titlebar-btn nav-btn" data-tauri-drag-region="false" onclick={onGoForward} title="Forward" disabled={!canGoForward}>
        <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
          <path d="M4.5 2.5L8 6 4.5 9.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
      </button>
    </div>
    <button class="titlebar-btn" data-tauri-drag-region="false" onclick={onOpenSearch} title="Search (Ctrl+K)">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <circle cx="11" cy="11" r="8"></circle>
        <path d="m21 21-4.35-4.35"></path>
      </svg>
    </button>
    <button class="titlebar-btn" data-tauri-drag-region="false" onclick={onToggleReferencePanel} title="Knowledge Panel">
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"></path>
        <path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"></path>
      </svg>
    </button>
    <button class="titlebar-btn" data-tauri-drag-region="false" onclick={minimize} title="Minimize">
      <svg width="12" height="12" viewBox="0 0 12 12">
        <rect x="2" y="5.5" width="8" height="1" fill="currentColor" />
      </svg>
    </button>
    <button class="titlebar-btn" data-tauri-drag-region="false" onclick={toggleMaximize} title={isMaximized ? "Restore" : "Maximize"}>
      {#if isMaximized}
        <svg width="12" height="12" viewBox="0 0 12 12">
          <rect x="3" y="1" width="7" height="7" fill="none" stroke="currentColor" stroke-width="1" />
          <rect x="1" y="3" width="7" height="7" fill="var(--bg-secondary)" stroke="currentColor" stroke-width="1" />
        </svg>
      {:else}
        <svg width="12" height="12" viewBox="0 0 12 12">
          <rect x="2" y="2" width="8" height="8" fill="none" stroke="currentColor" stroke-width="1.2" />
        </svg>
      {/if}
    </button>
    <button class="titlebar-btn titlebar-close" data-tauri-drag-region="false" onclick={close} title="Close">
      <svg width="12" height="12" viewBox="0 0 12 12">
        <path d="M2 2 L10 10 M10 2 L2 10" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
      </svg>
    </button>
  </div>
</div>

<style>
  .titlebar {
    height: calc(36px + env(safe-area-inset-top, 0px));
    padding-top: env(safe-area-inset-top, 0px);
    display: flex;
    align-items: center;
    user-select: none;
    -webkit-user-select: none;
    flex-shrink: 0;
  }

  .titlebar-left {
    width: 260px;
    height: 100%;
    background: var(--bg-sidebar);
    border-right: 1px solid var(--border);
    flex-shrink: 0;
  }

  .titlebar-right {
    flex: 1;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 2px;
    padding-right: 4px;
    background: var(--bg-primary);
  }

  .nav-controls {
    display: flex;
    align-items: center;
    gap: 2px;
    margin-right: auto;
    padding-left: 8px;
  }

  .titlebar-btn {
    width: 36px;
    height: 36px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    color: var(--text-secondary);
    cursor: pointer;
    border-radius: 0;
    transition: background 0.1s;
  }

  .titlebar-btn:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .titlebar-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .titlebar-btn:disabled:hover {
    background: none;
    color: var(--text-secondary);
  }

  .titlebar-close:hover {
    background: #e81123;
    color: #fff;
  }
</style>
