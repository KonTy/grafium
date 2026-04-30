<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import AppMenu from "./AppMenu.svelte";

  interface Props {
    sidebarVisible?: boolean;
  }

  let { sidebarVisible = true }: Props = $props();

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
    <AppMenu />
    <button class="titlebar-btn" onclick={minimize} title="Minimize">
      <svg width="12" height="12" viewBox="0 0 12 12">
        <rect x="2" y="5.5" width="8" height="1" fill="currentColor" />
      </svg>
    </button>
    <button class="titlebar-btn" onclick={toggleMaximize} title={isMaximized ? "Restore" : "Maximize"}>
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
    <button class="titlebar-btn titlebar-close" onclick={close} title="Close">
      <svg width="12" height="12" viewBox="0 0 12 12">
        <path d="M2 2 L10 10 M10 2 L2 10" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
      </svg>
    </button>
  </div>
</div>

<style>
  .titlebar {
    height: 36px;
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

  .titlebar-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .titlebar-close:hover {
    background: #e81123;
    color: #fff;
  }
</style>
