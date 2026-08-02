import { searchFts, searchPageTitles } from "./api";
import type { Block, PageSummary } from "./api";

export interface SidebarSearchController<T> {
  submit: (query: string) => void;
  cancel: () => void;
}

interface SidebarSearchControllerOptions<T> {
  debounceMs?: number;
  run: (query: string) => Promise<T>;
  apply: (query: string, value: T) => void;
  clear: () => void;
}

export type SidebarSearchResult =
  | { kind: "page"; page: PageSummary }
  | { kind: "block"; block: Block };

export async function runSidebarSearch(query: string): Promise<SidebarSearchResult[]> {
  try {
    const [pages, blocks] = await Promise.all([
      searchPageTitles(query, 10),
      query.length >= 2
        ? searchFts(query, 20)
        : Promise.resolve<Block[]>([]),
    ]);

    return [
      ...pages.map((page) => ({ kind: "page" as const, page })),
      ...blocks.slice(0, 12).map((block) => ({ kind: "block" as const, block })),
    ];
  } catch (error) {
    console.error("Sidebar search failed:", error);
    return [];
  }
}

export function createSidebarSearchController<T>({
  debounceMs = 120,
  run,
  apply,
  clear,
}: SidebarSearchControllerOptions<T>): SidebarSearchController<T> {
  let version = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const clearTimer = () => {
    if (timer === null) return;
    clearTimeout(timer);
    timer = null;
  };

  const cancel = () => {
    version += 1;
    clearTimer();
  };

  return {
    submit(query: string) {
      const trimmed = query.trim();
      version += 1;
      const requestVersion = version;
      clearTimer();

      if (!trimmed) {
        clear();
        return;
      }

      timer = setTimeout(() => {
        timer = null;
        void run(trimmed).then((value) => {
          if (version !== requestVersion) return;
          apply(trimmed, value);
        });
      }, debounceMs);
    },
    cancel,
  };
}
