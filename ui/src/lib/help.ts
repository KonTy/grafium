export type HelpContext =
  | "general"
  | "editor"
  | "journal"
  | "graph"
  | "flashcards"
  | "tasks"
  | "chat"
  | "settings"
  | "ai"
  | "sync"
  | "books"
  | "search";

const HELP_PAGES: Record<HelpContext, string> = {
  general: "Help - Grafium Guide",
  editor: "Help - Editor",
  journal: "Help - Journal Guide",
  graph: "Help - Graph",
  flashcards: "Help - Flashcards",
  tasks: "Help - Tasks",
  chat: "Help - Chat",
  settings: "Help - Settings",
  ai: "AI Setup And Privacy",
  sync: "Help - Sync",
  books: "Help - Books",
  search: "Help - Search",
};

export function isHelpContext(value: string | null): value is HelpContext {
  return value !== null && Object.prototype.hasOwnProperty.call(HELP_PAGES, value);
}

export function helpPageTitle(context: HelpContext): string {
  return HELP_PAGES[context];
}

export async function loadHelpPage(context: HelpContext): Promise<string> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<string>("help_get_page", { context });
}
