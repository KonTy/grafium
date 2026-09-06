<script lang="ts">
  import { toasts, dismissToast, runToastAction } from "../lib/toast.svelte";
</script>

{#if toasts.length > 0}
  <div class="toaster" role="status" aria-live="polite">
    {#each toasts as toast (toast.id)}
      <div class="toast {toast.severity}">
        <span class="toast-message">{toast.message}</span>
        {#if toast.action}
          <button class="toast-action" onclick={() => runToastAction(toast)}>
            {toast.action.label}
          </button>
        {/if}
        <button
          class="toast-dismiss"
          onclick={() => dismissToast(toast.id)}
          aria-label="Dismiss notification"
        >×</button>
      </div>
    {/each}
  </div>
{/if}

<style>
  .toaster {
    position: fixed;
    bottom: 16px;
    right: 16px;
    z-index: 3000;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 380px;
  }

  .toast {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-left-width: 3px;
    border-radius: 6px;
    background: var(--bg-secondary);
    color: var(--text);
    font-size: 13px;
    line-height: 1.4;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.25);
  }

  .toast.error {
    border-left-color: var(--error);
  }

  .toast.info,
  /* No themed success colour exists, and inventing a green would clash with
     several of the 17 themes. The accent is already the "this is fine" hue. */
  .toast.success {
    border-left-color: var(--accent);
  }

  .toast-action {
    flex-shrink: 0;
    align-self: center;
    background: none;
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text);
    font: inherit;
    font-size: 12px;
    padding: 3px 8px;
    cursor: pointer;
  }

  .toast-action:hover {
    background: var(--bg-hover);
    border-color: var(--accent);
  }

  .toast-message {
    flex: 1;
    word-break: break-word;
  }

  .toast-dismiss {
    flex-shrink: 0;
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: 16px;
    line-height: 1;
    cursor: pointer;
    padding: 0 2px;
  }

  .toast-dismiss:hover {
    color: var(--text);
  }

  @media (prefers-reduced-motion: no-preference) {
    .toast {
      animation: toast-in 140ms ease-out;
    }
  }

  @keyframes toast-in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
</style>
