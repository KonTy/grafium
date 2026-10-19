<script lang="ts">
  import { calibrateZoom, placeContextMenu } from "../lib/contextMenu";
  import {
    activateContextMenuItem,
    closeContextMenu,
    getOpenContextMenu,
    type OpenContextMenu,
  } from "../lib/contextMenuHost.svelte";
  import { isStatus } from "../lib/contextMenuSections";
  import { handleMenuKeydown } from "../lib/menuKeyboard";

  /** The node this view is drawn in; it moves into whichever dialog or popover the menu is about. */
  let { container }: { container: HTMLElement } = $props();

  /** The ruler's width in CSS pixels; how wide it renders gives the zoom. */
  const RULER_WIDTH = 1000;

  let menuEl = $state<HTMLDivElement>();
  let rulerEl = $state<HTMLDivElement>();
  let focusedToken = -1;

  const open = $derived(getOpenContextMenu());
  const isOpen = $derived(open !== null);
  const layer = $derived(open?.layer ?? null);

  /**
   * A modal dialog makes the rest of the page inert and the top layer is drawn
   * over it, so a menu for something inside one must be inside it as well.
   * Closed, the container goes back to the page so nothing is left in a dialog.
   * Moving it drops focus, so it only moves when its place changes.
   */
  function moveInto(host: HTMLElement | null) {
    const wanted = host ?? document.body;
    if (container.parentNode !== wanted) wanted.append(container);
  }

  function place(menu: HTMLElement, ruler: HTMLElement, request: OpenContextMenu) {
    // Measured from the corner and unconstrained, so the size never depends on where it was last put.
    menu.style.maxHeight = "none";
    menu.style.left = "0px";
    menu.style.top = "0px";
    const rulerRect = ruler.getBoundingClientRect();
    const rect = menu.getBoundingClientRect();
    const placed = placeContextMenu({
      anchor: request.anchor,
      flipAnchor: request.flipAnchor,
      size: { width: rect.width, height: rect.height },
      viewport: { width: window.innerWidth, height: window.innerHeight },
      zoom: calibrateZoom(rulerRect.width, RULER_WIDTH),
      origin: { x: rulerRect.left, y: rulerRect.top },
    });
    menu.style.left = `${placed.left}px`;
    menu.style.top = `${placed.top}px`;
    menu.style.maxHeight = `${placed.maxHeight}px`;
    menu.style.visibility = "visible";
  }

  // Runs again whenever the open menu is replaced, which is how items that
  // arrive late change its size. Hidden until placed, so it never shows in the
  // wrong spot; and an element that is hidden cannot take focus, so focusing
  // follows placing. The ruler travels with the menu, so both are measured
  // where the menu is drawn.
  $effect(() => {
    const request = open;
    const menu = menuEl;
    const ruler = rulerEl;
    if (request?.layer && !request.layer.isConnected) {
      // Taken out of the page along with the dialog it was drawn in.
      closeContextMenu({ restoreFocus: false });
      return;
    }
    moveInto(request?.layer ?? null);
    if (!request || !menu || !ruler) return;

    place(menu, ruler, request);
    if (focusedToken !== request.token || !menu.contains(menu.ownerDocument.activeElement)) {
      focusedToken = request.token;
      // The container, not the first item: Enter must not run a command nobody chose.
      menu.focus({ preventScroll: true });
    }
  });

  // Captured, because the editors stop some of these events before they reach the window.
  $effect(() => {
    if (!isOpen) return;
    const host = layer;
    const dismiss = (event: Event) => {
      if (!(event.target instanceof Node && menuEl?.contains(event.target))) closeContextMenu();
    };
    const dismissAlways = () => closeContextMenu();
    // Closing the dialog the menu is drawn in takes the menu with it, and whatever closed it has put focus back.
    const layerClosed = (event: Event) => {
      if (event.type === "toggle" && (event as ToggleEvent).newState !== "closed") return;
      closeContextMenu({ restoreFocus: false });
    };
    window.addEventListener("pointerdown", dismiss, true);
    window.addEventListener("wheel", dismiss, { capture: true, passive: true });
    window.addEventListener("resize", dismissAlways);
    window.addEventListener("blur", dismissAlways);
    host?.addEventListener("close", layerClosed);
    host?.addEventListener("toggle", layerClosed);
    return () => {
      window.removeEventListener("pointerdown", dismiss, true);
      window.removeEventListener("wheel", dismiss, true);
      window.removeEventListener("resize", dismissAlways);
      window.removeEventListener("blur", dismissAlways);
      host?.removeEventListener("close", layerClosed);
      host?.removeEventListener("toggle", layerClosed);
    };
  });

  function onKeydown(event: KeyboardEvent) {
    // The menu is modal, as a native one is: nothing pressed in it may reach
    // the page behind, where Delete would remove the selected blocks.
    event.stopPropagation();
    if (event.key === "Tab") {
      event.preventDefault();
      closeContextMenu();
      return;
    }
    handleMenuKeydown(event, () => closeContextMenu());
  }

  function onFocusOut(event: FocusEvent) {
    const next = event.relatedTarget;
    if (next instanceof Node && !menuEl?.contains(next)) closeContextMenu({ restoreFocus: false });
  }

  function keepPageState(event: Event) {
    event.preventDefault();
    event.stopPropagation();
  }
