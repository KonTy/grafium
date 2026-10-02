<script lang="ts">
  import { tick, type Snippet } from "svelte";
  import { dialogKeydown } from "../lib/modal";
  import { clampContextMenuPosition } from "../lib/contextMenu";
  let { label, heading, children, closeOnAction = false }: {
    label: string; heading: string; children: Snippet<[close: () => void]>; closeOnAction?: boolean;
  } = $props();
  const id = $props.id();
  let opened = $state(false);
  let error = $state("");
  let trigger = $state<HTMLButtonElement>();
  let dialog = $state<HTMLDialogElement>();
  let left = $state(8), top = $state(8);
  function closed() { opened = false; trigger?.focus({ preventScroll: true }); }
  function close() { dialog?.close(); closed(); }
  function dismissAfterAction(node: HTMLElement) {
    const click = (event: MouseEvent) => {
      const button = (event.target as Element | null)?.closest?.("button");
      if (closeOnAction && button?.closest("dialog") === dialog && !button?.hasAttribute("aria-haspopup")) close();
    };
    node.addEventListener("click", click);
    return { destroy: () => node.removeEventListener("click", click) };
  }
  async function show() {
    opened = true; error = "";
    await tick();
    try {
      if (!dialog || !trigger) throw new Error("Menu is unavailable.");
      dialog.showModal();
      const anchor = trigger.getBoundingClientRect();
      const bounds = dialog.getBoundingClientRect();
      const position = clampContextMenuPosition(anchor.right - bounds.width, anchor.top - bounds.height - 6,
        { width: bounds.width, height: bounds.height });
      left = position.x; top = position.y;
      (dialog.querySelector<HTMLElement>(".menu-content button:not(:disabled),.menu-content input,.menu-content select,.menu-content textarea")
        ?? dialog.querySelector("button"))?.focus({ preventScroll: true });
    } catch (cause) { opened = false; error = `Could not open menu: ${String(cause)}`; }
  }
</script>

<button bind:this={trigger} type="button" class="more-button" aria-label={label} title={label}
  aria-haspopup="dialog" aria-expanded={opened} onclick={show}>
  <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <circle cx="5" cy="12" r="2" /><circle cx="12" cy="12" r="2" /><circle cx="19" cy="12" r="2" />
  </svg>
</button>
{#if opened}
  <dialog bind:this={dialog} data-reader-menu aria-modal="true" aria-labelledby={`${id}-heading`}
    style:left={`${left}px`} style:top={`${top}px`} onclose={closed}
    oncancel={event => { event.preventDefault(); close(); }} onkeydown={dialogKeydown(close)}>
    <header><h2 id={`${id}-heading`}>{heading}</h2><button type="button" title="Close (Escape)" onclick={close}>Close</button></header>
    <div class="menu-content" use:dismissAfterAction>{@render children(close)}</div>
  </dialog>
{/if}
{#if error}<span role="alert">{error}</span>{/if}

<style>
  button { font: inherit; color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); border-radius: 6px; padding: 6px 9px; cursor: pointer; }
  .more-button { display: inline-grid; place-items: center; min-width: 36px; height: 36px; padding: 4px; flex-shrink: 0; }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  dialog { position: fixed; box-sizing: border-box; margin: 0; width: min(420px, calc(100vw - 16px)); max-height: calc(100dvh - 16px); overflow-y: auto; padding: 12px; color: var(--text-primary); background: var(--bg-secondary); border: 1px solid var(--border); border-radius: 8px; box-shadow: 0 8px 28px #0005; }
  dialog::backdrop { background: #0002; }
  header { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 10px; }
  h2 { margin: 0; font-size: 14px; }
  .menu-content { display: flex; flex-direction: column; gap: 6px; }
  .menu-content :global(button) { text-align: left; }
  [role="alert"] { color: var(--danger); }
  @media (pointer: coarse) { .more-button { min-width: 44px; height: 44px; } }
</style>
