/**
 * Destinations for the phone's bottom bar and its "More" menu.
 *
 * The order is the desktop sidebar's (`Sidebar.svelte`), so a destination keeps
 * the same place relative to its neighbours at every screen size. The bar shows
 * the first `MOBILE_NAV_BAR_COUNT`; "More" continues the same list. A test
 * fails if this list and the sidebar drift apart.
 */

export type NavIconName =
  | "journal" | "tasks" | "library" | "studies" | "pages" | "graph" | "flashcards" | "chat" | "jobs" | "settings"
  | "new" | "panel" | "media" | "books" | "folder" | "reindex" | "more";

export interface MobileNavDestination {
  /** `currentView` value for which the destination is the open one. */
  view: string;
  /** Name shown in the sidebar and in the More menu. */
  label: string;
  /** Shorter name that fits under a bar icon. */
  barLabel?: string;
  /** Target understood by the app's `handleNavigate`. */
  target: string;
  icon: NavIconName;
}

export const MOBILE_NAV_DESTINATIONS: readonly MobileNavDestination[] = [
  { view: "journal", label: "Journal", target: "__journal__", icon: "journal" },
  { view: "statistics", label: "Tasks", target: "__statistics__", icon: "tasks" },
  { view: "library", label: "Library", target: "__library__", icon: "library" },
  { view: "studies", label: "Studies", target: "__studies__", icon: "studies" },
  { view: "all-pages", label: "All Pages", barLabel: "Pages", target: "__all_pages__", icon: "pages" },
  { view: "graph", label: "Graph View", target: "__graph__", icon: "graph" },
  { view: "flashcards", label: "Flashcards", target: "__flashcards__", icon: "flashcards" },
  { view: "chat", label: "Chat", target: "__chat__", icon: "chat" },
  { view: "jobs", label: "Jobs", target: "__jobs__", icon: "jobs" },
  { view: "settings", label: "Settings", target: "__settings__", icon: "settings" },
];

/** Destinations that fit on the bar without scrolling; "More" takes the last slot. */
export const MOBILE_NAV_BAR_COUNT = 5;

export const mobileBarDestinations = MOBILE_NAV_DESTINATIONS.slice(0, MOBILE_NAV_BAR_COUNT);
export const mobileMoreDestinations = MOBILE_NAV_DESTINATIONS.slice(MOBILE_NAV_BAR_COUNT);

/** True when the open view is reached through "More", so the button can show where the user is. */
export function isMoreView(view: string): boolean {
  return mobileMoreDestinations.some((destination) => destination.view === view);
}
