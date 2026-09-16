<script lang="ts">
  import { tick, onMount } from "svelte";
  import Sidebar from "./components/Sidebar.svelte";
  import PageContent from "./components/PageContent.svelte";
  import JournalView from "./components/JournalView.svelte";
  import GoToLink from "./components/GoToLink.svelte";
  import LazyView from "./components/LazyView.svelte";
  import { lazyComponent } from "./lib/lazy";
  import { revealStartupWindow } from "./lib/startupWindow";
  import { getLayoutPreferences, saveLayoutPreferences, type LayoutPreferences } from "./lib/api";
  import { handleMainPanePageKey, hasKeyboardOverlay } from "./lib/mainPaneScroll";
  import TitleBar from "./components/TitleBar.svelte";
  import JobActivity from "./components/JobActivity.svelte";
  import Toaster from "./components/Toaster.svelte";
  import FolderBrowser from "./components/FolderBrowser.svelte";
  import { getPage, createPage, recordPageOpen, getAppTheme, getSmplosTheme, getGraphInfo, openGraph, validateGraph, createGraph, reindexCurrent, listGraphs, mediaImportVideo, bookImportDirectory, type GraphInfo } from "./lib/api";
  import { keymap_manager, registerDefaultShortcuts } from "./lib/keymap";
  import { formatLocalIsoDate, isJournalDateTitle, shiftIsoDate } from "./lib/journalDate";
  import {
    dispatchEditPageEnd,
    personalDiarySnippet,
    timeStampSnippet,
    tryInsertIntoActiveEditor,
  } from "./lib/editorInsert";
  import { formatBinding, formatBindingList, groupShortcutRows } from "./lib/shortcuts";
  import type { PageNavigationTarget } from "./lib/navigation";
  import { isPageNotFoundError, resolvePageLookup } from "./lib/navigation";
  import { applyTheme, getThemeById } from "./lib/themes";
  import { attachAppUndoRedoListeners } from "./lib/undoEvents";
  import { initJobs, notifyJobFinished } from "./lib/jobs.svelte";
  import { showToast } from "./lib/toast.svelte";
  import { setCurrentBlockAnchor } from "./lib/currentBlockAnchor";
  import { readingSelection } from "./lib/readingSelection";
  import { uiLog } from "./lib/uiLog";
  import {
    loadBionicReaderPreference as readBionicReaderPreference,
    setBionicReaderEnabled,
  } from "./lib/bionicReader";
  import { listen } from "@tauri-apps/api/event";
  import { documentDir, downloadDir, homeDir } from "@tauri-apps/api/path";
  import { open } from "@tauri-apps/plugin-dialog";
  import type { Page } from "./lib/api";

  const loadAllPages = lazyComponent(() => import("./components/AllPages.svelte"));
  const loadGraphView = lazyComponent(() => import("./components/GraphView.svelte"));
  const loadGraphView3D = lazyComponent(() => import("./components/GraphView3D.svelte"));
  const loadStatistics = lazyComponent(() => import("./components/Statistics.svelte"));
  const loadFlashcardReview = lazyComponent(() => import("./components/FlashcardReview.svelte"));
  const loadChatView = lazyComponent(() => import("./components/ChatView.svelte"));
  const loadSettings = lazyComponent(() => import("./components/Settings.svelte"));
  const loadJobsView = lazyComponent(() => import("./components/JobsView.svelte"));
  const loadReferencePanel = lazyComponent(() => import("./components/ReferencePanel.svelte"));
  const loadGlobalSearchDialog = lazyComponent(() => import("./components/GlobalSearchDialog.svelte"));

  function isAndroidClient(): boolean {
    return typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent);
  }

  function openGoToLink() {
    if (hasKeyboardOverlay(document)) return;
    goToLinkOpen = true;
  }

  let showFolderBrowser = $state(false);
  let folderBrowserTitle = $state("Select Folder");
  let folderBrowserResolve: ((path: string | null) => void) | null = null;

  function openFolderBrowser(title: string): Promise<string | null> {
    return new Promise((resolve) => {
      folderBrowserTitle = title;
      folderBrowserResolve = resolve;
      showFolderBrowser = true;
    });
  }

  function finishFolderBrowser(path: string | null) {
    showFolderBrowser = false;
    const resolve = folderBrowserResolve;
    folderBrowserResolve = null;
    resolve?.(path);
  }

  /** Native folder dialog on desktop; in-app browser on Android (no directory picker). */
  async function pickFolder(title = "Select Folder", defaultPath?: string): Promise<string | null> {
    if ((window as any).FolderPickerBridge) {
      return new Promise<string | null>((resolve) => {
        (window as any).__FOLDER_PICKER_RESOLVE = (result: string | null) => {
          delete (window as any).__FOLDER_PICKER_RESOLVE;
          resolve(result);
        };
        (window as any).FolderPickerBridge.pickFolder();
      });
    }
    if (isAndroidClient()) {
      return openFolderBrowser(title);
    }
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title,
        defaultPath,
      });
      if (selected && typeof selected === "string") {
        return selected;
      }
    } catch (e) {
      console.error("[graph] native folder picker failed:", e);
      return openFolderBrowser(title);
    }
    return null;
  }

  async function defaultExternalBookImportFolder(): Promise<string | undefined> {
    for (const resolveDir of [downloadDir, documentDir, homeDir]) {
      try {
        return await resolveDir();
      } catch {
        // Try the next standard user directory.
      }
    }
    return undefined;
  }

  type View = "page" | "journal" | "all-pages" | "flashcards" | "statistics" | "chat" | "settings" | "graph" | "jobs";

  let currentView: View = $state("page");
  let currentPage: Page | null = $state(null);
  // Journals are a scrolling feed, not a single page — this tracks whichever day-entry is
  // currently most visible, so the Reference/Knowledge panel has something to analyze there too.
  let journalActivePage: Page | null = $state(null);
  const assistantSourcePage = $derived.by((): Page | null => currentView === "page" ? currentPage
    : currentView === "journal" ? journalActivePage : null);
  let journalEditTodayRequestId = $state(0);
  let journalCalendarRequested = $state(false);
  let goToLinkOpen = $state(false);
  let globalSearchOpen = $state(false);
  let loading = $state(true);
  let error: string | null = $state(null);
  let chatVisited = $state(false);
  let expandedConversationId = $state<string | null>(null);
  const chatActive = $derived.by(() => currentView === "chat" && !loading && !error);
  $effect(() => {
    if (chatActive) chatVisited = true;
  });
  let sidebarVisible = $state(true);
  const changedLayoutPreferences = new Set<keyof LayoutPreferences>();
  let layoutSaveQueue: Promise<void> = Promise.resolve();
  let sidebarWidth = $state(260);
  let isResizingSidebar = $state(false);
  let referencePanelWidth = $state(380);
  let isResizingReferencePanel = $state(false);
  let appLayoutEl: HTMLDivElement | null = null;
  let sidebarRef: {
    focusSearch: () => void;
    hasFocus: () => boolean;
    refresh: () => Promise<void>;
  } | null = $state(null);
  let showBlockGuides = $state(true);
  let bionicReaderMode = $state(false);
  let zenMode = $state(false);
  let wideMode = $state(true);
  const DEFAULT_NARROW_PADDING_PCT = 15;
  const MIN_NARROW_PADDING_PCT = 0;
  const MAX_NARROW_PADDING_PCT = 40;
  let narrowPaddingPct = $state(DEFAULT_NARROW_PADDING_PCT);
  let settingsOpenSection = $state("");
  let commandPaletteOpen = $state(false);
  let commandPaletteQuery = $state("");
  let commandPaletteIndex = $state(0);
  let referencePanelVisible = $state(false);
  let referencePanelTab = $state<"chat" | "writing" | "notes">("chat");
  let referencePanelFocusTrigger = $state(0);
  let readingNoteFocus = $state({ pageId: "", label: "", trigger: 0 });
  let mainContentEl: HTMLElement | null = null;
  let restoreTimer: number | null = null;
  let pendingJournalRestore: HistoryEntry | null = $state(null);
  let journalRestoreRequestId = $state(0);
  let uiZoom = $state(1);

  // Navigation history for back/forward
  type HistoryEntry = {
    kind: View;
    title?: string;
    scrollTop: number;
    sourceBlockId?: string;
    sourcePageTitle?: string;
    conversationId?: string | null;
  };

  type LinkNavigateDetail = {
    pageName: string;
    pageId?: string;
    sourceBlockId?: string;
    sourcePageTitle?: string;
    targetBlockId?: string;
  };

  let navHistory: HistoryEntry[] = $state([]);
  let navIndex = $state(-1);

  const DEFAULT_SIDEBAR_WIDTH = 260;
  const SIDEBAR_MIN_WIDTH = 180;
  const SIDEBAR_MAX_WIDTH = 520;
  const MAIN_CONTENT_MIN_WIDTH = 360;
  const DEFAULT_REFERENCE_PANEL_WIDTH = 380;
  const REFERENCE_PANEL_MIN_WIDTH = 280;
  const REFERENCE_PANEL_VIEWPORT_EDGE_GAP = 24;
  const DEFAULT_UI_ZOOM = 1;
  const MIN_UI_ZOOM = 0.7;
  const MAX_UI_ZOOM = 1.8;
  const UI_ZOOM_STEP = 0.05;

  function applyUiZoom(zoom: number) {
    uiZoom = zoom;
    document.documentElement.style.zoom = String(zoom);
  }

  function saveUiZoomPreference(zoom: number) {
    try {
      localStorage.setItem("grafium.ui.zoom", String(Math.round(zoom * 100)));
    } catch {
      // Ignore localStorage failures.
    }
  }

  function loadUiZoomPreference() {
    try {
      const raw = localStorage.getItem("grafium.ui.zoom");
      if (!raw) {
        applyUiZoom(DEFAULT_UI_ZOOM);
        return;
      }
      const percent = Number(raw);
      if (!Number.isFinite(percent)) {
        applyUiZoom(DEFAULT_UI_ZOOM);
        return;
      }
      const zoom = Math.max(MIN_UI_ZOOM, Math.min(MAX_UI_ZOOM, percent / 100));
      applyUiZoom(zoom);
    } catch {
      applyUiZoom(DEFAULT_UI_ZOOM);
    }
  }

  function adjustUiZoom(direction: 1 | -1) {
    const base = Number.isFinite(uiZoom) ? uiZoom : DEFAULT_UI_ZOOM;
    const next = Math.max(MIN_UI_ZOOM, Math.min(MAX_UI_ZOOM, Math.round((base + direction * UI_ZOOM_STEP) * 100) / 100));
    applyUiZoom(next);
    saveUiZoomPreference(next);
  }

  function resetUiZoom() {
    applyUiZoom(DEFAULT_UI_ZOOM);
    saveUiZoomPreference(DEFAULT_UI_ZOOM);
  }

  function loadSidebarWidthPreference() {
    try {
      const raw = localStorage.getItem("grafium.sidebar.width");
      if (!raw) return;
      const parsed = Number(raw);
      if (!Number.isFinite(parsed)) return;
      sidebarWidth = Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, parsed));
    } catch {
      // Ignore localStorage failures and keep defaults.
    }
  }

  // Logseq-style bullet threading — L/T elbows from parent bullets into
  // children (default on). Settings > General can turn it off.
  function loadShowBlockGuidesPreference() {
    try {
      const raw = localStorage.getItem("grafium.pageContent.showBlockGuides");
      if (raw !== null) showBlockGuides = raw === "true";
    } catch {
      // Ignore localStorage failures and keep the default (on).
    }
  }

  function setShowBlockGuides(value: boolean) {
    showBlockGuides = value;
    try {
      localStorage.setItem("grafium.pageContent.showBlockGuides", String(value));
    } catch {
      // Ignore localStorage failures.
    }
  }

  function applyNarrowPadding(pct: number) {
    const next = Math.max(
      MIN_NARROW_PADDING_PCT,
      Math.min(MAX_NARROW_PADDING_PCT, Math.round(Number.isFinite(pct) ? pct : DEFAULT_NARROW_PADDING_PCT)),
    );
    narrowPaddingPct = next;
    document.documentElement.style.setProperty("--narrow-padding-x", `${next}%`);
  }

  function loadNarrowPaddingPreference() {
    try {
      const raw = localStorage.getItem("grafium.ui.narrowPaddingPct");
      if (raw === null) {
        applyNarrowPadding(DEFAULT_NARROW_PADDING_PCT);
        return;
      }
      applyNarrowPadding(Number(raw));
    } catch {
      applyNarrowPadding(DEFAULT_NARROW_PADDING_PCT);
    }
  }

  function setNarrowPaddingPct(value: number) {
    applyNarrowPadding(value);
    try {
      localStorage.setItem("grafium.ui.narrowPaddingPct", String(narrowPaddingPct));
    } catch {
      // Ignore localStorage failures.
    }
  }

  function loadBionicReaderPreference() {
    bionicReaderMode = readBionicReaderPreference();
  }

  function toggleBionicReader() {
    bionicReaderMode = !bionicReaderMode;
    setBionicReaderEnabled(bionicReaderMode);
  }

  function resetSidebarWidth() {
    sidebarWidth = DEFAULT_SIDEBAR_WIDTH;
    saveSidebarWidthPreference(sidebarWidth);
  }

  function saveSidebarWidthPreference(width: number) {
    try {
      localStorage.setItem("grafium.sidebar.width", String(Math.round(width)));
    } catch {
      // Ignore localStorage failures.
    }
  }

  function applySidebarWidthFromPointer(clientX: number) {
    if (!appLayoutEl) return;
    const rect = appLayoutEl.getBoundingClientRect();
    const maxByLayout = Math.max(SIDEBAR_MIN_WIDTH, rect.width - MAIN_CONTENT_MIN_WIDTH);
    const maxWidth = Math.min(SIDEBAR_MAX_WIDTH, maxByLayout);
    const next = clientX - rect.left;
    sidebarWidth = Math.max(SIDEBAR_MIN_WIDTH, Math.min(maxWidth, next));
  }

  function startSidebarResize(e: PointerEvent) {
    if (!sidebarVisible || zenMode || window.innerWidth <= 640) return;
    e.preventDefault();
    isResizingSidebar = true;
    document.body.style.userSelect = "none";
    document.body.style.cursor = "col-resize";

    const onMove = (moveEvent: PointerEvent) => {
      applySidebarWidthFromPointer(moveEvent.clientX);
    };

    const onUp = () => {
      isResizingSidebar = false;
      document.body.style.userSelect = "";
      document.body.style.cursor = "";
      saveSidebarWidthPreference(sidebarWidth);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }

  // Graph view has two renderers: the default 2D canvas (GraphView.svelte,
  // works everywhere) and an opt-in 3D orbit view (GraphView3D.svelte,
  // WebGL via three.js/3d-force-graph). Persisted like other layout
  // preferences; defaults to 2D on Android since drag-to-orbit + pinch-zoom
  // touch gestures are more prone to jank/incompatibility on mobile
  // WebViews even where WebGL itself works fine.
  const isAndroid = typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent);
  let graphViewMode = $state<"2d" | "3d">("2d");

  function loadGraphViewModePreference() {
    try {
      const raw = localStorage.getItem("grafium.graphView.mode");
      if (raw === "2d" || raw === "3d") {
        graphViewMode = raw;
        return;
      }
    } catch {
      // Ignore localStorage failures and keep the default.
    }
    // No saved preference yet — default to 2D on Android (touch-based
    // orbit/pinch gestures on the 3D view are less reliable there), 3D
    // isn't blocked, just not the first thing you land on.
    if (isAndroid) graphViewMode = "2d";
  }

  function saveGraphViewModePreference(mode: "2d" | "3d") {
    try {
      localStorage.setItem("grafium.graphView.mode", mode);
    } catch {
      // Ignore localStorage failures.
    }
  }

  function setGraphViewMode(mode: "2d" | "3d") {
    graphViewMode = mode;
    saveGraphViewModePreference(mode);
  }

  function loadReferencePanelWidthPreference() {
    try {
      const raw = localStorage.getItem("grafium.referencePanel.width");
      if (!raw) return;
      const parsed = Number(raw);
      if (!Number.isFinite(parsed)) return;
      const maxWidth = typeof window === "undefined"
        ? parsed
        : Math.max(REFERENCE_PANEL_MIN_WIDTH, window.innerWidth - REFERENCE_PANEL_VIEWPORT_EDGE_GAP);
      referencePanelWidth = Math.max(
        REFERENCE_PANEL_MIN_WIDTH,
        Math.min(maxWidth, parsed)
      );
    } catch {
      // Ignore localStorage failures and keep defaults.
    }
  }

  function saveReferencePanelWidthPreference(width: number) {
    try {
      localStorage.setItem("grafium.referencePanel.width", String(Math.round(width)));
    } catch {
      // Ignore localStorage failures.
    }
  }

  function resetReferencePanelWidth() {
    referencePanelWidth = DEFAULT_REFERENCE_PANEL_WIDTH;
    saveReferencePanelWidthPreference(referencePanelWidth);
  }

  function applyReferencePanelWidthFromPointer(clientX: number) {
    if (!appLayoutEl) return;
    const rect = appLayoutEl.getBoundingClientRect();
    const maxWidth = Math.max(REFERENCE_PANEL_MIN_WIDTH, rect.width - REFERENCE_PANEL_VIEWPORT_EDGE_GAP);
    // The panel is pinned to the right edge, so dragging the handle on its
    // left side means width = distance from the pointer to the right edge.
    const next = rect.right - clientX;
    referencePanelWidth = Math.max(REFERENCE_PANEL_MIN_WIDTH, Math.min(maxWidth, next));
  }

  function startReferencePanelResize(e: PointerEvent) {
    e.preventDefault();
    isResizingReferencePanel = true;
    document.body.style.userSelect = "none";
    document.body.style.cursor = "col-resize";

    const onMove = (moveEvent: PointerEvent) => {
      applyReferencePanelWidthFromPointer(moveEvent.clientX);
    };

    const onUp = () => {
      isResizingReferencePanel = false;
      document.body.style.userSelect = "";
      document.body.style.cursor = "";
      saveReferencePanelWidthPreference(referencePanelWidth);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }

  function logNav(event: string, data: unknown) {
    const message = `[nav] ${event} ${JSON.stringify(data)}`;
    console.log(message);
    uiLog(message);
  }

  function errorText(error: unknown): string {
    if (typeof error === "object" && error !== null && "message" in error &&
        typeof error.message === "string") return error.message;
    return error instanceof Error ? error.message : String(error);
  }

  function withNavigationTimeout<T>(
    promise: Promise<T>,
    label: string,
    timeoutMs = 12_000
  ): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        reject(new Error(`${label} timed out after ${timeoutMs / 1000}s`));
      }, timeoutMs);
      promise.then(
        (value) => {
          window.clearTimeout(timer);
          resolve(value);
        },
        (err) => {
          window.clearTimeout(timer);
          reject(err);
        }
      );
    });
  }

  function installFrontendDiagnostics() {
    const report = (kind: string, value: unknown) => {
      const message = value instanceof Error ? `${value.name}: ${value.message}` : String(value);
      uiLog(`[ui] ${kind}: ${message}`);
    };
    const onError = (event: ErrorEvent) => report("error", event.error ?? event.message);
    const onUnhandledRejection = (event: PromiseRejectionEvent) => report("unhandledrejection", event.reason);
    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onUnhandledRejection);
    return () => {
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onUnhandledRejection);
    };
  }

  function currentScrollTop(): number {
    return mainContentEl?.scrollTop ?? 0;
  }

  function getCurrentHistoryEntry(): HistoryEntry | null {
    if (currentView === "page" && currentPage) {
      return { kind: "page", title: currentPage.title, scrollTop: currentScrollTop() };
    }
    if (currentView === "journal") {
      return { kind: "journal", scrollTop: currentScrollTop() };
    }
    if (currentView === "all-pages") {
      return { kind: "all-pages", scrollTop: currentScrollTop() };
    }
    if (currentView === "graph") {
      return { kind: "graph", scrollTop: 0 };
    }
    if (currentView === "flashcards") {
      return { kind: "flashcards", scrollTop: currentScrollTop() };
    }
    if (currentView === "statistics") {
      return { kind: "statistics", scrollTop: currentScrollTop() };
    }
    if (currentView === "settings") {
      return { kind: "settings", scrollTop: currentScrollTop() };
    }
    if (currentView === "chat") {
      return { kind: "chat", scrollTop: currentScrollTop(), conversationId: expandedConversationId };
    }
    if (currentView === "jobs") {
      return { kind: "jobs", scrollTop: currentScrollTop() };
    }
    return null;
  }

  function saveCurrentHistoryState(sourceBlockId?: string, sourcePageTitle?: string) {
    if (navIndex < 0 || navIndex >= navHistory.length) return;

    const current = getCurrentHistoryEntry();
    if (!current) return;

    navHistory = navHistory.map((entry, index) => {
      if (index !== navIndex) return entry;
      return {
        ...entry,
        ...current,
        sourceBlockId,
        sourcePageTitle,
      };
    });
  }

  function pushHistoryEntry(entry: HistoryEntry) {
    const nextHistory = [...navHistory.slice(0, navIndex + 1), entry];
    navHistory = nextHistory;
    navIndex = nextHistory.length - 1;
    logNav("push", { navIndex: nextHistory.length - 1, entry, historyLength: nextHistory.length });
  }

  function clearRestoreTimer() {
    if (restoreTimer !== null) {
      window.clearInterval(restoreTimer);
      restoreTimer = null;
    }
  }

  function restoreHistoryState(entry: HistoryEntry) {
    clearRestoreTimer();

    const startedAt = Date.now();
    let pageFallbackApplied = false;
    const tryRestore = () => {
      if (!mainContentEl) return false;

      if (entry.sourceBlockId) {
        const blockEl = mainContentEl.querySelector(`#block-${entry.sourceBlockId}, [data-block-id="${entry.sourceBlockId}"]`) as HTMLElement | null;
        if (blockEl) {
          blockEl.scrollIntoView({ block: "center" });
          if (entry.kind === "page" && currentPage) {
            window.dispatchEvent(new CustomEvent("page-content-reveal-block", {
              detail: {
                pageId: currentPage.id,
                blockId: entry.sourceBlockId,
                align: "center",
                select: true,
              },
            }));
          }
          logNav("restored block", { sourceBlockId: entry.sourceBlockId, sourcePageTitle: entry.sourcePageTitle, kind: entry.kind, title: entry.title });
          return true;
        }
        if (entry.kind === "page" && currentPage) {
          window.dispatchEvent(new CustomEvent("page-content-reveal-block", {
            detail: {
              pageId: currentPage.id,
              blockId: entry.sourceBlockId,
              align: "center",
              select: true,
            },
          }));
        }
      }

      if (entry.kind === "journal" && entry.sourcePageTitle) {
        const pageEl = mainContentEl.querySelector(`#journal-page-${CSS.escape(entry.sourcePageTitle)}, [data-page-title="${entry.sourcePageTitle}"]`) as HTMLElement | null;
        if (pageEl) {
          if (!pageFallbackApplied) {
            pageEl.scrollIntoView({ block: "center" });
            pageFallbackApplied = true;
            logNav("restored page fallback", { sourcePageTitle: entry.sourcePageTitle, sourceBlockId: entry.sourceBlockId, kind: entry.kind });
          }
          if (!entry.sourceBlockId) {
            return true;
          }
        }
      }

      if (!pageFallbackApplied) {
        mainContentEl.scrollTop = entry.scrollTop;
      }
      return !entry.sourceBlockId;
    };

    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (tryRestore()) {
          return;
        }

        restoreTimer = window.setInterval(() => {
          if (tryRestore()) {
            clearRestoreTimer();
            return;
          }

          if (Date.now() - startedAt > 3000) {
            logNav("restore timeout", { sourceBlockId: entry.sourceBlockId, sourcePageTitle: entry.sourcePageTitle, kind: entry.kind, title: entry.title, scrollTop: entry.scrollTop });
            clearRestoreTimer();
          }
        }, 75);
      });
    });
  }

  async function navigateToHistoryEntry(entry: HistoryEntry) {
    if (entry.kind === "journal") {
      await navigateToJournal(true, entry);
      return;
    }

    if (entry.kind === "all-pages") {
      currentView = "all-pages";
      currentPage = null;
      loading = false;
      error = null;
      await tick();
      restoreHistoryState(entry);
      return;
    }

    if (entry.kind === "graph") {
      currentView = "graph";
      loading = false;
      error = null;
      await tick();
      return;
    }

    if (entry.kind === "flashcards") {
      currentView = "flashcards";
      currentPage = null;
      loading = false;
      error = null;
      await tick();
      restoreHistoryState(entry);
      return;
    }

    if (entry.kind === "statistics") {
      currentView = "statistics";
      currentPage = null;
      loading = false;
      error = null;
      await tick();
      restoreHistoryState(entry);
      return;
    }

    if (entry.kind === "settings") {
      currentView = "settings";
      currentPage = null;
      loading = false;
      error = null;
      await tick();
      restoreHistoryState(entry);
      return;
    }

    if (entry.kind === "chat") {
      await navigateToPage("__chat__", false, true, entry);
      return;
    }

    if (entry.title) {
      await navigateToPage(entry.title, false, true, entry);
    }
  }

  async function goBack() {
    if (navIndex <= 0) return;
    saveCurrentHistoryState();
    navIndex -= 1;
    logNav("back", { navIndex, entry: navHistory[navIndex] });
    await navigateToHistoryEntry(navHistory[navIndex]);
  }

  async function goForward() {
    if (navIndex >= navHistory.length - 1) return;
    saveCurrentHistoryState();
    navIndex += 1;
    logNav("forward", { navIndex, entry: navHistory[navIndex] });
    await navigateToHistoryEntry(navHistory[navIndex]);
  }

  function openGlobalSearch() {
    if (hasKeyboardOverlay(document)) return;
    globalSearchOpen = true;
  }

  function openReferencePanelTab(tab: "chat" | "writing" | "notes") {
    referencePanelTab = tab;
    referencePanelFocusTrigger += 1;
    referencePanelVisible = true;
  }

  async function expandAssistantConversation(id: string) {
    try {
      const { getAssistantConversation } = await import("./lib/assistantConversations");
      const conversation = getAssistantConversation(id);
      if (!conversation) throw new Error("This conversation is no longer available.");
      if ((await getGraphInfo()).path !== conversation.graphPath) {
        throw new Error("Return to the original graph before opening this conversation.");
      }
      referencePanelVisible = false;
      await navigateToPage("__chat__", false, false, { kind: "chat", scrollTop: 0, conversationId: id });
    } catch (error) {
      showToast(`Could not expand Chat: ${errorText(error)}`, "error");
    }
  }

  async function restoreLayoutPreferences() {
    try {
      const preferences = await getLayoutPreferences();
      if (!changedLayoutPreferences.has("sidebarVisible")) sidebarVisible = preferences.sidebarVisible;
      if (!changedLayoutPreferences.has("wideMode")) wideMode = preferences.wideMode;
    } catch (e) {
      const message = `Could not restore layout settings: ${errorText(e)}`;
      console.error(message);
      showToast(message, "error");
    }
  }

  function setLayoutPreferences(preferences: Partial<LayoutPreferences>) {
    if (preferences.sidebarVisible !== undefined) {
      changedLayoutPreferences.add("sidebarVisible");
      sidebarVisible = preferences.sidebarVisible;
    }
    if (preferences.wideMode !== undefined) {
      changedLayoutPreferences.add("wideMode");
      wideMode = preferences.wideMode;
    }
    // Persist explicit choices, not temporary hiding by Zen mode or phone CSS.
    // Serialize writes so rapid toggles cannot save an older choice last.
    layoutSaveQueue = layoutSaveQueue
      .then(() => saveLayoutPreferences(preferences))
      .catch((e) => {
        const message = `Could not save layout settings: ${errorText(e)}`;
        console.error(message);
        showToast(message, "error");
      });
  }

  // Ctrl+B "seamless" focus/close for the left sidebar:
  // - hidden -> show it (also leaving zen mode) and focus its search box
  // - visible but not focused -> just focus its search box
  // - visible and already focused -> close it
  // This mirrors the request that Ctrl+B behave like a real toggle+focus
  // combo instead of only ever opening/focusing and never closing.
  async function focusLeftSidebar() {
    if (!sidebarVisible || zenMode) {
      if (zenMode) zenMode = false;
      setLayoutPreferences({ sidebarVisible: true });
      await tick();
      sidebarRef?.focusSearch();
      return;
    }
    if (sidebarRef?.hasFocus()) {
      setLayoutPreferences({ sidebarVisible: false });
      return;
    }
    sidebarRef?.focusSearch();
  }

  // Register hotkeys
  function triggerNativeUndo() {
    const handler = (window as any).__handleNativeUndo;
    if (typeof handler === "function") {
      handler();
    } else {
      window.dispatchEvent(new CustomEvent("app-undo"));
    }
  }

  function triggerNativeRedo() {
    const handler = (window as any).__handleNativeRedo;
    if (typeof handler === "function") {
      handler();
    } else {
      window.dispatchEvent(new CustomEvent("app-redo"));
    }
  }

  function defaultAutoThemeId(): string {
    // Desktop follows smplOS, then GitHub Light. Phones have no smplOS theme
    // file, so auto would otherwise land on a light canvas instead of OLED.
    if (typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent)) return "oled";
    return "github";
  }

  function toggleWideMode() {
    setLayoutPreferences({ wideMode: !wideMode });
  }

  function focusLocalSearch(): boolean {
    const el = document.querySelector("[data-local-search]") as HTMLInputElement | null;
    if (!el || el.disabled || el.closest("[hidden]") || el.getClientRects().length === 0) return false;
    el.focus();
    el.select();
    return true;
  }

  function journalCursorTitle(): string {
    if (currentView === "journal" && isJournalDateTitle(journalActivePage?.title)) {
      return journalActivePage!.title;
    }
    if (isJournalDateTitle(currentPage?.title)) {
      return currentPage!.title;
    }
    return formatLocalIsoDate();
  }

  function shiftJournalDay(days: number) {
    const next = shiftIsoDate(journalCursorTitle(), days);
    if (currentView === "journal") {
      const el = document.getElementById(`journal-page-${next}`);
      if (el) {
        el.scrollIntoView({ block: "start" });
        return;
      }
      pendingJournalRestore = { kind: "journal", scrollTop: 0, sourcePageTitle: next };
      journalRestoreRequestId += 1;
      return;
    }
    void navigateToPage(next, true);
  }

  function openThemeSettings() {
    settingsOpenSection = "theme";
    void navigateToPage("__settings__");
  }

  function toggleSettingsView() {
    if (currentView === "settings") {
      goBack();
      return;
    }
    settingsOpenSection = "";
    void navigateToPage("__settings__");
  }

  function toggleCommandPalette() {
    commandPaletteOpen = !commandPaletteOpen;
    commandPaletteQuery = "";
    commandPaletteIndex = 0;
  }

  const commandPaletteRows = $derived(
    [...groupShortcutRows(keymap_manager.getShortcuts()), {
      id: "ai-writing", description: "Open writing assistance", category: "Tools",
      chords: [], modifiers: [],
    }, {
      id: "reading-notes", description: "Open reading notes", category: "Reading",
      chords: [], modifiers: [],
    }].filter((row) => {
      const q = commandPaletteQuery.trim().toLowerCase();
      if (!q) return true;
      return (
        row.description.toLowerCase().includes(q) ||
        row.chords.some((b) => b.toLowerCase().includes(q)) ||
        row.modifiers.some((b) => formatBinding(b).toLowerCase().includes(q))
      );
    }),
  );

  async function runCommandPaletteRow(index: number) {
    const row = commandPaletteRows[index];
    if (!row) return;
    const match = keymap_manager.getShortcuts().find((s) => (s.id || s.description) === row.id);
    commandPaletteOpen = false;
    await tick();
    if (row.id === "ai-writing") {
      openReferencePanelTab("writing");
      return;
    }
    if (row.id === "reading-notes") {
      openReferencePanelTab("notes");
      return;
    }
    match?.action();
  }

  registerDefaultShortcuts({
    goJournal: () => navigateToJournal(),
    goLink: openGoToLink,
    goJournalDate: () => {
      if (hasKeyboardOverlay(document)) return;
      journalCalendarRequested = true;
      if (currentView !== "journal") void navigateToJournal();
    },
    goJournalEdit: () => { void goJournalAndEdit(); },
    goHome: () => navigateToJournal(),
    goAllPages: () => navigateToPage("__all_pages__"),
    goGraph: () => navigateToPage("__graph__"),
    goFlashcards: () => navigateToPage("__flashcards__"),
    goTomorrow: () => navigateToPage(shiftIsoDate(formatLocalIsoDate(), 1), true),
    goTasks: () => navigateToPage("__statistics__"),
    goChat: () => navigateToPage("__chat__"),
    goNextJournal: () => shiftJournalDay(1),
    goPrevJournal: () => shiftJournalDay(-1),
    goForward: () => {
      goForward();
    },
    goBackward: () => {
      goBack();
    },
    search: () => {
      openGlobalSearch();
    },
    searchInPage: () => {
      window.dispatchEvent(new CustomEvent("toggle-search"));
    },
    focusLocalSearch: () => {
      focusLocalSearch();
    },
    toggleSidebar: () => {
      void focusLeftSidebar();
    },
    toggleRightSidebar: () => {
      referencePanelVisible = !referencePanelVisible;
    },
    toggleTheme: () => openThemeSettings(),
    toggleSettings: () => toggleSettingsView(),
    toggleWideMode,
    toggleZenMode: () => {
      zenMode = !zenMode;
    },
    commandPalette: () => toggleCommandPalette(),
    importMedia: () => openImportMediaDialog(),
    importBooks: () => void openImportBooksDirectory(),
    insertTimeStamp: () => insertEditorSnippet(timeStampSnippet()),
    insertPersonalDiary: () => insertEditorSnippet(personalDiarySnippet()),
  });

  // Global keydown handler
  function handleGlobalKeydown(e: KeyboardEvent) {
    if (goToLinkOpen || globalSearchOpen || e.isComposing) return;
    if (e.key === "Escape" && !hasKeyboardOverlay(document)
      && (e.target as Element | null)?.closest?.('[data-keyboard-block-selection="true"]')) return;
    if (commandPaletteOpen) {
      if (e.key === "Escape") {
        e.preventDefault();
        commandPaletteOpen = false;
        return;
      }
      if (e.key === "ArrowDown") {
        e.preventDefault();
        commandPaletteIndex = Math.min(commandPaletteIndex + 1, Math.max(0, commandPaletteRows.length - 1));
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        commandPaletteIndex = Math.max(commandPaletteIndex - 1, 0);
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        runCommandPaletteRow(commandPaletteIndex);
        return;
      }
      if (!e.ctrlKey && !e.metaKey && !e.altKey) return;
    }

    if (["Escape", "PageUp", "PageDown", "Home", "End"].includes(e.key)
      && hasKeyboardOverlay(document)) return;

    if (e.ctrlKey || e.metaKey) {
      const key = e.key.toLowerCase();
      if (key === "0") {
        e.preventDefault();
        resetUiZoom();
        return;
      }
      if (key === "+" || key === "=" || (key === "-" && e.shiftKey)) {
        e.preventDefault();
        adjustUiZoom(1);
        return;
      }
      if (key === "-") {
        e.preventDefault();
        adjustUiZoom(-1);
        return;
      }
      // Ctrl-F focuses a visible page filter; otherwise leave browser/editor find.
      if (!e.shiftKey && !e.altKey && key === "f") {
        if (focusLocalSearch()) e.preventDefault();
        return;
      }
    }

    if (e.key === "Escape" && referencePanelVisible) {
      referencePanelVisible = false;
      e.preventDefault();
      return;
    }

    if (zenMode && e.key === "Escape") {
      zenMode = false;
      e.preventDefault();
      return;
    }

    if (showNewPageDialog || showImportMediaDialog) return;

    if (keymap_manager.handleKeydown(e)) return;

    if (handleMainPanePageKey(e, mainContentEl, currentView)) return;

    const target = e.target as HTMLElement | null;
    const editableContainer = target?.closest?.("[contenteditable='true'], [role='textbox']");
    const isNativeInput =
      target?.tagName === "INPUT" ||
      target?.tagName === "TEXTAREA" ||
      target?.tagName === "SELECT";
    if (isNativeInput || target?.isContentEditable || !!editableContainer || keymap_manager.isEditing) {
      return;
    }

    if (mainContentEl && (e.key === "Home" || e.key === "End")) {
      e.preventDefault();
      if (e.key === "Home") {
        mainContentEl.scrollTo({ top: 0, behavior: "auto" });
      } else if (e.key === "End") {
        mainContentEl.scrollTo({ top: mainContentEl.scrollHeight, behavior: "auto" });
      }
    }
  }

  function handleMouseNavigation(e: MouseEvent) {
    if (e.button === 3) {
      e.preventDefault();
      goBack();
    } else if (e.button === 4) {
      e.preventDefault();
      goForward();
    }
  }

  function handleWheelZoom(e: WheelEvent) {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    const direction: 1 | -1 = e.deltaY < 0 ? 1 : -1;
    adjustUiZoom(direction);
  }

  $effect(() => {
    const detachUndoRedo = attachAppUndoRedoListeners();
    return () => {
      detachUndoRedo();
    };
  });

  onMount(installFrontendDiagnostics);

  $effect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    initJobs((job) => {
      void sidebarRef?.refresh();
      notifyJobFinished(job, (pageId) => {
        void navigateToPage({ id: pageId });
      });
    })
      .then((fn) => {
        if (disposed) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((e) => {
        showToast(`Could not initialize background jobs: ${e instanceof Error ? e.message : String(e)}`, "error");
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  });

  $effect(() => {
    loadUiZoomPreference();
    window.addEventListener("keydown", handleGlobalKeydown, true);
    window.addEventListener("mouseup", handleMouseNavigation);
    window.addEventListener("wheel", handleWheelZoom, { passive: false });
    window.addEventListener("toggle-reference-panel", () => {
      referencePanelVisible = !referencePanelVisible;
    });
    // Snapshot the last location on window close so the next launch
    // can restore it. beforeunload fires reliably on Tauri window
    // close as well as on webview reloads.
    const persistOnClose = () => saveLastLocation();
    window.addEventListener("beforeunload", persistOnClose);
    return () => {
      window.removeEventListener("keydown", handleGlobalKeydown, true);
      window.removeEventListener("mouseup", handleMouseNavigation);
      window.removeEventListener("wheel", handleWheelZoom);
      window.removeEventListener("beforeunload", persistOnClose);
      clearRestoreTimer();
    };
  });

  // Also persist eagerly on every navigation so a hard crash doesn't
  // lose the "last location" — beforeunload alone isn't enough if the
  // webview dies unexpectedly.
  $effect(() => {
    currentView;
    currentPage;
    if (!hasInitialized) return;
    saveLastLocation();
  });

  // Navigate to tutorial welcome page on start (only once)
  let hasInitialized = false;
  $effect(() => {
    if (!hasInitialized) {
      hasInitialized = true;
      void navigateToStartupPage().catch((e) => {
        const message = `Startup navigation failed: ${errorText(e)}`;
        logNav("startup failed", { error: message });
        error = message;
        loading = false;
      });
      // Restore the theme and menu before mapping the native window.
      void Promise.all([initTheme(), restoreLayoutPreferences()]).then(revealStartupWindow).then(() => {
        requestAnimationFrame(() => {
          const sidebar = document.querySelector(".sidebar-container");
          uiLog(`[layout] ${JSON.stringify({
            sidebarVisible,
            sidebarDisplayed: !!sidebar && getComputedStyle(sidebar).display !== "none",
            wideMode,
            viewportWidth: window.innerWidth,
          })}`);
        });
      }).catch((e) => {
        const message = `Could not reveal the startup window: ${errorText(e)}`;
        console.error(message);
        uiLog(message);
      });
    }
  });

  // ─── Last-location persistence ────────────────────────────────
  // Restores whatever page/view was open on close. Journal entries
  // (both the scrolling journal feed and dated day-pages) are treated
  // as "today" on restore — yesterday's journal is rarely what you
  // want the next morning; today's is.
  const LAST_LOCATION_KEY = "grafium.session.lastLocation";

  type SavedLocation =
    | { kind: "page"; title: string }
    | { kind: "journal" }
    | { kind: "all-pages" | "flashcards" | "statistics" | "chat" | "settings" | "graph" | "jobs" | "notifications" };

  function saveLastLocation() {
    try {
      let payload: SavedLocation | null = null;
      if (currentView === "page" && currentPage) {
        payload = { kind: "page", title: currentPage.title };
      } else if (currentView === "journal") {
        payload = { kind: "journal" };
      } else if (
        currentView === "all-pages" ||
        currentView === "flashcards" ||
        currentView === "statistics" ||
        currentView === "chat" ||
        currentView === "settings" ||
        currentView === "graph" ||
        currentView === "jobs"
      ) {
        payload = { kind: currentView };
      }
      if (payload) {
        localStorage.setItem(LAST_LOCATION_KEY, JSON.stringify(payload));
      }
    } catch {
      // Ignore localStorage failures.
    }
  }

  function loadLastLocation(): SavedLocation | null {
    try {
      const raw = localStorage.getItem(LAST_LOCATION_KEY);
      if (!raw) return null;
      const parsed = JSON.parse(raw) as SavedLocation | null;
      if (!parsed || typeof parsed !== "object" || !("kind" in parsed)) return null;
      return parsed;
    } catch {
      return null;
    }
  }

  function todayJournalTitle(): string {
    const d = new Date();
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  }

  async function navigateToStartupPage() {
    // Prefer whatever the user was looking at when they closed the
    // app last. Journal entries always come back as today's journal.
    const saved = loadLastLocation();
    logNav("startup", { savedKind: saved?.kind ?? null, savedTitle: saved?.kind === "page" ? saved.title : null });
    if (saved) {
      try {
        if (saved.kind === "journal") {
          await navigateToJournal();
          return;
        }
        if (saved.kind === "page") {
          if (isJournalDateTitle(saved.title)) {
            // Was a dated journal page → open today's date instead.
            await navigateToPage(todayJournalTitle(), true);
            return;
          }
          await navigateToPage(saved.title);
          return;
        }
        if (saved.kind === "all-pages") {
          await navigateToPage("__all_pages__");
          return;
        }
        if (saved.kind === "graph") {
          await navigateToPage("__graph__");
          return;
        }
        if (saved.kind === "flashcards") {
          await navigateToPage("__flashcards__");
          return;
        }
        if (saved.kind === "statistics") {
          await navigateToPage("__statistics__");
          return;
        }
        if (saved.kind === "chat") {
          await navigateToPage("__chat__");
          return;
        }
        if (saved.kind === "jobs" || saved.kind === "notifications") {
          await navigateToPage("__jobs__");
          return;
        }
        // Deliberately don't auto-open settings — nobody wants to
        // land on the settings screen on every launch.
      } catch (e) {
        logNav("startup restore failed", { saved, error: errorText(e) });
        // Fall through to the legacy defaults if restoring failed.
      }
    }

    try {
      // Only open Welcome when it already exists (tutorial graph).
      await withNavigationTimeout(getPage({ title: "Welcome To Grafium" }), "Checking startup page");
      await navigateToPage("Welcome To Grafium");
      return;
    } catch {
      // Fallback for non-tutorial/custom graphs.
      await navigateToJournal();
    }
  }

  async function initTheme() {
    // Register listener first — must always succeed regardless of saved theme state
    listen<{ theme: string }>("smplos-theme-changed", (event) => {
      const t = getThemeById(event.payload.theme);
      if (t) {
        applyTheme(t.colors);
      }
    });

    // Apply saved/smplos theme on startup
    try {
      const [appTheme, smplosTheme] = await Promise.all([getAppTheme(), getSmplosTheme()]);
      const themeId = appTheme === "auto" ? (smplosTheme ?? defaultAutoThemeId()) : appTheme;
      const theme = getThemeById(themeId);
      if (theme) {
        applyTheme(theme.colors);
      }
    } catch (e) {
      // If theme commands fail, fall back to smplos or default
      try {
        const smplos = await getSmplosTheme();
        const t = getThemeById(smplos ?? defaultAutoThemeId());
        if (t) applyTheme(t.colors);
      } catch (_) {}
    }
  }

  async function goJournalAndEdit() {
    journalEditTodayRequestId += 1;
    await navigateToJournal();
  }

  function insertEditorSnippet(text: string) {
    if (tryInsertIntoActiveEditor(text)) return;
    const pageTitle = currentView === "journal"
      ? (journalActivePage?.title ?? formatLocalIsoDate())
      : currentPage?.title;
    if (!pageTitle) return;
    dispatchEditPageEnd({ pageTitle, insert: text });
  }

  async function navigateToJournal(skipHistory = false, restoreEntry?: HistoryEntry) {
    if (!skipHistory) {
      saveCurrentHistoryState();
    }
    pendingJournalRestore = restoreEntry ?? null;
    if (restoreEntry) {
      journalRestoreRequestId += 1;
    }
    error = null;
    currentView = "journal";
    currentPage = null;
    loading = false;
    if (!skipHistory) {
      pushHistoryEntry({ kind: "journal", scrollTop: 0 });
    }
    await tick();
    if (restoreEntry) {
      restoreHistoryState(restoreEntry);
    }
  }

  async function navigateToPage(
    target: PageNavigationTarget,
    isJournal = false,
    skipHistory = false,
    restoreEntry?: HistoryEntry,
    sourceBlockId?: string,
    sourcePageTitle?: string
  ) {
    if (!skipHistory) {
      saveCurrentHistoryState(sourceBlockId, sourcePageTitle);
    }

    // Handle special routes
    if (target === "__all_pages__") {
      currentView = "all-pages";
      currentPage = null;
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "all-pages", scrollTop: 0 });
      }
      await tick();
      if (restoreEntry) {
        restoreHistoryState(restoreEntry);
      }
      return;
    }
    if (target === "__graph__") {
      currentView = "graph";
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "graph", scrollTop: 0 });
      }
      await tick();
      return;
    }
    if (target === "__flashcards__") {
      currentView = "flashcards";
      currentPage = null;
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "flashcards", scrollTop: 0 });
      }
      await tick();
      if (restoreEntry) {
        restoreHistoryState(restoreEntry);
      }
      return;
    }
    if (target === "__statistics__") {
      currentView = "statistics";
      currentPage = null;
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "statistics", scrollTop: 0 });
      }
      await tick();
      if (restoreEntry) {
        restoreHistoryState(restoreEntry);
      }
      return;
    }
    if (target === "__settings__") {
      currentView = "settings";
      currentPage = null;
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "settings", scrollTop: 0 });
      }
      await tick();
      if (restoreEntry) {
        restoreHistoryState(restoreEntry);
      }
      return;
    }
    if (target === "__chat__") {
      expandedConversationId = restoreEntry?.conversationId ?? null;
      currentView = "chat";
      currentPage = null;
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "chat", scrollTop: 0, conversationId: expandedConversationId });
      }
      await tick();
      if (restoreEntry) {
        restoreHistoryState(restoreEntry);
      }
      return;
    }
    if (target === "__jobs__" || target === "__notifications__") {
      currentView = "jobs";
      currentPage = null;
      error = null;
      loading = false;
      if (!skipHistory) {
        pushHistoryEntry({ kind: "jobs", scrollTop: 0 });
      }
      await tick();
      if (restoreEntry) {
        restoreHistoryState(restoreEntry);
      }
      return;
    }

    const pageLookup = resolvePageLookup(target);

    loading = true;
    error = null;
    logNav("navigate start", { pageLookup });
    try {
      // Try to get existing page
      currentPage = await withNavigationTimeout(getPage(pageLookup), "Loading page");
      if (!currentPage.file_path) {
        currentPage = (await loadLegacyBookIndexPage(pageLookup.title)) ?? currentPage;
      }
    } catch (e) {
      if (!isPageNotFoundError(e)) {
        error = `Failed to load page: ${errorText(e)}`;
        logNav("navigate failed", { pageLookup, error });
        loading = false;
        return;
      }
      currentPage = await loadLegacyBookIndexPage(pageLookup.title);
      if (currentPage) {
        error = null;
      } else {
        // Create it if it doesn't exist
        if (!pageLookup.title) {
          error = `Failed to load page: ${errorText(e)}`;
          logNav("navigate failed", { pageLookup, error });
          loading = false;
          return;
        }
        try {
          currentPage = await withNavigationTimeout(
            createPage(
              pageLookup.title,
              isJournal || /^\d{4}-\d{2}-\d{2}$/.test(pageLookup.title)
            ),
            "Creating page"
          );
        } catch (e) {
          error = `Failed to load page: ${errorText(e)}`;
          logNav("navigate failed", { pageLookup, error });
          loading = false;
          return;
        }
      }
    }

    if (currentPage) {
      recordPageOpen(currentPage.id).catch(() => {});
    }

    currentView = "page";
    loading = false;
    logNav("navigate loaded", { pageId: currentPage.id, title: currentPage.title });
    if (!skipHistory) {
      pushHistoryEntry({
        kind: "page",
        title: currentPage.title,
        scrollTop: 0,
        sourceBlockId,
        sourcePageTitle,
      });
    }
    await tick();
    if (restoreEntry) {
      restoreHistoryState(restoreEntry);
    } else if (mainContentEl) {
      mainContentEl.scrollTop = 0;
    }
  }

  let showNewPageDialog = $state(false);
  let newPageName = $state("");
  let showMoreMenu = $state(false);
  let showCreateGraphDialog = $state(false);
  let newGraphName = $state("");

  /// Term to highlight on the page being navigated to, set by callers that
  /// know why the user is going there (the graph passes its active filter).
  /// Cleared on every navigation so a stale term can't follow the user around.
  let pendingHighlight = $state("");

  function handleNavigate(target: PageNavigationTarget, highlight = "") {
    pendingHighlight = highlight;
    if (target === "__journal__") {
      navigateToJournal();
      return;
    }
    if (target === "__new_page__") {
      newPageName = "";
      showNewPageDialog = true;
      return;
    }
    if (target === "__import_media__") {
      openImportMediaDialog();
      return;
    }
    if (target === "__import_media_journal__") {
      openImportMediaDialog("journal");
      return;
    }
    if (target === "__import_books__") {
      void openImportBooksDirectory();
      return;
    }
    if (target === "__jobs__" || target === "__notifications__") {
      navigateToPage("__jobs__");
      return;
    }
    navigateToPage(target);
  }

  async function handleFindLinksForPage(page: Pick<Page, "id">, exactOnly = false) {
    pendingHighlight = "";
    if (currentView !== "page" || currentPage?.id !== page.id) {
      await navigateToPage({ id: page.id });
    } else {
      await tick();
    }
    if (currentView !== "page" || currentPage?.id !== page.id) return;
    window.dispatchEvent(new CustomEvent("page-content-find-links", {
      detail: { pageId: page.id, exactOnly },
    }));
  }

  function legacyBookIndexTitle(title: string | undefined): string | null {
    if (!title) return null;
    const parts = title.split("/").filter(Boolean);
    return parts.length === 2 && parts[0] === "Books" ? `${title}/index` : null;
  }

  async function loadLegacyBookIndexPage(title: string | undefined): Promise<Page | null> {
    const legacyIndexTitle = legacyBookIndexTitle(title);
    if (!legacyIndexTitle) return null;
    try {
      const page = await withNavigationTimeout(
        getPage({ title: legacyIndexTitle }),
        "Loading book index"
      );
      logNav("navigate redirected to legacy book index", {
        requestedTitle: title,
        indexTitle: legacyIndexTitle,
      });
      return page;
    } catch {
      return null;
    }
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

  // Import from media (video/audio -> transcript page): the dialog only starts
  // the background job; progress and the finished-page link live in Jobs.
  let showImportMediaDialog = $state(false);
  let importMediaUrl = $state("");
  let importMediaBusy = $state(false);
  let importMediaError = $state("");
  let importMediaProgress = $state("");
  let importMediaTarget: "new_page" | "journal" = $state("new_page");

  function openImportMediaDialog(defaultTarget: "new_page" | "journal" = "new_page") {
    importMediaUrl = "";
    importMediaError = "";
    importMediaProgress = "";
    importMediaBusy = false;
    importMediaTarget = defaultTarget;
    showImportMediaDialog = true;
  }

  function cancelImportMedia() {
    if (importMediaBusy) return;
    showImportMediaDialog = false;
    importMediaUrl = "";
    importMediaError = "";
  }

  async function submitImportMedia() {
    const url = importMediaUrl.trim();
    if (!url || importMediaBusy) return;
    importMediaBusy = true;
    importMediaError = "";
    importMediaProgress = "Adding media import job...";
    try {
      await mediaImportVideo(url, undefined, undefined, importMediaTarget);
      showImportMediaDialog = false;
      importMediaUrl = "";
      showToast("Media import job added", "info");
    } catch (e) {
      importMediaError = e instanceof Error ? e.message : String(e);
    } finally {
      importMediaBusy = false;
      importMediaProgress = "";
    }
  }

  function handleImportMediaKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") submitImportMedia();
    if (e.key === "Escape") cancelImportMedia();
  }

  let importBooksBusy = $state(false);

  async function openImportBooksDirectory() {
    if (importBooksBusy) return;
    importBooksBusy = true;
    try {
      const defaultPath = await defaultExternalBookImportFolder();
      const dir = await pickFolder(
        "Select Folder Containing Book Files",
        defaultPath
      );
      if (!dir) return;
      await bookImportDirectory(dir);
      showToast("Book import job added", "info");
    } catch (e) {
      showToast(`Could not start book import: ${e instanceof Error ? e.message : String(e)}`, "error");
    } finally {
      importBooksBusy = false;
    }
  }

  function toggleMoreMenu() {
    showMoreMenu = !showMoreMenu;
  }

  function closeMoreMenu() {
    showMoreMenu = false;
  }

  async function runReindex(showSuccessAlert = false) {
    try {
      await reindexCurrent();
      handleGraphChanged();
      if (showSuccessAlert) {
        alert("Graph re-index complete.");
      }
    } catch (e) {
      console.error("[graph] reindex error:", e);
      alert("Re-index failed: " + e);
    }
  }

  async function handleMobileReindex() {
    closeMoreMenu();
    await runReindex(true);
  }

  async function handleMobileOpenGraph() {
    closeMoreMenu();
    console.log("[graph] opening folder picker for Open Graph");
    const selected = await pickFolder();
    console.log("[graph] pickFolder returned:", selected);
    if (selected) {
      try {
        const report = await validateGraph(selected);
        if (!report.is_valid) {
          const missing = [
            !report.has_pages_dir    && "pages/",
            !report.has_journals_dir && "journals/",
            !report.has_metadata_dir && "metadata/",
            !report.has_valid_db     && "metadata/index.db (corrupted)",
          ].filter(Boolean).join(", ");
          alert(
            `Not a valid Grafium graph.\n\n` +
            `Missing: ${missing}\n\n` +
            `Use New Graph to create a graph here instead.`
          );
          return;
        }
        await openGraph(selected);
        handleGraphChanged();
      } catch (e) {
        console.error("[graph] openGraph error:", e);
        alert("Failed to open graph: " + e);
      }
    }
  }

  function handleMobileCreateGraph() {
    closeMoreMenu();
    newGraphName = "";
    showCreateGraphDialog = true;
  }

  async function confirmCreateGraph() {
    if (!newGraphName.trim()) return;
    console.log("[graph] opening folder picker for Create Graph");
    showCreateGraphDialog = false;
    const selected = await pickFolder();
    console.log("[graph] pickFolder returned:", selected);
    if (selected) {
      const graphPath = selected + "/" + newGraphName.trim();
      try {
        await createGraph(graphPath, newGraphName.trim());
        handleGraphChanged();
      } catch (e) {
        console.error("[graph] createGraph error:", e);
        alert("Failed to create graph: " + e);
      }
    }
  }

  function cancelCreateGraph() {
    showCreateGraphDialog = false;
    newGraphName = "";
  }

  function handleGraphChanged() {
    goToLinkOpen = false;
    globalSearchOpen = false;
    expandedConversationId = null;
    void import("./lib/assistantConversations")
      .then(({ stopAllAssistantConversations }) => stopAllAssistantConversations())
      .catch((error) => showToast(`Could not stop previous Chat requests: ${errorText(error)}`, "error"));
    readingNoteFocus = { pageId: "", label: "", trigger: readingNoteFocus.trigger + 1 };
    readingSelection.set({ selection: null, error: null, pageIds: [] });
    // Do not carry a previous graph's conversation into the new graph.
    chatVisited = false;
    // Reload after graph switch — bump request ID so JournalView's $effect re-fires
    journalRestoreRequestId += 1;
    navigateToJournal();
  }

  // Listen for page navigation events from rendered content
  function handlePageNav(e: Event) {
    const detail = (e as CustomEvent<string | LinkNavigateDetail>).detail;
    if (typeof detail === "string") {
      navigateToPage(detail);
      return;
    }
    const target: PageNavigationTarget = detail.pageId ? { id: detail.pageId } : detail.pageName;

    if (detail.targetBlockId) {
      const restoreEntry: HistoryEntry = {
        kind: "page",
        title: detail.pageName,
        scrollTop: 0,
        sourceBlockId: detail.targetBlockId,
        sourcePageTitle: detail.sourcePageTitle ?? detail.pageName,
      };
      navigateToPage(
        target,
        false,
        false,
        restoreEntry,
        detail.sourceBlockId,
        detail.sourcePageTitle
      );
      return;
    }

    navigateToPage(target, false, false, undefined, detail.sourceBlockId, detail.sourcePageTitle);
  }

  function handleReadingNoteNav(event: Event) {
    const detail = (event as CustomEvent<{ pageId: string; footnoteLabel: string; blockId?: string }>).detail;
    if (!detail?.pageId || !/^grafium-note-[1-9]\d*$/.test(detail.footnoteLabel)) {
      showToast("This reading-note reference is invalid.", "error");
      return;
    }
    if (detail.blockId) setCurrentBlockAnchor(detail.pageId, detail.blockId);
    readingNoteFocus = {
      pageId: detail.pageId, label: detail.footnoteLabel, trigger: readingNoteFocus.trigger + 1,
    };
    openReferencePanelTab("notes");
  }

  $effect(() => {
    window.addEventListener("navigate-page", handlePageNav);
    window.addEventListener("open-reading-note", handleReadingNoteNav);
    return () => {
      window.removeEventListener("navigate-page", handlePageNav);
      window.removeEventListener("open-reading-note", handleReadingNoteNav);
    };
  });

  $effect(() => {
    loadSidebarWidthPreference();
    loadReferencePanelWidthPreference();
    loadGraphViewModePreference();
    loadShowBlockGuidesPreference();
    loadNarrowPaddingPreference();
    loadBionicReaderPreference();
  });
