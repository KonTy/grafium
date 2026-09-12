import { invoke } from "@tauri-apps/api/core";

export type ChatScope = "local" | "internet";

export const CHAT_SCOPE_PREF_KEY = "grafium.chat.scope";
const RESEARCH_PREF_KEY = "grafium.chat.research";

export interface ChatPreferences {
  scope: ChatScope;
  research: boolean;
}

export async function loadChatPreferences(): Promise<ChatPreferences> {
  const saved = await invoke<ChatPreferences | null>("get_chat_preferences");
  if (saved) return saved;
  let research = false;
  try {
    research = localStorage.getItem(RESEARCH_PREF_KEY) === "1";
  } catch (error) {
    console.warn("Could not read previous Research preference:", error);
  }
  const preferences = { scope: loadChatScope(), research };
  await saveChatPreferences(preferences);
  return preferences;
}

export async function saveChatPreferences(preferences: ChatPreferences): Promise<void> {
  // The native webview is incognito; browser storage alone cannot survive a restart.
  await invoke("set_chat_preferences", { preferences });
  saveChatScope(preferences.scope);
  try {
    localStorage.setItem(RESEARCH_PREF_KEY, preferences.research ? "1" : "0");
  } catch (error) {
    console.warn("Could not cache Research preference:", error);
  }
}

export function loadChatScope(): ChatScope {
  try {
    return localStorage.getItem(CHAT_SCOPE_PREF_KEY) === "internet" ? "internet" : "local";
  } catch (error) {
    console.warn("Could not read chat scope preference:", error);
    return "local";
  }
}

export function saveChatScope(scope: ChatScope): void {
  try {
    localStorage.setItem(CHAT_SCOPE_PREF_KEY, scope);
  } catch (error) {
    console.warn("Could not cache Chat scope preference:", error);
  }
}