</script>

<div class="ctx-menu-ruler" bind:this={rulerEl} aria-hidden="true"></div>

{#if open}
  <div
    bind:this={menuEl}
    class="ctx-menu app-context-menu"
    role="menu"
    tabindex="-1"
    aria-label={open.label}
    onkeydown={onKeydown}
    onfocusout={onFocusOut}
    onpointerdown={keepPageState}
    onmousedown={keepPageState}
    oncontextmenu={keepPageState}
    onclick={(event) => event.stopPropagation()}
  >
    {#each open.sections as section, index (section.id)}
      {#if index > 0}
        <div class="ctx-menu-separator" role="separator"></div>
      {/if}
      <div class="ctx-menu-section" role="group" aria-label={section.title ?? section.ariaLabel}>
        {#if section.title}
          <div class="ctx-menu-title" aria-hidden="true" title={section.title}>{section.title}</div>
        {/if}
        {#each section.items as item (item.id)}
          {#if isStatus(item)}
            <div class="ctx-menu-status" role="menuitem" aria-disabled="true">{item.label}</div>
          {:else}
            <button
              class="ctx-menu-item"
              class:emphasis={item.emphasis}
              class:danger={item.danger}
              type="button"
              role="menuitem"
              disabled={item.disabled}
              title={item.title}
              onclick={() => activateContextMenuItem(item)}
            >
              <span class="ctx-menu-label" dir="auto">{item.label}</span>
              {#if item.hint}
                <span class="ctx-menu-hint">{item.hint}</span>
              {/if}
            </button>
          {/if}
        {/each}
      </div>
    {/each}
  </div>
{/if}

<style>
  .ctx-menu-ruler {
    position: fixed;
    left: 0;
    top: 0;
    width: 1000px;
    height: 0;
    visibility: hidden;
    pointer-events: none;
  }

  .ctx-menu {
    visibility: hidden;
    min-width: 170px;
    padding: 6px;
    /* Drawn inside whichever dialog the menu is about, so it must not pick up that dialog's type. */
    font-style: normal;
    font-weight: 400;
    letter-spacing: normal;
    text-align: left;
    text-transform: none;
  }

  .ctx-menu:focus {
    outline: none;
  }

  .ctx-menu-separator {
    height: 1px;
    margin: 5px 2px;
    background: var(--border);
  }

  /* Shown as given: a page title keeps its capitals. */
  .ctx-menu-title {
    padding: 5px 8px 6px;
    overflow: hidden;
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ctx-menu-item,
  .ctx-menu-status {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    width: 100%;
    padding: 7px 8px;
    border: none;
    border-radius: 6px;
    background: transparent;
    font: inherit;
    font-size: 13px;
    text-align: left;
  }

  .ctx-menu-item {
    color: var(--text-primary);
    cursor: pointer;
  }

  .ctx-menu-item:hover:not(:disabled),
  .ctx-menu-item:focus-visible {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent);
  }

  .ctx-menu-item:disabled {
    color: var(--text-muted);
    cursor: default;
    opacity: 0.6;
  }

  .ctx-menu-item.emphasis .ctx-menu-label {
    font-weight: 600;
  }

  .ctx-menu-item.danger {
    color: var(--danger, #f38ba8);
  }

  .ctx-menu-item.danger:hover:not(:disabled),
  .ctx-menu-item.danger:focus-visible {
    background: color-mix(in srgb, var(--danger, #f38ba8) 18%, transparent);
    color: var(--danger, #f38ba8);
  }

  .ctx-menu-status {
    color: var(--text-muted);
    cursor: default;
  }

  .ctx-menu-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .ctx-menu-hint {
    flex: none;
    color: var(--text-muted);
    font-size: 11px;
  }
</style>