</script>

<div class="app-shell" class:zen={zenMode} class:wide-mode={wideMode}>
  {#if !zenMode}
    <TitleBar
      {sidebarVisible}
      {uiZoom}
      canGoBack={navIndex > 0}
      canGoForward={navIndex < navHistory.length - 1}
      onGoBack={goBack}
      onGoForward={goForward}
      onToggleReferencePanel={() => (referencePanelVisible = !referencePanelVisible)}
      onOpenSearch={openGlobalSearch}
      onOpenSettings={() => navigateToPage("__settings__")}
      bionicReaderMode={bionicReaderMode}
      onToggleBionicReader={toggleBionicReader}
      onZoomIn={() => adjustUiZoom(1)}
      onZoomOut={() => adjustUiZoom(-1)}
      onZoomReset={resetUiZoom}
    />
  {/if}
  <div class="app-layout" bind:this={appLayoutEl}>
    {#if sidebarVisible && !zenMode}
      <div class="sidebar-container" style={`width: ${sidebarWidth}px;`}>
        <Sidebar
          bind:this={sidebarRef}
          {currentPage}
          {sidebarWidth}
          onNavigate={handleNavigate}
          onGraphChanged={handleGraphChanged}
        />
      </div>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="sidebar-resizer"
        class:resizing={isResizingSidebar}
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize sidebar"
        onpointerdown={startSidebarResize}
        ondblclick={resetSidebarWidth}
      ></div>
    {/if}

    <main bind:this={mainContentEl} class="main-content" class:zen-content={zenMode}>
    {#if error}
      <div class="error-state">
        <p>{error}</p>
        <button onclick={() => navigateToJournal()}>Retry</button>
      </div>
    {:else if loading}
      <div class="loading">Loading...</div>
    {:else if currentView === "all-pages"}
      <LazyView load={loadAllPages} name="all pages">
        {#snippet children(AllPages)}
          <AllPages onNavigate={handleNavigate} onPageDeleted={() => { void sidebarRef?.refresh(); }} />
        {/snippet}
      </LazyView>
    {:else if currentView === "graph"}
      <div class="graph-view-wrapper">
        <div class="graph-renderer-toggle">
          <button class:active={graphViewMode === "2d"} onclick={() => setGraphViewMode("2d")}>2D</button>
          <button class:active={graphViewMode === "3d"} onclick={() => setGraphViewMode("3d")}>3D</button>
        </div>
        {#if graphViewMode === "3d"}
          <LazyView load={loadGraphView3D} name="3D graph">
            {#snippet children(GraphView3D)}
              <GraphView3D
                onNavigate={handleNavigate}
                currentPageId={currentPage?.id ?? ""}
                currentPageTitle={currentPage?.title ?? ""}
              />
            {/snippet}
          </LazyView>
        {:else}
          <LazyView load={loadGraphView} name="graph">
            {#snippet children(GraphView)}
              <GraphView
                onNavigate={handleNavigate}
                currentPageId={currentPage?.id ?? ""}
                currentPageTitle={currentPage?.title ?? ""}
              />
            {/snippet}
          </LazyView>
        {/if}
      </div>
    {:else if currentView === "statistics"}
      <LazyView load={loadStatistics} name="statistics">
        {#snippet children(Statistics)}
          <Statistics onNavigate={handleNavigate} />
        {/snippet}
      </LazyView>
    {:else if currentView === "flashcards"}
      <LazyView load={loadFlashcardReview} name="flashcards">
        {#snippet children(FlashcardReview)}
          <FlashcardReview onNavigate={handleNavigate} />
        {/snippet}
      </LazyView>
    {:else if currentView === "settings"}
      <LazyView load={loadSettings} name="settings">
        {#snippet children(Settings)}
          <Settings
            {showBlockGuides}
            onSetShowBlockGuides={setShowBlockGuides}
            {narrowPaddingPct}
            onSetNarrowPaddingPct={setNarrowPaddingPct}
            openSection={settingsOpenSection}
          />
        {/snippet}
      </LazyView>
    {:else if currentView === "jobs"}
      <LazyView load={loadJobsView} name="jobs">
        {#snippet children(JobsView)}
          <JobsView onOpenPage={(link) => navigateToPage(link.page_title ? { title: link.page_title } : { id: link.page_id })} />
        {/snippet}
      </LazyView>
    {:else if currentView === "journal"}
      <JournalView
        onGoToLink={openGoToLink}
        openCalendar={journalCalendarRequested}
        onCalendarOpened={() => (journalCalendarRequested = false)}
        restorePageTitle={pendingJournalRestore?.sourcePageTitle}
        restoreRequestId={journalRestoreRequestId}
        editTodayRequestId={journalEditTodayRequestId}
        {showBlockGuides}
        onNavigate={handleNavigate}
        onActivePageChange={(page) => (journalActivePage = page)}
        onPageDeleted={() => { void sidebarRef?.refresh(); }}
      />
    {:else if currentView === "page" && currentPage}
      {#key currentPage.id}
        <PageContent
          page={currentPage}
          highlight={pendingHighlight}
          {showBlockGuides}
          onPageRenamed={(page) => {
            const previous = currentPage;
            currentPage = page;
            if (!previous || (previous.id === page.id && previous.title === page.title)) return;
            navHistory = navHistory.map((entry) => {
              let next = entry;
              if (entry.kind === "page" && entry.title === previous.title) {
                next = { ...next, title: page.title };
              }
              if (entry.sourcePageTitle === previous.title) {
                next = { ...next, sourcePageTitle: page.title };
              }
              return next;
            });
          }}
          onPageDeleted={(parentTitle) => {
            void sidebarRef?.refresh();
            if (parentTitle) {
              void navigateToPage(parentTitle);
            } else {
              void navigateToPage("__all_pages__");
            }
          }}
        />
      {/key}
    {/if}
    {#if chatVisited}
      <div class="chat-session" hidden={!chatActive} inert={!chatActive}>
        <LazyView load={loadChatView} name="chat">
          {#snippet children(ChatView)}
            <ChatView active={chatActive} conversationId={expandedConversationId}
              onOpenSettings={() => handleNavigate("__settings__")}
              onNavigate={handleNavigate}
              onFindLinks={handleFindLinksForPage} />
          {/snippet}
        </LazyView>
      </div>
    {/if}
    </main>

    <!-- Chat and reading notes -->
    {#if referencePanelVisible}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="reference-panel-resizer"
        class:resizing={isResizingReferencePanel}
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize right panel"
        style="position:fixed;top:0;bottom:0;right:{referencePanelWidth - 3}px;width:6px;z-index:10000;"
        onpointerdown={startReferencePanelResize}
        ondblclick={resetReferencePanelWidth}
      ></div>
      <LazyView load={loadReferencePanel} name="Chat and notes">
        {#snippet children(ReferencePanel)}
          <ReferencePanel
            visible={true}
            pageId={assistantSourcePage?.id ?? ""}
            pageTitle={assistantSourcePage?.title ?? ""}
            initialTab={referencePanelTab}
            conversationId={currentView === "chat" ? expandedConversationId : null}
            focusTrigger={referencePanelFocusTrigger}
            noteFocusPageId={readingNoteFocus.pageId}
            noteFocusLabel={readingNoteFocus.label}
            noteFocusTrigger={readingNoteFocus.trigger}
            width={referencePanelWidth}
            preferFocusedPageForPageScope={currentView === "journal"}
            onClose={() => (referencePanelVisible = false)}
            onNavigate={(target) => { referencePanelVisible = false; handleNavigate(target); }}
            onFindLinks={handleFindLinksForPage}
            onExpandConversation={expandAssistantConversation}
            onOpenSettings={() => handleNavigate("__settings__")}
          />
        {/snippet}
      </LazyView>
    {/if}

    <!-- Bottom nav for narrow screens -->
    <nav class="bottom-nav">
      <button class="bottom-nav-item" class:active={currentView === "journal"} onclick={() => handleNavigate("__journal__")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <rect x="3" y="4" width="18" height="18" rx="2" ry="2"></rect>
          <line x1="16" y1="2" x2="16" y2="6"></line>
          <line x1="8" y1="2" x2="8" y2="6"></line>
          <line x1="3" y1="10" x2="21" y2="10"></line>
        </svg>
        <span>Journal</span>
      </button>
      <button class="bottom-nav-item" class:active={currentView === "statistics"} onclick={() => handleNavigate("__statistics__")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M18 20V10"></path>
          <path d="M12 20V4"></path>
          <path d="M6 20v-6"></path>
        </svg>
        <span>Tasks</span>
      </button>
      <button class="bottom-nav-item" class:active={currentView === "all-pages"} onclick={() => handleNavigate("__all_pages__")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
          <polyline points="14 2 14 8 20 8"></polyline>
        </svg>
        <span>Pages</span>
      </button>
      <button class="bottom-nav-item" class:active={currentView === "graph"} onclick={() => handleNavigate("__graph__")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="5" cy="6" r="2"></circle>
          <circle cx="19" cy="6" r="2"></circle>
          <circle cx="12" cy="18" r="2"></circle>
          <line x1="6.7" y1="7" x2="10.5" y2="16.3"></line>
          <line x1="17.3" y1="7" x2="13.5" y2="16.3"></line>
          <line x1="7" y1="6" x2="17" y2="6"></line>
        </svg>
        <span>Graph</span>
      </button>
      <button class="bottom-nav-item" class:active={currentView === "chat"} onclick={() => handleNavigate("__chat__")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
        </svg>
        <span>Chat</span>
      </button>
      <button class="bottom-nav-item" onclick={() => handleNavigate("__new_page__")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <line x1="12" y1="5" x2="12" y2="19"></line>
          <line x1="5" y1="12" x2="19" y2="12"></line>
        </svg>
        <span>New</span>
      </button>
      <button class="bottom-nav-item" class:active={referencePanelVisible} onclick={() => (referencePanelVisible = !referencePanelVisible)}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"></path>
          <path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"></path>
        </svg>
        <span>AI</span>
      </button>
      <button class="bottom-nav-item" class:active={showMoreMenu || currentView === "settings" || currentView === "jobs"} onclick={toggleMoreMenu}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="5" r="1"></circle>
          <circle cx="12" cy="12" r="1"></circle>
          <circle cx="12" cy="19" r="1"></circle>
        </svg>
        <span>More</span>
      </button>
    </nav>

    <!-- More menu popup -->
    {#if showMoreMenu}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="more-menu-backdrop" onclick={closeMoreMenu}></div>
      <div class="more-menu">
        <button class="more-menu-item" onclick={() => { closeMoreMenu(); handleNavigate("__settings__"); }}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <circle cx="12" cy="12" r="3"></circle>
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
          </svg>
          <span>Settings</span>
        </button>
        <button class="more-menu-item" onclick={() => { closeMoreMenu(); handleNavigate("__chat__"); }}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
          </svg>
          <span>Chat</span>
        </button>
        <button class="more-menu-item" onclick={() => { closeMoreMenu(); handleNavigate("__jobs__"); }}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 7h18s-3 0-3-7"></path>
            <path d="M13.73 21a2 2 0 0 1-3.46 0"></path>
          </svg>
          <span>Jobs</span>
        </button>
        <button class="more-menu-item" onclick={handleMobileOpenGraph}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
          </svg>
          <span>Open Graph</span>
        </button>
        <button class="more-menu-item" onclick={() => { closeMoreMenu(); handleNavigate("__import_media__"); }}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <polygon points="23 7 16 12 23 17 23 7"></polygon>
            <rect x="1" y="5" width="15" height="14" rx="2" ry="2"></rect>
          </svg>
          <span>Import Media</span>
        </button>
        <button class="more-menu-item" onclick={() => { closeMoreMenu(); handleNavigate("__import_books__"); }}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"></path>
            <path d="M4 4.5A2.5 2.5 0 0 1 6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5z"></path>
          </svg>
          <span>Import Books</span>
        </button>
        <button class="more-menu-item" onclick={handleMobileCreateGraph}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <line x1="12" y1="5" x2="12" y2="19"></line>
            <line x1="5" y1="12" x2="19" y2="12"></line>
          </svg>
          <span>New Graph</span>
        </button>
        <button class="more-menu-item" onclick={handleMobileReindex}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <polyline points="23 4 23 10 17 10"></polyline>
            <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"></path>
          </svg>
          <span>Re-index Graph (Manual)</span>
        </button>
      </div>
    {/if}
  </div>
</div>

{#if goToLinkOpen}
  <GoToLink
    onCancel={() => (goToLinkOpen = false)}
    onSelect={(page) => {
      goToLinkOpen = false;
      handleNavigate({ id: page.id });
    }}
  />
{/if}

{#if globalSearchOpen}
  <LazyView load={loadGlobalSearchDialog} name="search">
    {#snippet children(GlobalSearchDialog)}
      <GlobalSearchDialog open={true} onClose={() => (globalSearchOpen = false)}
        onNavigate={(target) => { globalSearchOpen = false; handleNavigate(target); }}
        onOpenSettings={() => { globalSearchOpen = false; handleNavigate("__settings__"); }} />
    {/snippet}
  </LazyView>
{/if}

{#if currentView !== "jobs"}
  <JobActivity />
{/if}
{#if commandPaletteOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="command-palette-backdrop" onclick={() => (commandPaletteOpen = false)}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="command-palette" onclick={(e) => e.stopPropagation()}>
      <input
        class="command-palette-input"
        type="text"
        placeholder="Run a command…"
        bind:value={commandPaletteQuery}
        oninput={() => (commandPaletteIndex = 0)}
        autofocus
      />
      <div class="command-palette-list">
        {#each commandPaletteRows as row, index}
          <button
            class="command-palette-item"
            class:active={index === commandPaletteIndex}
            onclick={() => runCommandPaletteRow(index)}
          >
            <span class="command-palette-desc">{row.description}</span>
            <span class="command-palette-keys">
              {formatBindingList([...(row.chords.length ? row.chords : []), ...(row.modifiers.length ? row.modifiers : [])])}
            </span>
          </button>
        {:else}
          <div class="command-palette-empty">No matching commands</div>
        {/each}
      </div>
    </div>
  </div>
{/if}

<Toaster />

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

{#if showImportMediaDialog}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="dialog-backdrop" onclick={cancelImportMedia}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog" onclick={(e) => e.stopPropagation()}>
      <h3 class="dialog-title">Import from Video/Audio</h3>
      <p class="dialog-description">
        Paste a YouTube (or other yt-dlp-supported) URL, or a local file path. Captions are used
        if available; otherwise it falls back to local Whisper transcription if enabled in
        Settings.
      </p>
      <label class="dialog-label" for="import-media-target">Save as</label>
      <select id="import-media-target" class="dialog-input" bind:value={importMediaTarget} disabled={importMediaBusy}>
        <option value="new_page">New page</option>
        <option value="journal">Add to today's journal</option>
      </select>
      <input
        type="text"
        class="dialog-input"
        placeholder="https://youtube.com/watch?v=... or /path/to/video.mp4"
        bind:value={importMediaUrl}
        onkeydown={handleImportMediaKeydown}
        disabled={importMediaBusy}
        autofocus
      />
      {#if importMediaBusy && importMediaProgress}
        <pre class="dialog-progress">{importMediaProgress}</pre>
      {/if}
      {#if importMediaError}
        <p class="dialog-error">{importMediaError}</p>
      {/if}
      <div class="dialog-actions">
        <button class="dialog-btn dialog-btn-cancel" onclick={cancelImportMedia} disabled={importMediaBusy}>Cancel</button>
        <button class="dialog-btn dialog-btn-ok" onclick={submitImportMedia} disabled={importMediaBusy || !importMediaUrl.trim()}>
          {importMediaBusy ? "Importing…" : "Import"}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if showFolderBrowser}
  <FolderBrowser
    title={folderBrowserTitle}
    onSelect={(path) => finishFolderBrowser(path)}
    onCancel={() => finishFolderBrowser(null)}
  />
{/if}

{#if showCreateGraphDialog}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="dialog-backdrop" onclick={cancelCreateGraph}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog" onclick={(e) => e.stopPropagation()}>
      <h3 class="dialog-title">Create New Graph</h3>
      <p class="dialog-hint">Enter a name, then choose where to save it.</p>
      <input
        type="text"
        class="dialog-input"
        placeholder="Graph name..."
        bind:value={newGraphName}
        onkeydown={(e) => { if (e.key === "Enter") confirmCreateGraph(); if (e.key === "Escape") cancelCreateGraph(); }}
        autofocus
      />
      <div class="dialog-actions">
        <button class="dialog-btn dialog-btn-cancel" onclick={cancelCreateGraph}>Cancel</button>
        <button class="dialog-btn dialog-btn-ok" onclick={confirmCreateGraph} disabled={!newGraphName.trim()}>Choose Folder...</button>
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
    min-width: 0;
  }

  .sidebar-container {
    flex: 0 0 auto;
    min-width: 0;
    overflow: hidden;
  }

  .sidebar-container :global(.sidebar) {
    width: 100%;
    min-width: 0;
  }

  .sidebar-resizer {
    flex: 0 0 6px;
    cursor: col-resize;
    position: relative;
    background: transparent;
    border-left: 1px solid var(--border);
  }

  .sidebar-resizer::after {
    content: "";
    position: absolute;
    top: 0;
    left: 2px;
    width: 1px;
    height: 100%;
    background: color-mix(in srgb, var(--text-muted) 28%, transparent);
    opacity: 0;
    transition: opacity 0.12s ease;
  }

  .sidebar-resizer:hover::after,
  .sidebar-resizer.resizing::after {
    opacity: 1;
  }

  .graph-view-wrapper {
    position: relative;
    height: 100%;
    width: 100%;
  }

  .graph-renderer-toggle {
    position: absolute;
    top: 12px;
    left: 12px;
    z-index: 20;
    display: flex;
    gap: 2px;
    background: var(--bg-secondary, #1e1e2e);
    border: 1px solid var(--border-color, #333);
    border-radius: 6px;
    padding: 2px;
  }

  .graph-renderer-toggle button {
    padding: 4px 10px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-secondary, #aaa);
    cursor: pointer;
    font-size: 12px;
  }

  .graph-renderer-toggle button.active {
    background: var(--accent, #6ea8fe);
    color: #0b0b10;
  }

  .reference-panel-resizer {
    cursor: col-resize;
    background: transparent;
  }

  .reference-panel-resizer::after {
    content: "";
    position: absolute;
    top: 0;
    left: 2px;
    width: 1px;
    height: 100%;
    background: color-mix(in srgb, var(--text-muted) 28%, transparent);
    opacity: 0;
    transition: opacity 0.12s ease;
  }

  .reference-panel-resizer:hover::after,
  .reference-panel-resizer.resizing::after {
    opacity: 1;
  }

  .main-content {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    background: var(--bg-primary);
    position: relative;
    min-width: 0;
    overflow-wrap: break-word;
    word-break: break-word;
    padding: 0;
    margin: 0;
  }

  .chat-session {
    height: 100%;
    min-height: 0;
  }

  .chat-session[hidden] {
    display: none;
  }

  .app-shell.zen {
    background: var(--bg-primary);
  }

  .main-content.zen-content {
    padding: 0;
  }

  .main-content.zen-content :global(.page-content) {
    max-width: 100%;
    padding: 2px 4px;
    margin: 0;
  }

  .main-content.zen-content :global(.journal-view) {
    max-width: 100%;
    padding: 2px 4px;
  }

  .main-content.zen-content :global(.backlinks-section) {
    display: none;
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
    background: var(--btn-primary-bg);
    color: var(--btn-primary-fg);
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
    z-index: 2500;
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

  .dialog-hint {
    font-size: 12px;
    color: var(--text-muted);
    margin-bottom: 12px;
    font-family: monospace;
    word-break: break-all;
  }

  .dialog-description {
    font-size: 12px;
    color: var(--text-muted);
    margin: 0 0 12px 0;
    line-height: 1.4;
  }

  .dialog-error {
    font-size: 12px;
    color: var(--error-color, #e57373);
    margin: 8px 0 0 0;
  }

  .dialog-progress {
    font-size: 12px;
    color: var(--text-secondary, #999);
    margin: 8px 0 0 0;
    font-style: italic;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: inherit;
    max-height: 220px;
    overflow-y: auto;
    line-height: 1.4;
  }

  .dialog-label {
    display: block;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-secondary, #999);
    margin: 0 0 6px 0;
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

  .graph-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 12px;
    max-height: 200px;
    overflow-y: auto;
  }

  .graph-list-item {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding: 10px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    transition: background 0.1s;
  }

  .graph-list-item:hover {
    background: var(--bg-hover);
  }

  .graph-list-name {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-primary);
  }

  .graph-list-path {
    font-size: 11px;
    color: var(--text-muted);
    font-family: monospace;
    word-break: break-all;
  }

  /* Bottom nav - hidden by default, shown on narrow screens */
  .bottom-nav {
    display: none;
  }

  @media (max-width: 640px) {
    .sidebar-resizer,
    .reference-panel-resizer {
      display: none !important;
    }

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
      justify-content: space-around;
      align-items: center;
    }

    .bottom-nav-item {
      display: flex;
      flex-direction: column;
      align-items: center;
      gap: 2px;
      background: none;
      border: none;
      color: var(--text-muted);
      cursor: pointer;
      padding: 6px 12px;
      border-radius: 8px;
      font-size: 0.6rem;
      transition: color 0.15s;
    }

    .bottom-nav-item.active {
      color: var(--accent);
    }

    .bottom-nav-item:hover {
      color: var(--text-primary);
    }

    /* More menu popup */
    .more-menu-backdrop {
      position: fixed;
      top: 0;
      left: 0;
      right: 0;
      bottom: 0;
      z-index: 199;
    }

    .more-menu {
      position: fixed;
      bottom: calc(56px + env(safe-area-inset-bottom, 0px));
      right: 8px;
      left: auto;
      background: var(--bg-secondary);
      border: 1px solid var(--border);
      border-radius: 12px;
      box-shadow: 0 -4px 24px rgba(0,0,0,0.4);
      z-index: 200;
      padding: 6px;
      min-width: min(180px, calc(100vw - 16px));
      max-width: calc(100vw - 16px);
      max-height: min(70dvh, calc(100dvh - 72px - env(safe-area-inset-top, 0px) - env(safe-area-inset-bottom, 0px)));
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

    .more-menu-item:hover, .more-menu-item:active {
      background: var(--bg-hover);
    }

    /* Hide the whole sidebar column, not just its contents — otherwise a 260px
       empty strip remains and journal titles wrap one character per line. */
    .sidebar-container {
      display: none !important;
      width: 0 !important;
    }

    :global(.sidebar) {
      display: none !important;
    }

    .main-content,
    .main-content.zen-content {
      padding-bottom: 60px;
      word-break: normal;
    }
  }

  .command-palette-backdrop {
    position: fixed;
    inset: 0;
    z-index: 4000;
    background: color-mix(in srgb, var(--bg-primary) 35%, transparent);
    display: flex;
    justify-content: center;
    padding-top: 12vh;
  }

  .command-palette {
    width: min(560px, calc(100vw - 32px));
    max-height: min(70vh, 520px);
    display: flex;
    flex-direction: column;
    background: var(--surface-overlay, var(--bg-secondary));
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 18px 48px rgba(0, 0, 0, 0.45);
    overflow: hidden;
  }

  .command-palette-input {
    width: 100%;
    padding: 12px 14px;
    border: none;
    border-bottom: 1px solid var(--border);
    background: transparent;
    color: var(--text-primary);
    font-size: 14px;
    outline: none;
  }

  .command-palette-list {
    overflow-y: auto;
    padding: 6px;
  }

  .command-palette-item {
    width: 100%;
    display: flex;
    justify-content: space-between;
    gap: 12px;
    align-items: center;
    padding: 8px 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
    text-align: left;
  }

  .command-palette-item.active,
  .command-palette-item:hover {
    background: var(--bg-hover);
  }

  .command-palette-desc {
    font-size: 13px;
  }

  .command-palette-keys {
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
  }

  .command-palette-empty {
    padding: 12px 10px;
    color: var(--text-muted);
    font-size: 13px;
  }
</style>
