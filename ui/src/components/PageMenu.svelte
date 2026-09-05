<script lang="ts">
  import { tick } from "svelte";

  interface Props {
    collectionStatus: "page" | "collection" | "loading" | "unavailable" | "error";
    collectionKind?: string | null;
    busy?: boolean;
    onSetCollection: (kind: string | null) => void | Promise<void>;
  }

  let {
    collectionStatus,
    collectionKind = null,
    busy = false,
    onSetCollection,
  }: Props = $props();

  let open = $state(false);
  let container: HTMLDivElement | null = $state(null);
  let trigger: HTMLButtonElement | null = $state(null);
  let menu: HTMLDivElement | null = $state(null);
  let bookAction: HTMLButtonElement | null = $state(null);
  let paperAction: HTMLButtonElement | null = $state(null);
  let removeAction: HTMLButtonElement | null = $state(null);

  $effect(() => {
    if (!open) return;

    const closeOutside = (event: PointerEvent) => {
      if (!container?.contains(event.target as Node)) open = false;
    };
    window.addEventListener("pointerdown", closeOutside);
    return () => {
      window.removeEventListener("pointerdown", closeOutside);
    };
  });

  async function setOpen(nextOpen: boolean, focusTarget = false) {
    open = nextOpen;
    if (!focusTarget) return;
    await tick();
    if (nextOpen) {
      const firstAction = focusableActions()[0];
      if (firstAction) firstAction.focus();
      else menu?.focus();
    } else {
      trigger?.focus();
    }
  }

  function focusableActions(): HTMLButtonElement[] {
    return [bookAction, paperAction, removeAction]
      .filter((element): element is HTMLButtonElement => Boolean(element && !element.disabled));
  }

  function handleTriggerKeydown(event: KeyboardEvent) {
    if (event.key !== "ArrowDown") return;
    event.preventDefault();
    void setOpen(true, true);
  }

  function handleMenuKeydown(event: KeyboardEvent) {
    if (event.key === "Tab") {
      open = false;
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      void setOpen(false, true);
      return;
    }
    const actions = focusableActions();
    if (actions.length === 0) return;
    const current = actions.indexOf(document.activeElement as HTMLButtonElement);
    let next = current;
    if (event.key === "ArrowDown") next = (current + 1) % actions.length;
    else if (event.key === "ArrowUp") next = (current - 1 + actions.length) % actions.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = actions.length - 1;
    else return;
    event.preventDefault();
    actions[next]?.focus();
  }

  async function updateCollection(kind: string | null) {
    if (
      busy
      || collectionStatus === "loading"
      || collectionStatus === "unavailable"
      || collectionStatus === "error"
    ) return;
    await onSetCollection(kind);
    void setOpen(false, true);
  }
</script>

<div class="page-menu" bind:this={container}>
  <button
    type="button"
    class="page-menu-trigger"
    aria-label="Page menu"
    aria-haspopup="menu"
    aria-expanded={open}
    bind:this={trigger}
    onclick={() => { void setOpen(!open, true); }}
    onkeydown={handleTriggerKeydown}
  >
    <svg width="18" height="18" viewBox="0 0 18 18" fill="none" aria-hidden="true">
      <circle cx="4" cy="9" r="1.25" fill="currentColor" />
      <circle cx="9" cy="9" r="1.25" fill="currentColor" />
      <circle cx="14" cy="9" r="1.25" fill="currentColor" />
    </svg>
  </button>

  {#if open}
    <div
      class="page-menu-popover"
      role="menu"
      aria-label="Page actions"
      tabindex="-1"
      bind:this={menu}
      aria-busy={busy}
      onkeydown={handleMenuKeydown}
    >
      {#if collectionStatus === "loading" || collectionStatus === "unavailable" || collectionStatus === "error"}
        <button type="button" class="page-menu-item" role="menuitem" disabled>
          <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden="true">
            <path d="M2.25 3.25h4l1.25 1.5h6.25v8H2.25z" stroke="currentColor" stroke-width="1.25" stroke-linejoin="round" />
            <path d="M5 8h6M5 10.5h4" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" />
          </svg>
          <span>
            {collectionStatus === "loading"
              ? "Loading page kind…"
              : collectionStatus === "unavailable"
                ? "Collections unavailable"
                : "Collection status unavailable"}
          </span>
        </button>
      {:else}
        <button
          type="button"
          class="page-menu-item"
          role="menuitemradio"
          aria-checked={collectionKind === "book"}
          bind:this={bookAction}
          disabled={busy}
          onclick={() => { void updateCollection("book"); }}
        >
          <span>{collectionStatus === "collection" ? "Book collection" : "Mark as book collection"}</span>
        </button>
        <button
          type="button"
          class="page-menu-item"
          role="menuitemradio"
          aria-checked={collectionKind === "paper"}
          bind:this={paperAction}
          disabled={busy}
          onclick={() => { void updateCollection("paper"); }}
        >
          <span>{collectionStatus === "collection" ? "Paper collection" : "Mark as paper collection"}</span>
        </button>
        {#if collectionStatus === "collection"}
          <div class="menu-separator" role="separator"></div>
          <button
            type="button"
            class="page-menu-item"
            role="menuitem"
            bind:this={removeAction}
            disabled={busy}
            onclick={() => { void updateCollection(null); }}
          >
            <span>{busy ? "Updating…" : "Convert to regular page"}</span>
          </button>
        {/if}
      {/if}
      {#if busy}
        <p class="menu-note" role="status">Updating collection…</p>
      {/if}
      {#if collectionStatus === "unavailable" || collectionStatus === "error"}
        <p class="menu-note">
          {collectionStatus === "unavailable"
            ? "This build does not include collection commands yet."
            : "Reload the page and try again."}
        </p>
      {/if}
    </div>
  {/if}
</div>

<style>
  .page-menu {
    position: relative;
    flex: 0 0 auto;
  }

  .page-menu-trigger {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
  }

  .page-menu-trigger:hover,
  .page-menu-trigger[aria-expanded="true"] {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .page-menu-trigger:focus-visible,
  .page-menu-item:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .page-menu-popover {
    position: absolute;
    top: calc(100% + 5px);
    right: 0;
    z-index: 20;
    width: max-content;
    min-width: 220px;
    padding: 5px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-overlay);
    box-shadow: 0 8px 24px color-mix(in srgb, var(--bg-primary) 70%, transparent);
  }

  .page-menu-item {
    display: flex;
    align-items: center;
    width: 100%;
    gap: 9px;
    padding: 8px 9px;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--text-secondary);
    font: inherit;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }

  .page-menu-item:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .page-menu-item:disabled {
    color: var(--text-secondary);
    cursor: default;
  }

  .page-menu-item[role="menuitemradio"]::before {
    width: 15px;
    color: var(--text-link);
    content: "";
  }

  .page-menu-item[role="menuitemradio"][aria-checked="true"]::before {
    content: "✓";
  }

  .menu-separator {
    height: 1px;
    margin: 4px 6px;
    background: var(--border);
  }

  .menu-note {
    max-width: 210px;
    margin: 3px 9px 6px 33px;
    color: var(--text-muted);
    font-size: 11px;
    line-height: 1.4;
  }
</style>
