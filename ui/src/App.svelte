<script lang="ts">
  import Sidebar from "./components/Sidebar.svelte";
  import PageContent from "./components/PageContent.svelte";
  import JournalView from "./components/JournalView.svelte";
  import AllPages from "./components/AllPages.svelte";
  import TitleBar from "./components/TitleBar.svelte";
  import { getPage, createPage, recordPageOpen } from "./lib/api";
  import { keymap_manager, registerDefaultShortcuts } from "./lib/keymap";
  import type { Page } from "./lib/api";

  type View = "page" | "journal" | "all-pages" | "flashcards";

  let currentView: View = $state("page");
  let currentPage: Page | null = $state(null);
  let loading = $state(true);
  let error: string | null = $state(null);
  let sidebarVisible = $state(true);
  let wideMode = $state(true);
  let zenMode = $state(false);

  // Navigation history for back/forward
  let navHistory: string[] = $state([]);
  let navIndex = $state(-1);

  // Register hotkeys
  registerDefaultShortcuts({
    goJournal: () => navigateToJournal(),
    goHome: () => navigateToJournal(),
    goAllPages: () => navigateToPage("__all_pages__"),
    goFlashcards: () => navigateToPage("__flashcards__"),
    goTomorrow: () => {
      const d = new Date();
      d.setDate(d.getDate() + 1);
      navigateToPage(d.toISOString().split("T")[0], true);
    },
    goNextJournal: () => {
      if (currentPage && /^\d{4}-\d{2}-\d{2}$/.test(currentPage.title)) {
        const d = new Date(currentPage.title);
        d.setDate(d.getDate() + 1);
        navigateToPage(d.toISOString().split("T")[0], true);
      }
    },
    goPrevJournal: () => {
      if (currentPage && /^\d{4}-\d{2}-\d{2}$/.test(currentPage.title)) {
        const d = new Date(currentPage.title);
        d.setDate(d.getDate() - 1);
        navigateToPage(d.toISOString().split("T")[0], true);
      }
    },
    goForward: () => {
      if (navIndex < navHistory.length - 1) {
        navIndex++;
        navigateToPage(navHistory[navIndex], false, true);
      }
    },
    goBackward: () => {
      if (navIndex > 0) {
        navIndex--;
        navigateToPage(navHistory[navIndex], false, true);
      }
    },
    search: () => {
      // Trigger sidebar search
      window.dispatchEvent(new CustomEvent("toggle-search"));
    },
    searchInPage: () => {
      window.dispatchEvent(new CustomEvent("toggle-search"));
    },
    toggleSidebar: () => {
      sidebarVisible = !sidebarVisible;
    },
    toggleRightSidebar: () => {}, // Not implemented yet
    toggleTheme: () => {}, // Not implemented yet
    toggleHelp: () => {
      window.dispatchEvent(new CustomEvent("toggle-help"));
    },
    toggleSettings: () => {
      window.dispatchEvent(new CustomEvent("toggle-settings"));
    },
    toggleWideMode: () => { wideMode = !wideMode; },
    toggleZenMode: () => { zenMode = !zenMode; },
    newPage: () => {
      newPageName = "";
      showNewPageDialog = true;
    },
    reindex: () => {},
    undo: () => {},
    redo: () => {},
    commandPalette: () => {},
  });

  // Global keydown handler
  function handleGlobalKeydown(e: KeyboardEvent) {
    // Don't intercept if a dialog is open
    if (showNewPageDialog) return;
    // Don't intercept if editing a block
    if (keymap_manager.isEditing) return;
    // Don't intercept if target is an editable element
    const target = e.target as HTMLElement;
    if (target.isContentEditable || target.tagName === "INPUT" || target.tagName === "TEXTAREA") return;
    // Escape exits zen mode
    if (zenMode && e.key === "Escape") {
      zenMode = false;
      e.preventDefault();
      return;
    }
    keymap_manager.handleKeydown(e);
  }

  $effect(() => {
    window.addEventListener("keydown", handleGlobalKeydown);
    return () => window.removeEventListener("keydown", handleGlobalKeydown);
  });

  // Navigate to today's journal on start (only once)
  let hasInitialized = false;
  $effect(() => {
    if (!hasInitialized) {
      hasInitialized = true;
      navigateToJournal();
    }
  });

  async function navigateToJournal() {
    currentView = "journal";
    currentPage = null;
    loading = false;
  }

  async function navigateToPage(title: string, isJournal = false, skipHistory = false) {
    // Handle special routes
    if (title === "__all_pages__") {
      currentView = "all-pages";
      loading = false;
      if (!skipHistory) pushHistory(title);
      return;
    }
    if (title === "__flashcards__") {
      currentView = "flashcards";
      loading = false;
      if (!skipHistory) pushHistory(title);
      return;
    }

    loading = true;
    error = null;
    try {
      // Try to get existing page
      currentPage = await getPage({ title });
    } catch {
      // Create it if it doesn't exist
      try {
        currentPage = await createPage(title, isJournal || /^\d{4}-\d{2}-\d{2}$/.test(title));
      } catch (e) {
        error = `Failed to load page: ${e}`;
        loading = false;
        return;
      }
    }

    if (currentPage) {
      recordPageOpen(currentPage.id).catch(() => {});
    }

    currentView = "page";
    loading = false;
    if (!skipHistory) pushHistory(title);
  }

  function pushHistory(title: string) {
    // Trim forward history when navigating to a new page
    navHistory = navHistory.slice(0, navIndex + 1);
    navHistory.push(title);
    navIndex = navHistory.length - 1;
  }

  let showNewPageDialog = $state(false);
  let newPageName = $state("");

  function handleNavigate(title: string) {
    if (title === "__new_page__") {
      newPageName = "";
      showNewPageDialog = true;
      return;
    }
    navigateToPage(title);
  }

  function submitNewPage() {
    if (newPageName.trim()) {
      navigateToPage(newPageName.trim());
    }
    showNewPageDialog = false;
    newPageName = "";
  }

  function cancelNewPage() {
    showNewPageDialog = false;
    newPageName = "";
  }

  function handleNewPageKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") submitNewPage();
    if (e.key === "Escape") cancelNewPage();
  }

  function handleGraphChanged() {
    // Reload after graph switch
    navigateToJournal();
  }

  // Listen for page navigation events from rendered content
  function handlePageNav(e: Event) {
    const detail = (e as CustomEvent).detail;
    navigateToPage(detail);
  }

  $effect(() => {
    window.addEventListener("navigate-page", handlePageNav);
    return () => window.removeEventListener("navigate-page", handlePageNav);
  });
