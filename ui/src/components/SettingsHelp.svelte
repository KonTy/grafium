<script lang="ts">
  import { tick, type Snippet } from "svelte";
  import { dialogKeydown } from "../lib/modal";

  let { title, children }: { title: string; children: Snippet } = $props();
  const id = $props.id();
  let opened = $state(false);
  let error = $state("");
  let trigger = $state<HTMLButtonElement>();
  let dialog = $state<HTMLDialogElement>();
  let closeButton = $state<HTMLButtonElement>();

  async function show() {
    if (opened) return;
    error = "";
    opened = true;
    await tick();
    try {
      if (!dialog) throw new Error("Help dialog is unavailable");
      dialog.showModal();
      closeButton?.focus({ preventScroll: true });
    } catch (cause) {
      opened = false;
      error = `Could not open help: ${String(cause)}`;
    }
  }

  function close() {
    dialog?.close();
    opened = false;
    trigger?.focus({ preventScroll: true });
  }
</script>

<div class="settings-help">
  <button bind:this={trigger} type="button" class="help-trigger"
    aria-label={`Help: ${title}`} title={`Help: ${title}`} aria-haspopup="dialog"
    aria-expanded={opened} aria-controls={opened ? `${id}-dialog` : undefined}
    onclick={show}>?</button>
  {#if opened}
    <dialog bind:this={dialog} id={`${id}-dialog`} data-settings-help-dialog aria-labelledby={`${id}-title`}
      aria-modal="true" onkeydown={dialogKeydown(close)}
      oncancel={(event) => { event.preventDefault(); close(); }}>
      <header>
        <h2 id={`${id}-title`}>{title}</h2>
        <button bind:this={closeButton} type="button" class="close-help" onclick={close}>Close</button>
      </header>
      <div class="help-content">{@render children()}</div>
    </dialog>
  {:else}
    <div hidden data-settings-help-text>
      <span>{title}</span>
      {@render children()}
    </div>
  {/if}
  {#if error}<span role="alert">{error}</span>{/if}
</div>

<style>
  .settings-help { display: inline-block; vertical-align: middle; }
  .help-trigger { display: inline-grid; place-items: center; width: 26px; height: 26px; padding: 0; border: 1px solid var(--border); border-radius: 50%; background: var(--bg-secondary); color: var(--text-secondary); font: inherit; font-weight: 600; cursor: pointer; }
  .help-trigger:hover { background: var(--bg-hover); color: var(--text-primary); }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  dialog { box-sizing: border-box; margin: auto; width: min(620px, calc(100vw - 32px)); max-height: calc(100dvh - 32px); overflow-y: auto; padding: 20px; border: 1px solid var(--border); border-radius: 10px; background: var(--bg-secondary); color: var(--text-primary); font-size: 14px; text-align: left; box-shadow: 0 16px 64px rgb(0 0 0 / .4); }
  dialog::backdrop { background: rgb(0 0 0 / .6); }
  header { display: flex; align-items: center; justify-content: space-between; gap: 16px; }
  h2 { margin: 0; font-size: 18px; }
  .close-help { padding: 6px 12px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg-primary); color: var(--text-primary); font: inherit; cursor: pointer; }
  .help-content { line-height: 1.6; overflow-wrap: anywhere; }
  .help-content :global(p) { margin: 12px 0; font-size: inherit; color: inherit; line-height: inherit; }
  [role="alert"] { color: var(--danger); }
</style>
