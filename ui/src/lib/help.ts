export type HelpContext =
  | "general"
  | "editor"
  | "journal"
  | "graph"
  | "flashcards"
  | "tasks"
  | "chat"
  | "settings"
  | "sync"
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
  sync: "Help - Sync",
  search: "Help - Search",
};

export function isHelpContext(value: string | null): value is HelpContext {
  return value !== null && value in HELP_PAGES;
}

export function helpPageTitle(context: HelpContext): string {
  return HELP_PAGES[context];
}

export async function loadHelpPage(context: HelpContext): Promise<string> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<string>("help_get_page", { context });
}
