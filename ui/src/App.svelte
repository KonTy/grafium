<script lang="ts">
  import { tick, onMount, onDestroy, untrack } from "svelte";
  import Sidebar from "./components/Sidebar.svelte";
  import PageContent from "./components/PageContent.svelte";
  import JournalView from "./components/JournalView.svelte";
  import GoToLink from "./components/GoToLink.svelte";
  import LazyView from "./components/LazyView.svelte";
  import MobileNav from "./components/MobileNav.svelte";
  import { lazyComponent } from "./lib/lazy";
  import { autofocus } from "./lib/autofocus";
  import { dismissOnBackdrop, dialogKeydown } from "./lib/modal";
  import {
    ANDROID_BACK_EVENT,
    dismissOverlayForAndroidBack,
  } from "./lib/androidBack";
  import { hasPersistentStorageAccess, requestPersistentStorageAccess } from "./lib/androidStorageAccess";
  import { revealStartupWindowWhenRestored, reportStartupContent } from "./lib/startupWindow";
  import { getLayoutPreferences, saveLayoutPreferences, type LayoutPreferences } from "./lib/api";
  import { handleMainPanePageKey, hasKeyboardOverlay } from "./lib/mainPaneScroll";
  import { readerOwnsNavigation } from "./lib/readerNavigation";
  import { loadReaderFlowPreference, loadReaderTextSizePreference } from "./lib/readerPreferences";
  import { loadReaderPlaybackPreferences } from "./lib/readerPlaybackPreferences";
  import { readerShortcut } from "./lib/readerHotkeys";
  import TitleBar from "./components/TitleBar.svelte";
  import Toaster from "./components/Toaster.svelte";
  import ContextMenuHost from "./components/ContextMenuHost.svelte";
  import { installContextMenu } from "./lib/contextMenuHost.svelte";
  import PrivateReaderToolbar from "./components/PrivateReaderToolbar.svelte";
  import PrivateReaderBook from "./components/PrivateReaderBook.svelte";
  import PrivateReaderLibrary from "./components/PrivateReaderLibrary.svelte";
  import { privateLibrary, refreshPrivateLibrary, type ReaderBook, type ReaderBookmark } from "./lib/privateReader";
  import { libraryBookmarkJournalSnippet, routeLibraryLink } from "./lib/libraryLinks";
  import { studyClockItem } from "./lib/studyLibrary";
  import { attachPrivatePlayback, privatePlayback, bookmarkPrivatePlayback } from "./lib/privateReaderPlayback";
  import { ORIGINAL_BOOK_EXTENSIONS, CONVERTIBLE_BOOK_EXTENSIONS, type BookImportMode } from "./lib/bookImport";
  import { isOriginalBookPage, isBookAnnotationPage } from "./lib/books";
  import { getPage, createPage, recordPageOpen, getGraphInfo, openGraph, validateGraph, createGraph, reindexCurrent, listGraphs, getTutorialGraphPath, mediaImportVideo, type GraphInfo } from "./lib/api";
  import { appCommands, keymap_manager, registerDefaultShortcuts } from "./lib/keymap";
  import { matchesShortcut } from "./lib/shortcutRegistry";
  import { formatLocalIsoDate, isJournalDateTitle, shiftIsoDate } from "./lib/journalDate";
  import {
    dispatchEditPageEnd,
    personalDiarySnippet,
    timeStampSnippet,
    tryInsertIntoActiveEditor,
  } from "./lib/editorInsert";
  import { formatBinding, formatBindingList } from "./lib/shortcuts";
  import type { PageNavigationTarget } from "./lib/navigation";
  import { isPageNotFoundError, resolvePageLookup } from "./lib/navigation";
  import { appearance } from "./lib/appearance";
  import { attachAppUndoRedoListeners } from "./lib/undoEvents";
  import { initJobs, notifyJobFinished } from "./lib/jobs.svelte";
  import { initSyncActivity } from "./lib/syncActivity.svelte";
  import { initSyncMonitor } from "./lib/sync";
  import { showToast } from "./lib/toast.svelte";
  import { setCurrentBlockAnchor } from "./lib/currentBlockAnchor";
  import { readingSelection } from "./lib/readingSelection";
  import { uiLog } from "./lib/uiLog";
  import {
    loadBionicReaderPreference as readBionicReaderPreference,
    setBionicReaderEnabled,
    bionicReaderEnabled,
  } from "./lib/bionicReader";
  import { listen } from "@tauri-apps/api/event";
  import { documentDir, downloadDir, homeDir } from "@tauri-apps/api/path";
  import { open } from "@tauri-apps/plugin-dialog";
  import type { Page } from "./lib/api";
  import { closeSettingsHelpForContextualHelp, helpPageTitle, isHelpContext, loadHelpPage, type HelpContext } from "./lib/help";
  import { recordStudyActivity, type StudyItem, type StudyProgress } from "./lib/studies";
  import { StudyTracker, type StudyClockState } from "./lib/studyTracker";
  import { trackReading } from "./lib/studyReading";

  const loadStudies = lazyComponent(() => import("./components/Studies.svelte"));
  const loadStudyMedia = lazyComponent(() => import("./components/StudyMedia.svelte"));
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
  const loadHelpOverlay = lazyComponent(() => import("./components/HelpOverlay.svelte"));
  const loadFolderBrowser = lazyComponent(() => import("./components/FolderBrowser.svelte"));
  const loadBookImportDialog = lazyComponent(() => import("./components/BookImportDialog.svelte"));
  const loadPrintDialog = lazyComponent(() => import("./components/PrintDialog.svelte"));
  const loadOriginalBookPage = lazyComponent(() => import("./components/OriginalBookPage.svelte"));
  const loadBookAnnotationPage = lazyComponent(() => import("./components/BookAnnotationPage.svelte"));

  let shuttingDown = $state(false);
  let privateBookId = $state<string | null>(null);
  let privateBookmarkId = $state<string | undefined>();
  let privateOpenTarget = $state<{ nonce: number; bookId: string; trackId?: string | null; startMs?: number | null; quote?: string | null; chapter?: string | null } | null>(null);
  let privateLibraryRef: { focusSearch: () => boolean } | undefined = $state();
  onMount(() => {
    const detachPlayback = attachPrivatePlayback();
    const openLibrary = (event: Event) => {
      const detail = (event as CustomEvent).detail;
      if (detail?.bookId) openPrivateLibraryPosition(detail);
    };
    window.addEventListener("grafium-open-library-source", openLibrary);
    return () => { detachPlayback?.(); window.removeEventListener("grafium-open-library-source", openLibrary); };
  });

  async function openPrivateBook(bookId: string, bookmarkId?: string, target?: { trackId?: string | null; startMs?: number | null; quote?: string | null; chapter?: string | null }) {
    if (currentView === "library" && privateBookId === bookId && !bookmarkId && !target) return true;
    const request = studyNavigation + 1;
    await navigateToPage("__library__");
    if (request !== studyNavigation) return false;
    try {
      await refreshPrivateLibrary();
      if (request !== studyNavigation || currentView !== "library") return false;
      if (!$privateLibrary.books.some(book => book.id === bookId))
        throw new Error("This Library item is unavailable on this device. Restore its private library backup or relink its source.");
      privateBookId = bookId;
      privateBookmarkId = bookmarkId;
      privateOpenTarget = target ? { ...target, bookId, nonce: Date.now() } : null;
      return true;
    } catch (cause) {
      if (request === studyNavigation) showToast(`Could not open Library item: ${String(cause)}`, "error");
      return false;
    }
  }

  function openPrivateLibraryPosition(detail: { bookId: string; trackId?: string | null; startMs?: number | null; quote?: string | null; chapter?: string | null }) {
    void openPrivateBook(detail.bookId, undefined, detail);
  }

  function openAiSettings() {
    void navigateToPage("__settings__").then(() => { settingsOpenSection = "ai"; });
  }

  async function addLibraryToStudies(book: ReaderBook) {
    studyAddPage = null;
    studyAddLibrary = book;
    await navigateToPage("__studies__");
  }

  async function writeLibraryBookmarkToJournal(book: ReaderBook, bookmark: ReaderBookmark) {
    const generation = graphGeneration;
    try {
      const insert = libraryBookmarkJournalSnippet(book, bookmark);
      const title = todayJournalTitle();
      await navigateToPage(title, true);
      if (generation !== graphGeneration || error || currentPage?.title !== title) return;
      dispatchEditPageEnd({ pageId: currentPage.id, insert });
    } catch (cause) {
      showToast(`Could not open a journal note: ${String(cause)}`, "error");
    }
  }

  function openPrivateLibrarySettings() {
    void navigateToPage("__settings__").then(() => { settingsOpenSection = "library"; });
  }

  function isAndroidClient(): boolean {
    return typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent);
  }

  function openGoToLink() {
    if (hasKeyboardOverlay(document)) return;
    goToLinkOpen = true;
  }

  function openJournalCalendar() {
    if (hasKeyboardOverlay(document)) return;
    journalCalendarRequested = true;
    if (currentView !== "journal") void navigateToJournal();
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
      if (!hasPersistentStorageAccess()) {
        alert("Grafium needs persistent file access to open and save graph folders after the app restarts. Enable “Allow access to manage all files”, then choose the folder again.");
        requestPersistentStorageAccess();
        return null;
      }
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

  type View = "page" | "journal" | "all-pages" | "flashcards" | "statistics" | "studies" | "library" | "chat" | "settings" | "graph" | "jobs";

  let currentView: View = $state("page");
  let studyGraphPath = $state("");
  let studyAddPage = $state<Page | null>(null);
  let studyAddLibrary = $state<ReaderBook | null>(null);
  let activeStudy = $state<StudyItem | null>(null);
  let studyClock: StudyTracker | null = null;
  let studyClockState = $state<StudyClockState>("idle");
  let studySessionSeconds = $state(0);
  let studySaveError = $state("");
  const failedStudyWrites = new Set<StudyTracker>();
  let studyNavigation = 0;

  async function saveStudyClock(clock: StudyTracker, stop = false) {
    try {
      await (stop ? clock.stop() : clock.flush());
      failedStudyWrites.delete(clock);
      if (!failedStudyWrites.size) studySaveError = "";
    } catch (cause) {
      failedStudyWrites.add(clock);
      studySaveError = `Study time or progress was not saved: ${String(cause)}. Keep Grafium open and retry.`;
    }
  }
  async function finishStudy() {
    const clock = studyClock;
    studyClock = null;
    activeStudy = null;
    if (clock) await saveStudyClock(clock, true);
  }
  async function openStudy(item: StudyItem) {
    const request = ++studyNavigation;
    const graph = studyGraphPath;
    await finishStudy();
    if (request !== studyNavigation || currentView !== "studies") return;
    for (const clock of failedStudyWrites) {
      if (clock.item.id === item.id) {
        await saveStudyClock(clock);
        if (failedStudyWrites.has(clock)) return;
      }
    }
    if (item.kind === "page" || item.kind === "book") {
      await navigateToPage({ id: item.source });
      if (request + 1 !== studyNavigation || error || currentPage?.id !== item.source) return;
    }
    if (item.kind === "library") {
      if (!await openPrivateBook(item.source) || request + 1 !== studyNavigation) return;
    }
    if (graph !== studyGraphPath) return;
    studyAddPage = null;
    studyAddLibrary = null;
    activeStudy = item;
    if (item.kind !== "page" && item.kind !== "book") {
      await tick();
      if (mainContentEl) mainContentEl.scrollTop = 0;
    }
  }
  function updateStudyProgress(progress: StudyProgress) {
    studyClock?.updateProgress(progress);
    if (activeStudy?.kind === "website" && studyClock) void saveStudyClock(studyClock);
  }
  async function returnToStudies() {
    await finishStudy();
    await navigateToPage("__studies__");
  }
  function addCurrentPageToStudies() {
    studyAddLibrary = null;
    studyAddPage = currentPage;
    void navigateToPage("__studies__");
  }
  $effect(() => {
    const item = activeStudy;
    const graph = studyGraphPath;
    if (!item || !graph) return;
    let tracked: StudyItem;
    try {
      tracked = untrack(() => studyClockItem(item, $privateLibrary.books));
    } catch (cause) {
      showToast(`Could not start study timer: ${String(cause)}`, "error");
      activeStudy = null;
      return;
    }
    const clock = new StudyTracker(tracked, (seconds, day, progress, requestId) =>
      recordStudyActivity(graph, item.id, seconds, day, item.kind === "library" ? null : progress, requestId));
    studyClock = clock;
    studySessionSeconds = 0;
    let count = 0;
    const visibility = () => {
      clock.setVisible(!document.hidden && document.hasFocus());
      studyClockState = clock.state;
      if (document.hidden) void saveStudyClock(clock);
    };
    visibility();
    const interval = setInterval(() => {
      visibility();
      clock.tick();
      studyClockState = clock.state;
      studySessionSeconds = clock.seconds;
      if (++count % 10 === 0) void saveStudyClock(clock);
    }, 1000);
    const pagehide = () => { void saveStudyClock(clock); };
    document.addEventListener("visibilitychange", visibility);
    window.addEventListener("pagehide", pagehide);
    const cleanupReading = item.kind === "page" && mainContentEl
      ? trackReading(mainContentEl, item.progress, progress => clock.updateProgress(progress), () => clock.activity())
      : undefined;
    return () => {
      clearInterval(interval);
      document.removeEventListener("visibilitychange", visibility);
      window.removeEventListener("pagehide", pagehide);
      cleanupReading?.();
      if (studyClock === clock) studyClock = null;
      void saveStudyClock(clock, true);
    };
  });
  let helpVisible = $state(false);
  let helpLoading = $state(false);
  let helpTitle = $state("");
  let helpContent = $state("");
  let currentPage: Page | null = $state(null);
  // Journals are a scrolling feed, not a single page — this tracks whichever day-entry is
  // currently most visible, so the Reference/Knowledge panel has something to analyze there too.
  let journalActivePage: Page | null = $state(null);
  const assistantSourcePage = $derived.by((): Page | null => currentView === "page" ? currentPage
    : currentView === "journal" ? journalActivePage : null);
  /** What Print acts on: the open page, or whichever journal day is in view. */
  const printablePage = $derived.by((): Page | null => currentView === "page" ? currentPage
    : currentView === "journal" ? journalActivePage : null);
  let printPageId: string | null = $state(null);
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
    focusNavigation: () => void;
    hasFocus: () => boolean;
    refresh: () => Promise<void>;
  } | null = $state(null);
  let showBlockGuides = $state(true);
  const bionicReaderMode = $derived($bionicReaderEnabled);
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
  let referencePanelTab = $state<"chat" | "writing" | "notes" | "conflicts">("chat");
  let referencePanelFocusTrigger = $state(0);
  let readingNoteFocus = $state({ pageId: "", label: "", trigger: 0 });
  let mainContentEl: HTMLElement | null = null;
  let restoreTimer: number | null = null;
  let pendingJournalRestore: HistoryEntry | null = $state(null);
  let journalRestoreRequestId = $state(0);
  let graphGeneration = $state(0);
  let uiZoom = $state(1);

  // Navigation history for back/forward
  type HistoryEntry = {
    kind: View;
    title?: string;
    scrollTop: number;
    sourceBlockId?: string;
    sourcePageTitle?: string;
    conversationId?: string | null;
    bookId?: string;
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
  const SIDEBAR_RAIL_WIDTH = 52;
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
    readBionicReaderPreference();
  }

  function toggleBionicReader() {
    setBionicReaderEnabled(!bionicReaderMode);
  }

  async function bookmarkCurrentReading() {
    if (!window.dispatchEvent(new CustomEvent("grafium-bookmark", { cancelable: true }))) return;
    // A graph original has annotations, not Library bookmarks. Never bookmark
    // unrelated background audio while the user is reading that original.
    if (currentView === "page" && currentPage && isOriginalBookPage(currentPage)) {
      showToast("Use the book's Notes panel to save a passage, or open a Library item to add a bookmark.", "info", {
        label: "Open Notes", run: () => openReferencePanelTab("notes"),
      });
      return;
    }
    if ($privatePlayback.bookId && ["playing", "paused"].includes($privatePlayback.status)
      && !(currentView === "library" && privateBookId && privateBookId !== $privatePlayback.bookId)) {
      try {
        await bookmarkPrivatePlayback();
        showToast("Library bookmark saved on this device.", "success");
      } catch (cause) {
        showToast(`Could not save Library bookmark: ${String(cause)}`, "error");
      }
      return;
    }
    showToast("Open a book in Library or start Library playback to add a bookmark. For graph books, use the Notes panel.", "info", {
      label: "Open Library", run: () => { void navigateToPage("__library__"); },
    });
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
    if (currentView === "studies") {
      return { kind: "studies", scrollTop: currentScrollTop() };
    }
    if (currentView === "library") {
      return { kind: "library", scrollTop: currentScrollTop(), bookId: privateBookId ?? undefined };
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
    ++studyNavigation;
    await finishStudy();
    if (entry.kind === "studies") {
      await navigateToPage("__studies__", false, true, entry);
      return;
    }
    if (entry.kind === "library") {
      await navigateToPage("__library__", false, true, entry);
      return;
    }
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

  function handleAndroidBack() {
    const focused = document.activeElement ?? window;
    if (dismissOverlayForAndroidBack(focused)) return;

    if (goToLinkOpen) {
      goToLinkOpen = false;
      return;
    }
    if (globalSearchOpen) {
      globalSearchOpen = false;
      return;
    }
    if (helpVisible) {
      helpVisible = false;
      return;
    }
    if (showMoreMenu) {
      showMoreMenu = false;
      return;
    }
    if (referencePanelVisible) {
      referencePanelVisible = false;
      return;
    }
    if (commandPaletteOpen) {
      commandPaletteOpen = false;
      return;
    }
    if (zenMode) {
      zenMode = false;
      return;
    }
    if (showFolderBrowser) {
      finishFolderBrowser(null);
      return;
    }
    if (showNewPageDialog) {
      cancelNewPage();
      return;
    }
    if (showImportMediaDialog) {
      cancelImportMedia();
      return;
    }
    if (showImportBooksDialog) {
      showImportBooksDialog = false;
      return;
    }
    if (showCreateGraphDialog) {
      showCreateGraphDialog = false;
      return;
    }

    if (navIndex > 0) void goBack();
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

  function openReferencePanelTab(tab: "chat" | "writing" | "notes" | "conflicts") {
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

  // Ctrl+B "seamless" focus/collapse for the left sidebar:
  // - collapsed -> expand it (also leaving zen mode) and focus navigation
  // - visible but not focused -> just focus navigation
  // - visible and already focused -> collapse it to the navigation icon rail
  async function focusLeftSidebar() {
    if (!sidebarVisible || zenMode) {
      if (zenMode) zenMode = false;
      setLayoutPreferences({ sidebarVisible: true });
      await tick();
      sidebarRef?.focusNavigation();
      return;
    }
    if (sidebarRef?.hasFocus()) {
      setLayoutPreferences({ sidebarVisible: false });
      return;
    }
    sidebarRef?.focusNavigation();
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

  function toggleWideMode() {
    setLayoutPreferences({ wideMode: !wideMode });
  }

  function focusLocalSearch(): boolean {
    if (hasKeyboardOverlay(document)) return false;
    if (currentView === "library") return privateLibraryRef?.focusSearch() ?? false;
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

  // Read when the palette opens, so it lists every command with its keys as
  // they are now, including commands whose keys were all removed.
  let commandPaletteCommands = $state.raw<Array<{
    id: string; description: string; bindings: string[]; run: () => unknown;
  }>>([]);

  function toggleCommandPalette() {
    commandPaletteOpen = !commandPaletteOpen;
    commandPaletteQuery = "";
    commandPaletteIndex = 0;
    if (!commandPaletteOpen) return;
    commandPaletteCommands = [
      ...appCommands().map(({ id, label, bindings, run }) => ({ id, description: label, bindings, run })),
      { id: "ai-writing", description: "Open writing assistance", bindings: [], run: () => openReferencePanelTab("writing") },
      { id: "reading-notes", description: "Open reading notes", bindings: [], run: () => openReferencePanelTab("notes") },
    ];
  }

  const commandPaletteRows = $derived(commandPaletteCommands.filter((row) => {
    const q = commandPaletteQuery.trim().toLowerCase();
    if (!q) return true;
    return row.description.toLowerCase().includes(q)
      || row.bindings.some((binding) => binding.includes(q) || formatBinding(binding).toLowerCase().includes(q));
  }));

  async function runCommandPaletteRow(index: number) {
    const row = commandPaletteRows[index];
    if (!row) return;
    commandPaletteOpen = false;
    await tick();
    row.run();
  }

  // Made once, not in the markup: the global key handler closes the palette
  // on Escape before this runs, and a handler built inside the closed block
  // would no longer exist.
  const commandPaletteKeydown = dialogKeydown(() => (commandPaletteOpen = false));

  registerDefaultShortcuts({
    goJournal: () => navigateToJournal(),
    goLink: openGoToLink,
    goJournalDate: openJournalCalendar,
    goJournalEdit: () => { void goJournalAndEdit(); },
    goHome: () => navigateToJournal(),
    goAllPages: () => navigateToPage("__all_pages__"),
    goGraph: () => navigateToPage("__graph__"),
    goFlashcards: () => navigateToPage("__flashcards__"),
    goTomorrow: () => navigateToPage(shiftIsoDate(formatLocalIsoDate(), 1), true),
    goTasks: () => navigateToPage("__statistics__"),
    goStudies: () => navigateToPage("__studies__"),
    goLibrary: () => navigateToPage("__library__"),
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
    focusLocalSearch: () => focusLocalSearch(),
    toggleSidebar: () => {
      void focusLeftSidebar();
    },
    toggleRightSidebar: () => {
      referencePanelVisible = !referencePanelVisible;
    },
    toggleTheme: () => openThemeSettings(),
    toggleSettings: () => toggleSettingsView(),
    toggleBionicReader,
    bookmark: () => { void bookmarkCurrentReading(); },
    toggleHelp: () => openCurrentContextualHelp(document.activeElement),
    toggleWideMode,
    toggleZenMode: () => {
      zenMode = !zenMode;
    },
    commandPalette: () => toggleCommandPalette(),
    print: () => openPrintDialog(),
    importMedia: () => openImportMediaDialog(),
    importBooks: () => void openImportBooksDirectory(),
    insertTimeStamp: () => insertEditorSnippet(timeStampSnippet()),
    insertPersonalDiary: () => insertEditorSnippet(personalDiarySnippet()),
    zoomIn: () => adjustUiZoom(1),
    zoomOut: () => adjustUiZoom(-1),
    zoomReset: () => resetUiZoom(),
  });

  /** Print needs a page; the other views have nothing page-shaped to print. */
  function openPrintDialog() {
    const page = printablePage;
    if (!page) {
      showToast("Open a page or a journal day to print", "info");
      return;
    }
    printPageId = page.id;
  }

  async function openContextualHelp(context: HelpContext) {
    helpTitle = helpPageTitle(context);
    helpContent = "";
    helpVisible = true;
    helpLoading = true;
    try {
      if (document.fullscreenElement) await document.exitFullscreen();
      helpContent = await loadHelpPage(context);
    } catch (error) {
      helpContent = `# Help unavailable\n\nCould not load the bundled help page.\n\n\`${errorText(error)}\``;
    } finally {
      helpLoading = false;
    }
  }

  function openCurrentContextualHelp(eventTarget: EventTarget | null) {
    const section =
      eventTarget instanceof Element
        ? eventTarget.closest("[data-help-context]")?.getAttribute("data-help-context") ?? null
        : null;
    const context = isHelpContext(section) ? section : null;
    const currentContext: HelpContext =
      context ||
      (currentView === "library" && privateBookId ? "reader" : null) ||
      ((
        {
          page: currentPage && (isOriginalBookPage(currentPage) || isBookAnnotationPage(currentPage)) ? "books" : "editor",
          journal: "journal",
          "all-pages": "search",
          graph: "graph",
          flashcards: "flashcards",
          statistics: "tasks",
          studies: "studies",
          library: "library",
          chat: "chat",
          settings: "settings",
          jobs: "general",
        } as Record<string, HelpContext>
      )[currentView] ?? "general");
    closeSettingsHelpForContextualHelp(eventTarget);
    void openContextualHelp(currentContext);
  }

  // Global keydown handler
  function handleGlobalKeydown(e: KeyboardEvent) {
    if (e.defaultPrevented || e.isComposing || e.keyCode === 229) return;
    // Settings is recording a new shortcut: these keys are its, not the app's.
    if ((e.target as Element | null)?.closest?.("[data-shortcut-recorder]")) return;
    if (readerShortcut(e)) {
      if (hasKeyboardOverlay(document)) return;
      keymap_manager.handleKeydown(e);
      return;
    }
    const reader = document.querySelector(".reading-surface");
    if (reader && !hasKeyboardOverlay(document)
      && (readerOwnsNavigation(e, reader) || e.key === "F8" || e.key === "F11" || (e.key === "Escape"
        && (reader.classList.contains("expanded") || reader.querySelector('[aria-expanded="true"]'))))) return;
    if (matchesShortcut(e, "help")) {
      e.preventDefault();
      e.stopPropagation();
      if (!e.repeat) openCurrentContextualHelp(e.target);
      return;
    }

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

    if (matchesShortcut(e, "zoom-reset")) {
      e.preventDefault();
      resetUiZoom();
      return;
    }
    if (matchesShortcut(e, "zoom-in")) {
      e.preventDefault();
      adjustUiZoom(1);
      return;
    }
    if (matchesShortcut(e, "zoom-out")) {
      e.preventDefault();
      adjustUiZoom(-1);
      return;
    }
    // Focuses a visible page filter; otherwise leave browser/editor find.
    if (matchesShortcut(e, "search-local")) {
      if (focusLocalSearch()) e.preventDefault();
      return;
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

    if (showNewPageDialog || showImportMediaDialog || showImportBooksDialog) return;

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
  onMount(() => installContextMenu());

  $effect(() => {
    let disposed = false;
    const stops: (() => void)[] = [];
    const subscribe = async () => {
      const subscriptions = [
        listen<{ graphPath: string; message: string }>("graph-index-error", async ({ payload }) => {
          try {
            if (!disposed && (await getGraphInfo()).path === payload.graphPath) {
              showToast(`Source indexing needs attention: ${payload.message}`, "error");
            }
          } catch (cause) {
            console.error("Could not inspect the graph after an index error:", cause);
          }
        }),
        listen<{ graphPath: string; message: string }>("asset-cleanup-warning", async ({ payload }) => {
          try {
            if (disposed) return;
            const graph = await getGraphInfo();
            if (!disposed && graph.path === payload.graphPath) {
              showToast(payload.message, "error");
            }
          } catch (cause) {
            console.error("Could not inspect the graph after an asset cleanup warning:", cause);
          }
        }),
        listen<{ graphPath: string }>("graph-sources-changed", async ({ payload }) => {
          try {
            if (disposed || (await getGraphInfo()).path !== payload.graphPath) return;
            void sidebarRef?.refresh();
            window.dispatchEvent(new CustomEvent("page-tree-refresh"));
            window.dispatchEvent(new CustomEvent("graph-sources-changed", { detail: payload }));
          } catch (cause) {
            showToast(`Could not refresh changed graph sources: ${String(cause)}`, "error");
          }
        }),
      ];
      const results = await Promise.allSettled(subscriptions);
      for (const result of results) {
        if (result.status === "fulfilled") {
          if (disposed) result.value();
          else stops.push(result.value);
        } else if (!disposed) showToast(`Could not watch graph maintenance status: ${String(result.reason)}`, "error");
      }
    };
    void subscribe();
    return () => { disposed = true; stops.forEach(stop => stop()); };
  });

  $effect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen("app-shutdown-started", () => {
      shuttingDown = true;
      void finishStudy();
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    }).catch((error) => {
      console.error("Could not initialize shutdown status:", error);
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  });

  $effect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    initJobs((job) => {
      void sidebarRef?.refresh();
      window.dispatchEvent(new CustomEvent("page-tree-refresh"));
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
    let disposed = false;
    const unlisteners: (() => void)[] = [];
    for (const initialize of [initSyncActivity, initSyncMonitor]) {
      initialize()
        .then((fn) => {
          if (disposed) fn();
          else unlisteners.push(fn);
        })
        .catch((e) => {
          console.error("Could not initialize sync activity tracking:", e);
        });
    }
    return () => {
      disposed = true;
      for (const unlisten of unlisteners) unlisten();
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
      }).finally(() => {
        void reportStartupContent().catch(() => undefined);
      });
      // Restore the theme and menu before mapping the native window, but never
      // let a slow restore leave the user without a window at all.
      void revealStartupWindowWhenRestored(
        Promise.all([appearance.start(), restoreLayoutPreferences()]),
      ).then(() => {
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
    | { kind: "all-pages" | "flashcards" | "statistics" | "studies" | "library" | "chat" | "settings" | "graph" | "jobs" | "notifications" };

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
        currentView === "studies" ||
        currentView === "library" ||
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
        if (saved.kind === "studies") {
          await navigateToPage("__studies__");
          return;
        }
        if (saved.kind === "library") {
          await navigateToPage("__library__");
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

  onDestroy(() => appearance.stop());

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
    const request = ++studyNavigation;
    await finishStudy();
    if (request !== studyNavigation) return;
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
    const navigationRequest = ++studyNavigation;
    await finishStudy();
    if (navigationRequest !== studyNavigation) return;
    if (!skipHistory) {
      saveCurrentHistoryState(sourceBlockId, sourcePageTitle);
    }
    privateBookId = null;
    privateBookmarkId = undefined;
    privateOpenTarget = null;

    // Handle special routes
    if (target === "__library__") {
      const request = studyNavigation;
      currentView = "library";
      currentPage = null;
      loading = false;
      error = null;
      if (!skipHistory) pushHistoryEntry({ kind: "library", scrollTop: 0 });
      if (restoreEntry?.bookId) {
        try {
          await refreshPrivateLibrary();
          if (request !== studyNavigation) return;
          privateBookId = restoreEntry.bookId;
        } catch (cause) {
          showToast(`Could not restore Library view: ${String(cause)}`, "error");
        }
      }
      await tick();
      if (restoreEntry) restoreHistoryState(restoreEntry);
      return;
    }
    if (target === "__studies__") {
      const request = studyNavigation;
      currentView = "studies";
      currentPage = null;
      loading = true;
      error = null;
      try {
        const graph = await getGraphInfo();
        if (request !== studyNavigation) return;
        studyGraphPath = graph.path;
        if (!skipHistory) pushHistoryEntry({ kind: "studies", scrollTop: 0 });
      } catch (cause) {
        if (request === studyNavigation) error = `Could not open Studies: ${String(cause)}`;
      } finally {
        if (request === studyNavigation) loading = false;
      }
      return;
    }
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

  async function browseImportMedia() {
    if (importMediaBusy) return;
    importMediaError = "";
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        title: "Choose Video or Audio File",
        filters: [{
          name: "Video and audio",
          extensions: [
            "webm", "mp4", "mkv", "mov", "avi", "m4v",
            "mp3", "m4a", "aac", "wav", "flac", "ogg", "opus",
          ],
        }],
      });
      if (typeof selected === "string") {
        importMediaUrl = selected;
      }
    } catch (e) {
      importMediaError = `Could not open the media picker: ${e instanceof Error ? e.message : String(e)}`;
    }
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
  }

  let showImportBooksDialog = $state(false);

  function openImportBooksDirectory() {
    showImportBooksDialog = true;
  }

  async function chooseBookFile(mode: BookImportMode): Promise<string | null> {
    const selected = await open({
      title: "Choose Book File", directory: false, multiple: false,
      defaultPath: await defaultExternalBookImportFolder(),
      filters: [{ name: "Books and documents", extensions: mode === "original" ? ORIGINAL_BOOK_EXTENSIONS : CONVERTIBLE_BOOK_EXTENSIONS }],
    });
    return typeof selected === "string" ? selected : null;
  }

  async function chooseBookFolder(): Promise<string | null> {
    return pickFolder("Select Folder Containing Book Files", await defaultExternalBookImportFolder());
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
    await runReindex(true);
  }

  async function handleMobileOpenGraph() {
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
    graphGeneration += 1;
    privateBookId = null;
    ++studyNavigation;
    void finishStudy();
    studyGraphPath = "";
    studyAddPage = null;
    studyAddLibrary = null;
    privateBookmarkId = undefined;
    privateOpenTarget = null;
    goToLinkOpen = false;
    globalSearchOpen = false;
    showImportBooksDialog = false;
    referencePanelVisible = false;
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

  function handleBookOpenNotes() {
    if (currentView === "page" && currentPage && isOriginalBookPage(currentPage)) {
      openReferencePanelTab("notes");
    }
  }

  function handleLibraryLink(event: MouseEvent) {
    routeLibraryLink(event,
      (bookId, bookmarkId) => { void openPrivateBook(bookId, bookmarkId); },
      () => showToast("This Library link is invalid.", "error"));
  }

  $effect(() => {
    window.addEventListener("click", handleLibraryLink, true);
    return () => window.removeEventListener("click", handleLibraryLink, true);
  });

  $effect(() => {
    window.addEventListener("navigate-page", handlePageNav);
    window.addEventListener("open-reading-note", handleReadingNoteNav);
    window.addEventListener("book-open-notes", handleBookOpenNotes);
    window.addEventListener(ANDROID_BACK_EVENT, handleAndroidBack);
    return () => {
      window.removeEventListener("navigate-page", handlePageNav);
      window.removeEventListener("open-reading-note", handleReadingNoteNav);
      window.removeEventListener("book-open-notes", handleBookOpenNotes);
      window.removeEventListener(ANDROID_BACK_EVENT, handleAndroidBack);
    };
  });

  $effect(() => {
    loadSidebarWidthPreference();
    loadReferencePanelWidthPreference();
    loadGraphViewModePreference();
    loadShowBlockGuidesPreference();
    loadNarrowPaddingPreference();
    loadBionicReaderPreference();
    loadReaderFlowPreference();
    loadReaderTextSizePreference();
    loadReaderPlaybackPreferences();
  });
</script>

<div class="app-shell" class:zen={zenMode} class:wide-mode={wideMode}>
  {#if !zenMode}
    <TitleBar
      {sidebarVisible}
      {sidebarWidth}
      {uiZoom}
      canGoBack={navIndex > 0}
      canGoForward={navIndex < navHistory.length - 1}
      onGoBack={goBack}
      onGoForward={goForward}
      onToggleReferencePanel={() => (referencePanelVisible = !referencePanelVisible)}
      showJournalActions={currentView === "journal"}
      onGoToDate={openJournalCalendar}
      onGoToLink={openGoToLink}
      onOpenSearch={openGlobalSearch}
      onOpenJobs={() => handleNavigate("__jobs__")}
      onOpenSettings={() => navigateToPage("__settings__")}
      onPrint={openPrintDialog}
      onZoomIn={() => adjustUiZoom(1)}
      onZoomOut={() => adjustUiZoom(-1)}
      onZoomReset={resetUiZoom}
    />
  {/if}
  <PrivateReaderToolbar onOpen={bookId => { void openPrivateBook(bookId); }} />
  <div class="app-layout" bind:this={appLayoutEl}>
    {#if !zenMode}
      <div
        class="sidebar-container"
        class:collapsed={!sidebarVisible}
        style={`width: ${sidebarVisible ? sidebarWidth : SIDEBAR_RAIL_WIDTH}px;`}
      >
        <Sidebar
          bind:this={sidebarRef}
          {currentPage}
          {currentView}
          {sidebarWidth}
          collapsed={!sidebarVisible}
          onExpand={() => setLayoutPreferences({ sidebarVisible: true })}
          onCollapse={() => setLayoutPreferences({ sidebarVisible: false })}
          onNavigate={handleNavigate}
          onGraphChanged={handleGraphChanged}
        />
      </div>
      {#if sidebarVisible}
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
    {/if}

    <main bind:this={mainContentEl} class="main-content" class:zen-content={zenMode} class:chat-content={chatActive}
      class:full-width-view={currentView === "graph" || currentView === "all-pages"}>
    {#key graphGeneration}
    {#if studySaveError || activeStudy}
    <div class="study-session-controls">
    {#if studySaveError}
      <div class="study-session-bar" role="alert">
        <span>{studySaveError}</span>
        <button onclick={() => { for (const clock of failedStudyWrites) void saveStudyClock(clock); }}>Retry saving</button>
      </div>
    {/if}
    {#if activeStudy}
      <div class="study-session-bar" data-help-context="studies">
        <button onclick={() => { void returnToStudies(); }}>Back to Studies</button>
        <strong>{activeStudy.title}</strong>
        {#if activeStudy.kind !== "website"}
          <span>{Math.floor(studySessionSeconds / 60)}m {Math.floor(studySessionSeconds % 60)}s · {studyClockState}</span>
          <button onclick={() => {
            studyClock?.setPaused(studyClockState !== "paused");
            studyClockState = studyClock?.state ?? "stopped";
          }}>{studyClockState === "paused" ? "Resume clock" : "Pause clock"}</button>
        {/if}
      </div>
    {/if}
    </div>
    {/if}
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
    {:else if currentView === "library"}
      {#if privateBookId}
        {#key privateBookId}
          <PrivateReaderBook bookId={privateBookId} initialBookmarkId={privateBookmarkId} initialOpenTarget={privateOpenTarget}
            onBack={() => { void navigateToPage("__library__"); }} onVoiceSettings={openPrivateLibrarySettings}
            onAddToStudies={book => { void addLibraryToStudies(book); }}
            onJournalNote={(book, bookmark) => { void writeLibraryBookmarkToJournal(book, bookmark); }}
            onProgress={updateStudyProgress} onPlayback={playing => studyClock?.playback(playing)}
            onActivity={() => studyClock?.activity()} />
        {/key}
      {:else}
        <PrivateReaderLibrary bind:this={privateLibraryRef} onOpen={(bookId, position) => { void openPrivateBook(bookId, undefined, position); }}
          onSettings={openPrivateLibrarySettings} onAddToStudies={book => { void addLibraryToStudies(book); }} />
      {/if}
    {:else if currentView === "studies"}
      {#if activeStudy}
        {#key activeStudy.id}
          {@const selectedStudy = activeStudy}
          {#if activeStudy.kind === "flashcards"}
            <LazyView load={loadFlashcardReview} name="study flashcards">
              {#snippet children(FlashcardReview)}
                <FlashcardReview initialTopic={selectedStudy.source === "*" ? null : selectedStudy.source}
                  onNavigate={handleNavigate} onExit={() => { void returnToStudies(); }}
                  onStudyActivity={() => studyClock?.activity()}
                  onStudyProgress={updateStudyProgress}
                  onStudyComplete={() => studyClock?.setPaused(true)} />
              {/snippet}
            </LazyView>
          {:else}
            <LazyView load={loadStudyMedia} name="study source">
              {#snippet children(StudyMedia)}
                <StudyMedia graphPath={studyGraphPath} item={selectedStudy}
                  onProgress={updateStudyProgress}
                  onPlayback={(playing) => studyClock?.playback(playing)}
                  onActivity={() => studyClock?.activity()} />
              {/snippet}
            </LazyView>
          {/if}
        {/key}
      {:else}
        <LazyView load={loadStudies} name="studies">
          {#snippet children(Studies)}
            <Studies graphPath={studyGraphPath} addPage={studyAddPage} addLibrary={studyAddLibrary}
              onLibraryConsumed={() => { studyAddLibrary = null; }}
              onOpen={(item) => { void openStudy(item); }} />
          {/snippet}
        </LazyView>
      {/if}
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
        showNavigationToolbar={zenMode}
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
      {#if !activeStudy}
        <div class="study-page-action"><button onclick={addCurrentPageToStudies}>Add to Studies</button></div>
      {/if}
      {#key currentPage.id}
        {#if isBookAnnotationPage(currentPage)}
          {@const annotationPage = currentPage}
          <LazyView load={loadBookAnnotationPage} name="book annotation editor">
            {#snippet children(BookAnnotationPage)}
              <BookAnnotationPage page={annotationPage} onNavigate={handleNavigate} />
            {/snippet}
          </LazyView>
        {:else if isOriginalBookPage(currentPage)}
          {@const bookPage = currentPage}
          <LazyView load={loadOriginalBookPage} name="book reader">
            {#snippet children(OriginalBookPage)}
              <OriginalBookPage page={bookPage} onStudyProgress={(progress) => {
                updateStudyProgress(progress);
                studyClock?.activity();
              }} />
            {/snippet}
          </LazyView>
        {:else}
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
        {/if}
      {/key}
    {/if}
    {#if chatVisited}
      <div class="chat-session" hidden={!chatActive} inert={!chatActive}>
        <LazyView load={loadChatView} name="chat">
          {#snippet children(ChatView)}
            <ChatView active={chatActive} conversationId={expandedConversationId}
              readingInset={wideMode || zenMode ? 0 : narrowPaddingPct}
              onOpenSettings={openAiSettings}
              onNavigate={handleNavigate}
              onFindLinks={handleFindLinksForPage} />
          {/snippet}
        </LazyView>
      </div>
    {/if}
    {/key}
    </main>

    <!-- Chat and reading notes -->
    {#if referencePanelVisible}
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
            originalBook={assistantSourcePage ? isOriginalBookPage(assistantSourcePage) : false}
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
            onOpenSettings={openAiSettings}
          />
        {/snippet}
      </LazyView>
    {/if}

    {#if !zenMode}
      <MobileNav
        {currentView}
        bind:menuOpen={showMoreMenu}
        onNavigate={handleNavigate}
        onToggleKnowledgePanel={() => (referencePanelVisible = !referencePanelVisible)}
        onOpenGraph={handleMobileOpenGraph}
        onCreateGraph={handleMobileCreateGraph}
        onReindex={handleMobileReindex}
      />
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
        onOpenSettings={() => { globalSearchOpen = false; openAiSettings(); }} />
    {/snippet}
  </LazyView>
{/if}

{#if commandPaletteOpen}
  <div class="command-palette-backdrop" role="presentation" use:dismissOnBackdrop={() => (commandPaletteOpen = false)}>
    <div class="command-palette" role="dialog" aria-modal="true" aria-label="Command palette" tabindex="-1" onkeydown={commandPaletteKeydown}>
      <input
        class="command-palette-input"
        type="text"
        placeholder="Run a command…"
        bind:value={commandPaletteQuery}
        oninput={() => (commandPaletteIndex = 0)}
        use:autofocus
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
              {formatBindingList(row.bindings)}
            </span>
          </button>
        {:else}
          <div class="command-palette-empty">No matching commands</div>
        {/each}
      </div>
    </div>
  </div>
{/if}

{#if helpVisible}
  <LazyView load={loadHelpOverlay} name="help">
    {#snippet children(HelpComponent)}
      <HelpComponent
        title={helpTitle}
        content={helpContent}
        loading={helpLoading}
        onClose={() => (helpVisible = false)}
      />
    {/snippet}
  </LazyView>
{/if}

<ContextMenuHost />
<Toaster />

{#if shuttingDown}
  <div class="shutdown-backdrop">
    <div
      class="shutdown-dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="shutdown-title"
      aria-describedby="shutdown-description"
    >
      <div class="shutdown-spinner" aria-hidden="true"></div>
      <div>
        <h3 id="shutdown-title">Shutting down Grafium</h3>
        <p id="shutdown-description">Saving your place and releasing local AI models…</p>
      </div>
    </div>
  </div>
{/if}

{#if showNewPageDialog}
  <div class="dialog-backdrop" role="presentation" use:dismissOnBackdrop={cancelNewPage}>
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="new-page-title" tabindex="-1" onkeydown={dialogKeydown(cancelNewPage)}>
      <h3 class="dialog-title" id="new-page-title">New Page</h3>
      <input
        type="text"
        class="dialog-input"
        placeholder="Page name..."
        bind:value={newPageName}
        onkeydown={handleNewPageKeydown}
        use:autofocus
      />
      <div class="dialog-actions">
        <button class="dialog-btn dialog-btn-cancel" onclick={cancelNewPage}>Cancel</button>
        <button class="dialog-btn dialog-btn-ok" onclick={submitNewPage}>Create</button>
      </div>
    </div>
  </div>
{/if}

{#if printPageId}
  {@const printId = printPageId}
  <LazyView load={loadPrintDialog} name="print">
    {#snippet children(PrintComponent)}
      <PrintComponent pageId={printId} onClose={() => (printPageId = null)} />
    {/snippet}
  </LazyView>
{/if}

{#if showImportBooksDialog}
  <LazyView load={loadBookImportDialog} name="book import">
    {#snippet children(BookImportComponent)}
      <BookImportComponent
        onChooseFile={chooseBookFile}
        onChooseFolder={chooseBookFolder}
        onClose={() => (showImportBooksDialog = false)}
        onQueued={() => {
          showImportBooksDialog = false;
          showToast("Book import job added", "info");
        }}
      />
    {/snippet}
  </LazyView>
{/if}

{#if showImportMediaDialog}
  <div class="dialog-backdrop" role="presentation" use:dismissOnBackdrop={cancelImportMedia}>
    <div class="dialog import-media-dialog" role="dialog" aria-modal="true" aria-labelledby="import-media-title" tabindex="-1" onkeydown={dialogKeydown(cancelImportMedia)}>
      <h3 class="dialog-title" id="import-media-title">Import from Video/Audio</h3>
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
      <label class="dialog-label" for="import-media-source">URL or local file</label>
      <div class="dialog-input-with-action">
        <input
          id="import-media-source"
          type="text"
          class="dialog-input"
          placeholder="https://youtube.com/watch?v=... or /path/to/video.webm"
          bind:value={importMediaUrl}
          onkeydown={handleImportMediaKeydown}
          disabled={importMediaBusy}
          use:autofocus
        />
        <button
          class="dialog-file-picker"
          type="button"
          onclick={browseImportMedia}
          disabled={importMediaBusy}
          aria-label="Choose local media file"
          title="Choose local media file"
        >
          <svg viewBox="0 0 16 16" width="18" height="18" aria-hidden="true">
            <path d="M1.75 3.75h4l1.25 1.5h7.25v7.5H1.75z" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linejoin="round"/>
            <path d="M5.25 9h5.5M8 6.25v5.5" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round"/>
          </svg>
        </button>
      </div>
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
  <LazyView load={loadFolderBrowser} name="folders">
    {#snippet children(FoldersComponent)}
      <FoldersComponent
        title={folderBrowserTitle}
        onSelect={(path) => finishFolderBrowser(path)}
        onCancel={() => finishFolderBrowser(null)}
      />
    {/snippet}
  </LazyView>
{/if}

{#if showCreateGraphDialog}
  <div class="dialog-backdrop" role="presentation" use:dismissOnBackdrop={cancelCreateGraph}>
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="create-graph-title" tabindex="-1" onkeydown={dialogKeydown(cancelCreateGraph)}>
      <h3 class="dialog-title" id="create-graph-title">Create New Graph</h3>
      <p class="dialog-hint">Enter a name, then choose where to save it.</p>
      <input
        type="text"
        class="dialog-input"
        placeholder="Graph name..."
        bind:value={newGraphName}
        onkeydown={(e) => { if (e.key === "Enter") confirmCreateGraph(); }}
        use:autofocus
      />
      <div class="dialog-actions">
        <button class="dialog-btn dialog-btn-cancel" onclick={cancelCreateGraph}>Cancel</button>
        <button class="dialog-btn dialog-btn-ok" onclick={confirmCreateGraph} disabled={!newGraphName.trim()}>Choose Folder...</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .study-session-controls { position: sticky; top: 0; z-index: 20; }
  .study-session-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
    padding: 10px 20px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-secondary);
    color: var(--text-secondary);
    font-size: 13px;
  }
  .study-session-bar strong { color: var(--text-primary); }
  .study-session-bar button, .study-page-action button {
    color: var(--text-primary);
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 10px;
    cursor: pointer;
  }
  .study-page-action { display: flex; justify-content: flex-end; padding: 8px 24px 0; }
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

  .sidebar-container.collapsed {
    flex-basis: 52px;
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

  .shutdown-backdrop {
    position: fixed;
    inset: 0;
    z-index: 10000;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.56);
    backdrop-filter: blur(2px);
  }

  .shutdown-dialog {
    display: flex;
    align-items: center;
    gap: 16px;
    min-width: min(360px, calc(100vw - 40px));
    padding: 22px 24px;
    color: var(--text-primary);
    background: var(--surface-overlay);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  }

  .shutdown-dialog h3 {
    margin: 0 0 5px;
    font-size: 16px;
    font-weight: 600;
  }

  .shutdown-dialog p {
    margin: 0;
    color: var(--text-muted);
    font-size: 13px;
  }

  .shutdown-spinner {
    width: 22px;
    height: 22px;
    flex: 0 0 auto;
    border: 2px solid color-mix(in srgb, var(--accent) 28%, transparent);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: shutdown-spin 0.8s linear infinite;
  }

  @keyframes shutdown-spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .shutdown-spinner {
      animation: none;
      border-color: var(--accent);
    }
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

  .dialog.import-media-dialog {
    width: min(560px, calc(100vw - 32px));
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
    background-color: var(--bg-primary);
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

  .dialog-input-with-action {
    display: flex;
    align-items: stretch;
    gap: 8px;
    margin-bottom: 20px;
  }

  .dialog-input-with-action .dialog-input {
    min-width: 0;
    margin-bottom: 0;
  }

  .dialog-file-picker {
    width: 42px;
    min-width: 42px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--btn-bg);
    color: var(--text-primary);
    cursor: pointer;
  }

  .dialog-file-picker:hover {
    background: var(--btn-bg-hover);
    border-color: var(--accent);
  }

  .dialog-file-picker:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .dialog-file-picker:disabled {
    cursor: default;
    opacity: 0.55;
  }

  .dialog-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  /* Matches the other dialogs: bordered secondary actions, one filled primary. */
  .dialog-btn {
    padding: 8px 16px;
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    border: 1px solid var(--border);
  }

  .dialog-btn:disabled {
    cursor: default;
    opacity: 0.55;
  }

  .dialog-btn-cancel {
    background: var(--btn-bg);
    color: var(--text-primary);
  }

  .dialog-btn-cancel:hover:not(:disabled) {
    background: var(--btn-bg-hover);
    border-color: var(--text-secondary);
  }

  .dialog-btn-ok {
    background: var(--btn-primary-bg);
    color: var(--btn-primary-fg);
    border-color: transparent;
  }

  .dialog-btn-ok:hover:not(:disabled) {
    background: var(--btn-primary-hover);
  }

  @media (max-width: 640px) {
    .sidebar-resizer,
    .reference-panel-resizer {
      display: none !important;
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
