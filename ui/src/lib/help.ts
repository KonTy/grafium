export type HelpContext =
  | "general"
  | "editor"
  | "journal"
  | "graph"
  | "flashcards"
  | "tasks"
  | "studies"
  | "chat"
  | "settings"
  | "ai"
  | "sync"
  | "books"
  | "reader"
  | "library"
  | "search";

const HELP_PAGES: Record<HelpContext, string> = {
  general: "Help - Grafium Guide",
  editor: "Help - Editor",
  journal: "Help - Journal Guide",
  graph: "Help - Graph",
  flashcards: "Help - Flashcards",
  tasks: "Help - Tasks",
  studies: "Help - Studies",
  chat: "Help - Chat",
  settings: "Help - Settings",
  ai: "AI Setup And Privacy",
  sync: "Help - Sync",
  books: "Help - Books",
  reader: "Help - Private Reader",
  library: "Help - Library",
  search: "Help - Search",
};

export function isHelpContext(value: string | null): value is HelpContext {
  return value !== null && Object.prototype.hasOwnProperty.call(HELP_PAGES, value);
}

export function helpPageTitle(context: HelpContext): string {
  return HELP_PAGES[context];
}

export function closeSettingsHelpForContextualHelp(target: EventTarget | null): void {
  if (target instanceof Element) {
    target.closest("dialog[data-settings-help-dialog][open], dialog[open][data-reader-menu]")
      ?.dispatchEvent(new Event("cancel", { cancelable: true }));
  }
}

export async function loadHelpPage(context: HelpContext): Promise<string> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<string>("help_get_page", { context });
}
