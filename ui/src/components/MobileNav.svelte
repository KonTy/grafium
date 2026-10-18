<script lang="ts">
  import NavIcon from "./NavIcon.svelte";
  import { autofocus } from "../lib/autofocus";
  import { handleMenuKeydown } from "../lib/menuKeyboard";
  import { isMoreView, mobileBarDestinations, mobileMoreDestinations } from "../lib/mobileNav";

  interface Props {
    currentView: string;
    /** Bindable so the app can close the menu from Android's back gesture. */
    menuOpen?: boolean;
    onNavigate: (target: string) => void;
    onToggleKnowledgePanel: () => void;
    onOpenGraph: () => void;
    onCreateGraph: () => void;
    onReindex: () => void;
  }

  let {
    currentView,
    menuOpen = $bindable(false),
    onNavigate,
    onToggleKnowledgePanel,
    onOpenGraph,
    onCreateGraph,
    onReindex,
  }: Props = $props();

  function closeMenu() {
    menuOpen = false;
  }

  // Close first so the menu never lingers over a dialog the action opens.
  function run(action: () => void) {
    closeMenu();
    action();
  }

  // An unmounted bar (Zen mode) must not leave a hidden open menu that eats the back gesture.
  $effect(() => closeMenu);
</script>

<nav class="bottom-nav" aria-label="Primary">
  {#each mobileBarDestinations as item (item.view)}
    <button
      type="button"
      class="bottom-nav-item"
      class:active={currentView === item.view}
      aria-current={currentView === item.view ? "page" : undefined}
      onclick={() => onNavigate(item.target)}
    >
      <NavIcon name={item.icon} />
      <span>{item.barLabel ?? item.label}</span>
    </button>
  {/each}
  <button
    type="button"
    class="bottom-nav-item"
    class:active={menuOpen || isMoreView(currentView)}
    aria-haspopup="menu"
    aria-expanded={menuOpen}
    onclick={() => (menuOpen = !menuOpen)}
  >
    <NavIcon name="more" />
    <span>More</span>
  </button>
</nav>

{#if menuOpen}
  <div class="more-menu-backdrop" role="presentation" onclick={closeMenu}></div>
  <div
    class="more-menu"
    role="menu"
    aria-label="More"
    tabindex="-1"
    use:autofocus
    onkeydown={(event) => handleMenuKeydown(event, closeMenu)}
  >
    <button role="menuitem" class="more-menu-item" onclick={() => run(() => onNavigate("__new_page__"))}>
      <NavIcon name="new" size={18} />
      <span>New Page</span>
    </button>
    <div class="more-menu-separator" role="separator"></div>
    {#each mobileMoreDestinations as item (item.view)}
      <button
        role="menuitem"
        class="more-menu-item"
        class:active={currentView === item.view}
        aria-current={currentView === item.view ? "page" : undefined}
        onclick={() => run(() => onNavigate(item.target))}
      >
        <NavIcon name={item.icon} size={18} />
        <span>{item.label}</span>
      </button>
    {/each}
    <div class="more-menu-separator" role="separator"></div>
    <button role="menuitem" class="more-menu-item" onclick={() => run(onToggleKnowledgePanel)}>
      <NavIcon name="panel" size={18} />
      <span>Knowledge Panel</span>
    </button>
    <button role="menuitem" class="more-menu-item" onclick={() => run(() => onNavigate("__import_media__"))}>
      <NavIcon name="media" size={18} />
      <span>Import Media</span>
    </button>
    <button role="menuitem" class="more-menu-item" onclick={() => run(() => onNavigate("__import_books__"))}>
      <NavIcon name="books" size={18} />
      <span>Import Books</span>
    </button>
    <button role="menuitem" class="more-menu-item" onclick={() => run(onOpenGraph)}>
      <NavIcon name="folder" size={18} />
      <span>Open Graph</span>
    </button>
    <button role="menuitem" class="more-menu-item" onclick={() => run(onCreateGraph)}>
      <NavIcon name="new" size={18} />
      <span>New Graph</span>
    </button>
    <button role="menuitem" class="more-menu-item" onclick={() => run(onReindex)}>
      <NavIcon name="reindex" size={18} />
      <span>Re-index Graph (Manual)</span>
    </button>
  </div>
{/if}

<style>
  .bottom-nav,
  .more-menu-backdrop,
  .more-menu {
    display: none;
  }

  @media (max-width: 640px) {
    .bottom-nav {
      display: flex;
      position: fixed;
      bottom: 0;
      left: 0;
      right: 0;
      background: var(--bg-sidebar);
      border-top: 1px solid var(--border);
      padding: 4px 0;
      padding-bottom: env(safe-area-inset-bottom, 4px);
      z-index: 100;
    }

    /* Equal slots that never overflow, so "More" is always on screen. */
    .bottom-nav-item {
      display: flex;
      flex: 1 1 0;
      min-width: 0;
      flex-direction: column;
      align-items: center;
      gap: 2px;
      background: none;
      border: none;
      color: var(--text-muted);
      cursor: pointer;
      padding: 6px 2px;
      border-radius: 8px;
      font-size: 0.7rem;
      transition: color 0.15s;
    }

    .bottom-nav-item span {
      max-width: 100%;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }

    .bottom-nav-item.active {
      color: var(--accent);
    }

    .bottom-nav-item:hover {
      color: var(--text-primary);
    }

    .more-menu-backdrop {
      display: block;
      position: fixed;
      inset: 0;
      z-index: 199;
    }

    .more-menu {
      display: block;
      position: fixed;
      bottom: calc(60px + env(safe-area-inset-bottom, 0px));
      right: 8px;
      left: auto;
      background: var(--bg-secondary);
      border: 1px solid var(--border);
      border-radius: 12px;
      box-shadow: 0 -4px 24px rgba(0, 0, 0, 0.4);
      z-index: 200;
      padding: 6px;
      min-width: min(180px, calc(100vw - 16px));
      max-width: calc(100vw - 16px);
      max-height: min(70dvh, calc(100dvh - 76px - env(safe-area-inset-top, 0px) - env(safe-area-inset-bottom, 0px)));
      overflow-y: auto;
    }

    .more-menu-item {
      display: flex;
      align-items: center;
      gap: 10px;
      width: 100%;
      padding: 12px 14px;
      background: none;
      border: none;
      color: var(--text-primary);
      font-size: 14px;
      cursor: pointer;
      border-radius: 8px;
      transition: background 0.1s;
    }

    .more-menu-item.active {
      color: var(--accent);
    }

    .more-menu-item:hover,
    .more-menu-item:active {
      background: var(--bg-hover);
    }

    .more-menu-separator {
      height: 1px;
      margin: 4px 6px;
      background: var(--border);
    }
  }
</style>