</script>

<div class="app-shell" class:zen={zenMode}>
  {#if !zenMode}
    <TitleBar {sidebarVisible} />
  {/if}
  <div class="app-layout">
    {#if sidebarVisible && !zenMode}
      <Sidebar {currentPage} onNavigate={handleNavigate} onGraphChanged={handleGraphChanged} />
    {/if}

    <main class="main-content" class:narrow={!wideMode && !zenMode} class:zen-content={zenMode} class:zen-wide={zenMode && wideMode}>
    {#if error}
      <div class="error-state">
        <p>{error}</p>
        <button onclick={() => navigateToJournal()}>Retry</button>
      </div>
    {:else if loading}
      <div class="loading">Loading...</div>
    {:else if currentView === "all-pages"}
      <AllPages onNavigate={handleNavigate} />
    {:else if currentView === "journal"}
      <JournalView />
    {:else if currentView === "page" && currentPage}
      {#key currentPage.id}
        <PageContent page={currentPage} />
      {/key}
    {/if}
    </main>
  </div>
</div>

{#if showNewPageDialog}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="dialog-backdrop" onclick={cancelNewPage}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog" onclick={(e) => e.stopPropagation()}>
      <h3 class="dialog-title">New Page</h3>
      <input
        type="text"
        class="dialog-input"
        placeholder="Page name..."
        bind:value={newPageName}
        onkeydown={handleNewPageKeydown}
        autofocus
      />
      <div class="dialog-actions">
        <button class="dialog-btn dialog-btn-cancel" onclick={cancelNewPage}>Cancel</button>
        <button class="dialog-btn dialog-btn-ok" onclick={submitNewPage}>Create</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .app-shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }

  .app-layout {
    display: flex;
    flex: 1;
    overflow: hidden;
  }

  .main-content {
    flex: 1;
    overflow-y: auto;
    background: var(--bg-primary);
    position: relative;
  }

  .main-content.narrow {
    padding-left: var(--narrow-padding-x);
    padding-right: var(--narrow-padding-x);
  }

  .app-shell.zen {
    background: var(--bg-primary);
  }

  .main-content.zen-content {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: var(--zen-padding-top) 0;
  }

  .main-content.zen-content :global(.page-content) {
    max-width: var(--zen-max-width);
    width: 100%;
    padding: 0;
  }

  .main-content.zen-content :global(.backlinks-section) {
    display: none;
  }

  .main-content.zen-content :global(.blocks-container) {
    font-size: var(--zen-font-size);
    line-height: var(--zen-line-height);
  }

  .main-content.zen-wide {
    padding: var(--zen-padding-top) 0;
  }

  .main-content.zen-wide :global(.page-content) {
    max-width: 100%;
    padding: 0 var(--zen-wide-padding-x);
  }



  .loading {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
    font-size: 16px;
  }

  .error-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--danger);
    gap: 16px;
    padding: 24px;
    text-align: center;
  }

  .error-state button {
    padding: 8px 16px;
    background: var(--accent);
    color: white;
    border: none;
    border-radius: 6px;
    cursor: pointer;
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
    background: var(--surface-overlay);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 24px;
    width: 360px;
    max-width: 90vw;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  }

  .dialog-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: 16px;
  }

  .dialog-input {
    width: 100%;
    padding: 10px 12px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: 14px;
    outline: none;
    margin-bottom: 20px;
  }

  .dialog-input:focus {
    border-color: var(--accent);
  }

  .dialog-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  .dialog-btn {
    padding: 8px 16px;
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    border: none;
  }

  .dialog-btn-cancel {
    background: var(--btn-bg);
    color: var(--text-secondary);
  }

  .dialog-btn-cancel:hover {
    background: var(--btn-bg-hover);
    color: var(--text-primary);
  }

  .dialog-btn-ok {
    background: var(--btn-primary-bg);
    color: var(--btn-primary-fg);
  }

  .dialog-btn-ok:hover {
    background: var(--btn-primary-hover);
  }
</style>
