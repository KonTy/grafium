<script lang="ts">
  import type { NavIconName } from "../lib/mobileNav";

  let { name, size = 20 }: { name: NavIconName; size?: number } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
  {#if name === "journal"}
    <rect x="3" y="4" width="18" height="18" rx="2" ry="2"></rect>
    <line x1="16" y1="2" x2="16" y2="6"></line>
    <line x1="8" y1="2" x2="8" y2="6"></line>
    <line x1="3" y1="10" x2="21" y2="10"></line>
  {:else if name === "tasks"}
    <path d="M18 20V10"></path>
    <path d="M12 20V4"></path>
    <path d="M6 20v-6"></path>
  {:else if name === "library"}
    <path d="M3 3h4v18H3zM10 3h4v18h-4zM17 3l4 1v17l-4-1z" />
  {:else if name === "studies"}
    <path d="M2 4h7l3 3 3-3h7v16h-7l-3 2-3-2H2zM12 7v15" />
  {:else if name === "pages"}
    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
    <polyline points="14 2 14 8 20 8"></polyline>
  {:else if name === "graph"}
    <circle cx="5" cy="6" r="2"></circle>
    <circle cx="19" cy="6" r="2"></circle>
    <circle cx="12" cy="18" r="2"></circle>
    <line x1="6.7" y1="7" x2="10.5" y2="16.3"></line>
    <line x1="17.3" y1="7" x2="13.5" y2="16.3"></line>
    <line x1="7" y1="6" x2="17" y2="6"></line>
  {:else if name === "flashcards"}
    <rect x="2" y="4" width="20" height="16" rx="2"></rect>
    <path d="M12 8v8"></path>
    <path d="M8 12h8"></path>
  {:else if name === "chat"}
    <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
  {:else if name === "jobs"}
    <path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 7h18s-3 0-3-7"></path>
    <path d="M13.73 21a2 2 0 0 1-3.46 0"></path>
  {:else if name === "settings"}
    <circle cx="12" cy="12" r="3"></circle>
    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
  {:else if name === "new"}
    <line x1="12" y1="5" x2="12" y2="19"></line>
    <line x1="5" y1="12" x2="19" y2="12"></line>
  {:else if name === "panel"}
    <path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"></path>
    <path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"></path>
  {:else if name === "media"}
    <polygon points="23 7 16 12 23 17 23 7"></polygon>
    <rect x="1" y="5" width="15" height="14" rx="2" ry="2"></rect>
  {:else if name === "books"}
    <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"></path>
    <path d="M4 4.5A2.5 2.5 0 0 1 6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5z"></path>
  {:else if name === "folder"}
    <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
  {:else if name === "reindex"}
    <polyline points="23 4 23 10 17 10"></polyline>
    <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"></path>
  {:else if name === "more"}
    <circle cx="5" cy="12" r="2" fill="currentColor" stroke="none"></circle>
    <circle cx="12" cy="12" r="2" fill="currentColor" stroke="none"></circle>
    <circle cx="19" cy="12" r="2" fill="currentColor" stroke="none"></circle>
  {/if}
</svg>
